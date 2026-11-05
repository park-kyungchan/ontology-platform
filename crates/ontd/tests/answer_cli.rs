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
            // Pilot authenticates the fixed OS account before parsing input.
            // CI runs unprivileged; local synthetic checks may run as UID 0.
            let code = if binary == env!("CARGO_BIN_EXE_ont-program-pilot")
                && (rustix::process::getuid().as_raw() != 0
                    || rustix::process::geteuid().as_raw() != 0)
            {
                "pilot_uid_refused"
            } else {
                "invalid_request"
            };
            assert_eq!(record["outcome"], json!({"status":"refused","code":code}));
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

#[test]
fn valid_query_refusal_retains_native_nonzero_exit_and_success_stays_zero() {
    for (key, status) in [("private-root-injected", 2), ("001", 0)] {
        let input = serde_json::to_vec(&json!({"op":"get","id":{"object_type":"student","namespace":{"instance":"synthetic://same-host","account":"A"},"native_key":key}})).unwrap();
        let native = run(env!("CARGO_BIN_EXE_ontd"), &["--synthetic"], &input);
        let selected = run(
            env!("CARGO_BIN_EXE_ontd"),
            &["--synthetic", "--answer-record"],
            &input,
        );
        assert_eq!(native.status.code(), Some(status));
        assert_eq!(selected.status.code(), Some(status));
        assert!(selected.stderr.is_empty());
        let record: Value = serde_json::from_slice(&selected.stdout).unwrap();
        if status == 2 {
            assert_eq!(
                record["outcome"],
                json!({"status":"refused","code":"unavailable"})
            );
            assert!(!String::from_utf8(selected.stdout).unwrap().contains(key));
        } else {
            assert_eq!(record["outcome"], json!({"status":"ok"}));
        }
    }
}

