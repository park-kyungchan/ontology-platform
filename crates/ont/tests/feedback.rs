//! O17 feedback contracts. All stores/files are disposable synthetic fixtures.
#![allow(clippy::unwrap_used)]
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let t = Self(std::env::temp_dir().join(format!(
            "ont-feedback-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )));
        std::fs::create_dir(&t.0).unwrap();
        std::fs::write(
            t.0.join("schema.json"),
            include_bytes!("fixtures/render-board.json"),
        )
        .unwrap();
        assert_eq!(
            t.cli(
                &[
                    "schema",
                    "install",
                    t.0.join("schema.json").to_str().unwrap()
                ],
                json!({})
            )
            .0,
            0
        );
        t
    }
    fn cli(&self, command: &[&str], input: Value) -> (i32, Value) {
        let mut args = vec![
            "--store".into(),
            self.0.join("store.sqlite").display().to_string(),
            "--now".into(),
            "1000".into(),
        ];
        args.extend(command.iter().map(|s| s.to_string()));
        let out = ont::run(&args, input.to_string().as_bytes(), 0, 0);
        assert!(out.stderr.is_empty(), "{}", out.stderr);
        (out.code, serde_json::from_str(&out.stdout).unwrap())
    }
    fn mcp(&self, tool: &str, input: Value) -> Value {
        let config = ont::mcp::Config {
            store: self.0.join("store.sqlite"),
            now: Some(1000),
            client: "test".into(),
        };
        let mut server = ont::mcp::Server::new(config, &ont::cli::execute);
        let answer = server.handle_line(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":tool,"arguments":input}}).to_string().as_bytes()).unwrap();
        assert_eq!(answer["result"]["isError"], true, "{answer}");
        serde_json::from_str(answer["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn diagnostic(answer: &Value, code: &str, field: &str) {
    assert_eq!(answer["status"], "refused", "{answer}");
    assert_eq!(answer["code"], code, "{answer}");
    assert_eq!(answer["field"], field, "{answer}");
    assert!(
        answer["rule"].as_str().is_some_and(|s| !s.is_empty()),
        "{answer}"
    );
}

#[test]
fn every_help_form_is_json_without_stdin_or_a_store() {
    for args in [vec!["help"], vec!["--help"], vec!["-h"]] {
        let out = ont::run(
            &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            &b"not json"[..],
            1000,
            1000,
        );
        assert_eq!(out.code, 0, "{}", out.stdout);
        assert!(out.stderr.is_empty());
        assert!(out.stdout.len() < 6000);
        let body: Value = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(body["status"], "ok");
        for command in body["commands"].as_array().unwrap() {
            let name = command["command"].as_str().unwrap();
            let mut args: Vec<String> = name.split_whitespace().map(str::to_string).collect();
            args.push("--help".into());
            let out = ont::run(&args, &b""[..], 1000, 1000);
            assert_eq!(out.code, 0, "{}", out.stdout);
            let sub: Value = serde_json::from_str(&out.stdout).unwrap();
            assert_eq!(sub["commands"].as_array().unwrap(), &vec![command.clone()]);
        }
    }
    // Exercise the executable, including its stdout/exit plumbing.
    for args in [
        &["help"][..],
        &["--help"],
        &["-h"],
        &["render", "--help"],
        &["action", "apply", "--help"],
    ] {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_ont"))
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(out.status.success());
        assert!(out.stderr.is_empty());
        assert_eq!(
            serde_json::from_slice::<Value>(&out.stdout).unwrap()["status"],
            "ok"
        );
    }
}

#[test]
fn invalid_request_paths_missing_unknown_wrong_type_and_op_cli_and_mcp() {
    let t = Temp::new();
    let id = json!({"object_type":"item","namespace":{"instance":"test","account":"root"},"native_key":"x"});
    let mut nested = id.clone();
    nested["namespace"]["instance"] = json!(false);
    let adversarial = json!({"id":"secret, expected private-value"});
    let (exit, answer) = t.cli(&["get"], adversarial.clone());
    assert_eq!(exit, 2);
    diagnostic(&answer, "invalid_request", "/id");
    assert_eq!(t.mcp("ont_get", adversarial), answer);
    assert!(!answer.to_string().contains("private-value"));
    assert!(!answer.to_string().contains("secret"));
    let mut missing = id.clone();
    missing["namespace"]
        .as_object_mut()
        .unwrap()
        .remove("account");
    let mut unknown = id;
    unknown["a/b~c"] = json!("secret-value");
    for (request, field) in [
        (json!({}), "/id"),
        (json!({"id":nested}), "/id/namespace/instance"),
        (json!({"id":missing}), "/id/namespace/account"),
        (json!({"id":unknown}), "/id/a~1b~0c"),
        (json!({"op":"secret-value", "id":{}}), "/id/object_type"),
    ] {
        let (code, cli) = t.cli(&["get"], request.clone());
        assert_eq!(code, 2);
        diagnostic(&cli, "invalid_request", field);
        let mcp = t.mcp("ont_get", request);
        assert_eq!(mcp, cli);
        assert!(!cli.to_string().contains("secret-value"));
    }
    for (command, request, field) in [
        (vec!["describe"], json!({"op":"secret-value"}), "/op"),
        (
            vec!["render"],
            json!({"view":"items","args":{"x":false}}),
            "/args/x",
        ),
        (
            vec!["action", "apply"],
            json!({"action_type":"create_item","expected":[{"id":{},"revision":"x"}]}),
            "/expected/0/id/object_type",
        ),
        (
            vec!["search"],
            json!({"object_type":"item","where":{"eq":[false,"secret-value"]}}),
            "/where/eq/0",
        ),
    ] {
        let (code, answer) = t.cli(&command, request);
        assert_eq!(code, 2);
        diagnostic(&answer, "invalid_request", field);
        assert!(!answer.to_string().contains("secret-value"));
    }
}

#[test]
fn request_id_colon_and_parameter_refusals_name_the_field_without_value_cli_and_mcp() {
    let t = Temp::new();
    let invalid = json!({"action_type":"create_item","parameters":{"title":"secret-value","rank":1},"request_id":"secret:value"});
    for command in ["apply", "validate"] {
        let (code, cli) = t.cli(&["action", command], invalid.clone());
        assert_eq!(code, 2);
        diagnostic(&cli, "invalid_request", "/request_id");
        assert_eq!(
            cli["rule"],
            "1 to 64 ASCII characters: letters, digits, '.', '_', '-'"
        );
        let mcp = t.mcp(&format!("ont_action_{command}"), invalid.clone());
        // Existing MCP invalid_argument code is intentionally preserved.
        diagnostic(&mcp, "invalid_argument", "/request_id");
        assert_eq!(cli["rule"], mcp["rule"]);
        for answer in [cli, mcp] {
            assert!(!answer.to_string().contains("secret"), "{answer}");
        }
    }
    for (params, field) in [
        (json!({"title":123,"rank":1}), "/parameters/title"),
        (json!({"title":"secret-value"}), "/parameters/rank"),
        (
            json!({"title":"secret-value","rank":1,"extra":"secret-value"}),
            "/parameters/extra",
        ),
    ] {
        let request =
            json!({"action_type":"create_item","parameters":params,"request_id":"request-1"});
        let (code, cli) = t.cli(&["action", "apply"], request.clone());
        assert_eq!(code, 2);
        diagnostic(&cli, "invalid_parameter", field);
        assert_eq!(t.mcp("ont_action_apply", request), cli);
        assert!(!cli.to_string().contains("secret-value"));
    }
    assert!(!t.0.join("edits.jsonl").exists());
}

#[test]
fn invalid_argument_options_have_rules_and_unknown_inputs_have_help_hint() {
    let t = Temp::new();
    for (args, field) in [
        (vec!["--now", "secret-value", "get"], "--now"),
        (vec!["--source"], "--source"),
        (vec!["--unknown=secret-value"], "--unknown"),
        (vec!["secret-value"], "command"),
        (vec!["schema", "install", "--from", "/repo"], "--commit"),
    ] {
        let (code, answer) = t.cli(&args, json!({}));
        assert_eq!(code, 2);
        diagnostic(&answer, "invalid_argument", field);
        assert_eq!(answer["hint"], "ont help");
        assert!(!answer.to_string().contains("secret-value"));
    }
}

#[test]
fn filter_refusal_codes_have_fields_and_rules_cli_and_mcp() {
    let t = Temp::new();
    for (extra, code, field) in [
        (json!({"limit":0}), "invalid_request", "/limit"),
        (json!({"limit":1001}), "limit_too_large", "/limit"),
        (
            json!({"continuation":"secret-value"}),
            "invalid_continuation",
            "/continuation",
        ),
        (
            json!({"where":{"all":vec![json!({"eq":["rank",0]});65]}}),
            "filter_too_large",
            "/where",
        ),
        (
            json!({"where":{"eq":["no_such_property",0]}}),
            "undeclared_property",
            "/where",
        ),
        (
            json!({"where":{"prefix":["rank","secret-value"]}}),
            "invalid_filter_operator",
            "/where",
        ),
        (
            json!({"where":{"eq":["rank","secret-value"]}}),
            "wrong_value_type",
            "/where",
        ),
    ] {
        let mut request = json!({"object_type":"item"});
        request
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let (exit, cli) = t.cli(&["search"], request.clone());
        assert_eq!(exit, 2);
        diagnostic(&cli, code, field);
        assert_eq!(t.mcp("ont_search", request), cli);
        assert!(!cli.to_string().contains("secret-value"));
    }
    let answer = t.mcp("ont_index", json!({"sources":[]}));
    diagnostic(&answer, "invalid_request", "/sources");
    assert!(ont::mcp::tools_list().to_string().len() <= 5000);
}

#[test]
fn schema_and_render_argument_diagnostics_name_fields() {
    let t = Temp::new();
    let mut schema: Value =
        serde_json::from_slice(include_bytes!("fixtures/render-board.json")).unwrap();
    schema["format"] = json!("secret-value");
    let file = t.0.join("bad-schema.json");
    std::fs::write(&file, schema.to_string()).unwrap();
    let (code, answer) = t.cli(&["schema", "check", file.to_str().unwrap()], json!({}));
    assert_eq!(code, 2);
    diagnostic(&answer, "schema_violation", "/format");
    assert_eq!(answer["rule"], "must be ontology-schema/v1");
    assert!(!answer.to_string().contains("secret-value"));
    for input in [
        json!({"view":"named_items"}),
        json!({"view":"named_items","args":{"title":"secret value"}}),
    ] {
        let (code, answer) = t.cli(&["render"], input);
        assert_eq!(code, 2);
        diagnostic(&answer, "invalid_argument", "/args/title");
        assert!(!answer.to_string().contains("secret value"));
    }
    let (code, answer) = t.cli(&["--client", "secret:value", "action", "list"], json!({}));
    assert_eq!(code, 2);
    diagnostic(&answer, "invalid_request", "--client");
    assert!(!answer.to_string().contains("secret:value"));
    let request = json!({"action_type":"create_item","parameters":{"title":"secret-value","rank":1},"request_id":"valid-1","agent":"secret value"});
    let (code, answer) = t.cli(&["action", "apply"], request.clone());
    assert_eq!(code, 2);
    diagnostic(&answer, "invalid_request", "/agent");
    assert_eq!(t.mcp("ont_action_apply", request), answer);
    assert!(!answer.to_string().contains("secret"));
}

#[test]
fn tools_list_wire_answer_is_at_most_5000_bytes() {
    let config = ont::mcp::Config {
        store: PathBuf::from("/unused-o17-store"),
        now: Some(1000),
        client: "test".into(),
    };
    let mut server = ont::mcp::Server::new(config, &ont::cli::execute);
    let answer = server
        .handle_line(br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
        .unwrap();
    // Leave one byte for the trailing newline.
    assert!(answer.to_string().len() < 5000);
    assert_eq!(answer["result"]["tools"].as_array().unwrap().len(), 7);
    let index = answer["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "ont_index")
        .unwrap();
    assert!(
        index["description"]
            .as_str()
            .unwrap()
            .contains("append what changed")
    );
}

#[test]
fn unknown_lookup_codes_name_fields_and_do_not_echo_values_cli_and_mcp() {
    let t = Temp::new();
    for (command, tool, request, code, field) in [
        (
            vec!["describe"],
            "ont_describe",
            json!({"domain":"secret-value"}),
            "unknown_domain",
            "/domain",
        ),
        (
            vec!["action", "list"],
            "ont_action_list",
            json!({"domain":"secret-value"}),
            "unknown_domain",
            "/domain",
        ),
        (
            vec!["action", "apply"],
            "ont_action_apply",
            json!({"action_type":"secret-value","parameters":{},"request_id":"valid-1"}),
            "unknown_action",
            "/action_type",
        ),
        (
            vec!["action", "validate"],
            "ont_action_validate",
            json!({"action_type":"secret-value","parameters":{}}),
            "unknown_action",
            "/action_type",
        ),
    ] {
        let (exit, answer) = t.cli(&command, request.clone());
        assert_eq!(exit, 2);
        diagnostic(&answer, code, field);
        assert_eq!(t.mcp(tool, request), answer);
        assert!(!answer.to_string().contains("secret-value"));
    }
    let (exit, answer) = t.cli(&["index", "--source", "secret-value"], json!({}));
    assert_eq!(exit, 2);
    diagnostic(&answer, "unknown_source", "/sources");
    assert_eq!(
        t.mcp("ont_index", json!({"sources":["secret-value"]})),
        answer
    );
    assert!(!answer.to_string().contains("secret-value"));
}

#[test]
fn action_parameter_now_is_reserved_but_ordinary_name_is_valid() {
    let t = Temp::new();
    for name in ["now", "payload"] {
        let mut schema: Value =
            serde_json::from_slice(include_bytes!("fixtures/render-board.json")).unwrap();
        schema["action_types"]["create_item"]["parameters"][name] = json!({"type":"string"});
        let file = t.0.join("candidate.json");
        std::fs::write(&file, schema.to_string()).unwrap();
        let (exit, answer) = t.cli(&["schema", "check", file.to_str().unwrap()], json!({}));
        if name == "now" {
            assert_eq!(exit, 2);
            diagnostic(
                &answer,
                "schema_violation",
                "/action_types/create_item/parameters/now",
            );
            assert_eq!(answer["violations"].as_array().unwrap().len(), 1);
            assert_eq!(answer["violations"][0]["code"], "invalid_name");
            assert!(
                answer["rule"].as_str().unwrap().contains("now is reserved"),
                "{answer}"
            );
            assert!(
                answer["rule"]
                    .as_str()
                    .unwrap()
                    .contains("1 to 64 ASCII characters")
            );
        } else {
            assert_eq!(exit, 0, "{answer}");
            assert_eq!(answer["status"], "ok");
            assert_eq!(answer["violations"], json!([]));
        }
    }
}

#[test]
fn schema_reserved_and_fixed_vocabulary_rules_state_the_constraint() {
    let t = Temp::new();
    for (case, code, field, required_text) in [
        (
            "domain",
            "reserved_domain",
            "/domain",
            "program is a reserved domain",
        ),
        (
            "kind",
            "unknown_source_kind",
            "/sources/input/kind",
            "markdown_fields, markdown_table, file_digest, git_local, github_api, json_records or command",
        ),
        (
            "option",
            "invalid_source_option",
            "/sources/input/unlisted",
            "markdown_fields allows only glob, exclude, head_lines, complete_listing, fresh_for_s",
        ),
        (
            "transform",
            "invalid_transform",
            "/object_types/item/properties/title/transform",
            "first_word, parse_integer, {segment_after:string} or {replace_prefix:[from,to]}",
        ),
        (
            "type",
            "schema_shape",
            "/object_types/item/properties/title/type",
            "string, integer, boolean, decimal, timestamp, date",
        ),
        (
            "owner",
            "schema_shape",
            "/object_types/item/owner",
            "ontology",
        ),
        (
            "direction",
            "schema_shape",
            "/views/items/order/0/direction",
            "asc",
        ),
    ] {
        let mut schema: Value =
            serde_json::from_slice(include_bytes!("fixtures/render-board.json")).unwrap();
        match case {
            "domain" => schema["domain"] = json!("program"),
            "kind" => schema["sources"] = json!({"input":{"kind":"unlisted"}}),
            "option" => {
                schema["sources"] = json!({"input":{"kind":"markdown_fields","glob":"/tmp/*.md","fresh_for_s":1,"unlisted":true}})
            }
            "transform" => {
                schema["object_types"]["item"]["properties"]["title"]["transform"] =
                    json!("unlisted")
            }
            "type" => {
                schema["object_types"]["item"]["properties"]["title"]["type"] = json!("unlisted")
            }
            "owner" => schema["object_types"]["item"]["owner"] = json!("unlisted"),
            "direction" => schema["views"]["items"]["order"][0]["direction"] = json!("unlisted"),
            _ => unreachable!(),
        }
        let file = t.0.join("candidate.json");
        std::fs::write(&file, schema.to_string()).unwrap();
        let (exit, answer) = t.cli(&["schema", "check", file.to_str().unwrap()], json!({}));
        assert_eq!(exit, 2, "{case}: {answer}");
        diagnostic(&answer, "schema_violation", field);
        if code != "schema_shape" {
            assert_eq!(answer["violations"][0]["code"], code, "{case}: {answer}");
        }
        assert!(
            answer["rule"].as_str().unwrap().contains(required_text),
            "{case}: {answer}"
        );
        assert!(!answer["rule"].as_str().unwrap().contains("unlisted"));
    }
}

#[test]
fn source_commit_feedback_matches_ascii_hex_admission() {
    let t = Temp::new();
    let file = t.0.join("schema.json");
    for commit in ["aB".repeat(20), "F".repeat(40), "0".repeat(40)] {
        assert_eq!(
            t.cli(
                &[
                    "schema",
                    "install",
                    file.to_str().unwrap(),
                    "--source-commit",
                    &commit
                ],
                json!({})
            )
            .0,
            0
        );
    }
    let (exit, answer) = t.cli(
        &[
            "schema",
            "install",
            file.to_str().unwrap(),
            "--source-commit",
            "not-a-commit",
        ],
        json!({}),
    );
    assert_eq!(exit, 2);
    diagnostic(&answer, "invalid_argument", "--source-commit");
    assert_eq!(
        answer["rule"],
        "40 ASCII hexadecimal characters (0-9, a-f, A-F)"
    );
    let help = ont::run(&["help".into()], &b""[..], 0, 0);
    let help: Value = serde_json::from_str(&help.stdout).unwrap();
    assert_eq!(
        help["options"]["--source-commit"],
        "40 ASCII hex (0-9, a-f, A-F)"
    );
    assert_eq!(help["options"]["--commit"], "40 lowercase hex");
}

#[test]
fn help_returns_while_stdin_pipe_remains_open() {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let mut commands = vec![
        vec!["help".to_string()],
        vec!["--help".into()],
        vec!["-h".into()],
    ];
    let help = ont::run(&["help".into()], &b""[..], 0, 0);
    let help: Value = serde_json::from_str(&help.stdout).unwrap();
    for command in help["commands"].as_array().unwrap() {
        let mut args: Vec<_> = command["command"]
            .as_str()
            .unwrap()
            .split_whitespace()
            .map(str::to_string)
            .collect();
        args.push("--help".into());
        commands.push(args);
    }
    for args in commands {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ont"))
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let writer = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).unwrap();
            bytes
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                drop(writer);
                reader.join().unwrap();
                panic!("help read or waited on stdin: {args:?}");
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(status.success(), "{args:?}");
        // This writer is held until after the child has exited.
        drop(writer);
        let bytes = reader.join().unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap()["status"],
            "ok"
        );
        let mut stderr = String::new();
        child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        assert!(stderr.is_empty(), "{stderr}");
    }
}

#[test]
fn continuation_rule_describes_existing_signed_hex_pairs() {
    let t = Temp::new();
    // The existing decoder uses u8::from_str_radix, which accepts a leading +.
    assert_eq!(
        t.cli(
            &["search"],
            json!({"object_type":"item","continuation":"c1:+1"})
        )
        .0,
        0
    );
    let (code, answer) = t.cli(
        &["search"],
        json!({"object_type":"item","continuation":"c1:+g"}),
    );
    assert_eq!(code, 2);
    diagnostic(&answer, "invalid_continuation", "/continuation");
    assert!(
        answer["rule"]
            .as_str()
            .unwrap()
            .contains("pair may start with '+'")
    );
}

#[test]
fn planned_field_origins_are_request_pointers_cli_and_mcp() {
    for (mode, parameters, field) in [
        ("key", json!({"spec":{"key":7}}), "/parameters/spec/key"),
        (
            "property",
            json!({"payload":"too long"}),
            "/parameters/payload",
        ),
        ("absent_target", json!({"payload":"text"}), "/parameters"),
    ] {
        let t = Temp::new();
        let mut schema: Value =
            serde_json::from_slice(include_bytes!("fixtures/render-board.json")).unwrap();
        schema["version"] = json!(2);
        match mode {
            "key" => {
                schema["object_types"]["item"]["primary_key"]["from"] = json!("/key");
                schema["action_types"]["create_item"] = json!({"parameters":{"spec":{"type":{"struct":{"key":"integer"}},"required":true}},"rules":[{"create_object":"item","set":{"title":"literal"}}]});
            }
            "property" => {
                schema["object_types"]["item"]["properties"]["title"]["max_bytes"] = json!(1);
                schema["action_types"]["create_item"] = json!({"parameters":{"payload":{"type":"string","required":true}},"rules":[{"create_object":"item","set":{"title":"$payload"}}]});
            }
            _ => {
                schema["action_types"]["create_item"] = json!({"parameters":{"subject":{"type":{"ref":"item"},"required":false},"payload":{"type":"string","required":true}},"rules":[{"modify_object":"item","set":{"title":"$payload"}}]});
            }
        }
        let path = t.0.join("origins.json");
        std::fs::write(&path, schema.to_string()).unwrap();
        let (code, answer) = t.cli(&["schema", "install", path.to_str().unwrap()], json!({}));
        assert_eq!(code, 0, "{answer}");
        let request =
            json!({"action_type":"create_item","parameters":parameters,"request_id":"origin-1"});
        for operation in ["validate", "apply"] {
            let (code, answer) = t.cli(&["action", operation], request.clone());
            assert_eq!(code, 2, "{answer}");
            diagnostic(&answer, "invalid_parameter", field);
            assert_eq!(
                t.mcp(&format!("ont_action_{operation}"), request.clone()),
                answer
            );
            assert!(request.pointer(answer["field"].as_str().unwrap()).is_some());
            if mode != "absent_target" {
                assert!(answer["rule"].as_str().unwrap().contains("schema rule"));
            }
        }
        assert!(!t.0.join("edits.jsonl").exists());
    }
}
