//! Held directory descriptors plus full ancestor revalidation, before and after
//! accepting evidence. No symlink/FIFO traversal or arbitrary row-provided path.
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
struct Directory {
    fds: Vec<OwnedFd>,
    names: Vec<String>,
}
impl Directory {
    fn open(path: &str) -> Result<Self> {
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
        Ok(Self { fds, names })
    }
    fn check(&self) -> Result<()> {
        let mut fresh =
            open("/", DIR, Mode::empty()).map_err(|_| IntakeError::SourceUnavailable)?;
        for (i, held) in self.fds.iter().enumerate() {
            let a = fstat(held).map_err(|_| IntakeError::SourceUnavailable)?;
            let b = fstat(&fresh).map_err(|_| IntakeError::SourceUnavailable)?;
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
        Ok(fd.into())
    }
}
pub(super) fn read_evidence(id: &str, refs: &[ArtifactRef]) -> Result<BTreeMap<String, Vec<u8>>> {
    read_at(EVIDENCE_ROOT, id, refs, || {})
}
fn read_at(
    root: &str,
    id: &str,
    refs: &[ArtifactRef],
    mut after_open: impl FnMut(),
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
    let dir = Directory::open(&format!("{root}/{id}"))?;
    after_open();
    dir.check()?;
    let mut out = BTreeMap::new();
    for a in refs {
        let file = dir.file(&a.path)?;
        let before = file
            .metadata()
            .map_err(|_| IntakeError::SourceUnavailable)?;
        let mut bytes = Vec::new();
        (&file)
            .take(a.bytes + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| IntakeError::SourceUnavailable)?;
        dir.check()?;
        let after = dir
            .file(&a.path)?
            .metadata()
            .map_err(|_| IntakeError::SourceUnavailable)?;
        use std::os::unix::fs::MetadataExt;
        if before.dev() != after.dev() || before.ino() != after.ino() {
            return Err(IntakeError::SourceUnavailable);
        }
        out.insert(a.path.clone(), bytes);
        artifact(a, &out)?;
    }
    dir.check()?;
    Ok(out)
}
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    #[test]
    fn evidence_no_follow_fifo_bounds_and_ancestor_substitution() {
        let base = std::env::temp_dir().join(format!(
            "ont-intake-v2-source-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(base.join("root/test")).unwrap();
        let root = base.join("root");
        let root = root.to_str().unwrap();
        std::fs::write(base.join("root/test/a.json"), b"{}").unwrap();
        let a = ArtifactRef::new("a.json", b"{}");
        assert_eq!(
            read_at(root, "test", std::slice::from_ref(&a), || {}).unwrap()["a.json"],
            b"{}"
        );
        let mut huge = a.clone();
        huge.bytes = EVIDENCE_LIMIT as u64 + 1;
        assert!(matches!(
            read_at(root, "test", &[huge], || {}),
            Err(IntakeError::Bounds)
        ));
        std::fs::remove_file(base.join("root/test/a.json")).unwrap();
        std::os::unix::fs::symlink(base.join("target"), base.join("root/test/a.json")).unwrap();
        assert!(read_at(root, "test", std::slice::from_ref(&a), || {}).is_err());
        std::fs::remove_file(base.join("root/test/a.json")).unwrap();
        rustix::fs::mknodat(
            rustix::fs::CWD,
            base.join("root/test/a.json"),
            FileType::Fifo,
            Mode::RUSR | Mode::WUSR,
            0,
        )
        .unwrap();
        assert!(read_at(root, "test", std::slice::from_ref(&a), || {}).is_err());
        std::fs::remove_file(base.join("root/test/a.json")).unwrap();
        std::fs::write(base.join("root/test/a.json"), b"{}").unwrap();
        assert!(
            read_at(root, "test", &[a], || {
                std::fs::rename(base.join("root"), base.join("retained")).unwrap();
                std::fs::create_dir_all(base.join("root/test")).unwrap();
                std::fs::write(base.join("root/test/a.json"), b"{}").unwrap();
            })
            .is_err()
        );
        std::fs::remove_dir_all(base).unwrap();
    }
}
