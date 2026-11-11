//! Action tests: temporary stores only, through `ont::run` and the
//! public library API.
#![allow(clippy::unwrap_used)]
use ont::action::{__set_fail_after_commit, log_line};
use ont::{Context, Failure};
use ont_store::Store;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const FIXTURE: &[u8] = include_bytes!("fixtures/action-work.json");
const EXAMPLE: &[u8] = include_bytes!("../../ont-core/tests/schema-fixtures/example.json");
const NOW: u64 = 1000;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "ont-action-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn store(&self) -> String {
        self.0.join("store.sqlite").display().to_string()
    }
    fn store_path(&self) -> PathBuf {
        self.0.join("store.sqlite")
    }
    fn edits(&self) -> PathBuf {
        self.0.join("edits.jsonl")
    }
    fn file(&self, name: &str, bytes: &[u8]) -> String {
        let p = self.0.join(name);
        std::fs::write(&p, bytes).unwrap();
        p.display().to_string()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn run(args: &[&str], stdin: &[u8]) -> (i32, Value) {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let out = ont::run(&args, stdin, 0, 0);
    let value: Value = if out.stdout.trim().is_empty() {
        json!({"stderr": out.stderr})
    } else {
        serde_json::from_str(out.stdout.trim()).unwrap()
    };
    (out.code, value)
}
fn run_store(t: &Temp, cmd: &[&str], stdin: &Value) -> (i32, Value) {
    let store = t.store();
    let now = NOW.to_string();
    let mut args: Vec<&str> = vec!["--store", &store, "--now", &now];
    args.extend_from_slice(cmd);
    run(&args, &serde_json::to_vec(stdin).unwrap())
}
fn install(t: &Temp) -> Value {
    install_bytes(t, FIXTURE)
}
fn install_bytes(t: &Temp, bytes: &[u8]) -> Value {
    let file = t.file("schema.json", bytes);
    let (code, ans) = run(&["--store", &t.store(), "schema", "install", &file], b"");
    assert_eq!(code, 0, "{ans}");
    ans
}
fn ns() -> ont_core::Namespace {
    ont_core::Namespace {
        instance: "vps://palantirkc".into(),
        account: "root".into(),
    }
}
fn task_id() -> Value {
    json!({"object_type":"task","namespace":{"instance":"vps://palantirkc","account":"root"},"native_key":"task1"})
}
fn id_json(object_type: &str, key: &str) -> Value {
    json!({"object_type":object_type,"namespace":{"instance":"vps://palantirkc","account":"root"},"native_key":key})
}
fn append_task(t: &Temp) {
    use ont_core::{Coverage, Id, Knowledge, Lineage, Observation, Scope};
    let mut store = Store::open_schema(&t.store_path()).unwrap();
    let fields = BTreeMap::from([
        (
            "directory".to_string(),
            Knowledge::Value(json!("/tmp/task1")),
        ),
        ("state_sha256".to_string(), Knowledge::Value(json!("abc"))),
        (
            "checkpoint_line".to_string(),
            Knowledge::Value(json!("line")),
        ),
    ]);
    store
        .append_schema(
            &[ont_store::Append {
                id: Id {
                    object_type: "task".into(),
                    namespace: ns(),
                    native_key: "task1".into(),
                },
                scope: Scope::domain(ns(), "work", BTreeMap::new()).unwrap(),
                observation: Observation {
                    revision: "r1".into(),
                    source_revision: "s1".into(),
                    observed_at: 10,
                    fresh_until: 2000,
                    change: ont_core::Change::Initial,
                    supersedes: None,
                    coverage: Coverage::Complete,
                    fields,
                    lineage: Lineage {
                        artifact_sha256: "a".repeat(64),
                        pointer: "/0".into(),
                    },
                },
            }],
            15,
        )
        .unwrap();
}
fn setup() -> Temp {
    let t = Temp::new();
    install(&t);
    append_task(&t);
    t
}
fn apply(t: &Temp, body: Value) -> (i32, Value) {
    run_store(t, &["action", "apply"], &body)
}
fn counts(t: &Temp) -> (usize, usize) {
    let mut store = Store::open_schema(&t.store_path()).unwrap();
    let log = store.action_log().unwrap().len();
    let holds = store.scan_heads("hold", 100, None).unwrap().len();
    (log, holds)
}

#[test]
fn list_validate_apply_and_failed_criterion_writes_nothing() {
    let t = setup();
    let (code, ans) = run_store(&t, &["action", "list"], &json!({"op":"list_actions"}));
    assert_eq!(code, 0, "{ans}");
    let names: Vec<&str> = ans["result"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"set_hold"), "{names:?}");
    let set_hold = ans["result"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "set_hold")
        .unwrap();
    assert_eq!(set_hold["edits_object_types"], json!(["hold"]));
    assert_eq!(set_hold["criteria"][0]["code"], "task_not_current");
    // describe lists action types too.
    let (code, ans) = run_store(&t, &["describe"], &json!({}));
    assert_eq!(code, 0, "{ans}");
    assert!(
        ans["result"]["action_types"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["name"] == "set_hold")
    );
    // Validate a good set_hold.
    let body = json!({"op":"validate_action","action_type":"set_hold","agent":"lead-exec",
        "parameters":{"task":task_id(),"effect":"merge","until":"later"}});
    let (code, ans) = run_store(&t, &["action", "validate"], &body);
    assert_eq!(code, 0, "{ans}");
    assert_eq!(ans["result"]["valid"], true, "{ans}");
    assert!(
        ans["result"]["criteria"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["passed"] == true)
    );
    assert_eq!(counts(&t), (0, 0), "validate writes nothing");
    // Apply it.
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"set_hold","agent":"lead-exec",
        "parameters":{"task":task_id(),"effect":"merge","until":"later"}}),
    );
    assert_eq!(code, 0, "{ans}");
    assert_eq!(ans["result"]["outcome"], "applied");
    let hold_key = ans["result"]["edits"][0]["id"]["native_key"]
        .as_str()
        .unwrap()
        .to_string();
    // Duplicate active hold: criterion fails, nothing written.
    let before = counts(&t);
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"set_hold",
        "parameters":{"task":task_id(),"effect":"merge","until":"later"}}),
    );
    assert_eq!(code, 2, "{ans}");
    assert_eq!(ans["code"], "submission_criteria");
    assert_eq!(ans["failed"], json!(["duplicate_active_hold"]));
    assert_eq!(counts(&t), before);
    // Search returns the hold; get shows it.
    let (code, ans) = run_store(
        &t,
        &["search"],
        &json!({"op":"search","object_type":"hold","where":{"eq":["status","active"]}}),
    );
    assert_eq!(code, 0, "{ans}");
    assert_eq!(ans["result"]["objects"].as_array().unwrap().len(), 1);
    let (code, ans) = run_store(&t, &["get"], &json!({"id": id_json("hold", &hold_key)}));
    assert_eq!(code, 0, "{ans}");
    assert_eq!(
        ans["result"]["objects"][0]["observations"][0]["fields"]["status"]["value"],
        "active"
    );
}

