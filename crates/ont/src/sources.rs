//! Readers of the two compiled file sources: `markdown_fields` and
//! `file_digest`. Pure file reading: nothing here touches a store.
//!
//! A symlink is never followed: a symlinked file or directory that a glob
//! reaches is skipped and counted. Only regular files are read. A file is
//! read in chunks with a bound of 16 MiB (a larger file fails the source);
//! `sha256` and `size` cover everything read, while at most 65,536 bytes
//! (and at most `head_lines` lines) are parsed for `markdown_fields`.
use ont_core::schema::{FileSourceKind, FileSourceSpec, Glob, GlobSegment};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Largest file that is read (and hashed) whole.
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// Bytes of a file that are parsed for `markdown_fields`.
pub const MAX_HEAD_BYTES: usize = 65_536;
/// Most records one source may list.
pub const MAX_RECORDS: usize = 10_000;
/// Most directory entries one listing may look at.
const MAX_VISITED: usize = 200_000;
/// Deepest directory nesting a listing follows.
const MAX_DEPTH: usize = 64;

/// Why a source could not be read: a stable code and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceError {
    pub code: &'static str,
    pub at: String,
}
impl SourceError {
    fn new(code: &'static str, at: impl Into<String>) -> Self {
        Self {
            code,
            at: at.into(),
        }
    }
}

/// One record: the file path, the sha256 of every byte read, and the record
/// object that mappings point into.
#[derive(Debug, Clone)]
pub struct Record {
    pub path: String,
    pub sha256: String,
    pub value: Value,
}

/// The records of one read of a source.
#[derive(Debug, Default)]
pub struct Listing {
    pub records: Vec<Record>,
    pub skipped_symlinks: usize,
}

/// The bounds of one source run. `Bounds::default()` is what `ont` uses;
/// tests inject smaller ones.
#[derive(Debug, Clone, Copy)]
pub struct Bounds {
    /// Directory entries one listing may look at (each directory is read
    /// once per glob).
    pub max_visited: usize,
    pub max_records: usize,
    /// Bytes read from all files of one run.
    pub max_total_bytes: u64,
    /// Retained parsed text (record JSON) of one run.
    pub max_text_bytes: usize,
}
impl Default for Bounds {
    fn default() -> Self {
        Self {
            max_visited: MAX_VISITED,
            max_records: MAX_RECORDS,
            max_total_bytes: 1024 * 1024 * 1024,
            max_text_bytes: 64 * 1024 * 1024,
        }
    }
}

/// Read every record of a file source.
pub fn read_files(spec: &FileSourceSpec) -> Result<Listing, SourceError> {
    read_files_with(spec, &Bounds::default())
}

pub fn read_files_with(spec: &FileSourceSpec, bounds: &Bounds) -> Result<Listing, SourceError> {
    let mut found: BTreeSet<PathBuf> = BTreeSet::new();
    let mut symlinks: BTreeSet<PathBuf> = BTreeSet::new();
    // The listing bound is per source run, not per glob: `visited` is
    // carried across the globs of one source. A file matched by two globs
    // enters `found` once, so the listing is the union.
    let mut visited = 0usize;
    for glob in &spec.globs {
        let mut walker = Walker {
            exclude: &spec.exclude,
            found: &mut found,
            symlinks: &mut symlinks,
            roots_required: spec.complete_listing,
            visited,
            max_visited: bounds.max_visited,
            listed: BTreeMap::new(),
            done: BTreeSet::new(),
        };
        let result = walker.walk(Path::new("/"), glob.segments(), 0, true);
        visited = walker.visited;
        result?;
    }
    if found.len() > bounds.max_records {
        return Err(SourceError::new("too_many_records", spec.globs[0].as_str()));
    }
    let mut records = Vec::new();
    let (mut bytes, mut text) = (0u64, 0usize);
    for path in &found {
        let Some((record, size)) = read_one(path, spec)? else {
            continue;
        };
        bytes += size;
        text += record.value.to_string().len();
        if bytes > bounds.max_total_bytes {
            return Err(SourceError::new(
                "source_bytes_too_large",
                path.display().to_string(),
            ));
        }
        if text > bounds.max_text_bytes {
            return Err(SourceError::new(
                "source_text_too_large",
                path.display().to_string(),
            ));
        }
        records.push(record);
    }
    Ok(Listing {
        records,
        skipped_symlinks: symlinks.len(),
    })
}

