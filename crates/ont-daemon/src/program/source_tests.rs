#![allow(clippy::unwrap_used)]
use super::*;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

struct Temporary(PathBuf);
impl Temporary {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "ont-program-open-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&base).unwrap();
        Self(base)
    }
    fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn read(path: &Path) -> Result<Vec<u8>> {
    bounded_file(path.to_str().unwrap(), 64)
}

#[test]
fn normal_regular_source_preserves_bytes_and_bounds() {
    let t = Temporary::new();
    let source = t.path("source");
    std::fs::write(&source, b"synthetic safe bytes").unwrap();
    assert_eq!(read(&source).unwrap(), b"synthetic safe bytes");
    assert_eq!(
        bounded_file(source.to_str().unwrap(), 2),
        Err(IntakeError::Bounds)
    );
}
#[test]
fn fixed_sources_reject_final_symlink() {
    let t = Temporary::new();
    let outside = t.path("outside");
    std::fs::write(&outside, b"synthetic outside bytes").unwrap();
    let source = t.path("source");
    symlink(&outside, &source).unwrap();
    assert_eq!(read(&source), Err(IntakeError::SourceUnavailable));
}
#[test]
fn fixed_sources_reject_parent_directory_symlink() {
    let t = Temporary::new();
    std::fs::create_dir(t.path("outside")).unwrap();
    std::fs::write(t.path("outside/source"), b"synthetic outside bytes").unwrap();
    symlink(t.path("outside"), t.path("parent")).unwrap();
    assert_eq!(
        read(&t.path("parent/source")),
        Err(IntakeError::SourceUnavailable)
    );
}

#[test]
fn fixed_sources_reject_dangling_links_directories_and_non_directory_parents() {
    let t = Temporary::new();
    symlink(t.path("missing"), t.path("dangling")).unwrap();
    assert_eq!(
        read(&t.path("dangling")),
        Err(IntakeError::SourceUnavailable)
    );
    assert_eq!(read(&t.0), Err(IntakeError::SourceUnavailable));
    std::fs::write(t.path("regular"), b"ordinary").unwrap();
    assert_eq!(
        read(&t.path("regular/child")),
        Err(IntakeError::SourceUnavailable)
    );
    assert_eq!(
        read(&t.path("missing")),
        Err(IntakeError::SourceUnavailable)
    );
}

#[test]
fn fixed_sources_reject_fifo_without_blocking() {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    const CHILD_PATH: &str = "ONT_PROGRAM_SYNTHETIC_FIFO_CHILD";
    if let Some(path) = std::env::var_os(CHILD_PATH) {
        assert_eq!(read(Path::new(&path)), Err(IntakeError::SourceUnavailable));
        return;
    }
    let t = Temporary::new();
    let fifo = t.path("fifo");
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "program::source_tests::fixed_sources_reject_fifo_without_blocking",
        ])
        .env(CHILD_PATH, &fifo)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("synthetic FIFO read blocked");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // A final link to a FIFO refuses before reaching its target too.
    symlink(&fifo, t.path("fifo-link")).unwrap();
    assert_eq!(
        read(&t.path("fifo-link")),
        Err(IntakeError::SourceUnavailable)
    );
}

fn traversal_fixture() -> Temporary {
    let t = Temporary::new();
    std::fs::create_dir(t.path("authorized")).unwrap();
    std::fs::create_dir(t.path("outside")).unwrap();
    std::fs::write(t.path("authorized/source"), b"synthetic safe bytes").unwrap();
    std::fs::write(t.path("outside/source"), b"synthetic outside bytes").unwrap();
    t
}

#[test]
fn replacement_before_directory_open_refuses_the_new_symlink() {
    let t = traversal_fixture();
    let source = t.path("authorized/source");
    let mut replaced = false;
    let result = source::open_with_hook(source.to_str().unwrap(), |component| {
        if component == "authorized" {
            std::fs::rename(t.path("authorized"), t.path("retained")).unwrap();
            symlink(t.path("outside"), t.path("authorized")).unwrap();
            replaced = true;
        }
    });
    assert!(replaced);
    assert_eq!(result.err(), Some(IntakeError::SourceUnavailable));
}

#[test]
fn replacement_of_opened_parent_cannot_redirect_later_component() {
    let t = traversal_fixture();
    let source = t.path("authorized/source");
    let mut replaced = false;
    let file = source::open_with_hook(source.to_str().unwrap(), |component| {
        if component == "source" {
            std::fs::rename(t.path("authorized"), t.path("retained")).unwrap();
            symlink(t.path("outside"), t.path("authorized")).unwrap();
            replaced = true;
        }
    })
    .unwrap();
    assert!(replaced);
    assert_eq!(bounded_read(file, 64).unwrap(), b"synthetic safe bytes");
    assert_eq!(read(&source), Err(IntakeError::SourceUnavailable));
}

#[test]
fn replacement_before_final_open_refuses_the_new_symlink() {
    let t = traversal_fixture();
    let source = t.path("authorized/source");
    let mut replaced = false;
    let result = source::open_with_hook(source.to_str().unwrap(), |component| {
        if component == "source" {
            std::fs::remove_file(&source).unwrap();
            symlink(t.path("outside/source"), &source).unwrap();
            replaced = true;
        }
    });
    assert!(replaced);
    assert_eq!(result.err(), Some(IntakeError::SourceUnavailable));
}

#[test]
fn searchable_ancestor_does_not_require_directory_read_permission() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::os::unix::process::CommandExt;
    use std::process::Command;
    const CHILD_PATH: &str = "ONT_PROGRAM_SYNTHETIC_SEARCH_ONLY_CHILD";
    if let Some(path) = std::env::var_os(CHILD_PATH) {
        let path = Path::new(&path);
        assert_eq!(std::fs::read(path).unwrap(), b"synthetic safe bytes");
        assert_eq!(read(path).unwrap(), b"synthetic safe bytes");
        return;
    }
    let t = Temporary::new();
    let directory = t.path("search-only");
    std::fs::set_permissions(&t.0, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::create_dir(&directory).unwrap();
    let source = directory.join("source");
    std::fs::write(&source, b"synthetic safe bytes").unwrap();
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o644)).unwrap();
    let owner = std::fs::metadata(&source).unwrap().uid();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o111)).unwrap();
    let probe = t.path("probe");
    std::fs::copy(std::env::current_exe().unwrap(), &probe).unwrap();
    std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut command = Command::new(&probe);
    command
        .args([
            "--exact",
            "program::source_tests::searchable_ancestor_does_not_require_directory_read_permission",
        ])
        .env(CHILD_PATH, &source);
    // Root bypasses DAC; drop only this synthetic child to an unprivileged UID.
    if owner == 0 {
        command.gid(65534).uid(65534);
    }
    let output = command.output();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755)).unwrap();
    let output = output.unwrap();
    assert!(
        output.status.success(),
        "synthetic search-only child failed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}
