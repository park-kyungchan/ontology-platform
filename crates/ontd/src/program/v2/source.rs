//! V2-only custody enforcement. Root and the independently adopted source UID
//! may own ancestors; regular source files must belong to the adopted UID.
//! No ancestor/file may be group/world writable. Same-UID writers remain trusted.
use super::{
    ArtifactRef, BTreeMap, EVIDENCE_LIMIT, EVIDENCE_ROOT, IntakeError, Result, artifact,
    measurement_id,
};
use rustix::fd::OwnedFd;
use rustix::fs::{FileType, Mode, OFlags, fstat, open, openat};
use std::{fs::File, io::Read};
const DIR: OFlags = OFlags::PATH
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::NONBLOCK)
    .union(OFlags::CLOEXEC);
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Opened,
    Read,
}
fn custody(uid: u32, mode: u32, owner: u32, ancestor: bool) -> Result<()> {
    if (uid != owner && !(ancestor && uid == 0)) || mode & 0o022 != 0 {
        return Err(IntakeError::SourceUnavailable);
    }
    Ok(())
}
struct Directory {
    fds: Vec<OwnedFd>,
    names: Vec<String>,
    owner: u32,
}
impl Directory {
    fn open(path: &str, owner: u32) -> Result<Self> {
        if !super::absolute(path) {
            return Err(IntakeError::SourceUnavailable);
        }
        let names = path
            .split('/')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        let mut fds =
            vec![open("/", DIR, Mode::empty()).map_err(|_| IntakeError::SourceUnavailable)?];
        for name in &names {
            fds.push(
                openat(
                    fds.last().ok_or(IntakeError::SourceUnavailable)?,
                    name.as_str(),
                    DIR,
                    Mode::empty(),
                )
                .map_err(|_| IntakeError::SourceUnavailable)?,
            );
        }
        let directory = Self { fds, names, owner };
        directory.check()?;
        Ok(directory)
    }
    fn check(&self) -> Result<()> {
        let mut fresh =
            open("/", DIR, Mode::empty()).map_err(|_| IntakeError::SourceUnavailable)?;
        for (i, held) in self.fds.iter().enumerate() {
            let a = fstat(held).map_err(|_| IntakeError::SourceUnavailable)?;
            let b = fstat(&fresh).map_err(|_| IntakeError::SourceUnavailable)?;
            custody(a.st_uid, a.st_mode, self.owner, true)?;
            custody(b.st_uid, b.st_mode, self.owner, true)?;
            if a.st_dev != b.st_dev || a.st_ino != b.st_ino {
                return Err(IntakeError::SourceUnavailable);
            }
            if let Some(name) = self.names.get(i) {
                fresh = openat(&fresh, name.as_str(), DIR, Mode::empty())
                    .map_err(|_| IntakeError::SourceUnavailable)?;
            }
        }
        Ok(())
    }
    fn file(&self, name: &str) -> Result<File> {
        let fd = openat(
            self.fds.last().ok_or(IntakeError::SourceUnavailable)?,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| IntakeError::SourceUnavailable)?;
        let st = fstat(&fd).map_err(|_| IntakeError::SourceUnavailable)?;
        if FileType::from_raw_mode(st.st_mode) != FileType::RegularFile {
            return Err(IntakeError::SourceUnavailable);
        }
        custody(st.st_uid, st.st_mode, self.owner, false)?;
        Ok(fd.into())
    }
    fn read(&self, name: &str, limit: usize, hook: &mut impl FnMut(Phase)) -> Result<Vec<u8>> {
        self.check()?;
        let file = self.file(name)?;
        let before = fstat(&file).map_err(|_| IntakeError::SourceUnavailable)?;
        let mut bytes = Vec::new();
        (&file)
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| IntakeError::SourceUnavailable)?;
        if bytes.len() > limit {
            return Err(IntakeError::Bounds);
        }
        hook(Phase::Read);
        self.check()?;
        let held = fstat(&file).map_err(|_| IntakeError::SourceUnavailable)?;
        custody(held.st_uid, held.st_mode, self.owner, false)?;
        let fresh = fstat(&self.file(name)?).map_err(|_| IntakeError::SourceUnavailable)?;
        if before.st_dev != fresh.st_dev || before.st_ino != fresh.st_ino {
            return Err(IntakeError::SourceUnavailable);
        }
        self.check()?;
        Ok(bytes)
    }
}
pub(super) fn read_evidence(
    id: &str,
    refs: &[ArtifactRef],
    owner: u32,
) -> Result<BTreeMap<String, Vec<u8>>> {
    read_at(EVIDENCE_ROOT, id, refs, owner, |_| {})
}
fn read_at(
    root: &str,
    id: &str,
    refs: &[ArtifactRef],
    owner: u32,
    mut hook: impl FnMut(Phase),
) -> Result<BTreeMap<String, Vec<u8>>> {
    if !measurement_id(id)
        || refs.iter().any(|a| !a.valid())
        || refs.windows(2).any(|w| w[0].path >= w[1].path)
    {
        return Err(IntakeError::InvalidData);
    }
    let mut budget = 0u64;
    for a in refs {
        budget = budget.checked_add(a.bytes).ok_or(IntakeError::Bounds)?;
    }
    if budget > EVIDENCE_LIMIT as u64 {
        return Err(IntakeError::Bounds);
    }
    let dir = Directory::open(&format!("{root}/{id}"), owner)?;
    hook(Phase::Opened);
    dir.check()?;
    let mut out = BTreeMap::new();
    for a in refs {
        let bytes = dir.read(&a.path, a.bytes as usize, &mut hook)?;
        out.insert(a.path.clone(), bytes);
        artifact(a, &out)?;
    }
    dir.check()?;
    Ok(out)
}
pub(super) fn read_journal(owner: u32) -> Result<Vec<u8>> {
    read_journal_at(super::SNAPSHOTS_PATH, owner, |_| {})
}
fn read_journal_at(path: &str, owner: u32, mut hook: impl FnMut(Phase)) -> Result<Vec<u8>> {
    if !super::absolute(path) {
        return Err(IntakeError::SourceUnavailable);
    }
    let (parent, name) = path
        .rsplit_once('/')
        .ok_or(IntakeError::SourceUnavailable)?;
    if name.is_empty() {
        return Err(IntakeError::SourceUnavailable);
    }
    let dir = Directory::open(if parent.is_empty() { "/" } else { parent }, owner)?;
    hook(Phase::Opened);
    dir.check()?;
    dir.read(name, super::JOURNAL_LIMIT, &mut hook)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            // Build outputs are owner-controlled; /tmp is deliberately not an
            // allowed production ancestor and is not exempted for these tests.
            let p = std::env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .join(format!(
                    "v2-custody-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
            std::fs::DirBuilder::new().mode(0o700).create(&p).unwrap();
            Self(p)
        }
        fn path(&self, name: &str) -> String {
            self.0.join(name).to_str().unwrap().to_string()
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn uid() -> u32 {
        rustix::process::getuid().as_raw()
    }
    fn permissions(path: impl AsRef<std::path::Path>, mode: u32) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    fn setup() -> (Temp, ArtifactRef) {
        let t = Temp::new();
        std::fs::create_dir_all(t.0.join("root/test")).unwrap();
        std::fs::write(t.0.join("root/test/a.json"), b"{}").unwrap();
        (t, ArtifactRef::new("a.json", b"{}"))
    }
    #[test]
    fn evidence_no_follow_fifo_bounds_and_ancestor_substitution() {
        let (t, a) = setup();
        let root = t.path("root");
        assert_eq!(
            read_at(&root, "test", std::slice::from_ref(&a), uid(), |_| {}).unwrap()["a.json"],
            b"{}"
        );
        let mut huge = a.clone();
        huge.bytes = EVIDENCE_LIMIT as u64 + 1;
        assert!(matches!(
            read_at(&root, "test", &[huge], uid(), |_| {}),
            Err(IntakeError::Bounds)
        ));
        std::fs::remove_file(t.0.join("root/test/a.json")).unwrap();
        std::os::unix::fs::symlink(t.0.join("target"), t.0.join("root/test/a.json")).unwrap();
        assert!(read_at(&root, "test", std::slice::from_ref(&a), uid(), |_| {}).is_err());
        std::fs::remove_file(t.0.join("root/test/a.json")).unwrap();
        rustix::fs::mknodat(
            rustix::fs::CWD,
            t.0.join("root/test/a.json"),
            FileType::Fifo,
            Mode::RUSR | Mode::WUSR,
            0,
        )
        .unwrap();
        assert!(read_at(&root, "test", std::slice::from_ref(&a), uid(), |_| {}).is_err());
        std::fs::remove_file(t.0.join("root/test/a.json")).unwrap();
        std::fs::write(t.0.join("root/test/a.json"), b"{}").unwrap();
        assert!(
            read_at(&root, "test", &[a], uid(), |phase| {
                if phase == Phase::Opened {
                    std::fs::rename(t.0.join("root"), t.0.join("retained")).unwrap();
                    std::fs::create_dir_all(t.0.join("root/test")).unwrap();
                    std::fs::write(t.0.join("root/test/a.json"), b"{}").unwrap();
                }
            })
            .is_err()
        );
    }
    #[test]
    fn attacker_rehashed_recording_is_rejected_when_leaf_is_writable() {
        let (t, _) = setup();
        let path = t.path("root/test/disk.json");
        let bytes=br#"{"source_revision":"synthetic","capture_start":1,"capture_end":1,"outcome":{"kind":"disk","observed_at":1,"numerator":99999999999,"denominator":1,"unit":"bytes"}}"#;
        std::fs::write(&path, bytes).unwrap();
        permissions(&path, 0o666);
        assert!(
            read_at(
                &t.path("root"),
                "test",
                &[ArtifactRef::new("disk.json", bytes)],
                uid(),
                |_| {}
            )
            .is_err()
        );
    }
    #[test]
    fn custody_rejects_every_mutable_ancestor_and_foreign_owner() {
        for part in ["root", "root/test", "root/test/a.json"] {
            let (t, a) = setup();
            permissions(t.0.join(part), 0o770);
            assert!(
                read_at(&t.path("root"), "test", &[a], uid(), |_| {}).is_err(),
                "{part}"
            );
        }
        let (t, a) = setup();
        assert!(read_at(&t.path("root"), "test", &[a], uid().wrapping_add(1), |_| {}).is_err());
        // Publicly readable files and search-only ancestors remain legitimate.
        let (t, a) = setup();
        permissions(t.0.join("root/test/a.json"), 0o644);
        permissions(t.0.join("root/test"), 0o111);
        assert!(read_at(&t.path("root"), "test", &[a], uid(), |_| {}).is_ok());
        permissions(t.0.join("root/test"), 0o700);
    }
    #[test]
    fn custody_rechecks_mode_after_open_and_after_read() {
        for phase in [Phase::Opened, Phase::Read] {
            let (t, a) = setup();
            let path = t.0.join("root/test/a.json");
            assert!(
                read_at(&t.path("root"), "test", &[a], uid(), |p| if p == phase {
                    permissions(&path, 0o666);
                })
                .is_err()
            );
        }
        let (t, a) = setup();
        assert!(
            read_at(&t.path("root"), "test", &[a], uid(), |p| {
                if p == Phase::Read {
                    permissions(t.0.join("root"), 0o777);
                }
            })
            .is_err()
        );
    }
    #[test]
    fn journal_and_evidence_share_custody_and_identity_checks() {
        let t = Temp::new();
        let file = t.path("rows.fixture");
        std::fs::write(&file, b"{}\n").unwrap();
        assert_eq!(read_journal_at(&file, uid(), |_| {}).unwrap(), b"{}\n");
        permissions(&file, 0o666);
        assert!(read_journal_at(&file, uid(), |_| {}).is_err());
        permissions(&file, 0o600);
        assert!(read_journal_at(&file, uid().wrapping_add(1), |_| {}).is_err());
        assert!(
            read_journal_at(&file, uid(), |p| if p == Phase::Read {
                permissions(&file, 0o660);
            })
            .is_err()
        );
        permissions(&file, 0o600);
        assert!(
            read_journal_at(&file, uid(), |p| if p == Phase::Read {
                std::fs::rename(&file, t.0.join("held")).unwrap();
                std::fs::write(&file, b"{}\n").unwrap();
            })
            .is_err()
        );
    }
}