type Entries = Vec<(String, PathBuf)>;

struct Walker<'a> {
    exclude: &'a [Glob],
    found: &'a mut BTreeSet<PathBuf>,
    symlinks: &'a mut BTreeSet<PathBuf>,
    /// `complete_listing`: a wildcard-free directory of the glob's root that
    /// does not exist fails the source (`glob_root_missing`), so that an
    /// unmounted or renamed root cannot retract every object.
    roots_required: bool,
    visited: usize,
    max_visited: usize,
    /// Directories already read for this glob (each is read once).
    listed: BTreeMap<PathBuf, Entries>,
    /// `(directory, segments left)` pairs already walked.
    done: BTreeSet<(PathBuf, usize)>,
}
impl Walker<'_> {
    fn excluded(&self, path: &Path) -> bool {
        path.to_str()
            .is_some_and(|p| self.exclude.iter().any(|g| g.matches(p)))
    }

    /// `root`: every segment before this one is a wildcard-free name.
    fn walk(
        &mut self,
        dir: &Path,
        segs: &[GlobSegment],
        depth: usize,
        root: bool,
    ) -> Result<(), SourceError> {
        if depth > MAX_DEPTH {
            return Err(SourceError::new(
                "listing_too_large",
                dir.display().to_string(),
            ));
        }
        if !self.done.insert((dir.to_path_buf(), segs.len())) {
            return Ok(());
        }
        let Some((seg, rest)) = segs.split_first() else {
            return Ok(());
        };
        match seg {
            GlobSegment::Literal(name) => self.visit(&dir.join(name), rest, depth, root),
            GlobSegment::Pattern(_) => {
                for (name, path) in self.entries(dir)? {
                    if seg.matches(&name) {
                        self.visit(&path, rest, depth, false)?;
                    }
                }
                Ok(())
            }
            GlobSegment::AnyDirs => {
                self.walk(dir, rest, depth, false)?;
                for (_, path) in self.entries(dir)? {
                    if self.excluded(&path) {
                        continue;
                    }
                    let Some(meta) = lstat(&path)? else { continue };
                    if meta.file_type().is_symlink() {
                        self.symlinks.insert(path);
                    } else if meta.is_dir() {
                        self.walk(&path, segs, depth + 1, false)?;
                    }
                }
                Ok(())
            }
        }
    }

    /// The entries of `dir` with UTF-8 names, read once per glob; a missing
    /// or non-directory `dir` has none.
    fn entries(&mut self, dir: &Path) -> Result<Entries, SourceError> {
        if let Some(cached) = self.listed.get(dir) {
            return Ok(cached.clone());
        }
        let iter = match std::fs::read_dir(dir) {
            Ok(i) => i,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound) => return Ok(Vec::new()),
            Err(e) if e.raw_os_error() == Some(20) => return Ok(Vec::new()), // ENOTDIR
            Err(_) => {
                return Err(SourceError::new(
                    "directory_unreadable",
                    dir.display().to_string(),
                ));
            }
        };
        let mut out = Vec::new();
        for entry in iter {
            self.visited += 1;
            if self.visited > self.max_visited {
                return Err(SourceError::new(
                    "listing_too_large",
                    dir.display().to_string(),
                ));
            }
            let entry = entry
                .map_err(|_| SourceError::new("directory_unreadable", dir.display().to_string()))?;
            if let Some(name) = entry.file_name().to_str() {
                out.push((name.to_string(), entry.path()));
            }
        }
        out.sort();
        self.listed.insert(dir.to_path_buf(), out.clone());
        Ok(out)
    }

    /// `literal`: the segment and every one before it are wildcard-free
    /// names (the glob's root); a symlink there is a declaration error, not
    /// something to skip.
    fn visit(
        &mut self,
        path: &Path,
        rest: &[GlobSegment],
        depth: usize,
        literal: bool,
    ) -> Result<(), SourceError> {
        let last = rest.is_empty();
        // An exclude pattern that matches a path excludes it, and a directory
        // it matches is not read: nothing under it is a candidate.
        if self.excluded(path) {
            return Ok(());
        }
        let Some(meta) = lstat(path)? else {
            if literal && !last && self.roots_required {
                return Err(SourceError::new(
                    "glob_root_missing",
                    path.display().to_string(),
                ));
            }
            return Ok(());
        };
        if meta.file_type().is_symlink() {
            if literal {
                return Err(SourceError::new(
                    "glob_root_is_symlink",
                    path.display().to_string(),
                ));
            }
            self.symlinks.insert(path.to_path_buf());
            return Ok(());
        }
        if last {
            if meta.is_file() {
                self.found.insert(path.to_path_buf());
            } else if literal {
                return Err(SourceError::new(
                    "not_regular_file",
                    path.display().to_string(),
                ));
            }
        } else if meta.is_dir() {
            self.walk(path, rest, depth + 1, literal)?;
        } else if literal && self.roots_required {
            // A wildcard-free prefix that exists but is not a directory.
            return Err(SourceError::new(
                "glob_root_not_directory",
                path.display().to_string(),
            ));
        }
        Ok(())
    }
}