#[test]
fn expected_conflict_forbidden_and_invalid_parameters() {
    let t = setup();
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"set_hold",
        "parameters":{"task":task_id(),"effect":"merge","until":"later"}}),
    );
    assert_eq!(code, 0, "{ans}");
    let hold_key = ans["result"]["edits"][0]["id"]["native_key"]
        .as_str()
        .unwrap()
        .to_string();
    let hold_rev = ans["result"]["edits"][0]["revision"]
        .as_str()
        .unwrap()
        .to_string();
    let expected = json!([{"id": id_json("hold", &hold_key), "revision": hold_rev}]);
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"lift_hold",
        "parameters":{"hold": id_json("hold", &hold_key)}, "expected": expected}),
    );
    assert_eq!(code, 0, "{ans}");
    // Second apply with the same expected: revision_conflict.
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"lift_hold",
        "parameters":{"hold": id_json("hold", &hold_key)},
        "expected": [{"id": id_json("hold", &hold_key), "revision": hold_rev}]}),
    );
    assert_eq!(code, 2, "{ans}");
    assert_eq!(ans["code"], "revision_conflict");
    assert_eq!(ans["id"]["native_key"], hold_key);
    // Wrong type and missing required.
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"set_hold",
        "parameters":{"task":task_id(),"effect":42,"until":"later"}}),
    );
    assert_eq!(
        (code, ans["code"].as_str(), ans["parameter"].as_str()),
        (2, Some("invalid_parameter"), Some("effect")),
        "{ans}"
    );
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"set_hold",
        "parameters":{"task":task_id(),"effect":"x"}}),
    );
    assert_eq!(
        (code, ans["code"].as_str(), ans["parameter"].as_str()),
        (2, Some("invalid_parameter"), Some("until")),
        "{ans}"
    );
    // Missing grant.
    let t2 = Temp::new();
    let mut variant: Value = serde_json::from_slice(FIXTURE).unwrap();
    variant["grants"][0]["actions"] = json!(["record_note"]);
    install_bytes(&t2, &serde_json::to_vec(&variant).unwrap());
    append_task(&t2);
    let (code, ans) = apply(
        &t2,
        json!({"op":"apply_action","action_type":"set_hold",
        "parameters":{"task":task_id(),"effect":"merge","until":"later"}}),
    );
    assert_eq!(
        (code, ans["code"].as_str()),
        (2, Some("action_forbidden")),
        "{ans}"
    );
}

