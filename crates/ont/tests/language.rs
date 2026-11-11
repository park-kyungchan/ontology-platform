//! Language contracts through the command dispatcher and MCP action tools.
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
        serde_json::from_str(self.cli_line(cmd, input, now).trim()).unwrap()
    }
    fn cli_line(&self, cmd: &[&str], input: Value, now: u64) -> String {
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
        out.stdout
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
        self.tool(&format!("ont_action_{op}"), input, now)
    }
    fn tool(&self, tool: &str, input: Value, now: u64) -> Value {
        let (text, is_error) = self.tool_line(tool, input, now);
        let body: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(is_error, body["status"] != "ok");
        body
    }
    fn tool_line(&self, tool: &str, input: Value, now: u64) -> (String, bool) {
        let mut server = ont::mcp::Server::new(
            ont::mcp::Config {
                store: self.store(),
                now: Some(now),
                client: "language".into(),
            },
            &ont::cli::execute,
        );
        let response = server.handle_line(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":tool,"arguments":input}}).to_string().as_bytes()).unwrap();
        assert!(response.get("error").is_none(), "{response}");
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.len() <= ont::mcp::MAX_RESPONSE);
        (
            text.to_string(),
            response["result"]["isError"].as_bool().unwrap(),
        )
    }
    fn action_line(&self, mcp: bool, op: &str, input: Value, now: u64) -> String {
        if mcp {
            self.tool_line(&format!("ont_action_{op}"), input, now).0
        } else {
            self.cli_line(&["action", op], input, now)
        }
    }
    fn refuses_schema(&self, schema: Value, code: &str, pointer: &str) {
        let p = self.0.join("refused-schema.json");
        std::fs::write(&p, schema.to_string()).unwrap();
        let answer = self.cli(&["schema", "check", p.to_str().unwrap()], json!({}), NOW);
        assert_eq!(answer["code"], "schema_violation", "{answer}");
        assert!(
            answer["violations"]
                .as_array()
                .unwrap()
                .contains(&json!({"code":code,"pointer":pointer})),
            "{answer}"
        );
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
        let mut current = json!({"action_type":"copy_note","parameters":{"source":source},"request_id":"current"});
        current.as_object_mut().unwrap().remove("expected");
        let copied = t.call(mcp, "apply", current, NOW);
        assert_eq!(copied["status"], "ok", "{copied}");
        assert_eq!(
            t.fields(&copied["result"]["edits"][0]["id"])["title"]["value"],
            "changed"
        );
        let log = t.logs();
        assert_eq!(
            log.last().unwrap().1.basis[0]["revision"],
            edited["result"]["edits"][0]["revision"]
        );
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
    for (expression, code) in [
        ("$source.unknown", "unknown_property"),
        ("$source.task.directory", "unknown_property"),
        ("$label.title", "unknown_property"),
        ("$missing.title", "unknown_parameter"),
    ] {
        let mut s = schema();
        s["action_types"]["copy_note"]["rules"][0]["set"]["title"] = json!(expression);
        t.refuses_schema(s, code, "/action_types/copy_note/rules/0/set/title");
    }
    for expression in ["$source.title", "$now"] {
        let mut s = schema();
        s["action_types"]["copy_note"]["rules"][0]["set"]["task"] = json!(expression);
        t.refuses_schema(
            s,
            "invalid_rule",
            "/action_types/copy_note/rules/0/set/task",
        );
    }
    let mut s = schema();
    s["action_types"]["copy_note"]["parameters"]["now"] = json!({"type":"string"});
    t.refuses_schema(s, "invalid_name", "/action_types/copy_note/parameters/now");
    for target in [
        "/object_types/note/properties/title",
        "/action_types/copy_note/parameters/source",
    ] {
        for description in [json!("a".repeat(301)), json!("é".repeat(151)), json!(42)] {
            let mut s = schema();
            s.pointer_mut(target).unwrap()["description"] = description;
            t.refuses_schema(s, "invalid_schema", &format!("{target}/description"));
        }
    }
    let mut s = schema();
    s["object_types"]["note"]["properties"]["title"]["description"] = json!("é".repeat(150));
    let bytes = s.to_string();
    assert!(Schema::check_bytes(bytes.as_bytes(), &BTreeSet::new()).is_ok());
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
    let described = t.tool("ont_describe", json!({}), NOW);
    assert_eq!(described["code"], "response_too_large", "{described}");
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

fn put(t: &Temp, id: Value, keys: &[(&str, &str)], fields: Value, until: u64, retract: bool) {
    use ont_core::{Change, Coverage, Knowledge, Lineage, Observation, Scope};
    let mut store = ont_store::Store::open_schema_existing_writable(&t.store()).unwrap();
    let id: ont_core::Id = serde_json::from_value(id).unwrap();
    let old = store.head(&id).unwrap();
    let supersedes = old.as_ref().map(|h| h.head.observation.revision.clone());
    let change = if retract {
        Change::Retraction
    } else if old.is_some() {
        Change::Update
    } else {
        Change::Initial
    };
    let fields = fields
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), Knowledge::Value(v.clone())))
        .collect();
    store
        .append_schema(
            &[ont_store::Append {
                scope: Scope::domain(
                    id.namespace.clone(),
                    "work",
                    keys.iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect(),
                )
                .unwrap(),
                id,
                observation: Observation {
                    revision: format!("synthetic-{until}-{retract}"),
                    source_revision: "synthetic".into(),
                    observed_at: NOW - 10,
                    fresh_until: until,
                    change,
                    supersedes,
                    coverage: Coverage::Complete,
                    fields,
                    lineage: Lineage {
                        artifact_sha256: "a".repeat(64),
                        pointer: "/0".into(),
                    },
                },
            }],
            NOW,
        )
        .unwrap();
}

#[test]
fn reference_expressions_require_read_in_every_action_grant() {
    let t = Temp::new();
    let mut s = schema();
    s["grants"][0]["read"]
        .as_array_mut()
        .unwrap()
        .retain(|v| v != "note");
    t.refuses_schema(
        s.clone(),
        "reference_read_forbidden",
        "/action_types/copy_note/rules/0/set/title",
    );
    t.refuses_schema(
        s,
        "reference_read_forbidden",
        "/action_types/check_title/submission_criteria/0/all/0/eq/1",
    );
    let mut s = schema();
    s["grants"]
        .as_array_mut()
        .unwrap()
        .push(json!({"principal":"another","read":[],"actions":["copy_note"]}));
    t.refuses_schema(
        s,
        "reference_read_forbidden",
        "/action_types/copy_note/rules/0/set/title",
    );
}

