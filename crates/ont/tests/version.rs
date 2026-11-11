#![allow(clippy::unwrap_used)]
use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ont"))
        .args(args)
        .output()
        .unwrap()
}

fn assert_version_line(output: &std::process::Output) {
    assert_eq!(output.status.code(), Some(0), "stderr: {:?}", output.stderr);
    let text = String::from_utf8(output.stdout.clone()).unwrap();
    assert_eq!(text.matches('\n').count(), 1, "stdout: {text:?}");
    let line = text.strip_suffix('\n').unwrap();
    assert!(!line.contains('\r'), "stdout: {text:?}");
    let parts: Vec<&str> = line.split(' ').collect();
    assert_eq!(parts.len(), 3, "line: {line:?}");
    assert_eq!(parts[0], "ont", "line: {line:?}");
    assert!(!parts[1].is_empty(), "line: {line:?}");
    let revision = parts[2];
    if revision != "unknown" {
        let hex = revision.strip_suffix("-dirty").unwrap_or(revision);
        assert_eq!(hex.len(), 40, "line: {line:?}");
        assert!(
            hex.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "line: {line:?}"
        );
    }
}

#[test]
fn version_flag_prints_build_revision() {
    assert_version_line(&run(&["--version"]));
}

#[test]
fn version_revision_matches_git_head_when_available() {
    let git = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    if !git.status.success() {
        return;
    }
    let Ok(head) = String::from_utf8(git.stdout) else {
        return;
    };
    let head = head.trim().to_ascii_lowercase();
    if head.len() != 40 || !head.chars().all(|c| c.is_ascii_hexdigit()) {
        return;
    }
    let output = run(&["--version"]);
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8(output.stdout).unwrap();
    let line = text.strip_suffix('\n').unwrap();
    let revision = line.split(' ').nth(2).unwrap();
    let revision = revision.strip_suffix("-dirty").unwrap_or(revision);
    assert_eq!(revision, head, "line: {line:?}");
}

#[test]
fn spawned_describe_refuses_a_non_root_principal() {
    // /proc/self/status: "Uid: real, effective, ...".
    let text = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let fields = text
        .lines()
        .find(|l| l.starts_with("Uid:"))
        .map(|l| {
            l[4..]
                .split_whitespace()
                .filter_map(|f| f.parse::<u32>().ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let (real, effective) = (
        fields.first().copied().unwrap_or(0),
        fields.get(1).copied().unwrap_or(0),
    );
    if real == 0 && effective == 0 {
        return; // The refusal is covered at library level in cli.rs.
    }
    let output = Command::new(env!("CARGO_BIN_EXE_ont"))
        .args(["--store", "/nonexistent/store.sqlite", "describe"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(v["code"], "principal_not_trusted");
}
