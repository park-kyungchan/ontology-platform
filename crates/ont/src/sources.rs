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
use std::collections::BTreeSet;
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

/// Read every record of a file source.
pub fn read_files(spec: &FileSourceSpec) -> Result<Listing, SourceError> {
    let mut walker = Walker {
        exclude: &spec.exclude,
        found: BTreeSet::new(),
        symlinks: BTreeSet::new(),
        visited: 0,
    };
    for glob in &spec.globs {
        walker.walk(Path::new("/"), glob.segments(), 0)?;
    }
    if walker.found.len() > MAX_RECORDS {
        return Err(SourceError::new("too_many_records", spec.globs[0].as_str()));
    }
    let mut records = Vec::new();
    for path in &walker.found {
        let Some(read) = read_one(path, spec)? else {
            continue;
        };
        records.push(read);
    }
    Ok(Listing {
        records,
        skipped_symlinks: walker.symlinks.len(),
    })
}

struct Walker<'a> {
    exclude: &'a [Glob],
    found: BTreeSet<PathBuf>,
    symlinks: BTreeSet<PathBuf>,
    visited: usize,
}
impl Walker<'_> {
    fn walk(&mut self, dir: &Path, segs: &[GlobSegment], depth: usize) -> Result<(), SourceError> {
        if depth > MAX_DEPTH {
            return Err(SourceError::new(
                "listing_too_large",
                dir.display().to_string(),
            ));
        }
        let Some((seg, rest)) = segs.split_first() else {
            return Ok(());
        };
        match seg {
            GlobSegment::Literal(name) => self.visit(&dir.join(name), rest, depth),
            GlobSegment::Pattern(_) => {
                for (name, path) in self.entries(dir)? {
                    if seg.matches(&name) {
                        self.visit(&path, rest, depth)?;
                    }
                }
                Ok(())
            }
            GlobSegment::AnyDirs => {
                self.walk(dir, rest, depth)?;
                for (_, path) in self.entries(dir)? {
                    let Some(meta) = lstat(&path)? else { continue };
                    if meta.file_type().is_symlink() {
                        self.symlinks.insert(path);
                    } else if meta.is_dir() {
                        self.walk(&path, segs, depth + 1)?;
                    }
                }
                Ok(())
            }
        }
    }

    /// The entries of `dir` with UTF-8 names; a missing or non-directory
    /// `dir` has none.
    fn entries(&mut self, dir: &Path) -> Result<Vec<(String, PathBuf)>, SourceError> {
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
            if self.visited > MAX_VISITED {
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
        Ok(out)
    }

    fn visit(
        &mut self,
        path: &Path,
        rest: &[GlobSegment],
        depth: usize,
    ) -> Result<(), SourceError> {
        let last = rest.is_empty();
        if last
            && path
                .to_str()
                .is_some_and(|p| self.exclude.iter().any(|g| g.matches(p)))
        {
            return Ok(());
        }
        let Some(meta) = lstat(path)? else {
            return Ok(());
        };
        if meta.file_type().is_symlink() {
            self.symlinks.insert(path.to_path_buf());
            return Ok(());
        }
        if last {
            if meta.is_file() {
                self.found.insert(path.to_path_buf());
            }
        } else if meta.is_dir() {
            self.walk(path, rest, depth + 1)?;
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

/// Read, hash and map one file. `None` when it vanished after the listing.
fn read_one(path: &Path, spec: &FileSourceSpec) -> Result<Option<Record>, SourceError> {
    let shown = path.display().to_string();
    let Some(before) = lstat(path)? else {
        return Ok(None);
    };
    if !before.is_file() {
        return Ok(None);
    }
    if before.len() > MAX_FILE_BYTES {
        return Err(SourceError::new("file_too_large", shown));
    }
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound) => return Ok(None),
        Err(_) => return Err(SourceError::new("file_unreadable", shown)),
    };
    let meta = file
        .metadata()
        .map_err(|_| SourceError::new("file_unreadable", shown.clone()))?;
    if !meta.is_file() || meta.dev() != before.dev() || meta.ino() != before.ino() {
        return Err(SourceError::new("file_changed", shown));
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
    Ok(Some(Record {
        path: path_text,
        sha256,
        value,
    }))
}

/// The first line and the `Key: value` fields of the head of a file. The
/// head is the first `head_lines` lines of at most `MAX_HEAD_BYTES` bytes;
/// when the bytes were cut, the partial last line is dropped. The first
/// occurrence of a key wins; keys match exactly.
pub fn parse_head(head: &[u8], truncated: bool, head_lines: usize) -> (String, Map<String, Value>) {
    let mut text = String::from_utf8_lossy(head).into_owned();
    if truncated && let Some(cut) = text.rfind('\n') {
        text.truncate(cut + 1);
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

    #[test]
    fn head_lines_and_truncation_bound_the_parse() {
        let (_, f) = parse_head(b"A: 1\nB: 2\nC: 3\n", false, 2);
        assert!(f.contains_key("A") && f.contains_key("B") && !f.contains_key("C"));
        // Cut mid-line: the partial last line is dropped.
        let (_, f) = parse_head(b"A: 1\nB: 2 and mor", true, 10);
        assert!(f.contains_key("A") && !f.contains_key("B"));
    }
}
