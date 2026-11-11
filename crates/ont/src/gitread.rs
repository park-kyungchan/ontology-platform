//! Reading one schema file from a merged commit with `/usr/bin/git`:
//! argv only, no shell, a cleared environment, bounded stdout and wait.
use crate::{Failure, Refusal};
use std::io::Read;
use std::path::{Component, Path};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const GIT: &str = "/usr/bin/git";
/// The bound on schema bytes read from git (the source-file bound `index`
/// uses for one file).
pub const MAX_SCHEMA_BYTES: usize = 16 * 1024 * 1024;
const WAIT: Duration = Duration::from_secs(10);

pub struct GitSchema {
    pub bytes: Vec<u8>,
    pub repository: String,
}

pub fn valid_commit(commit: &str) -> bool {
    commit.len() == 40
        && commit
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

pub fn valid_repo_path(path: &str) -> bool {
    if path.is_empty() || Path::new(path).is_absolute() {
        return false;
    }
    !Path::new(path)
        .components()
        .any(|c| matches!(c, Component::ParentDir))
}

/// Remove any userinfo (a credential part) from a remote URL.
pub fn strip_credentials(url: &str) -> String {
    if let Some(scheme) = url.find("://") {
        let prefix_end = scheme + 3;
        let rest = &url[prefix_end..];
        let end = rest.find('/').unwrap_or(rest.len());
        let authority = &rest[..end];
        let at = authority.rfind('@').or_else(|| {
            // Recover malformed raw-slash passwords, while leaving a host
            // with a numeric port (and @ in its path) untouched.
            let (_, password) = authority.split_once(':')?;
            if authority.starts_with('[') || password.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let first_at = rest.find('@')?;
            let host_end = rest[first_at..]
                .find('/')
                .map_or(rest.len(), |i| first_at + i);
            rest[..host_end].rfind('@')
        });
        if let Some(at) = at {
            return format!("{}{}", &url[..prefix_end], &rest[at + 1..]);
        }
    } else if let Some(first_at) = url.find('@') {
        // The host separator follows userinfo; @ in the repository path
        // belongs to the path, not to the password.
        if let Some(colon) = url[first_at..].find(':') {
            let host_end = first_at + colon;
            if let Some(at) = url[..host_end].rfind('@') {
                return url[at + 1..].to_string();
            }
        }
    }
    url.to_string()
}

struct GitOut {
    success: bool,
    code: Option<i32>,
    stdout: Vec<u8>,
}

enum GitFail {
    Unavailable,
    TooLarge,
}

impl GitFail {
    fn into_failure(self) -> Failure {
        Refusal::new("git_unavailable").into()
    }
}

fn run_git(checkout: &Path, args: &[&str], limit: usize) -> Result<GitOut, GitFail> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::metadata(GIT).map_err(|_| GitFail::Unavailable)?;
    if !meta.is_file() || meta.permissions().mode() & 0o111 == 0 {
        return Err(GitFail::Unavailable);
    }
    let mut cmd = Command::new(GIT);
    cmd.arg("-C").arg(checkout).args(args);
    cmd.env_clear();
    cmd.env("PATH", "/usr/bin:/bin");
    if let Some(home) = std::env::var_os("HOME") {
        cmd.env("HOME", home);
    }
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.stdin(Stdio::null()).stderr(Stdio::null());
    cmd.stdout(Stdio::piped());
    let mut child = cmd.spawn().map_err(|_| GitFail::Unavailable)?;
    let pipe = child.stdout.take().ok_or(GitFail::Unavailable)?;
    let reader = std::thread::spawn(move || -> Vec<u8> {
        let mut buf = Vec::new();
        let _ = pipe.take(limit as u64 + 1).read_to_end(&mut buf);
        buf
    });
    let deadline = Instant::now() + WAIT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = reader.join();
                    return Err(GitFail::Unavailable);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(_) => return Err(GitFail::Unavailable),
        }
    };
    let stdout = reader.join().map_err(|_| GitFail::Unavailable)?;
    if stdout.len() > limit {
        return Err(GitFail::TooLarge);
    }
    Ok(GitOut {
        success: status.success(),
        code: status.code(),
        stdout,
    })
}