#[test]
fn reference_read_scope_and_unavailable_sources_refuse_identically() {
    for mcp in [false, true] {
        let t = Temp::new();
        let mut s = schema();
        s["scope_keys"] = json!(["tenant"]);
        s["grants"][0]["scope"] = json!({"tenant":"allowed"});
        s["object_types"]["task"]["owner"] = json!("ontology");
        s["object_types"]["task"]["primary_key"] = json!({"from":"$action_id"});
        for prop in s["object_types"]["task"]["properties"]
            .as_object_mut()
            .unwrap()
            .values_mut()
        {
            prop.as_object_mut().unwrap().remove("from");
        }
        t.install(s);
        for (key, tenant) in [("outside", "blocked"), ("inside", "allowed")] {
            let id = json!({"object_type":"note","namespace":task()["namespace"],"native_key":key});
            put(
                &t,
                id.clone(),
                &[("tenant", tenant)],
                json!({"title":"source","task":task()}),
                ont::action::FRESH_UNTIL_MAX,
                false,
            );
            let req =
                json!({"action_type":"copy_note","parameters":{"source":id},"request_id":key});
            if tenant == "blocked" {
                assert_eq!(
                    t.cli(&["get"], json!({"id":id}), NOW)["code"],
                    "unavailable"
                );
                for op in ["validate", "apply"] {
                    let mut with_expected = req.clone();
                    with_expected["expected"] = json!([{"id":id,"revision":"untrusted-revision"}]);
                    let expected_refusal = t.call(mcp, op, with_expected, NOW);
                    assert_eq!(
                        expected_refusal["code"], "rule_reference_unavailable",
                        "{expected_refusal}"
                    );
                    let refused = t.call(mcp, op, req.clone(), NOW);
                    assert_eq!(refused["code"], "rule_reference_unavailable");
                    for field in ["id", "parameter", "property", "message", "basis"] {
                        assert!(refused.get(field).is_none(), "{refused}");
                    }
                }
                let criterion=t.call(mcp,"apply",json!({"action_type":"check_title","parameters":{"source":id,"expected_title":"source"},"request_id":"hidden-criterion"}),NOW);
                assert_eq!(
                    criterion["code"], "rule_reference_unavailable",
                    "{criterion}"
                );
                assert!(t.logs().is_empty());
            } else {
                let applied = t.call(mcp, "apply", req, NOW);
                assert_eq!(applied["status"], "ok", "{applied}");
                assert_eq!(
                    t.fields(&applied["result"]["edits"][0]["id"])["title"]["value"],
                    "source"
                );
            }
        }
    }
}

#[test]
fn stale_source_rule_and_criterion_fail_preconditions_cli_and_mcp() {
    for mcp in [false, true] {
        let t = Temp::new();
        let mut s = schema();
        s["action_types"]["copy_task"] = json!({"parameters":{"source":{"type":{"ref":"task"},"required":true}},"rules":[{"create_object":"tag","set":{"label":"$source.directory"}}]});
        s["action_types"]["check_task"] = json!({"parameters":{"source":{"type":{"ref":"task"},"required":true}},"submission_criteria":[{"code":"directory_changed","message":"Directory differs.","all":[{"eq":["$source.directory","/synthetic/task"]}]}]});
        s["grants"][0]["actions"]
            .as_array_mut()
            .unwrap()
            .extend([json!("copy_task"), json!("check_task")]);
        t.install(s);
        put(
            &t,
            task(),
            &[],
            json!({"directory":"/synthetic/task","state_sha256":"abc","checkpoint_line":"synthetic"}),
            NOW,
            false,
        );
        let boundary = json!({"action_type":"copy_task","parameters":{"source":task()},"request_id":"boundary"});
        assert_eq!(t.call(mcp, "apply", boundary, NOW)["status"], "ok");
        let before = t.logs().len();
        for action in ["copy_task", "check_task"] {
            let req = json!({"action_type":action,"parameters":{"source":task()},"request_id":format!("stale-{action}")});
            let validation = t.call(mcp, "validate", req.clone(), NOW + 1);
            assert_eq!(validation["status"], "ok", "{validation}");
            assert_eq!(validation["result"]["valid"], false);
            assert_eq!(
                validation["result"]["preconditions"],
                json!([{"code":"rule_value_stale","passed":false}])
            );
            assert_eq!(validation["result"]["edits"], json!([]));
            assert_eq!(
                t.call(mcp, "apply", req, NOW + 1)["code"],
                "rule_value_stale"
            );
        }
        assert_eq!(t.logs().len(), before);
        let source = t.seed(mcp);
        let ontology=t.call(mcp,"apply",json!({"action_type":"copy_note","parameters":{"source":source},"request_id":"ontology"}),NOW+86400);
        assert_eq!(ontology["status"], "ok", "{ontology}");
    }
}

#[test]
fn missing_and_retracted_rule_references_have_distinct_refusal() {
    for mcp in [false, true] {
        let t = Temp::new();
        t.install(schema());
        let source = t.seed(mcp);
        let mut absent = source.clone();
        absent["native_key"] = json!("missing");
        for (i, id) in [absent, source.clone()].into_iter().enumerate() {
            if i == 1 {
                let deleted=t.call(mcp,"apply",json!({"action_type":"delete_note","parameters":{"note":source},"request_id":"delete"}),NOW);
                assert_eq!(deleted["status"], "ok");
            }
            let before = t.logs().len();
            for action in ["copy_note", "check_title"] {
                let mut params = json!({"source":id});
                if action == "check_title" {
                    params["expected_title"] = json!("original");
                }
                let req = json!({"action_type":action,"parameters":params,"request_id":format!("unavailable-{i}-{action}")});
                for op in ["validate", "apply"] {
                    let mut with_expected = req.clone();
                    with_expected["expected"] =
                        json!([{"id":id,"revision":"before-disappearance"}]);
                    assert_eq!(
                        t.call(mcp, op, with_expected, NOW)["code"],
                        "rule_reference_unavailable"
                    );
                    assert_eq!(
                        t.call(mcp, op, req.clone(), NOW)["code"],
                        "rule_reference_unavailable"
                    );
                }
            }
            assert_eq!(t.logs().len(), before);
        }
    }
}

