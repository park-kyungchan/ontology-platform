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
        self.tool(&format!("ont_action_{op}"), input, now)
    }
    fn tool(&self, tool: &str, input: Value, now: u64) -> Value {
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
        let body: Value = serde_json::from_str(text).unwrap();
        assert_eq!(response["result"]["isError"], body["status"] != "ok");
        body
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
fn reference_read_scope_refuses_like_get_and_admitted_copy_works() {
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
                let get = t.cli(&["get"], json!({"id":id}), NOW);
                for op in ["validate", "apply"] {
                    let mut with_expected = req.clone();
                    with_expected["expected"] = json!([{"id":id,"revision":"untrusted-revision"}]);
                    let expected_refusal = t.call(mcp, op, with_expected, NOW);
                    assert_eq!(
                        expected_refusal["code"], "unavailable",
                        "{expected_refusal}"
                    );
                    let refused = t.call(mcp, op, req.clone(), NOW);
                    assert_eq!(refused["code"], get["code"], "{refused}");
                    assert_eq!(refused["code"], "unavailable");
                    for field in ["id", "parameter", "property", "message", "basis"] {
                        assert!(refused.get(field).is_none(), "{refused}");
                    }
                }
                let criterion=t.call(mcp,"apply",json!({"action_type":"check_title","parameters":{"source":id,"expected_title":"source"},"request_id":"hidden-criterion"}),NOW);
                assert_eq!(criterion["code"], "unavailable", "{criterion}");
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
            let retry = t.call(mcp, "apply", req, NOW + 1);
            assert_eq!(retry["result"]["outcome"], "already_applied");
            assert_eq!(retry["result"]["edits"], json!([]));
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
