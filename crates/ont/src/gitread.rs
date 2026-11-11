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

/// Return a remote only when the credential boundary is certain; None makes
/// repository_of record the checkout path. Scp-like userinfo requires exactly
/// one @ and no / before the first colon; a colon before the first @ is
/// ambiguous and returns None, rather than guessing a host or exposing a password.
pub fn strip_credentials(url: &str) -> Option<String> {
    if let Some(scheme) = url.find("://") {
        let prefix_end = scheme + 3;
        let rest = &url[prefix_end..];
        let end = rest.find('/').unwrap_or(rest.len());
        let authority = &rest[..end];
        let path = &rest[end..];
        if let Some(at) = authority.rfind('@') {
            // A colon-bearing userinfo plus a later @ could instead be a
            // password spanning raw slashes: the remaining host is uncertain.
            if authority[at + 1..].is_empty()
                || (authority[..at].contains(':') && path.contains('@'))
            {
                return None;
            }
            return Some(format!("{}{}", &url[..prefix_end], &rest[at + 1..]));
        }
        if !authority.starts_with('[') && authority.contains(':') && path.contains('@') {
            return None;
        }
    } else if !url.starts_with('/')
        && !url.starts_with("./")
        && !url.starts_with("../")
        && let Some(first_at) = url.find('@')
        && let Some(first_colon) = url.find(':')
    {
        if first_colon < first_at {
            return None;
        }
        let host_part = &url[..first_colon];
        if host_part.contains('/')
            || host_part.contains('\\')
            || host_part.bytes().filter(|b| *b == b'@').count() != 1
        {
            return None;
        }
        return Some(url[first_at + 1..].to_string());
    }
    Some(url.to_string())
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
        if !url.is_empty()
            && let Some(remote) = strip_credentials(&url)
        {
            return remote;
        }
    }
    checkout.display().to_string()
}

/// Read `<commit>:<path>` from `checkout`. Refusals, in order:
/// `git_unavailable`, `invalid_request` (commit or path),
/// `commit_not_merged`, `schema_path_not_found`.
pub fn read_schema(checkout: &Path, commit: &str, path: &str) -> Result<GitSchema, Failure> {
    if !valid_commit(commit) {
        return Err(Refusal::input(
            "invalid_request",
            "--commit",
            "40 lowercase hexadecimal characters",
        )
        .into());
    }
    if !valid_repo_path(path) {
        return Err(Refusal::input(
            "invalid_request",
            "--path",
            "nonempty relative repository path without a parent (..) component",
        )
        .into());
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
        Err(GitFail::TooLarge) => {
            return Err(Refusal::input(
                "invalid_request",
                "--path",
                "schema file at most 16777216 bytes",
            )
            .into());
        }
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
    fn ambiguous_credentials_are_not_returned_to_future_callers() {
        for remote in [
            "https://u:12/34@h/x",
            "https://u:p/w@x/y@h/z",
            "user:p@w/x@host:path",
            "user:pass@host:path",
        ] {
            assert_eq!(
                serde_json::to_value(strip_credentials(remote))
                    .unwrap_or(serde_json::Value::Bool(false)),
                serde_json::Value::Null
            );
        }
    }
    #[test]
    fn credentials_are_stripped_and_paths_checked() {
        for (remote, stripped) in [
            (
                "https://user:secret@example.com/a/b.git",
                Some("https://example.com/a/b.git"),
            ),
            (
                "https://example.com/a/b.git",
                Some("https://example.com/a/b.git"),
            ),
            ("git@example.com:a/b.git", Some("example.com:a/b.git")),
            ("user:pass@host:path", None),
            ("user:pa/ss@host:path", None),
            ("https://host/path@name", Some("https://host/path@name")),
            ("https://TOKEN@host/o/r@x", Some("https://host/o/r@x")),
            ("user:pw@host:org/re@po", None),
            ("https://host:443/p@n", None),
            ("https://user:p@ss@host/x", Some("https://host/x")),
            ("ssh://user:pw@host:22/x", Some("ssh://host:22/x")),
            ("user:p@ss@host:org/re@po", None),
            ("https://[::1]:443/p@n", Some("https://[::1]:443/p@n")),
            ("https://u:12/34@h/x", None),
            ("https://u:12/34@h", None),
            ("https://host:443/p@n/x", None),
            ("./a@b/c:d", Some("./a@b/c:d")),
            ("/a@b/c:d", Some("/a@b/c:d")),
            ("git@host:path", Some("host:path")),
            ("ssh://user@host:22/x", Some("ssh://host:22/x")),
            ("https://host:443/x", Some("https://host:443/x")),
            ("ssh://user:password@[::1]:22/x", Some("ssh://[::1]:22/x")),
        ] {
            assert_eq!(
                strip_credentials(remote).as_deref(),
                stripped,
                "remote: {remote}"
            );
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
