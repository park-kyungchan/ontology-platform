//! O14 language contracts through the command dispatcher and MCP action tools.
//! All objects, schemas and stores are synthetic and disposable.
#![allow(clippy::unwrap_used)]
use ont_core::schema::Schema;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const NOW: u64 = 1_794_441_600; // 2026-11-12T00:00:00Z
const BASE: &[u8] = include_bytes!("fixtures/action-work.json");
fn schema() -> Value {
    let mut s: Value = serde_json::from_slice(BASE).unwrap();
    s["object_types"]["note"]["properties"]["title"]["description"] = json!("The note's title.");
    s["object_types"]["note"]["properties"]["applied_at"] = json!({"type":"string"});
    s["action_types"]["copy_note"] = json!({
        "parameters": {
            "source":{"type":{"ref":"note"},"required":true,"description":"The note to copy."},
            "label":{"type":"string","required":false,"description":"An optional label."}
        },
        "rules":[{"create_object":"note","set":{"title":"$source.title","task":"$source.task","label":"$label","applied_at":"$now"}}]
    });
    s["action_types"]["edit_note"] = json!({
        "parameters":{"note":{"type":{"ref":"note"},"required":true},"title":{"type":"string","required":false},"label":{"type":"string","required":false}},
        "rules":[{"modify_object":"note","set":{"title":"$title","label":"$label"}}]
    });
    s["action_types"]["require_label"] = s["action_types"]["copy_note"].clone();
    s["action_types"]["require_label"]["submission_criteria"] =
        json!([{"code":"label_absent","message":"Supply label.","all":[{"exists":"$label"}]}]);
    s["action_types"]["check_title"] = s["action_types"]["copy_note"].clone();
    s["action_types"]["check_title"]["parameters"]["expected_title"] =
        json!({"type":"string","required":true});
    s["action_types"]["check_title"]["submission_criteria"] = json!([{"code":"title_changed","message":"Title differs.","all":[{"eq":["$expected_title","$source.title"]}]}]);
    s["grants"][0]["actions"].as_array_mut().unwrap().extend([
        json!("copy_note"),
        json!("edit_note"),
        json!("require_label"),
        json!("check_title"),
    ]);
    s
}
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!(
            "ont-language-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn store(&self) -> PathBuf {
        self.0.join("store.sqlite")
    }
    fn cli(&self, cmd: &[&str], input: Value, now: u64) -> Value {
        let mut args = vec![
            "--store".into(),
            self.store().display().to_string(),
            "--now".into(),
            now.to_string(),
        ];
        if cmd.first() == Some(&"action") {
            args.extend(["--client".into(), "language".into()]);
        }
        args.extend(cmd.iter().map(|s| s.to_string()));
        let out = ont::run(&args, input.to_string().as_bytes(), 0, 0);
        assert!(out.stderr.is_empty(), "{}", out.stderr);
        serde_json::from_str(out.stdout.trim()).unwrap()
    }
    fn install(&self, s: Value) {
        let p = self.0.join("schema.json");
        std::fs::write(&p, s.to_string()).unwrap();
        assert_eq!(
            self.cli(&["schema", "install", p.to_str().unwrap()], json!({}), NOW)["status"],
            "ok"
        );
    }
    fn call(&self, mcp: bool, op: &str, input: Value, now: u64) -> Value {
        if !mcp {
            return self.cli(&["action", op], input, now);
        }
        let mut server = ont::mcp::Server::new(
            ont::mcp::Config {
                store: self.store(),
                now: Some(now),
                client: "language".into(),
            },
            &ont::cli::execute,
        );
        let tool = format!("ont_action_{op}");
        let response = server.handle_line(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":tool,"arguments":input}}).to_string().as_bytes()).unwrap();
        assert!(response.get("error").is_none(), "{response}");
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.len() <= ont::mcp::MAX_RESPONSE);
        let body: Value = serde_json::from_str(text).unwrap();
        assert_eq!(response["result"]["isError"], body["status"] != "ok");
        body
    }
    fn seed(&self, mcp: bool) -> Value {
        let source = self.call(mcp,"apply",json!({"action_type":"record_note","parameters":{"title":"original","task":task()},"request_id":"seed"}),NOW);
        assert_eq!(source["status"], "ok", "{source}");
        source["result"]["edits"][0]["id"].clone()
    }
    fn fields(&self, id: &Value) -> Value {
        let out = self.cli(&["get"], json!({"id":id}), NOW);
        assert_eq!(out["status"], "ok", "{out}");
        out["result"]["objects"][0]["observations"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["fields"]
            .clone()
    }
    fn logs(&self) -> Vec<(i64, ont_store::schema_mode::ActionLogEntry)> {
        ont_store::Store::open_schema_existing(&self.store())
            .unwrap()
            .action_log()
            .unwrap()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn task() -> Value {
    json!({"object_type":"task","namespace":{"instance":"vps://palantirkc","account":"root"},"native_key":"/synthetic/task"})
}

#[test]
fn reference_properties_now_optional_and_descriptions_cli_and_mcp() {
    for mcp in [false, true] {
        let t = Temp::new();
        t.install(schema());
        let source = t.seed(mcp);
        let request = json!({"action_type":"copy_note","parameters":{"source":source,"label":null},"request_id":"copy"});
        let preview = t.call(mcp, "validate", request.clone(), NOW - 1);
        assert_eq!(preview["result"]["valid"], true, "{preview}");
        assert_eq!(preview["result"]["application_time"], "set_at_application");
        assert_eq!(preview["result"]["basis"][0]["id"], source);
        assert_eq!(t.logs().len(), 1);
        let mut apply_request = request.clone();
        apply_request["expected"] = preview["result"]["basis"].clone();
        let applied = t.call(mcp, "apply", apply_request.clone(), NOW);
        assert_eq!(applied["result"]["outcome"], "applied", "{applied}");
        assert_eq!(applied["result"]["edits"], preview["result"]["edits"]);
        let id = applied["result"]["edits"][0]["id"].clone();
        let fields = t.fields(&id);
        assert_eq!(fields["title"]["value"], "original");
        assert_eq!(fields["task"]["value"], task());
        assert!(fields.get("label").is_none());
        assert_eq!(fields["applied_at"]["value"], "2026-11-12T00:00:00Z");
        let log_bytes = std::fs::read(t.0.join("edits.jsonl")).unwrap();
        let retry = t.call(mcp, "apply", apply_request, NOW + 100_000);
        assert_eq!(retry["result"]["outcome"], "already_applied", "{retry}");
        assert_eq!(retry["result"]["edits"], applied["result"]["edits"]);
        assert_eq!(t.fields(&id), fields);
        assert_eq!(std::fs::read(t.0.join("edits.jsonl")).unwrap(), log_bytes);
        let logs = t.logs();
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[1].1.submitted_at, NOW);
        assert_eq!(logs[1].1.basis, preview["result"]["basis"]);
        // The existing action-log keys and storage format are unchanged.
        let wire: Value = serde_json::from_slice(
            log_bytes
                .split(|b| *b == b'\n')
                .rfind(|l| !l.is_empty())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(wire.as_object().unwrap().len(), 13);
        let list = t.call(mcp, "list", json!({}), NOW);
        let action = list["result"]["actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["name"] == "copy_note")
            .unwrap();
        assert_eq!(action["parameters"][1]["description"], "The note to copy.");
        let described = t.cli(&["describe"], json!({}), NOW);
        let note = described["result"]["object_types"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["name"] == "note")
            .unwrap();
        let title = note["properties"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "title")
            .unwrap();
        assert_eq!(title["description"], "The note's title.");
    }
}

#[test]
fn rule_only_reference_basis_conflicts_and_criteria_operands_cli_and_mcp() {
    for mcp in [false, true] {
        let t = Temp::new();
        t.install(schema());
        let source = t.seed(mcp);
        let request =
            json!({"action_type":"copy_note","parameters":{"source":source},"request_id":"copy"});
        let preview = t.call(mcp, "validate", request.clone(), NOW);
        assert_eq!(preview["result"]["basis"].as_array().unwrap().len(), 1);
        let edited=t.call(mcp,"apply",json!({"action_type":"edit_note","parameters":{"note":source,"title":"changed"},"request_id":"edit"}),NOW);
        assert_eq!(edited["status"], "ok", "{edited}");
        let before = t.logs().len();
        let mut stale = request;
        stale["expected"] = preview["result"]["basis"].clone();
        let refused = t.call(mcp, "apply", stale, NOW);
        assert_eq!(refused["code"], "revision_conflict", "{refused}");
        assert_eq!(refused["id"], source);
        let checked = json!({"action_type":"check_title","parameters":{"source":source,"expected_title":"original"},"request_id":"criterion"});
        let refused = t.call(mcp, "apply", checked.clone(), NOW);
        assert_eq!(refused["code"], "submission_criteria", "{refused}");
        assert_eq!(t.logs().len(), before);
        let mut good = checked;
        good["parameters"]["expected_title"] = json!("changed");
        assert_eq!(
            t.call(mcp, "validate", good.clone(), NOW)["result"]["valid"],
            true
        );
        assert_eq!(t.call(mcp, "apply", good, NOW)["status"], "ok");
    }
}

#[test]
fn optional_presence_create_and_modify_cli_and_mcp() {
    for mcp in [false, true] {
        let t = Temp::new();
        t.install(schema());
        let source = t.seed(mcp);
        for label in [None, Some(Value::Null)] {
            let mut params = json!({"source":source});
            if let Some(label) = label {
                params["label"] = label;
            }
            let request =
                json!({"action_type":"require_label","parameters":params,"request_id":"absent"});
            assert_eq!(
                t.call(mcp, "validate", request.clone(), NOW)["result"]["valid"],
                false
            );
            assert_eq!(
                t.call(mcp, "apply", request, NOW)["code"],
                "submission_criteria"
            );
        }
        let first=t.call(mcp,"apply",json!({"action_type":"require_label","parameters":{"source":source,"label":"kept"},"request_id":"present"}),NOW);
        assert_eq!(first["status"], "ok", "{first}");
        let id = first["result"]["edits"][0]["id"].clone();
        for (i, params) in [
            json!({"note":id}),
            json!({"note":id,"title":null,"label":null}),
        ]
        .into_iter()
        .enumerate()
        {
            let edit=t.call(mcp,"apply",json!({"action_type":"edit_note","parameters":params,"request_id":format!("skip-{i}")}),NOW);
            assert_eq!(edit["status"], "ok", "{edit}");
            let fields = t.fields(&id);
            assert_eq!(fields["title"]["value"], "original");
            assert_eq!(fields["label"]["value"], "kept");
        }
        let bad = t.call(
            mcp,
            "apply",
            json!({"action_type":"copy_note","parameters":{"source":null},"request_id":"bad"}),
            NOW,
        );
        assert_eq!(bad["code"], "invalid_parameter");
        // Omitted and null optional values are the same request content.
        let omitted =
            json!({"action_type":"copy_note","parameters":{"source":source},"request_id":"same"});
        assert_eq!(
            t.call(mcp, "apply", omitted.clone(), NOW)["result"]["outcome"],
            "applied"
        );
        let mut null = omitted;
        null["parameters"]["label"] = Value::Null;
        assert_eq!(
            t.call(mcp, "apply", null, NOW + 1)["result"]["outcome"],
            "already_applied"
        );
    }
}

#[test]
fn schema_refuses_unsupported_expressions_and_description_bytes() {
    let t = Temp::new();
    let mut cases = Vec::new();
    for expression in [
        "$source.unknown",
        "$source.task.directory",
        "$label.title",
        "$missing.title",
    ] {
        let mut s = schema();
        s["action_types"]["copy_note"]["rules"][0]["set"]["title"] = json!(expression);
        cases.push(s);
    }
    let mut s = schema();
    s["action_types"]["copy_note"]["rules"][0]["set"]["task"] = json!("$source.title");
    cases.push(s);
    let mut s = schema();
    s["action_types"]["copy_note"]["rules"][0]["set"]["task"] = json!("$now");
    cases.push(s);
    let mut s = schema();
    s["action_types"]["copy_note"]["parameters"]["now"] = json!({"type":"string"});
    cases.push(s);
    for target in [
        "/object_types/note/properties/title",
        "/action_types/copy_note/parameters/source",
    ] {
        let mut s = schema();
        s.pointer_mut(target).unwrap()["description"] = json!("a".repeat(301));
        cases.push(s);
        let mut s = schema();
        s.pointer_mut(target).unwrap()["description"] = json!("é".repeat(151));
        cases.push(s);
        let mut s = schema();
        s.pointer_mut(target).unwrap()["description"] = json!(42);
        cases.push(s);
    }
    for (i, s) in cases.into_iter().enumerate() {
        let p = t.0.join(format!("bad-{i}.json"));
        std::fs::write(&p, s.to_string()).unwrap();
        let out = t.cli(&["schema", "check", p.to_str().unwrap()], json!({}), NOW);
        assert_eq!(out["code"], "schema_violation", "case {i}: {out}");
    }
    let mut s = schema();
    s["object_types"]["note"]["properties"]["title"]["description"] = json!("é".repeat(150));
    let bytes = s.to_string();
    assert!(Schema::check_bytes(bytes.as_bytes(), &BTreeSet::new()).is_ok());
    // The pre-extension fixture still passes with its exact original byte pin.
    let checked = Schema::check_bytes(BASE, &BTreeSet::new()).unwrap();
    assert_eq!(checked.pin().sha256, format!("{:x}", Sha256::digest(BASE)));
}

#[test]
fn descriptions_keep_mcp_response_and_tool_list_bounds() {
    let t = Temp::new();
    let mut s = schema();
    for i in 0..800 {
        s["action_types"]["copy_note"]["parameters"][format!("extra_{i}")] =
            json!({"type":"string","description":"x".repeat(300)});
    }
    t.install(s);
    let answer = t.call(true, "list", json!({}), NOW);
    assert_eq!(answer["code"], "response_too_large", "{answer}");
    assert!(ont::mcp::tools_list().to_string().len() <= 5000);
    assert!(t.logs().is_empty());
}

#[test]
fn referenced_rule_values_use_original_basis_across_atomic_rules() {
    for mcp in [false, true] {
        let t = Temp::new();
        let mut s = schema();
        s["action_types"]["copy_note"]["rules"]
            .as_array_mut()
            .unwrap()
            .insert(
                0,
                json!({"modify_object":"note","set":{"title":"rewritten"}}),
            );
        t.install(s);
        let source = t.seed(mcp);
        let copied = t.call(
            mcp,
            "apply",
            json!({"action_type":"copy_note","parameters":{"source":source},"request_id":"atomic"}),
            NOW,
        );
        assert_eq!(copied["status"], "ok", "{copied}");
        assert_eq!(t.fields(&source)["title"]["value"], "rewritten");
        let id = &copied["result"]["edits"][1]["id"];
        assert_eq!(t.fields(id)["title"]["value"], "original");
        assert_eq!(
            t.logs()[1].1.basis[0]["revision"],
            t.logs()[0].1.edits[0]["revision"]
        );
    }
}