fn repository_of(checkout: &Path) -> String {
    if let Ok(out) = run_git(checkout, &["config", "--get", "remote.origin.url"], 4096)
        && out.success
    {
        let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !url.is_empty() {
            return strip_credentials(&url);
        }
    }
    checkout.display().to_string()
}

/// Read `<commit>:<path>` from `checkout`. Refusals, in order:
/// `git_unavailable`, `invalid_request` (commit or path),
/// `commit_not_merged`, `schema_path_not_found`.
pub fn read_schema(checkout: &Path, commit: &str, path: &str) -> Result<GitSchema, Failure> {
    if !valid_commit(commit) {
        return Err(Refusal::new("invalid_request").into());
    }
    if !valid_repo_path(path) {
        return Err(Refusal::new("invalid_request").into());
    }
    let ancestor = run_git(
        checkout,
        &[
            "merge-base",
            "--is-ancestor",
            commit,
            "refs/remotes/origin/main",
        ],
        4096,
    )
    .map_err(GitFail::into_failure)?;
    if !ancestor.success {
        return Err(Refusal::new(if ancestor.code == Some(1) {
            "commit_not_merged"
        } else {
            "git_unavailable"
        })
        .into());
    }
    let shown = match run_git(
        checkout,
        &["show", &format!("{commit}:{path}")],
        MAX_SCHEMA_BYTES,
    ) {
        Ok(o) => o,
        Err(GitFail::TooLarge) => return Err(Refusal::new("invalid_request").into()),
        Err(e) => return Err(e.into_failure()),
    };
    if !shown.success {
        return Err(Refusal::new("schema_path_not_found").into());
    }
    Ok(GitSchema {
        bytes: shown.stdout,
        repository: repository_of(checkout),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credentials_are_stripped_and_paths_checked() {
        for (remote, stripped) in [
            (
                "https://user:secret@example.com/a/b.git",
                "https://example.com/a/b.git",
            ),
            ("https://example.com/a/b.git", "https://example.com/a/b.git"),
            ("git@example.com:a/b.git", "example.com:a/b.git"),
            ("user:pass@host:path", "host:path"),
            ("https://user:pa/ss@host/path", "https://host/path"),
            ("user:pa/ss@host:path", "host:path"),
            ("https://host/path@name", "https://host/path@name"),
            ("https://TOKEN@host/o/r@x", "https://host/o/r@x"),
            ("user:pw@host:org/re@po", "host:org/re@po"),
            ("https://host:443/p@n", "https://host:443/p@n"),
            ("https://user:p@ss@host/x", "https://host/x"),
            ("ssh://user:pw@host:22/x", "ssh://host:22/x"),
            ("user:p@ss@host:org/re@po", "host:org/re@po"),
            ("https://user:pa/ss@host/o/r@x", "https://host/o/r@x"),
            ("https://[::1]:443/p@n", "https://[::1]:443/p@n"),
        ] {
            assert_eq!(strip_credentials(remote), stripped, "remote: {remote}");
        }
        assert!(valid_commit("843195b82f8041edcfc585aa8198bd4dc1558b3c"));
        assert!(!valid_commit("843195B82F8041EDCFC585AA8198BD4DC1558B3C"));
        assert!(!valid_commit("abc"));
        assert!(valid_repo_path("ontology/work.schema.json"));
        assert!(!valid_repo_path("/ontology/work.schema.json"));
        assert!(!valid_repo_path("ontology/../work.schema.json"));
        assert!(!valid_repo_path("../work.schema.json"));
        assert!(!valid_repo_path(""));
    }
}