#[test]
fn absent_rule_inputs_do_not_change_object_revision_or_conflict() {
    for mcp in [false, true] {
        let t = Temp::new();
        let mut s = schema();
        s["action_types"]["optional_link"] = json!({"parameters":{"note":{"type":{"ref":"note"},"required":true},"task":{"type":{"ref":"task"}}},"rules":[{"set_reference":{"object_type":"note","property":"task","value":"$task"}}]});
        s["object_types"]["tag"]["properties"]["label"]["required"] = json!(false);
        s["action_types"]["optional_create"] = json!({"parameters":{"label":{"type":"string"}},"rules":[{"create_object":"tag","set":{"label":"$label"}}]});
        s["grants"][0]["actions"]
            .as_array_mut()
            .unwrap()
            .extend([json!("optional_link"), json!("optional_create")]);
        t.install(s);
        let source = t.seed(mcp);
        let original = t.cli(&["get"], json!({"id":source}), NOW);
        let revision = original["result"]["objects"][0]["head"].clone();
        for action in ["edit_note", "optional_link", "optional_create"] {
            let params = if action == "optional_create" {
                json!({})
            } else {
                json!({"note":source})
            };
            let req = json!({"action_type":action,"parameters":params,"request_id":format!("no-input-{action}")});
            let preview = t.call(mcp, "validate", req.clone(), NOW);
            assert_eq!(preview["result"]["edits"], json!([]), "{preview}");
            let applied = t.call(mcp, "apply", req.clone(), NOW);
            assert_eq!(applied["result"]["outcome"], "applied", "{applied}");
            assert_eq!(applied["result"]["edits"], json!([]));
            assert_eq!(t.logs().last().unwrap().1.edits, json!([]));
            let retry = t.call(mcp, "apply", req.clone(), NOW + 1);
            assert_eq!(retry["result"]["outcome"], "already_applied");
            assert_eq!(retry["result"]["edits"], json!([]));
            let before = t.logs().len();
            let mut changed = req;
            let parameter = match action {
                "edit_note" => "title",
                "optional_link" => "task",
                _ => "label",
            };
            changed["parameters"][parameter] = if action == "optional_link" {
                task()
            } else {
                json!("supplied later")
            };
            let reused = t.call(mcp, "apply", changed, NOW + 1);
            assert_eq!(reused["code"], "request_id_reused", "{reused}");
            assert_eq!(t.logs().len(), before);

            assert_eq!(t.cli(&["get"], json!({"id":source}), NOW), original);
        }
        assert!(
            ont_store::Store::open_schema_existing(&t.store())
                .unwrap()
                .scan_heads("tag", 100, None)
                .unwrap()
                .is_empty()
        );
        let other=t.call(mcp,"apply",json!({"action_type":"edit_note","parameters":{"note":source,"title":"another caller"},"expected":[{"id":source,"revision":revision}],"request_id":"other-caller","agent":"another"}),NOW);
        assert_eq!(other["status"], "ok", "{other}");
    }
}

#[test]
fn optional_required_create_and_incompatible_now_patterns_refuse_schema() {
    let t = Temp::new();
    for mode in ["create_object", "create_or_modify_object"] {
        let mut s = schema();
        s["action_types"]["create_tag"]["parameters"]["label"]["required"] = json!(false);
        s["action_types"]["create_tag"]["rules"][0] = json!({mode:"tag","set":{"label":"$label"}});
        t.refuses_schema(
            s,
            "invalid_rule",
            "/action_types/create_tag/rules/0/set/label",
        );
    }
    let mut s = schema();
    s["action_types"]["copy_note"]["parameters"]["source"]["required"] = json!(false);
    t.refuses_schema(
        s,
        "invalid_rule",
        "/action_types/copy_note/rules/0/set/title",
    );
    let mut s = schema();
    s["value_types"]["day_only"] = json!({"base":"string","pattern":"[0-9]{4}-[0-9]{2}-[0-9]{2}"});
    s["object_types"]["note"]["properties"]["applied_at"]["type"] =
        json!({"value_type":"day_only"});
    t.refuses_schema(
        s.clone(),
        "invalid_rule",
        "/action_types/copy_note/rules/0/set/applied_at",
    );
    s["value_types"]["day_only"]["pattern"] =
        json!("[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z");
    t.install(s);
    let source = t.seed(false);
    assert_eq!(t.call(false,"apply",json!({"action_type":"copy_note","parameters":{"source":source},"request_id":"full-time"}),NOW)["status"],"ok");
}

#[test]
fn exists_optional_reference_empty_string_and_reference_array_presence() {
    for mcp in [false, true] {
        let t = Temp::new();
        let mut s = schema();
        for (name, param) in [
            ("has_reference", json!({"type":{"ref":"note"}})),
            ("has_array", json!({"type":{"array":{"ref":"note"}}})),
        ] {
            s["action_types"][name] = json!({"parameters":{"value":param},"submission_criteria":[{"code":"absent","message":"Supply value.","all":[{"exists":"$value"}]}]});
            s["grants"][0]["actions"]
                .as_array_mut()
                .unwrap()
                .push(json!(name));
        }
        t.install(s);
        let source = t.seed(mcp);
        for value in [None, Some(Value::Null)] {
            let mut parameters = json!({});
            if let Some(value) = value {
                parameters["value"] = value;
            }
            let req = json!({"action_type":"has_reference","parameters":parameters,"request_id":"absent-reference"});
            assert_eq!(
                t.call(mcp, "validate", req.clone(), NOW)["result"]["valid"],
                false
            );
            assert_eq!(
                t.call(mcp, "apply", req, NOW)["code"],
                "submission_criteria"
            );
        }
        let present = t.call(
            mcp,
            "validate",
            json!({"action_type":"has_reference","parameters":{"value":source}}),
            NOW,
        );
        assert_eq!(present["result"]["valid"], true);
        let empty=t.call(mcp,"apply",json!({"action_type":"require_label","parameters":{"source":source,"label":""},"request_id":"empty-is-present"}),NOW);
        assert_eq!(empty["status"], "ok", "{empty}");
        assert_eq!(
            t.fields(&empty["result"]["edits"][0]["id"])["label"]["value"],
            ""
        );
        for value in [
            json!([]),
            json!([{"object_type":"note","namespace":task()["namespace"],"native_key":"not-live"}]),
        ] {
            assert_eq!(
                t.call(
                    mcp,
                    "validate",
                    json!({"action_type":"has_array","parameters":{"value":value}}),
                    NOW
                )["result"]["valid"],
                true
            );
        }
    }
}