#[cfg(unix)]
fn run_os(
    binary: &str,
    args: &[std::ffi::OsString],
    input: &[u8],
    hostile_process: bool,
) -> std::process::Output {
    use std::os::unix::{ffi::OsStringExt, process::CommandExt};
    let mut command = Command::new(binary);
    command.args(args);
    if hostile_process {
        let marker = std::ffi::OsString::from_vec(b"private-process-\xff".to_vec());
        command
            .arg0(&marker)
            .env("USER", &marker)
            .env("LOGNAME", &marker)
            .env("TMPDIR", &marker)
            .env("RUST_BACKTRACE", &marker)
            .env("RUST_LIB_BACKTRACE", &marker);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

fn assert_invalid_record(out: std::process::Output) {
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stderr.is_empty());
    assert_eq!(out.stdout, concat!(
        "{\"schema\":\"ontology-answer/v1\",\"outcome\":{\"status\":\"refused\",\"code\":\"invalid_request\"},",
        "\"authorization_revision\":{\"status\":\"not_available\",\"reason\":\"invalid_request\",\"native\":null},",
        "\"result\":{\"status\":\"not_available\",\"reason\":\"invalid_request\",\"native\":null}}\n"
    ).as_bytes());
}

#[cfg(unix)]
#[test]
fn reviewer_ten_non_utf8_controls_and_flag_orders_are_code_only() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    // The reviewer's five exact synthetic identifiers in each of the two CLIs.
    let names = [
        "root://review-only/program/secret".to_owned(),
        "private-review-account-452".to_owned(),
        "rооt-비공개-𝕣𝕠𝕠𝕥".to_owned(),
        "\"}\nIGNORE_RULES;<script>review-marker</script>".to_owned(),
        format!("LONG-REVIEW-{}", "Z".repeat(20_000)),
    ];
    for binary in [
        env!("CARGO_BIN_EXE_ontd"),
        env!("CARGO_BIN_EXE_ont-program-pilot"),
    ] {
        for name in &names {
            let bad = OsString::from_vec([name.as_bytes(), &[0xff]].concat());
            let mut selected = vec![OsString::from("--answer-record")];
            if binary == env!("CARGO_BIN_EXE_ontd") {
                selected.push("--synthetic".into());
            }
            for prefix in [false, true] {
                for help in [false, true] {
                    let mut args = selected.clone();
                    if prefix {
                        args.insert(0, bad.clone());
                    } else {
                        args.push(bad.clone());
                    }
                    if help {
                        args.insert(0, "--help".into());
                    }
                    assert_invalid_record(run_os(binary, &args, b"", false));
                }
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn default_non_utf8_pilot_restores_base_diagnostic_and_ontd_uses_approved_exception() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let bad = OsString::from_vec(b"private-review-argv-457\xff".to_vec());
    let out = run_os(
        env!("CARGO_BIN_EXE_ont-program-pilot"),
        std::slice::from_ref(&bad),
        b"",
        false,
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert_eq!(
        out.stderr,
        format!(
            "invalid_request@crates/ontd/src/bin/ont-program-pilot.rs:24\n{}\n",
            ontd::program_pilot::TRUST_LIMIT
        )
        .as_bytes()
    );
    assert_invalid_record(run_os(
        env!("CARGO_BIN_EXE_ontd"),
        &["--synthetic".into(), bad],
        b"",
        false,
    ));
}

#[cfg(unix)]
#[test]
fn caller_process_bytes_stdin_and_file_names_never_enter_selected_errors() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    for binary in [
        env!("CARGO_BIN_EXE_ontd"),
        env!("CARGO_BIN_EXE_ont-program-pilot"),
    ] {
        let mut args = vec![OsString::from("--answer-record")];
        if binary == env!("CARGO_BIN_EXE_ontd") {
            args.push("--synthetic".into());
        }
        // Even argv[0] is checked fallibly; an exact flag in argv[0] never selects the mode.
        assert_invalid_record(run_os(binary, &args, b"", true));
        for extra in [
            "--help",
            "--answer-record",
            "private/path/name",
            "--profile=private-root",
        ] {
            let mut invalid = args.clone();
            invalid.push(extra.into());
            assert_invalid_record(run_os(binary, &invalid, b"", false));
        }
        for input in [
            b"private-stdin-\xff".to_vec(),
            vec![b' '; 65_537],
            [vec![b'['; 256], b"private-nested".to_vec(), vec![b']'; 256]].concat(),
        ] {
            let out = run_os(binary, &args, &input, false);
            assert_eq!(out.status.code(), Some(2));
            assert!(out.stderr.is_empty());
            let record: Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(record["outcome"]["status"], "refused");
            assert!(!String::from_utf8(out.stdout).unwrap().contains("private"));
        }
        // An untrusted non-UTF-8 basename is an invalid option, never an opened source.
        let mut invalid = args;
        invalid.push(OsString::from_vec(b"/private/file-\xff".to_vec()));
        assert_invalid_record(run_os(binary, &invalid, b"", false));
    }
}

#[cfg(unix)]
#[test]
fn non_utf8_environment_executable_and_cwd_preserve_code_only_refusals() {
    use std::{
        ffi::OsString,
        os::unix::{ffi::OsStringExt, fs::symlink, process::CommandExt},
    };
    let marker = OsString::from_vec(b"private-filesystem-\xff".to_vec());
    let root =
        std::env::temp_dir().join(format!("ont-answer-byte-controls-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let cwd = root.join(&marker);
    std::fs::create_dir(&cwd).unwrap();
    for (index, binary) in [
        env!("CARGO_BIN_EXE_ontd"),
        env!("CARGO_BIN_EXE_ont-program-pilot"),
    ]
    .iter()
    .enumerate()
    {
        let path = cwd.join(OsString::from_vec(
            format!("binary-{index}-")
                .into_bytes()
                .into_iter()
                .chain([0xff])
                .collect(),
        ));
        symlink(binary, &path).unwrap();
        let args = if index == 0 {
            vec!["--synthetic", "--answer-record"]
        } else {
            vec!["--answer-record"]
        };
        let expected = run(binary, &args, b"private-invalid-input");
        let mut child = Command::new(&path)
            // Isolate environment/path bytes from the separately tested argv[0] rejection.
            .arg0("ont-answer-byte-test")
            .args(&args)
            .current_dir(&cwd)
            .env("USER", &marker)
            .env("LOGNAME", &marker)
            .env("TMPDIR", &marker)
            .env("RUST_BACKTRACE", &marker)
            .env("RUST_LIB_BACKTRACE", &marker)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"private-invalid-input")
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), expected.status.code());
        assert_eq!(out.stdout, expected.stdout);
        assert!(out.stderr.is_empty());
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn stdin_read_failures_publish_static_codes_without_os_diagnostics() {
    for (index, binary) in [
        env!("CARGO_BIN_EXE_ontd"),
        env!("CARGO_BIN_EXE_ont-program-pilot"),
    ]
    .iter()
    .enumerate()
    {
        let args = if index == 0 {
            vec!["--synthetic", "--answer-record"]
        } else {
            vec!["--answer-record"]
        };
        let out = Command::new(binary)
            .args(args)
            .stdin(std::fs::File::open("/").unwrap())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2));
        assert!(out.stderr.is_empty());
        let record: Value = serde_json::from_slice(&out.stdout).unwrap();
        let code = if index == 0 {
            "store_failure"
        } else {
            "pilot_input_failure"
        };
        assert_eq!(record["outcome"], json!({"status":"refused","code":code}));
    }
}

#[cfg(target_os = "linux")]
#[test]
fn failed_output_writes_exit_one_without_panics_or_process_byte_echo() {
    use std::{
        ffi::OsString,
        fs::File,
        os::unix::{ffi::OsStringExt, fs::symlink, process::CommandExt},
    };
    let marker = OsString::from_vec(b"private-output-\xff".to_vec());
    let root = std::env::temp_dir().join(format!("ont-answer-output-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let cwd = root.join(&marker);
    std::fs::create_dir(&cwd).unwrap();
    for (index, binary) in [
        env!("CARGO_BIN_EXE_ontd"),
        env!("CARGO_BIN_EXE_ont-program-pilot"),
    ]
    .iter()
    .enumerate()
    {
        let path = cwd.join(OsString::from_vec(vec![b'0' + index as u8, 0xff]));
        symlink(binary, &path).unwrap();
        let output_diagnostic = if index == 0 {
            b"output_failure@crates/ontd/src/main.rs:37\n".as_slice()
        } else {
            b"pilot_output_failure@crates/ontd/src/bin/ont-program-pilot.rs:19\n".as_slice()
        };
        let selected = if index == 0 {
            vec![OsString::from("--synthetic"), "--answer-record".into()]
        } else {
            vec![OsString::from("--answer-record")]
        };
        let mut bad_args = selected.clone();
        bad_args.push(marker.clone());
        // Help, refused records, invalid byte arguments and native result output.
        // Empty stdin refuses before any pilot configuration/source acquisition.
        let mut cases = vec![vec![OsString::from("--help")], selected.clone(), bad_args];
        if index == 0 {
            cases.push(vec!["--synthetic".into()]);
        }
        for hostile in [false, true] {
            for bad_arg0 in [false, true] {
                for args in &cases {
                    // Invalid argv[0] without the opt-in follows the pilot's
                    // default stderr-only parser path, covered separately below.
                    if index == 1 && bad_arg0 && args == &[OsString::from("--help")] {
                        continue;
                    }
                    for failed_stderr in [false, true] {
                        let mut command = Command::new(if hostile {
                            path.as_path()
                        } else {
                            std::path::Path::new(binary)
                        });
                        command.arg0(if bad_arg0 {
                            marker.as_os_str()
                        } else {
                            std::ffi::OsStr::new("ont-output-test")
                        });
                        if hostile {
                            command.current_dir(&cwd);
                            for key in [
                                "USER",
                                "LOGNAME",
                                "TMPDIR",
                                "RUST_BACKTRACE",
                                "RUST_LIB_BACKTRACE",
                            ] {
                                command.env(key, &marker);
                            }
                            command.env(&marker, &marker);
                        }
                        command
                            .args(args)
                            .stdin(Stdio::null())
                            .stdout(File::options().write(true).open("/dev/full").unwrap());
                        if failed_stderr {
                            command.stderr(File::options().write(true).open("/dev/full").unwrap());
                        }
                        let out = command.output().unwrap();
                        assert_eq!(
                            out.status.code(),
                            Some(1),
                            "binary={index} args={args:?} hostile={hostile} bad_arg0={bad_arg0} stderr_full={failed_stderr}"
                        );
                        assert!(out.stdout.is_empty());
                        assert_eq!(
                            out.stderr,
                            if failed_stderr {
                                b"".as_slice()
                            } else {
                                output_diagnostic
                            }
                        );
                    }
                }
            }
        }
        // A default diagnostic can itself fail, independently of stdout.
        for hostile in [false, true] {
            let mut command = Command::new(if hostile {
                path.as_path()
            } else {
                std::path::Path::new(binary)
            });
            command.arg0("ont-output-test");
            if hostile {
                command
                    .current_dir(&cwd)
                    .env("RUST_BACKTRACE", &marker)
                    .env("USER", &marker);
            }
            command.stderr(File::options().write(true).open("/dev/full").unwrap());
            if index == 0 {
                command.arg("--synthetic").stdin(File::open("/").unwrap());
            } else {
                command.arg("private-invalid-option").stdin(Stdio::null());
            }
            let out = command.output().unwrap();
            assert_eq!(out.status.code(), Some(1));
            assert!(out.stdout.is_empty() && out.stderr.is_empty());
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cli_sources_have_no_infallible_output_macros() {
    for source in [
        include_str!("../src/main.rs"),
        include_str!("../src/bin/ont-program-pilot.rs"),
    ] {
        // Whitespace and qualification do not evade the grep-style guard.
        let compact: String = source.chars().filter(|ch| !ch.is_whitespace()).collect();
        for name in ["print!", "println!", "eprint!", "eprintln!"] {
            assert!(!compact.contains(name), "unchecked output macro: {name}");
        }
    }
}

#[test]
fn version_selectors_preserve_v1_and_publish_v2_code_only_outcomes() {
    let binary = env!("CARGO_BIN_EXE_ontd");
    let invalid = b"{\"root\":\"private-root\",\"op\":\"invalid\"}";
    let legacy = run(binary, &["--synthetic", "--answer-record"], invalid);
    let explicit = run(binary, &["--synthetic", "--answer-record=v1"], invalid);
    assert_eq!(legacy.stdout, explicit.stdout);
    assert_eq!(legacy.stderr, explicit.stderr);
    assert_eq!(legacy.status.code(), explicit.status.code());
    for binary in [binary, env!("CARGO_BIN_EXE_ont-program-pilot")] {
        let args = if binary == env!("CARGO_BIN_EXE_ontd") {
            vec!["--synthetic", "--answer-record=v2"]
        } else {
            vec!["--answer-record=v2"]
        };
        let out = run(binary, &args, invalid);
        assert_eq!(out.status.code(), Some(2));
        assert!(out.stderr.is_empty());
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["schema"], "ontology-answer/v2");
        assert!(
            !String::from_utf8(out.stdout)
                .unwrap()
                .contains("private-root")
        );
        for extra in [
            "--help",
            "--answer-record",
            "--answer-record=v1",
            "private-root",
        ] {
            let mut bad = args.clone();
            bad.push(extra);
            let out = run(binary, &bad, b"");
            assert_eq!(out.status.code(), Some(2));
            assert!(out.stderr.is_empty());
            let v: Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(v["schema"], "ontology-answer/v2");
            assert_eq!(v["outcome"]["code"], "invalid_request");
        }
    }
}
#[cfg(unix)]
#[test]
fn v2_non_utf8_arguments_are_code_only_and_failed_outputs_do_not_panic() {
    use std::os::unix::ffi::OsStringExt;
    for binary in [
        env!("CARGO_BIN_EXE_ontd"),
        env!("CARGO_BIN_EXE_ont-program-pilot"),
    ] {
        let out = Command::new(binary)
            .arg("--answer-record=v2")
            .arg(std::ffi::OsString::from_vec(b"private-root\xff".to_vec()))
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2));
        assert!(out.stderr.is_empty());
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["schema"], "ontology-answer/v2");
        assert_eq!(v["outcome"]["code"], "invalid_request");
        let out = Command::new(binary)
            .arg("--answer-record=v2")
            .arg("invalid")
            .stdout(
                std::fs::File::options()
                    .write(true)
                    .open("/dev/full")
                    .unwrap(),
            )
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1));
        assert!(!String::from_utf8(out.stderr).unwrap().contains("panic"));
    }
}