#[test]
fn create_modify_delete_history_and_search() {
    let t = setup();
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"record_note",
        "parameters":{"title":"first","task":task_id()}}),
    );
    assert_eq!(code, 0, "{ans}");
    let note_key = ans["result"]["edits"][0]["id"]["native_key"]
        .as_str()
        .unwrap()
        .to_string();
    let note_id = id_json("note", &note_key);
    // Modify through create-or-modify with the note reference.
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"upsert_note",
        "parameters":{"note":note_id,"title":"second","task":task_id()}}),
    );
    assert_eq!(code, 0, "{ans}");
    // Set then clear the reference.
    let (code, _) = apply(
        &t,
        json!({"op":"apply_action","action_type":"link_note",
        "parameters":{"note":note_id,"task":task_id()}}),
    );
    assert_eq!(code, 0);
    let (code, _) = apply(
        &t,
        json!({"op":"apply_action","action_type":"clear_note_task",
        "parameters":{"note":note_id}}),
    );
    assert_eq!(code, 0);
    // Delete.
    let (code, _) = apply(
        &t,
        json!({"op":"apply_action","action_type":"delete_note",
        "parameters":{"note":note_id}}),
    );
    assert_eq!(code, 0);
    let (code, ans) = run_store(&t, &["get"], &json!({"id": note_id}));
    assert_eq!(code, 0, "{ans}");
    let obs = ans["result"]["objects"][0]["observations"]
        .as_array()
        .unwrap();
    assert!(obs.len() >= 4, "{obs:?}");
    assert_eq!(obs[0]["fields"]["title"]["value"], "first");
    assert_eq!(obs[1]["fields"]["title"]["value"], "second");
    assert_eq!(obs.last().unwrap()["change"], "retraction");
    let (code, ans) = run_store(
        &t,
        &["search"],
        &json!({"op":"search","object_type":"note","where":{"eq":["title","second"]}}),
    );
    assert_eq!(code, 0, "{ans}");
    assert_eq!(ans["result"]["objects"].as_array().unwrap().len(), 0);
}

#[test]
fn create_or_modify_creates_then_modifies_and_tag_key_from_action_id() {
    let t = setup();
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"upsert_note",
        "parameters":{"title":"made","task":task_id()}}),
    );
    assert_eq!(code, 0, "{ans}");
    assert_eq!(ans["result"]["edits"][0]["change"], "initial");
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"create_tag",
        "parameters":{"label":"hello"}}),
    );
    assert_eq!(code, 0, "{ans}");
    let mytag = ans["result"]["action_id"].as_str().unwrap().to_string();
    assert_eq!(ans["result"]["edits"][0]["id"]["native_key"], mytag);
    let (code, ans) = run_store(&t, &["get"], &json!({"id": id_json("tag", &mytag)}));
    assert_eq!(code, 0, "{ans}");
    assert_eq!(
        ans["result"]["objects"][0]["observations"][0]["fields"]["label"]["value"],
        "hello"
    );
}