#[test]
fn unavailable_references_are_byte_identical_before_other_refusals() {
    for mcp in [false, true] {
        let t = Temp::new();
        let mut s = schema();
        s["scope_keys"] = json!(["tenant"]);
        s["grants"][0]["scope"] = json!({"tenant":"allowed"});
        s["object_types"]["task"]["owner"] = json!("ontology");
        s["object_types"]["task"]["primary_key"] = json!({"from":"$action_id"});
        for prop in s["object_types"]["task"]["properties"]
            .as_object_mut()
            .unwrap()
            .values_mut()
        {
            prop.as_object_mut().unwrap().remove("from");
        }
        s["action_types"]["copy_pair"] = json!({
            "parameters":{"first":{"type":{"ref":"note"},"required":true},"second":{"type":{"ref":"note"},"required":true}},
            "submission_criteria":[{"code":"second_title","message":"Second title differs.","all":[{"eq":["$second.title","source"]}]}],
            "rules":[{"create_object":"tag","set":{"label":"$first.title"}}]
        });
        s["grants"][0]["actions"]
            .as_array_mut()
            .unwrap()
            .push(json!("copy_pair"));
        t.install(s);
        let id =
            |key| json!({"object_type":"note","namespace":task()["namespace"],"native_key":key});
        let fields = json!({"title":"source","task":task()});
        put(
            &t,
            id("hidden"),
            &[("tenant", "blocked")],
            fields.clone(),
            NOW - 1,
            false,
        );
        put(
            &t,
            id("retracted"),
            &[("tenant", "allowed")],
            fields.clone(),
            NOW - 1,
            false,
        );
        put(
            &t,
            id("retracted"),
            &[("tenant", "allowed")],
            json!({}),
            NOW - 1,
            true,
        );
        put(
            &t,
            id("stale"),
            &[("tenant", "allowed")],
            fields,
            NOW - 1,
            false,
        );
        for op in ["validate", "apply"] {
            let mut baseline = None;
            for key in ["missing", "hidden", "retracted"] {
                for action in ["copy_note", "check_title"] {
                    for expected in [false, true] {
                        let mut parameters = json!({"source":id(key)});
                        if action == "check_title" {
                            parameters["expected_title"] = json!("source");
                        }
                        let mut req = json!({"action_type":action,"parameters":parameters,"request_id":format!("{action}-{key}-{expected}")});
                        if expected {
                            req["expected"] = json!([{"id":id(key),"revision":"untrusted"}]);
                        }
                        let line = t.action_line(mcp, op, req, NOW);
                        let body: Value = serde_json::from_str(&line).unwrap();
                        assert_eq!(body["code"], "rule_reference_unavailable", "{line}");
                        for field in [
                            "message",
                            "id",
                            "parameter",
                            "property",
                            "basis",
                            "criteria",
                        ] {
                            assert!(body.get(field).is_none(), "{body}");
                        }
                        if let Some(ref baseline) = baseline {
                            assert_eq!(&line, baseline);
                        } else {
                            baseline = Some(line);
                        }
                    }
                }
                // Availability wins even when another reference is stale, in
                // either expression order, and expected would also conflict.
                for (first, second) in [(id("stale"), id(key)), (id(key), id("stale"))] {
                    let req = json!({"action_type":"copy_pair","parameters":{"first":first,"second":second},"expected":[{"id":id("stale"),"revision":"untrusted"}],"request_id":format!("pair-{key}")});
                    assert_eq!(
                        t.action_line(mcp, op, req, NOW),
                        *baseline.as_ref().unwrap()
                    );
                }
            }
        }
        assert!(t.logs().is_empty());
    }
}

#[test]
fn skipped_modify_and_reference_rules_still_require_live_targets() {
    for mcp in [false, true] {
        let t = Temp::new();
        let mut s = schema();
        s["action_types"]["optional_link"] = json!({"parameters":{"note":{"type":{"ref":"note"},"required":true},"task":{"type":{"ref":"task"}}},"rules":[{"set_reference":{"object_type":"note","property":"task","value":"$task"}}]});
        s["grants"][0]["actions"]
            .as_array_mut()
            .unwrap()
            .push(json!("optional_link"));
        t.install(s);
        let source = t.seed(mcp);
        assert_eq!(t.call(mcp,"apply",json!({"action_type":"delete_note","parameters":{"note":source},"request_id":"delete-target"}),NOW)["status"],"ok");
        let mut missing = source.clone();
        missing["native_key"] = json!("missing-target");
        let before = t.logs().len();
        for target in [missing, source] {
            for action in ["edit_note", "optional_link"] {
                for op in ["validate", "apply"] {
                    let mut supplied = json!({"note":target});
                    let field = if action == "edit_note" {
                        "title"
                    } else {
                        "task"
                    };
                    supplied[field] = if action == "edit_note" {
                        json!("changed")
                    } else {
                        task()
                    };
                    let baseline = t.call(
                        mcp,
                        op,
                        json!({"action_type":action,"parameters":supplied,"request_id":"supplied"}),
                        NOW,
                    );
                    assert_eq!(
                        baseline["code"],
                        if action == "edit_note" {
                            "rule_reference_unavailable"
                        } else {
                            "revision_conflict"
                        },
                        "{baseline}"
                    );
                    for mut absent in [json!({"note":target}), json!({"note":target,field:null})] {
                        // Both omission and null skip the value, after target resolution.
                        absent["note"] = target.clone();
                        let answer = t.call(
                            mcp,
                            op,
                            json!({"action_type":action,"parameters":absent,"request_id":"absent"}),
                            NOW,
                        );
                        assert_eq!(answer, baseline);
                    }
                }
            }
        }
        assert_eq!(t.logs().len(), before);
    }
}

#[test]
fn stale_validation_preserves_independent_criteria_and_basis() {
    for mcp in [false, true] {
        let t = Temp::new();
        let mut s = schema();
        s["action_types"]["stale_preview"] = json!({
            "parameters":{"source":{"type":{"ref":"task"},"required":true},"label":{"type":"string","required":true}},
            "submission_criteria":[
                {"code":"before","message":"Source absent.","all":[{"exists":"$source"}]},
                {"code":"stale_value","message":"Directory differs.","all":[{"eq":["$source.directory","/synthetic/task"]}]},
                {"code":"after","message":"Label differs.","all":[{"eq":["$label","wanted"]}]},
                {"code":"last","message":"Label absent.","all":[{"exists":"$label"}]}
            ],
            "rules":[{"create_object":"tag","set":{"label":"$source.directory"}}]
        });
        s["grants"][0]["actions"]
            .as_array_mut()
            .unwrap()
            .push(json!("stale_preview"));
        t.install(s);
        put(
            &t,
            task(),
            &[],
            json!({"directory":"/synthetic/task","state_sha256":"abc","checkpoint_line":"synthetic"}),
            NOW,
            false,
        );
        let req = json!({"action_type":"stale_preview","parameters":{"source":task(),"label":"other"},"request_id":"stale-preview"});
        let validation = t.call(mcp, "validate", req.clone(), NOW + 1);
        assert_eq!(validation["status"], "ok", "{validation}");
        let result = &validation["result"];
        assert_eq!(result["valid"], false);
        assert_eq!(result["edits"], json!([]));
        assert_eq!(
            result["preconditions"],
            json!([{"code":"rule_value_stale","passed":false}])
        );
        assert_eq!(
            result["basis"],
            json!([{"id":task(),"revision":format!("synthetic-{NOW}-false")} ])
        );
        assert_eq!(
            result["criteria"],
            json!([
                {"code":"before","message":"Source absent.","passed":true},
                {"code":"stale_value","message":"Directory differs.","passed":false,"precondition":"rule_value_stale"},
                {"code":"after","message":"Label differs.","passed":false},
                {"code":"last","message":"Label absent.","passed":true}
            ])
        );
        assert_eq!(
            t.call(mcp, "apply", req, NOW + 1)["code"],
            "rule_value_stale"
        );
        assert!(t.logs().is_empty());
    }
}