/// `symlink_metadata`; a missing path is `None`, any other failure fails the
/// source.
fn lstat(path: &Path) -> Result<Option<std::fs::Metadata>, SourceError> {
    match std::fs::symlink_metadata(path) {
        Ok(m) => Ok(Some(m)),
        Err(e)
            if matches!(e.kind(), std::io::ErrorKind::NotFound) || e.raw_os_error() == Some(20) =>
        {
            Ok(None)
        }
        Err(_) => Err(SourceError::new("stat_failed", path.display().to_string())),
    }
}

/// Read, hash and map one file; the byte count read comes with the record.
/// `None` when it vanished after the listing. The file is read only through
/// one descriptor that proves what was opened: opened with `O_NOFOLLOW |
/// O_NONBLOCK`, a regular file by the descriptor's own metadata, and its
/// real path (`/proc/self/fd/<n>`) byte-equal to the candidate path, so a
/// symlink anywhere in the path (a swapped parent directory included) is
/// refused (`path_not_canonical`, also when `/proc` cannot be read).
fn read_one(path: &Path, spec: &FileSourceSpec) -> Result<Option<(Record, u64)>, SourceError> {
    use rustix::fs::OFlags;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    let shown = path.display().to_string();
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags((OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC).bits() as i32)
        .open(path)
    {
        Ok(f) => f,
        Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound) => return Ok(None),
        // ELOOP: the final component became a symlink after the listing.
        Err(e) if e.raw_os_error() == Some(40) => {
            return Err(SourceError::new("path_not_canonical", shown));
        }
        Err(_) => return Err(SourceError::new("file_unreadable", shown)),
    };
    let meta = file
        .metadata()
        .map_err(|_| SourceError::new("file_unreadable", shown.clone()))?;
    if !meta.is_file() {
        return Err(SourceError::new("not_regular_file", shown));
    }
    let real = std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
        .map_err(|_| SourceError::new("path_not_canonical", shown.clone()))?;
    if real.as_os_str() != path.as_os_str() {
        return Err(SourceError::new("path_not_canonical", shown));
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(SourceError::new("file_too_large", shown));
    }
    let mut hasher = Sha256::new();
    let mut head: Vec<u8> = Vec::new();
    let mut total: u64 = 0;
    let mut reader = file.take(MAX_FILE_BYTES + 1);
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|_| SourceError::new("file_unreadable", shown.clone()))?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > MAX_FILE_BYTES {
            return Err(SourceError::new("file_too_large", shown));
        }
        hasher.update(&buf[..n]);
        if head.len() < MAX_HEAD_BYTES {
            let room = MAX_HEAD_BYTES - head.len();
            head.extend_from_slice(&buf[..n.min(room)]);
        }
    }
    let sha256 = format!("{:x}", hasher.finalize());
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    let path_text = path
        .to_str()
        .ok_or_else(|| SourceError::new("path_not_utf8", shown.clone()))?
        .to_string();
    let value = match spec.kind {
        FileSourceKind::FileDigest => json!({
            "path": path_text,
            "sha256": sha256,
            "size": total,
            "mtime": mtime,
            "mode": meta.mode() & 0o7777,
        }),
        FileSourceKind::MarkdownFields => {
            let truncated = total > head.len() as u64;
            let (line1, fields) = parse_head(&head, truncated, spec.head_lines);
            let parent = path.parent();
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let mut record = Map::new();
            record.insert("path".into(), json!(path_text));
            record.insert(
                "dir".into(),
                json!(
                    parent
                        .and_then(|p| p.file_name())
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                ),
            );
            record.insert(
                "dir_path".into(),
                json!(parent.and_then(|p| p.to_str()).unwrap_or("")),
            );
            if let Some((_, id)) = stem.split_once('-') {
                record.insert("stem_id".into(), json!(id));
            }
            record.insert("stem".into(), json!(stem));
            record.insert("sha256".into(), json!(sha256));
            record.insert("size".into(), json!(total));
            record.insert("mtime".into(), json!(mtime));
            record.insert("line1".into(), json!(line1));
            record.insert("fields".into(), Value::Object(fields));
            Value::Object(record)
        }
    };
    Ok(Some((
        Record {
            path: path_text,
            sha256,
            value,
        },
        total,
    )))
}