#[test]
fn idempotent_retry_reused_validate_equals_apply_and_log_line() {
    let t = setup();
    // validate == apply edits (deterministic allocation, no request_id).
    let (code, v) = run_store(
        &t,
        &["action", "validate"],
        &json!({"op":"validate_action","action_type":"record_note",
        "parameters":{"title":"same","task":task_id()}}),
    );
    assert_eq!(code, 0, "{v}");
    let (code, a) = apply(
        &t,
        json!({"op":"apply_action","action_type":"record_note",
        "parameters":{"title":"same","task":task_id()}}),
    );
    assert_eq!(code, 0, "{a}");
    assert_eq!(v["result"]["edits"], a["result"]["edits"]);
    // Idempotent retry with request_id.
    let body = json!({"op":"apply_action","action_type":"record_note","request_id":"req-1",
        "parameters":{"title":"once","task":task_id()}});
    let (code, first) = apply(&t, body.clone());
    assert_eq!(code, 0, "{first}");
    let before = counts(&t);
    let (code, second) = apply(&t, body);
    assert_eq!(code, 0, "{second}");
    assert_eq!(second["result"]["outcome"], "already_applied");
    assert_eq!(second["result"]["action_id"], first["result"]["action_id"]);
    assert_eq!(counts(&t), before, "retry writes nothing");
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"record_note","request_id":"req-1",
        "parameters":{"title":"different","task":task_id()}}),
    );
    assert_eq!(
        (code, ans["code"].as_str()),
        (2, Some("request_id_reused")),
        "{ans}"
    );
    // edits.jsonl line equals the action_log row.
    let mut store = Store::open_schema(&t.store_path()).unwrap();
    let log = store.action_log().unwrap();
    let text = std::fs::read_to_string(t.edits()).unwrap();
    let lines: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), log.len());
    for ((seq, entry), line) in log.iter().zip(lines.iter()) {
        assert_eq!(
            *line,
            serde_json::from_str::<Value>(&log_line(*seq, entry)).unwrap()
        );
    }
    // Mode 0600.
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(t.edits()).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn crash_between_commit_and_append_is_repaired_by_next_write() {
    let t = setup();
    let ctx = Context::new(t.store_path(), NOW);
    let req = ont::action::ApplyActionRequest {
        op: Some("apply_action".into()),
        action_type: "record_note".into(),
        parameters: json!({"title":"crashed","task":task_id()}),
        agent: Some("tester".into()),
        session: None,
        expected: None,
        request_id: Some("crash-1".into()),
    };
    __set_fail_after_commit(true);
    let err = ont::action::apply_action(&ctx, &req);
    assert!(matches!(err, Err(Failure::Internal(_))), "{err:?}");
    __set_fail_after_commit(false);
    let mut store = Store::open_schema(&t.store_path()).unwrap();
    assert_eq!(store.action_log().unwrap().len(), 1, "commit happened");
    let text = std::fs::read_to_string(t.edits()).unwrap_or_default();
    assert_eq!(text.lines().count(), 0, "append did not happen");
    drop(store);
    // Next write repairs the file.
    let (code, _) = apply(
        &t,
        json!({"op":"apply_action","action_type":"record_note",
        "parameters":{"title":"next","task":task_id()}}),
    );
    assert_eq!(code, 0);
    let mut store = Store::open_schema(&t.store_path()).unwrap();
    let log = store.action_log().unwrap();
    let text = std::fs::read_to_string(t.edits()).unwrap();
    assert_eq!(text.lines().count(), log.len());
    let first_line: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(
        first_line,
        serde_json::from_str::<Value>(&log_line(log[0].0, &log[0].1)).unwrap()
    );
    // Attribution reaches the log.
    assert_eq!(log[0].1.actor["agent"], "tester");
    assert_eq!(log[0].1.actor["principal"], "agent");
    assert_eq!(log[0].1.actor["client"], "cli");
}