fn clock_schema() -> Value {
    let mut s = schema();
    // This pattern is intentionally tighter than a $now rule destination:
    // a comparison only needs a string, not the intersection of two patterns.
    s["value_types"]["observation_time"] = json!({"base":"string","pattern":"[0-9]{4}-(0[1-9]|1[0-2])-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z"});
    for (name, op) in [("record_time", "le"), ("once", "eq")] {
        s["action_types"][name] = json!({
            "parameters":{"observed_at":{"type":{"value_type":"observation_time"},"required":true}},
            "submission_criteria":[{"code":"future_observation","message":"Observation exceeds application time.","all":[{op:["$observed_at","$now"]}]}],
            "rules":[{"create_object":"tag","set":{"label":"$now"}}]
        });
        s["grants"][0]["actions"]
            .as_array_mut()
            .unwrap()
            .push(json!(name));
    }
    s
}

#[test]
fn now_criterion_boundary_and_retry_cli_and_mcp() {
    for mcp in [false, true] {
        for reversed in [false, true] {
            let t = Temp::new();
            let mut s = clock_schema();
            if reversed {
                s["action_types"]["record_time"]["submission_criteria"][0]["all"][0] =
                    json!({"ge":["$now","$observed_at"]});
                s["action_types"]["once"]["submission_criteria"][0]["all"][0] =
                    json!({"eq":["$now","$observed_at"]});
            }
            t.install(s);
            let before_store = std::fs::read(t.store()).unwrap();
            let log = t.0.join("edits.jsonl");
            assert!(!log.exists());
            let future = json!({"action_type":"record_time","parameters":{"observed_at":"2026-11-12T00:00:01Z"},"request_id":"future"});
            let preview = t.call(mcp, "validate", future.clone(), NOW);
            assert_eq!(preview["result"]["valid"], false, "{preview}");
            assert_eq!(preview["result"]["application_time"], "set_at_application");
            assert_eq!(
                preview["result"]["criteria"][0]["code"],
                "future_observation"
            );
            assert_eq!(preview["result"]["criteria"][0]["passed"], false);
            let refused = t.call(mcp, "apply", future.clone(), NOW);
            assert_eq!(refused["code"], "submission_criteria", "{refused}");
            assert_eq!(refused["failed"], json!(["future_observation"]));
            assert_eq!(std::fs::read(t.store()).unwrap(), before_store);
            assert!(!log.exists());
            assert!(t.logs().is_empty());
            // Validation did not fix the clock: the same future input now passes.
            assert_eq!(
                t.call(mcp, "validate", future, NOW + 1)["result"]["valid"],
                true
            );
            for action in ["record_time", "once"] {
                let req = json!({"action_type":action,"parameters":{"observed_at":"2026-11-12T00:00:00Z"},"request_id":action});
                assert_eq!(
                    t.call(mcp, "validate", req.clone(), NOW)["result"]["valid"],
                    true
                );
                let applied = t.call(mcp, "apply", req.clone(), NOW);
                assert_eq!(applied["result"]["outcome"], "applied", "{applied}");
                let id = &applied["result"]["edits"][0]["id"];
                assert_eq!(t.fields(id)["label"]["value"], "2026-11-12T00:00:00Z");
                let before_store = std::fs::read(t.store()).unwrap();
                let before_log = std::fs::read(&log).unwrap();
                let retry = t.call(mcp, "apply", req.clone(), NOW + 1);
                assert_eq!(retry["result"]["outcome"], "already_applied", "{retry}");
                assert_eq!(retry["result"]["edits"], applied["result"]["edits"]);
                assert_eq!(std::fs::read(t.store()).unwrap(), before_store);
                assert_eq!(std::fs::read(&log).unwrap(), before_log);
                if action == "once" {
                    assert_eq!(
                        t.call(mcp, "validate", req.clone(), NOW + 1)["result"]["valid"],
                        false
                    );
                    // Even a clock outside the representable UTC range is never
                    // evaluated on retry of the applied request.
                    assert_eq!(
                        t.call(mcp, "apply", req, 253_402_300_800)["result"]["outcome"],
                        "already_applied"
                    );
                }
            }
            assert_eq!(t.logs().len(), 2);
        }
    }
}

