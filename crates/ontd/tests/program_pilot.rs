#![allow(clippy::unwrap_used)]
//! Process refusal controls only: never invoke acquisition with a valid query.
use std::io::Write;
use std::process::{Command, Stdio};
#[test]
fn untrusted_cli_inputs_do_not_reach_fixed_sources_or_live_transport() {
    for input in [
        b"{}".as_slice(),
        br#"{"op":"program_report","root":{},"principal":"root"}"#,
        br#"{"op":"get","id":{}}"#,
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ont-program-pilot"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.starts_with("invalid_request@") || stderr.starts_with("pilot_uid_refused@"));
        assert!(stderr.contains(ontd::program_pilot::TRUST_LIMIT));
    }
    let output = Command::new(env!("CARGO_BIN_EXE_ont-program-pilot"))
        .args(["--profile", "root"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .starts_with("invalid_request@")
    );
}
