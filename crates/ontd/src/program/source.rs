//! Descriptor-relative source opens. Path checks never precede an unguarded open.
use super::{IntakeError, Result};
use rustix::fs::{FileType, Mode, OFlags, fstat, open, openat};
use std::ffi::OsStr;
use std::fs::File;
use std::path::{Component, Path};

pub(super) fn open_file(path: &str) -> Result<File> {
    walk(path, |_| {})
}

// The no-op production hook also permits deterministic test replacements exactly
// before an actual component open, without timing-dependent racing threads.
fn walk(path: &str, mut before_component: impl FnMut(&OsStr)) -> Result<File> {
    let mut components = Path::new(path).components();
    if components.next() != Some(Component::RootDir) {
        return Err(IntakeError::SourceUnavailable);
    }
    let names: Vec<_> = components
        .map(|component| match component {
            Component::Normal(name) => Ok(name),
            _ => Err(IntakeError::SourceUnavailable),
        })
        .collect::<Result<_>>()?;
    if names.is_empty() {
        return Err(IntakeError::SourceUnavailable);
    }
    // O_PATH preserves the existing search-only ancestor permission contract;
    // O_DIRECTORY still rejects a symlink opened with PATH|NOFOLLOW.
    let directory_flags =
        OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    // '/' is the trusted namespace boundary. Each subsequent open receives just
    // one basename and the descriptor for the previously opened real directory.
    let mut directory =
        open("/", directory_flags, Mode::empty()).map_err(|_| IntakeError::SourceUnavailable)?;
    for (index, name) in names.iter().enumerate() {
        before_component(name);
        let last = index == names.len() - 1;
        let flags = if last {
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC
        } else {
            directory_flags
        };
        let child = openat(&directory, *name, flags, Mode::empty())
            .map_err(|_| IntakeError::SourceUnavailable)?;
        if last {
            let stat = fstat(&child).map_err(|_| IntakeError::SourceUnavailable)?;
            if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
                return Err(IntakeError::SourceUnavailable);
            }
            return Ok(File::from(child));
        }
        directory = child;
    }
    Err(IntakeError::SourceUnavailable)
}

#[cfg(test)]
pub(super) fn open_with_hook(path: &str, before_component: impl FnMut(&OsStr)) -> Result<File> {
    walk(path, before_component)
}