#[test]
fn now_all_comparisons_explicit_properties_and_literals_cli_and_mcp() {
    let equal = "2026-11-12T00:00:00Z";
    for mcp in [false, true] {
        let t = Temp::new();
        let mut s = clock_schema();
        s["object_types"]["note"]["properties"]["title"]["type"] =
            json!({"value_type":"observation_time"});
        for op in ["eq", "ne", "lt", "le", "gt", "ge"] {
            let mut criteria = Vec::new();
            for (i, operand) in [json!("$value"), json!("$source.title"), json!(equal)]
                .into_iter()
                .enumerate()
            {
                for reversed in [false, true] {
                    let pair = if reversed {
                        json!(["$now", operand])
                    } else {
                        json!([operand, "$now"])
                    };
                    criteria.push(json!({"code":format!("clock_{i}_{reversed}"),"message":"Clock comparison failed.","all":[{op:pair}]}));
                }
            }
            let name = format!("clock_{op}");
            // No rule uses $now here: application_time must reflect criteria.
            s["action_types"][&name] = json!({
                "parameters":{"source":{"type":{"ref":"note"},"required":true},"value":{"type":"string","required":true}},
                "submission_criteria":criteria,"rules":[]
            });
            s["grants"][0]["actions"]
                .as_array_mut()
                .unwrap()
                .push(json!(name));
        }
        t.install(s);
        let source = t.call(mcp,"apply",json!({"action_type":"record_note","parameters":{"title":equal,"task":task()},"request_id":"seed"}),NOW)["result"]["edits"][0]["id"].clone();
        assert!(!source.is_null());
        for op in ["eq", "ne", "lt", "le", "gt", "ge"] {
            // Exercise true and false for all operators, both operand orders,
            // parameters, string-based properties, and literals on either side.
            for now in [NOW - 1, NOW, NOW + 1] {
                let req = json!({"action_type":format!("clock_{op}"),"parameters":{"source":source,"value":equal},"request_id":format!("{op}-{now}")});
                let preview = t.call(mcp, "validate", req.clone(), now);
                assert_eq!(preview["result"]["application_time"], "set_at_application");
                let mut all_passed = true;
                for (i, criterion) in preview["result"]["criteria"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .enumerate()
                {
                    let ordering = if i % 2 == 1 {
                        now.cmp(&NOW)
                    } else {
                        NOW.cmp(&now)
                    };
                    use std::cmp::Ordering::{Equal, Greater, Less};
                    let expected = match op {
                        "eq" => ordering == Equal,
                        "ne" => ordering != Equal,
                        "lt" => ordering == Less,
                        "le" => ordering != Greater,
                        "gt" => ordering == Greater,
                        "ge" => ordering != Less,
                        _ => unreachable!(),
                    };
                    assert_eq!(criterion["passed"], expected, "{op} {now} {i}: {preview}");
                    all_passed &= expected;
                }
                assert_eq!(preview["result"]["criteria"].as_array().unwrap().len(), 6);
                assert_eq!(preview["result"]["valid"], all_passed);
                let applied = t.call(mcp, "apply", req, now);
                if all_passed {
                    assert_eq!(applied["result"]["outcome"], "applied", "{applied}");
                } else {
                    assert_eq!(applied["code"], "submission_criteria", "{applied}");
                }
            }
        }
    }
}

#[test]
fn now_criterion_rejects_non_strings_and_other_contexts_with_pointers() {
    let t = Temp::new();
    for op in ["eq", "ne", "lt", "le", "gt", "ge"] {
        for reversed in [false, true] {
            for operand in [
                json!(42),
                json!(false),
                Value::Null,
                json!([]),
                json!({}),
                json!("$bad"),
                json!("$source.count"),
            ] {
                let mut s = clock_schema();
                s["object_types"]["note"]["properties"]["count"] = json!({"type":"integer"});
                s["action_types"]["record_time"]["parameters"]["source"] =
                    json!({"type":{"ref":"note"},"required":true});
                s["value_types"]["integer_value"] = json!({"base":"integer"});
                s["action_types"]["record_time"]["parameters"]["bad"] =
                    json!({"type":{"value_type":"integer_value"},"required":true});
                let pair = if reversed {
                    json!(["$now", operand])
                } else {
                    json!([operand, "$now"])
                };
                s["action_types"]["record_time"]["submission_criteria"][0]["all"][0] =
                    json!({op:pair});
                let index = usize::from(reversed);
                t.refuses_schema(
                    s,
                    "invalid_criterion",
                    &format!("/action_types/record_time/submission_criteria/0/all/0/{op}/{index}"),
                );
            }
        }
    }
    for condition in [
        json!({"exists":"$now"}),
        json!({"fresh":"$now"}),
        json!({"actor":"$now"}),
        json!({"in":["$now",["time"]]}),
    ] {
        let mut s = clock_schema();
        let kind = condition.as_object().unwrap().keys().next().unwrap();
        let suffix = if kind == "in" { "in/0" } else { kind };
        let pointer = format!("/action_types/record_time/submission_criteria/0/all/0/{suffix}");
        s["action_types"]["record_time"]["submission_criteria"][0]["all"][0] = condition;
        t.refuses_schema(s, "unknown_parameter", &pointer);
    }
    let mut s = clock_schema();
    s["action_types"]["record_time"]["parameters"]["now"] = json!({"type":"string"});
    t.refuses_schema(
        s,
        "invalid_name",
        "/action_types/record_time/parameters/now",
    );
    let mut view: Value =
        serde_json::from_slice(include_bytes!("fixtures/render-board.json")).unwrap();
    view["views"]["clock"] = json!({"object_type":"item","columns":[{"header":"Title","property":"title"}],"order":[],"filter":{"le":["title","$now"]}});
    t.refuses_schema(view, "invalid_schema", "/views/clock/filter");
}

#[test]
fn now_criterion_library_constructor_uses_injected_clock() {
    let t = Temp::new();
    t.install(clock_schema());
    let req: ont::action::ValidateActionRequest = serde_json::from_value(
        json!({"action_type":"record_time","parameters":{"observed_at":"2026-11-12T00:00:01Z"}}),
    )
    .unwrap();
    for (now, expected) in [(NOW, false), (NOW + 1, true)] {
        let ctx = ont::Context::new(t.store(), now);
        let ont::Answer::Enveloped { result, .. } =
            ont::action::validate_action(&ctx, &req).unwrap()
        else {
            panic!("expected envelope")
        };
        assert_eq!(result["valid"], expected);
        assert_eq!(result["application_time"], "set_at_application");
    }
}

#[test]
fn now_refused_throughout_linked_where_with_operand_pointers() {
    let t = Temp::new();
    let nesting: &[&[&str]] = &[&[], &["all"], &["any"], &["none"], &["all", "any", "none"]];
    for op in ["eq", "ne", "lt", "le", "gt", "ge"] {
        for reversed in [false, true] {
            for outer in ["all", "any", "none"] {
                for nested in nesting {
                    let mut s = schema();
                    let pair = if reversed {
                        json!(["$now", "until"])
                    } else {
                        json!(["until", "$now"])
                    };
                    let mut condition = json!({op:pair});
                    for arm in nested.iter().rev() {
                        condition = json!({*arm:[condition]});
                    }
                    s["action_types"]["set_hold"]["submission_criteria"] = json!([
                        {"code":"clock","message":"Clock check.","all":[
                            {"linked":{"from":"$task","link":"holds","where":{outer:[condition]}}}
                        ]}
                    ]);
                    let mut pointer = format!(
                        "/action_types/set_hold/submission_criteria/0/all/0/linked/where/{outer}/0"
                    );
                    for arm in *nested {
                        pointer.push_str(&format!("/{arm}/0"));
                    }
                    pointer.push_str(&format!("/{op}/{}", usize::from(!reversed)));
                    t.refuses_schema(s.clone(), "unknown_parameter", &pointer);
                    // The same legacy property/parameter comparison remains
                    // accepted, including every linked boolean group shape.
                    let legacy = s["action_types"]["set_hold"]["submission_criteria"]
                        .to_string()
                        .replace("\"$now\"", "\"$until\"");
                    s["action_types"]["set_hold"]["submission_criteria"] =
                        serde_json::from_str(&legacy).unwrap();
                    let checked = Schema::check_bytes(s.to_string().as_bytes(), &BTreeSet::new());
                    assert!(
                        checked.is_ok(),
                        "{op} {reversed} {outer} {nested:?}: {checked:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn now_literal_parameter_names_require_dollar_with_message() {
    let t = Temp::new();
    for op in ["eq", "ne", "lt", "le", "gt", "ge"] {
        for reversed in [false, true] {
            for name in ["observed_at", "label"] {
                let mut s = clock_schema();
                s["action_types"]["record_time"]["parameters"]["label"] = json!({"type":"string"});
                let pair = if reversed {
                    json!(["$now", name])
                } else {
                    json!([name, "$now"])
                };
                s["action_types"]["record_time"]["submission_criteria"][0]["all"][0] =
                    json!({op:pair});
                let p = t.0.join("literal-parameter.json");
                std::fs::write(&p, s.to_string()).unwrap();
                let answer = t.cli(&["schema", "check", p.to_str().unwrap()], json!({}), NOW);
                assert_eq!(answer["code"], "schema_violation", "{answer}");
                let pointer = format!(
                    "/action_types/record_time/submission_criteria/0/all/0/{op}/{}",
                    usize::from(reversed)
                );
                let violation = answer["violations"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|v| v["code"] == "invalid_criterion" && v["pointer"] == pointer)
                    .unwrap();
                assert_eq!(
                    violation["message"],
                    format!("Write ${name} to use the declared parameter.")
                );
                // The suggested explicit parameter form still checks clean.
                let explicit = if reversed {
                    json!(["$now", format!("${name}")])
                } else {
                    json!([format!("${name}"), "$now"])
                };
                s["action_types"]["record_time"]["submission_criteria"][0]["all"][0] =
                    json!({op:explicit});
                let checked = Schema::check_bytes(s.to_string().as_bytes(), &BTreeSet::new());
                assert!(checked.is_ok(), "{op} {reversed} {name}: {checked:?}");
            }
        }
    }
}

#[test]
fn now_changed_parameter_retry_refuses_request_id_reused_cli_and_mcp() {
    for mcp in [false, true] {
        let t = Temp::new();
        t.install(clock_schema());
        let mut req = json!({"action_type":"once","parameters":{"observed_at":"2026-11-12T00:00:00Z"},"request_id":"one-shot"});
        let applied = t.call(mcp, "apply", req.clone(), NOW);
        assert_eq!(applied["result"]["outcome"], "applied");
        let before_store = std::fs::read(t.store()).unwrap();
        let before_log = std::fs::read(t.0.join("edits.jsonl")).unwrap();
        req["parameters"]["observed_at"] = json!("2026-11-12T00:00:01Z");
        // Changed content is rejected whether its criterion would fail or pass
        // at the retry's clock; it never changes the original application.
        for now in [NOW, NOW + 1] {
            let refused = t.call(mcp, "apply", req.clone(), now);
            assert_eq!(refused["code"], "request_id_reused", "{refused}");
            assert_eq!(std::fs::read(t.store()).unwrap(), before_store);
            assert_eq!(std::fs::read(t.0.join("edits.jsonl")).unwrap(), before_log);
        }
        assert_eq!(t.logs().len(), 1);
    }
}

#[test]
fn now_criterion_only_any_none_validation_marker_cli_and_mcp() {
    for mcp in [false, true] {
        let t = Temp::new();
        let mut s = clock_schema();
        for arm in ["any", "none"] {
            s["action_types"][arm] = json!({
                "parameters":{"observed_at":{"type":"string","required":true}},
                "submission_criteria":[{"code":"clock","message":"Clock check.",arm:[{"eq":["$observed_at","$now"]}]}],
                "rules":[]
            });
            s["grants"][0]["actions"]
                .as_array_mut()
                .unwrap()
                .push(json!(arm));
        }
        t.install(s);
        let before = std::fs::read(t.store()).unwrap();
        for arm in ["any", "none"] {
            let req =
                json!({"action_type":arm,"parameters":{"observed_at":"2026-11-12T00:00:00Z"}});
            for now in [NOW, NOW + 1] {
                let preview = t.call(mcp, "validate", req.clone(), now);
                assert_eq!(preview["result"]["application_time"], "set_at_application");
                let expected = (arm == "any") == (now == NOW);
                assert_eq!(preview["result"]["valid"], expected, "{preview}");
                assert_eq!(preview["result"]["criteria"][0]["passed"], expected);
                assert_eq!(preview["result"]["edits"], json!([]));
            }
        }
        assert_eq!(std::fs::read(t.store()).unwrap(), before);
        assert!(!t.0.join("edits.jsonl").exists());
        assert!(t.logs().is_empty());
    }
}

#[test]
fn now_string_order_examples_and_full_utc_parameter_pattern_cli_and_mcp() {
    let counterexamples = ["", "2026-11-12", "2026-11-11T23:30:00-02:00"];
    for mcp in [false, true] {
        let t = Temp::new();
        let mut s = clock_schema();
        s["action_types"]["record_time"]["parameters"]["observed_at"]["type"] = json!("string");
        t.install(s);
        for (i, value) in counterexamples.iter().enumerate() {
            let req = json!({"action_type":"record_time","parameters":{"observed_at":value},"request_id":format!("lexical-{i}")});
            let preview = t.call(mcp, "validate", req.clone(), NOW);
            assert_eq!(preview["result"]["valid"], true, "{preview}");
            assert_eq!(preview["result"]["application_time"], "set_at_application");
            let applied = t.call(mcp, "apply", req, NOW);
            assert_eq!(applied["result"]["outcome"], "applied", "{applied}");
        }
        // Requiring the full UTC representation rejects all three strings at
        // parameter typing, before the lexical comparison can admit them.
        let typed = Temp::new();
        typed.install(clock_schema());
        let before = std::fs::read(typed.store()).unwrap();
        for (i, value) in counterexamples.iter().enumerate() {
            let req = json!({"action_type":"record_time","parameters":{"observed_at":value},"request_id":format!("typed-{i}")});
            for op in ["validate", "apply"] {
                assert_eq!(
                    typed.call(mcp, op, req.clone(), NOW)["code"],
                    "invalid_parameter"
                );
            }
        }
        assert_eq!(std::fs::read(typed.store()).unwrap(), before);
        assert!(!typed.0.join("edits.jsonl").exists());
        assert!(typed.logs().is_empty());
    }
}

#[test]
fn ordered_all_guards_optional_and_stale_operands_cli_and_mcp() {
    for mcp in [false, true] {
        let t = Temp::new();
        let mut s = schema();
        let parameters = json!({
            "record":{"type":{"ref":"task"},"required":true},
            "supersedes":{"type":{"ref":"task"}},
            "label":{"type":"string"}
        });
        for (name, criterion) in [
            (
                "guard_optional",
                json!({"none":[{"all":[{"exists":"$supersedes"},{"eq":["$record","$supersedes"]}]}]}),
            ),
            (
                "error_before_false",
                json!({"all":[{"eq":["$label","value"]},{"exists":"$label"}]}),
            ),
            (
                "top_guard_stale",
                json!({"all":[{"exists":"$label"},{"eq":["$record.directory","/synthetic/task"]}]}),
            ),
            (
                "nested_guard_stale",
                json!({"none":[{"all":[{"exists":"$label"},{"eq":["$record.directory","/synthetic/task"]}]}]}),
            ),
            (
                "any_stale",
                json!({"any":[{"exists":"$record"},{"eq":["$record.directory","/synthetic/task"]}]}),
            ),
            (
                "none_stale",
                json!({"none":[{"exists":"$record"},{"eq":["$record.directory","/synthetic/task"]}]}),
            ),
        ] {
            let mut criterion = criterion;
            criterion["code"] = json!("own_code");
            criterion["message"] = json!("Own criterion message.");
            s["action_types"][name] =
                json!({"parameters":parameters,"submission_criteria":[criterion]});
            s["grants"][0]["actions"]
                .as_array_mut()
                .unwrap()
                .push(json!(name));
        }
        t.install(s);
        let mut other = task();
        other["native_key"] = json!("other");
        for id in [task(), other.clone()] {
            put(
                &t,
                id,
                &[],
                json!({"directory":"/synthetic/task","state_sha256":"abc","checkpoint_line":"synthetic"}),
                NOW - 1,
                false,
            );
        }
        for (action, parameters, expected) in [
            ("guard_optional", json!({"record":task()}), "applied"),
            (
                "guard_optional",
                json!({"record":task(),"supersedes":null}),
                "applied",
            ),
            (
                "guard_optional",
                json!({"record":task(),"supersedes":task()}),
                "own_code",
            ),
            (
                "guard_optional",
                json!({"record":task(),"supersedes":other}),
                "applied",
            ),
            (
                "error_before_false",
                json!({"record":task()}),
                "criterion_not_evaluable",
            ),
            ("top_guard_stale", json!({"record":task()}), "own_code"),
            ("nested_guard_stale", json!({"record":task()}), "applied"),
            ("any_stale", json!({"record":task()}), "rule_value_stale"),
            ("none_stale", json!({"record":task()}), "rule_value_stale"),
        ] {
            let req = json!({"action_type":action,"parameters":parameters,"request_id":format!("ordered-{action}-{}", t.logs().len())});
            let before = t.logs().len();
            let validation = t.call(mcp, "validate", req.clone(), NOW);
            assert_eq!(t.logs().len(), before, "validation writes nothing");
            let applied = t.call(mcp, "apply", req, NOW);
            match expected {
                "applied" => {
                    assert_eq!(validation["result"]["valid"], true, "{validation}");
                    assert_eq!(applied["result"]["outcome"], "applied", "{applied}");
                    // A skipped stale operand contributes neither basis nor precondition.
                    if action == "nested_guard_stale" {
                        assert_eq!(validation["result"]["basis"], json!([]));
                    }
                }
                "own_code" => {
                    assert_eq!(validation["result"]["valid"], false, "{validation}");
                    assert_eq!(applied["code"], "submission_criteria", "{applied}");
                    assert_eq!(applied["failed"], json!(["own_code"]));
                    assert_eq!(applied["message"], "Own criterion message.");
                    assert_eq!(t.logs().len(), before);
                }
                "rule_value_stale" => {
                    assert_eq!(validation["result"]["valid"], false, "{validation}");
                    assert_eq!(validation["result"]["preconditions"][0]["code"], expected);
                    assert_eq!(applied["code"], expected, "{applied}");
                    assert_eq!(t.logs().len(), before);
                }
                _ => {
                    assert_eq!(validation["code"], expected, "{validation}");
                    assert_eq!(applied["code"], expected, "{applied}");
                    assert_eq!(t.logs().len(), before);
                }
            }
        }
    }
}

#[test]
fn criterion_and_modify_reads_hide_unreadable_heads_cli_and_mcp() {
    for mcp in [false, true] {
        for action in [
            "read_exists",
            "read_fresh",
            "read_forward",
            "read_reverse",
            "edit_note",
        ] {
            for op in ["validate", "apply"] {
                for candidate_only in [false, true] {
                    if candidate_only && action != "read_forward" && action != "read_reverse" {
                        continue;
                    }
                    let mut missing = None;
                    for visibility in ["missing", "hidden", "readable"] {
                        let t = Temp::new();
                        let mut s = schema();
                        s["scope_keys"] = json!(["tenant"]);
                        s["grants"][0]["scope"] = json!({"tenant":"allowed"});
                        s["object_types"]["task"]["owner"] = json!("ontology");
                        s["object_types"]["task"]["primary_key"] = json!({"from":"$action_id"});
                        for prop in s["object_types"]["task"]["properties"]
                            .as_object_mut()
                            .unwrap()
                            .values_mut()
                        {
                            prop.as_object_mut().unwrap().remove("from");
                        }
                        for (name, condition) in [
                            ("read_exists", json!({"exists":"$note"})),
                            ("read_fresh", json!({"fresh":"$note"})),
                            (
                                "read_forward",
                                json!({"linked":{"from":"$note","link":"note_task"}}),
                            ),
                            (
                                "read_reverse",
                                json!({"linked":{"from":"$task","link":"notes"}}),
                            ),
                        ] {
                            s["action_types"][name] = json!({
                                "parameters":{"note":{"type":{"ref":"note"},"required":true},"task":{"type":{"ref":"task"},"required":true}},
                                "submission_criteria":[{"code":"read_denied","message":"Unavailable.","all":[condition]}]
                            });
                            s["grants"][0]["actions"]
                                .as_array_mut()
                                .unwrap()
                                .push(json!(name));
                        }
                        t.install(s);
                        let note = json!({"object_type":"note","namespace":task()["namespace"],"native_key":"target"});
                        for (id, fields, is_source) in [
                            (
                                note.clone(),
                                json!({"title":"source","task":task()}),
                                action == "read_forward",
                            ),
                            (
                                task(),
                                json!({"directory":"/synthetic/task","state_sha256":"abc","checkpoint_line":"synthetic"}),
                                action == "read_reverse",
                            ),
                        ] {
                            let always_readable = candidate_only && is_source;
                            if visibility == "missing" && !always_readable {
                                continue;
                            }
                            let tenant = if always_readable || visibility == "readable" {
                                "allowed"
                            } else {
                                "blocked"
                            };
                            put(&t, id, &[("tenant", tenant)], fields, NOW + 10, false);
                        }
                        let parameters = if action == "edit_note" {
                            json!({"note":note,"title":"changed"})
                        } else {
                            json!({"note":note,"task":task()})
                        };
                        let req = json!({"action_type":action,"parameters":parameters,"request_id":"admission"});
                        let line = t.action_line(mcp, op, req, NOW);
                        match visibility {
                            "missing" => {
                                missing = Some(line);
                            }
                            "hidden" => {
                                assert_eq!(
                                    Some(line),
                                    missing,
                                    "{action} {op} candidate={candidate_only}"
                                );
                            }
                            _ => {
                                let answer: Value = serde_json::from_str(&line).unwrap();
                                assert_eq!(answer["status"], "ok", "{answer}");
                                if op == "validate" {
                                    assert_eq!(answer["result"]["valid"], true, "{answer}");
                                } else {
                                    assert_eq!(answer["result"]["outcome"], "applied", "{answer}");
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