/// The first line and the `Key: value` fields of the head of a file. The
/// head is the first `head_lines` lines of at most `MAX_HEAD_BYTES` bytes;
/// when the bytes were cut, the partial last line is dropped. The first
/// occurrence of a key wins; keys match exactly.
pub fn parse_head(head: &[u8], truncated: bool, head_lines: usize) -> (String, Map<String, Value>) {
    let mut text = String::from_utf8_lossy(head).into_owned();
    if truncated {
        // The cut may fall inside a line: keep whole lines only; no newline
        // in the head means no lines.
        match text.rfind('\n') {
            Some(cut) => text.truncate(cut + 1),
            None => text.clear(),
        }
    }
    let mut fields = Map::new();
    let mut line1 = String::new();
    for (i, line) in text.lines().take(head_lines).enumerate() {
        if i == 0 {
            line1 = line.to_string();
        }
        if let Some((key, value)) = field_of(line)
            && !fields.contains_key(key)
        {
            fields.insert(key.to_string(), json!(value));
        }
    }
    (line1, fields)
}

/// `Key: value` or `- Key: value`. The key is non-empty, has no outer
/// whitespace, does not start with `#`, and `:` is followed by a space, a
/// tab or the end of the line (so `http://x` is not a field).
fn field_of(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix("- ").unwrap_or(line);
    let colon = rest.find(':')?;
    let (key, after) = (&rest[..colon], &rest[colon + 1..]);
    if key.is_empty()
        || key.len() > 128
        || key != key.trim()
        || key.starts_with('#')
        || !(after.is_empty() || after.starts_with([' ', '\t']))
    {
        return None;
    }
    Some((key, after.trim()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn fields_first_occurrence_exact_keys_and_bullets() {
        let text = b"# Title line\nOwner: lead\n- Owner: second\n- Next action: do it\nhttp://x.y: no\nowner: lower\nNote:\n";
        let (line1, f) = parse_head(text, false, 40);
        assert_eq!(line1, "# Title line");
        assert_eq!(f["Owner"], "lead");
        assert_eq!(f["Next action"], "do it");
        assert_eq!(f["owner"], "lower");
        assert_eq!(f["Note"], "");
        assert!(!f.contains_key("http"));
    }

    fn temp() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let d = std::env::temp_dir().join(format!(
            "ont-src-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn spec(glob: &str, exclude: &[String]) -> FileSourceSpec {
        let def: ont_core::schema::SourceDef = serde_json::from_value(json!({
            "kind":"markdown_fields","glob":glob,"exclude":exclude,"fresh_for_s":5
        }))
        .unwrap();
        def.file_spec().unwrap().unwrap()
    }

    #[test]
    fn exclude_prunes_a_directory_before_it_is_read() {
        let d = temp();
        std::fs::create_dir_all(d.join("keep/x")).unwrap();
        std::fs::write(d.join("keep/x/a.md"), "x").unwrap();
        std::fs::create_dir_all(d.join("big")).unwrap();
        for i in 0..50 {
            std::fs::write(d.join(format!("big/f{i}.md")), "x").unwrap();
        }
        let tight = Bounds {
            max_visited: 20,
            ..Bounds::default()
        };
        let glob = format!("{}/**/*.md", d.display());
        // Without the exclude the big directory breaks the bound.
        let err = read_files_with(&spec(&glob, &[]), &tight).unwrap_err();
        assert_eq!(err.code, "listing_too_large");
        // With it, the directory is never read and nothing under it is indexed.
        let ex = [format!("{}/big", d.display())];
        let listing = read_files_with(&spec(&glob, &ex), &tight).unwrap();
        let paths: Vec<&str> = listing.records.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, [format!("{}/keep/x/a.md", d.display())]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_swapped_parent_is_not_read_and_aggregate_bounds_hold() {
        let d = temp();
        std::fs::create_dir_all(d.join("real")).unwrap();
        std::fs::write(d.join("real/a.md"), "hello\n").unwrap();
        std::os::unix::fs::symlink(d.join("real"), d.join("link")).unwrap();
        let s = spec(&format!("{}/*/a.md", d.display()), &[]);
        let err = read_one(&d.join("link/a.md"), &s).unwrap_err();
        assert_eq!(err.code, "path_not_canonical");
        assert!(read_one(&d.join("real/a.md"), &s).unwrap().is_some());
        // The wildcard walk skips the symlinked directory and reads the real one.
        let listing = read_files(&s).unwrap();
        assert_eq!((listing.records.len(), listing.skipped_symlinks), (1, 1));
        let small = Bounds {
            max_total_bytes: 3,
            ..Bounds::default()
        };
        assert_eq!(
            read_files_with(&s, &small).unwrap_err().code,
            "source_bytes_too_large"
        );
        let small = Bounds {
            max_text_bytes: 10,
            ..Bounds::default()
        };
        assert_eq!(
            read_files_with(&s, &small).unwrap_err().code,
            "source_text_too_large"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_head_cut_inside_one_line_has_no_lines() {
        let (line1, f) = parse_head(b"Key: a very long line without a newline", true, 10);
        assert_eq!(line1, "");
        assert!(f.is_empty());
    }

    #[test]
    fn head_lines_and_truncation_bound_the_parse() {
        let (_, f) = parse_head(b"A: 1\nB: 2\nC: 3\n", false, 2);
        assert!(f.contains_key("A") && f.contains_key("B") && !f.contains_key("C"));
        // Cut mid-line: the partial last line is dropped.
        let (_, f) = parse_head(b"A: 1\nB: 2 and mor", true, 10);
        assert!(f.contains_key("A") && !f.contains_key("B"));
    }

    /// The record keys that `read_one` produces for each source kind are
    /// exactly the list the schema check accepts (`record_fields`).
    #[test]
    fn record_keys_equal_the_schema_check_list() {
        let d = temp();
        std::fs::write(d.join("a-1.md"), "# t\nOwner: x\n").unwrap();
        for (kind, glob) in [
            (FileSourceKind::MarkdownFields, "a-1.md"),
            (FileSourceKind::FileDigest, "a-1.md"),
        ] {
            let spec = FileSourceSpec {
                kind,
                globs: Vec::new(),
                exclude: Vec::new(),
                head_lines: 40,
                complete_listing: false,
                fresh_for_s: 1,
            };
            let (record, _) = read_one(&d.join(glob), &spec).unwrap().unwrap();
            let mut produced: Vec<&str> = record
                .value
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            produced.sort_unstable();
            let mut listed: Vec<&str> = kind.record_fields().to_vec();
            listed.sort_unstable();
            assert_eq!(produced, listed, "{kind:?}");
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}
