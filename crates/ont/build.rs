use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn run_git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

fn git_dir_for(manifest_dir: &Path) -> Option<PathBuf> {
    let mut current = Some(manifest_dir);
    while let Some(dir) = current {
        let candidate = dir.join(".git");
        if candidate.is_file() {
            let text = fs::read_to_string(&candidate).ok()?;
            for line in text.lines() {
                if let Some(rest) = line.trim().strip_prefix("gitdir:") {
                    let rest = rest.trim();
                    if rest.is_empty() {
                        return None;
                    }
                    let path = PathBuf::from(rest);
                    return Some(if path.is_absolute() {
                        path
                    } else {
                        dir.join(path)
                    });
                }
            }
            return None;
        }
        if candidate.is_dir() {
            return Some(candidate);
        }
        current = dir.parent();
    }
    None
}

fn emit_rerun(manifest_dir: &Path) {
    let workspace_root = run_git(manifest_dir, &["rev-parse", "--show-toplevel"])
        .map(|text| PathBuf::from(text.trim()))
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| {
            manifest_dir
                .parent()
                .and_then(|parent| parent.parent())
                .map(|path| path.to_path_buf())
                .unwrap_or_else(|| manifest_dir.to_path_buf())
        });
    println!(
        "cargo:rerun-if-changed={}",
        workspace_root.join("Cargo.toml").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        workspace_root.join("Cargo.lock").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        workspace_root.join("crates").display()
    );
    let Some(git_dir) = git_dir_for(manifest_dir) else {
        return;
    };
    let head_path = git_dir.join("HEAD");
    println!("cargo:rerun-if-changed={}", head_path.display());
    let Ok(head) = fs::read_to_string(&head_path) else {
        return;
    };
    let Some(reference) = head.trim().strip_prefix("ref:") else {
        return;
    };
    let reference = reference.trim();
    if reference.is_empty() {
        return;
    }
    let ref_path = match fs::read_to_string(git_dir.join("commondir")) {
        Ok(common) => {
            let common = common.trim();
            if common.is_empty() {
                git_dir.join(reference)
            } else {
                let base = PathBuf::from(common);
                let base = if base.is_absolute() {
                    base
                } else {
                    git_dir.join(base)
                };
                let candidate = base.join(reference);
                if candidate.exists() {
                    candidate
                } else {
                    git_dir.join(reference)
                }
            }
        }
        Err(_) => git_dir.join(reference),
    };
    println!("cargo:rerun-if-changed={}", ref_path.display());
}

fn build_revision(manifest_dir: &Path) -> String {
    let repo_root = run_git(manifest_dir, &["rev-parse", "--show-toplevel"])
        .map(|text| PathBuf::from(text.trim()))
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| manifest_dir.to_path_buf());
    let Some(commit) = run_git(&repo_root, &["rev-parse", "HEAD"]) else {
        return "unknown".to_string();
    };
    let commit = commit.trim().to_ascii_lowercase();
    if commit.len() != 40 || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
        return "unknown".to_string();
    }
    let dirty =
        run_git(&repo_root, &["status", "--porcelain"]).is_some_and(|text| !text.trim().is_empty());
    if dirty {
        format!("{commit}-dirty")
    } else {
        commit
    }
}

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    if !manifest_dir.as_os_str().is_empty() {
        emit_rerun(&manifest_dir);
        println!(
            "cargo:rustc-env=ONT_BUILD_REVISION={}",
            build_revision(&manifest_dir)
        );
    } else {
        println!("cargo:rustc-env=ONT_BUILD_REVISION=unknown");
    }
}
