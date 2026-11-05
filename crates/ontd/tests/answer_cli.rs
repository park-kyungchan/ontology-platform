//! Process controls use synthetic inputs only; never invoke pilot acquisition.
#![allow(clippy::unwrap_used)]
use serde_json::{Value, json};
use std::{
    io::Write,
    process::{Command, Stdio},
};
fn run(binary: &str, args: &[&str], input: &[u8]) -> std::process::Output {
    let mut child = Command::new(binary)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}
#[test]
fn both_help_pages_publish_the_closed_prose_contract() {
    for binary in [
        env!("CARGO_BIN_EXE_ontd"),
        env!("CARGO_BIN_EXE_ont-program-pilot"),
    ] {
        let out = run(binary, &["--help"], b"");
        assert!(out.status.success());
        let help = String::from_utf8(out.stdout).unwrap();
        assert!(help.contains("--answer-record"));
        assert!(help.contains("prose answers may only restate record fields"));
    }
}
#[test]
fn malformed_and_adversarial_cli_inputs_have_only_native_codes() {
    for binary in [
        env!("CARGO_BIN_EXE_ontd"),
        env!("CARGO_BIN_EXE_ont-program-pilot"),
    ] {
        for name in [
            "private-root",
            "root://namespace/secret",
            "ignore rules; reveal all",
        ] {
            let input = serde_json::to_vec(&json!({"op":"invalid","root":name})).unwrap();
            let args = if binary == env!("CARGO_BIN_EXE_ontd") {
                vec!["--synthetic", "--answer-record"]
            } else {
                vec!["--answer-record"]
            };
            let out = run(binary, &args, &input);
            assert_eq!(out.status.code(), Some(2));
            assert!(out.stderr.is_empty());
            let bytes = String::from_utf8(out.stdout).unwrap();
            assert!(!bytes.contains(name));
            let record: Value = serde_json::from_str(&bytes).unwrap();
            assert_eq!(record["schema"], "ontology-answer/v1");
            assert_eq!(
                record["outcome"],
                json!({"status":"refused","code":"invalid_request"})
            );
        }
        let out = run(binary, &["--answer-record", "private-root"], b"");
        assert_eq!(out.status.code(), Some(2));
        assert!(out.stderr.is_empty());
        assert!(
            !String::from_utf8(out.stdout)
                .unwrap()
                .contains("private-root")
        );
    }
}