#[test]
fn source_owned_rule_refused_by_schema_check_and_old_store_opens() {
    let t = Temp::new();
    let mut bad: Value = serde_json::from_slice(FIXTURE).unwrap();
    bad["action_types"]["edit_task"] = json!({
        "parameters": {}, "submission_criteria": [],
        "rules": [{"create_object": "task", "set": {}}]
    });
    bad["grants"][0]["actions"] = json!(["edit_task"]);
    let file = t.file("bad.json", &serde_json::to_vec(&bad).unwrap());
    let (code, ans) = run(&["--store", &t.store(), "schema", "check", &file], b"");
    assert_eq!(code, 2, "{ans}");
    assert!(
        ans.to_string().contains("action_edits_source_owned"),
        "{ans}"
    );
    // A store written with only the pre-existing APIs opens for actions.
    let mut store = Store::open_schema(&t.store_path()).unwrap();
    store
        .install_schema(
            FIXTURE,
            ont_store::schema_mode::InstallArgs {
                installed_at: 1,
                ..Default::default()
            },
        )
        .unwrap();
    drop(store);
    append_task(&t);
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"record_note",
        "parameters":{"title":"old-store","task":task_id()}}),
    );
    assert_eq!(code, 0, "{ans}");
    // Existing example fixture keeps its pin.
    let t2 = Temp::new();
    let file = t2.file("example.json", EXAMPLE);
    let (code, ans) = run(&["--store", &t2.store(), "schema", "check", &file], b"");
    assert_eq!(code, 0, "{ans}");
    assert_eq!(
        ans["pin"]["sha256"],
        "1f7c885dc163d8d6ffbaceb573f178bc239d890069effe93c8f1626ee0132ad0"
    );
}

#[test]
fn embedding_uses_only_public_library_items() {
    let t = Temp::new();
    // A domain server opens its own store file and installs its schema
    // through the store, then uses only `ont` public items.
    let mut store = Store::open_schema(&t.store_path()).unwrap();
    store
        .install_schema(
            FIXTURE,
            ont_store::schema_mode::InstallArgs {
                installed_at: 1,
                ..Default::default()
            },
        )
        .unwrap();
    drop(store);
    append_task(&t);
    let mut ctx = Context::new(t.store_path(), NOW);
    ctx.client = "embed-test".into();
    ctx.agent = Some("embedder".into());
    let list = ont::action::list_actions(
        &ctx,
        &ont::action::ListActionsRequest {
            op: None,
            domain: None,
        },
    )
    .unwrap();
    let ont::Answer::Enveloped { result, .. } = list else {
        panic!("enveloped")
    };
    assert!(
        result["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["name"] == "set_hold")
    );
    let val = ont::action::validate_action(
        &ctx,
        &ont::action::ValidateActionRequest {
            op: None,
            action_type: "record_note".into(),
            parameters: json!({"title":"embedded","task":task_id()}),
            agent: None,
            session: Some("s1".into()),
            expected: None,
        },
    )
    .unwrap();
    let ont::Answer::Enveloped { result, .. } = val else {
        panic!("enveloped")
    };
    assert_eq!(result["valid"], true);
    let app = ont::action::apply_action(
        &ctx,
        &ont::action::ApplyActionRequest {
            op: None,
            action_type: "record_note".into(),
            parameters: json!({"title":"embedded","task":task_id()}),
            agent: None,
            session: Some("s1".into()),
            expected: None,
            request_id: None,
        },
    )
    .unwrap();
    let ont::Answer::Enveloped { result, .. } = app else {
        panic!("enveloped")
    };
    assert_eq!(result["outcome"], "applied");
    let note_key = result["edits"][0]["id"]["native_key"]
        .as_str()
        .unwrap()
        .to_string();
    let got = ont::get::get(
        &ctx,
        &ont::get::GetRequest {
            op: None,
            id: serde_json::from_value(id_json("note", &note_key)).unwrap(),
        },
    )
    .unwrap();
    let ont::Answer::Enveloped { result, .. } = got else {
        panic!("enveloped")
    };
    assert_eq!(
        result["objects"][0]["observations"][0]["fields"]["title"]["value"],
        "embedded"
    );
    let found = ont::search::search(
        &ctx,
        &ont::search::SearchRequest {
            op: None,
            object_type: "note".into(),
            filter: Some(ont::search::Filter::Eq("title".into(), json!("embedded"))),
            limit: None,
            continuation: None,
        },
    )
    .unwrap();
    let ont::Answer::Enveloped { result, .. } = found else {
        panic!("enveloped")
    };
    assert_eq!(result["objects"].as_array().unwrap().len(), 1);
    let mut store = Store::open_schema(&t.store_path()).unwrap();
    let log = store.action_log().unwrap();
    assert_eq!(log[0].1.actor["client"], "embed-test");
    assert_eq!(log[0].1.actor["session"], "s1");
}
