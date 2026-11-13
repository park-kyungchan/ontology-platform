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
    // Retrying the crashed request_id: the caller saw an internal
    // error, the retry is already_applied and repairs the file.
    let retried = ont::action::apply_action(&ctx, &req).unwrap();
    let ont::Answer::Enveloped { result, .. } = retried else {
        panic!("enveloped")
    };
    assert_eq!(result["outcome"], "already_applied");
    assert_eq!(result["edits_log"], "written");
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
            request_id: None,
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

fn full_counts(t: &Temp) -> (usize, Vec<usize>, Vec<u8>) {
    let mut store = Store::open_schema(&t.store_path()).unwrap();
    let log = store.action_log().unwrap().len();
    let types = ["hold", "note", "tag", "widget", "probe"];
    let heads = types
        .iter()
        .map(|ty| store.scan_heads(ty, 100, None).unwrap().len())
        .collect();
    (log, heads, std::fs::read(t.edits()).unwrap_or_default())
}
fn assert_nothing_written(t: &Temp, before: &(usize, Vec<usize>, Vec<u8>)) {
    assert_eq!(&full_counts(t), before);
}
fn make_note(t: &Temp, title: &str) -> (String, Value) {
    let (code, ans) = apply(
        t,
        json!({"op":"apply_action","action_type":"record_note","parameters":{"title":title,"task":task_id()}}),
    );
    assert_eq!(code, 0, "{ans}");
    let key = ans["result"]["edits"][0]["id"]["native_key"]
        .as_str()
        .unwrap()
        .to_string();
    let id = id_json("note", &key);
    (key, id)
}

#[test]
fn b1_unwritable_log_is_pending_and_retry_already_applied() {
    let t = setup();
    std::fs::remove_file(t.edits()).ok();
    std::fs::create_dir(t.edits()).unwrap();
    let body = json!({"op":"apply_action","action_type":"record_note","request_id":"b1-1","parameters":{"title":"b1","task":task_id()}});
    let (code, ans) = apply(&t, body.clone());
    assert_eq!(code, 0, "{ans}");
    assert_eq!(ans["result"]["outcome"], "applied");
    assert_eq!(ans["result"]["edits_log"], "pending");
    assert_eq!(ans["result"]["actor_trust"], "self_reported");
    let mut store = Store::open_schema(&t.store_path()).unwrap();
    assert_eq!(store.action_log().unwrap().len(), 1);
    assert_eq!(
        store.action_log().unwrap()[0].1.actor["actor_trust"],
        "self_reported"
    );
    drop(store);
    let (code, ans) = apply(&t, body);
    assert_eq!(code, 0, "{ans}");
    assert_eq!(ans["result"]["outcome"], "already_applied");
}

#[test]
fn b2_truncated_extra_changed_and_non_utf8_are_repaired() {
    // Truncated mid-line.
    let t = setup();
    let (_, _) = make_note(&t, "one");
    let text = std::fs::read(t.edits()).unwrap();
    std::fs::write(t.edits(), &text[..text.len() - 8]).unwrap();
    let (_, _) = make_note(&t, "two");
    let mut store = Store::open_schema(&t.store_path()).unwrap();
    let log = store.action_log().unwrap();
    drop(store);
    let text = std::fs::read_to_string(t.edits()).unwrap();
    assert_eq!(text.lines().count(), log.len());
    for ((seq, e), line) in log.iter().zip(text.lines()) {
        assert_eq!(line, log_line(*seq, e));
    }
    let (_, _) = make_note(&t, "three");
    assert_eq!(
        std::fs::read_to_string(t.edits()).unwrap().lines().count(),
        3
    );
    // Extra line -> diverged, rebuilt.
    let t = setup();
    let (_, _) = make_note(&t, "one");
    let mut bytes = std::fs::read(t.edits()).unwrap();
    bytes.extend_from_slice(b"{\"seq\":999}\n");
    std::fs::write(t.edits(), &bytes).unwrap();
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"record_note","parameters":{"title":"two","task":task_id()}}),
    );
    assert_eq!(code, 0, "{ans}");
    assert_eq!(ans["result"]["edits_log"], "rebuilt");
    assert!(
        std::fs::read_dir(&t.0)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e
                .file_name()
                .to_string_lossy()
                .starts_with("edits.jsonl.diverged-"))
    );
    assert_eq!(
        std::fs::read_to_string(t.edits()).unwrap().lines().count(),
        2
    );
    // Changed line -> diverged.
    let t = setup();
    let (_, _) = make_note(&t, "one");
    let text = std::fs::read_to_string(t.edits())
        .unwrap()
        .replace("one", "eno");
    std::fs::write(t.edits(), text).unwrap();
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"record_note","parameters":{"title":"two","task":task_id()}}),
    );
    assert_eq!(ans["result"]["edits_log"], "rebuilt", "{ans}");
    assert_eq!(code, 0);
    // Non-UTF-8 complete line -> diverged.
    let t = setup();
    let (_, _) = make_note(&t, "one");
    let mut bytes = std::fs::read(t.edits()).unwrap();
    bytes.extend_from_slice(&[0xff, b'\n']);
    std::fs::write(t.edits(), &bytes).unwrap();
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"record_note","parameters":{"title":"two","task":task_id()}}),
    );
    assert_eq!(ans["result"]["edits_log"], "rebuilt", "{ans}");
    assert_eq!(code, 0);
    assert_eq!(
        std::fs::read_to_string(t.edits()).unwrap().lines().count(),
        2
    );
    // Wrong mode -> pending.
    let t = setup();
    let (_, _) = make_note(&t, "one");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(t.edits(), std::fs::Permissions::from_mode(0o644)).unwrap();
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"record_note","parameters":{"title":"two","task":task_id()}}),
    );
    assert_eq!(code, 0, "{ans}");
    assert_eq!(ans["result"]["edits_log"], "pending");
}

#[test]
fn b2_concurrent_repair_no_duplicates() {
    let t = setup();
    let (_, _) = make_note(&t, "one");
    let mut store = Store::open_schema(&t.store_path()).unwrap();
    let log = store.action_log().unwrap();
    drop(store);
    std::fs::write(t.edits(), b"").unwrap();
    let path = t.store_path();
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let path = path.clone();
            std::thread::spawn(move || ont::action::sync_edits_file(&path))
        })
        .collect();
    for h in handles {
        let _ = h.join().unwrap();
    }
    let text = std::fs::read_to_string(t.edits()).unwrap();
    assert_eq!(text.lines().count(), log.len());
    assert_eq!(text.lines().next().unwrap(), log_line(log[0].0, &log[0].1));
}

#[test]
fn b3_criteria_tri_state_and_literal_operands() {
    let t = setup();
    // Missing label property: ge, none and any are unevaluable.
    let (_, note_id) = make_note(&t, "first");
    for action in [
        "check_note_label_ge",
        "check_note_label_none",
        "check_note_label_any",
    ] {
        let before = full_counts(&t);
        let (code, ans) = apply(
            &t,
            json!({"op":"apply_action","action_type":action,"parameters":{"note":note_id}}),
        );
        assert_eq!(
            (code, ans["code"].as_str()),
            (2, Some("criterion_not_evaluable")),
            "{action} {ans}"
        );
        assert_nothing_written(&t, &before);
    }
    // Retracted object: ne is unevaluable.
    let (code, _) = apply(
        &t,
        json!({"op":"apply_action","action_type":"delete_note","parameters":{"note":note_id}}),
    );
    assert_eq!(code, 0);
    let before = full_counts(&t);
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"check_note_ne","parameters":{"note":note_id}}),
    );
    assert_eq!(
        (code, ans["code"].as_str()),
        (2, Some("criterion_not_evaluable")),
        "{ans}"
    );
    assert_nothing_written(&t, &before);
    // Reverse hazard and numeric equality on probes.
    let t = setup();
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"create_probe","parameters":{"kind":"hello","note":"hello","count":1}}),
    );
    assert_eq!(code, 0, "{ans}");
    let probe_id = ans["result"]["edits"][0]["id"].clone();
    let before = full_counts(&t);
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"check_probe_kind","parameters":{"probe":probe_id}}),
    );
    assert_eq!(
        (code, ans["code"].as_str()),
        (2, Some("submission_criteria")),
        "{ans}"
    );
    assert_nothing_written(&t, &before);
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"check_probe_count","parameters":{"probe":probe_id}}),
    );
    assert_eq!(code, 0, "{ans}");
    // Explicit $param.property with two reference parameters.
    let t = setup();
    let (_, note_id) = make_note(&t, "first");
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"check_note_explicit","parameters":{"note":note_id,"task":task_id()}}),
    );
    assert_eq!(code, 0, "{ans}");
    // Schema check refuses a bare name with two reference parameters.
    let mut variant: Value = serde_json::from_slice(FIXTURE).unwrap();
    variant["action_types"]["check_multi_bare"] = json!({
        "parameters":{"note":{"type":{"ref":"note"},"required":true},"task":{"type":{"ref":"task"},"required":true}},
        "submission_criteria":[{"code":"x","message":"x","all":[{"eq":["title","first"]}]}],
        "rules":[{"modify_object":"note","set":{"title":"z"}}]
    });
    variant["grants"][0]["actions"]
        .as_array_mut()
        .unwrap()
        .push(json!("check_multi_bare"));
    let file = t.file("multi.json", &serde_json::to_vec(&variant).unwrap());
    let (code, ans) = run(&["--store", &t.store(), "schema", "check", &file], b"");
    assert_eq!(code, 2, "{ans}");
}

#[test]
fn b4_pattern_optional_and_create_or_modify_pointer_key() {
    let t = setup();
    let before = full_counts(&t);
    let (code, v) = run_store(
        &t,
        &["action", "validate"],
        &json!({"op":"validate_action","action_type":"record_shout","parameters":{"word":"abc"}}),
    );
    assert_eq!(
        (code, v["code"].as_str(), v["parameter"].as_str()),
        (2, Some("invalid_parameter"), Some("word")),
        "{v}"
    );
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"record_shout","parameters":{"word":"abc"}}),
    );
    assert_eq!(
        (code, ans["code"].as_str(), ans["parameter"].as_str()),
        (2, Some("invalid_parameter"), Some("word")),
        "{ans}"
    );
    assert_nothing_written(&t, &before);
    // An absent optional parameter leaves the create property without a value.
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"record_note_optlabel","parameters":{"title":"x","task":task_id()}}),
    );
    assert_eq!(code, 0, "{ans}");
    let id = ans["result"]["edits"][0]["id"].clone();
    let (_, note) = run_store(&t, &["get"], &json!({"id":id}));
    assert!(
        note["result"]["objects"][0]["observations"][0]["fields"]
            .get("label")
            .is_none(),
        "{note}"
    );
    // create_or_modify with the key from its required widget reference.
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"upsert_widget","parameters":{"spec":id_json("widget", "w1"),"title":"first"}}),
    );
    assert_eq!(code, 0, "{ans}");
    assert_eq!(ans["result"]["edits"][0]["change"], "initial");
    assert_eq!(ans["result"]["edits"][0]["id"]["native_key"], "w1");
    let (code, v) = run_store(
        &t,
        &["action", "validate"],
        &json!({"op":"validate_action","action_type":"upsert_widget","parameters":{"spec":id_json("widget", "w1"),"title":"second"}}),
    );
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["result"]["edits"][0]["change"], "update");
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"upsert_widget","parameters":{"spec":id_json("widget", "w1"),"title":"second"}}),
    );
    assert_eq!(code, 0, "{ans}");
    assert_eq!(ans["result"]["edits"][0]["change"], "update");
}

#[test]
fn b5_same_request_id_different_agent_is_own_action() {
    let t = setup();
    let body_a = json!({"op":"apply_action","action_type":"record_note","agent":"agent-a","request_id":"shared-1","parameters":{"title":"once","task":task_id()}});
    let (code, a) = apply(&t, body_a.clone());
    assert_eq!(code, 0, "{a}");
    let mut body_b = body_a.clone();
    body_b["agent"] = json!("agent-b");
    let (code, b) = apply(&t, body_b);
    assert_eq!(code, 0, "{b}");
    assert_eq!(b["result"]["outcome"], "applied");
    assert_ne!(b["result"]["action_id"], a["result"]["action_id"]);
    let (code, again) = apply(&t, body_a);
    assert_eq!(code, 0, "{again}");
    assert_eq!(again["result"]["outcome"], "already_applied");
    assert_eq!(again["result"]["action_id"], a["result"]["action_id"]);
    let mut store = Store::open_schema(&t.store_path()).unwrap();
    assert_eq!(store.action_log().unwrap().len(), 2);
}

#[test]
fn refusals_write_nothing() {
    let t = setup();
    let (_, note_id) = make_note(&t, "first");
    let cases: Vec<(Value, &str)> = vec![
        (
            json!({"op":"apply_action","action_type":"no_such","parameters":{}}),
            "unknown_action",
        ),
        (
            json!({"op":"apply_action","action_type":"record_note","parameters":{"title":42,"task":task_id()}}),
            "invalid_parameter",
        ),
        (
            json!({"op":"apply_action","action_type":"check_note_ne","parameters":{"note":note_id},"expected":[{"id":note_id,"revision":"stale"}]}),
            "revision_conflict",
        ),
    ];
    for (body, want) in cases {
        let before = full_counts(&t);
        let (code, ans) = apply(&t, body);
        assert_eq!((code, ans["code"].as_str()), (2, Some(want)), "{ans}");
        assert_nothing_written(&t, &before);
    }
    // submission_criteria
    let (code, _) = apply(
        &t,
        json!({"op":"apply_action","action_type":"set_hold","parameters":{"task":task_id(),"effect":"merge","until":"later"}}),
    );
    assert_eq!(code, 0);
    let before = full_counts(&t);
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"set_hold","parameters":{"task":task_id(),"effect":"merge","until":"later"}}),
    );
    assert_eq!(
        (code, ans["code"].as_str()),
        (2, Some("submission_criteria")),
        "{ans}"
    );
    assert_nothing_written(&t, &before);
    // request_id_reused (same actor, different content)
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"record_note","request_id":"reuse-1","parameters":{"title":"a","task":task_id()}}),
    );
    assert_eq!(code, 0, "{ans}");
    let before = full_counts(&t);
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"record_note","request_id":"reuse-1","parameters":{"title":"b","task":task_id()}}),
    );
    assert_eq!(
        (code, ans["code"].as_str()),
        (2, Some("request_id_reused")),
        "{ans}"
    );
    assert_nothing_written(&t, &before);
    // action_forbidden on a limited grant store
    let t2 = Temp::new();
    let mut variant: Value = serde_json::from_slice(FIXTURE).unwrap();
    variant["grants"][0]["actions"] = json!(["record_note"]);
    install_bytes(&t2, &serde_json::to_vec(&variant).unwrap());
    append_task(&t2);
    let before = full_counts(&t2);
    let (code, ans) = apply(
        &t2,
        json!({"op":"apply_action","action_type":"set_hold","parameters":{"task":task_id(),"effect":"merge","until":"later"}}),
    );
    assert_eq!(
        (code, ans["code"].as_str()),
        (2, Some("action_forbidden")),
        "{ans}"
    );
    assert_nothing_written(&t2, &before);
}

fn refusal_code(f: &Failure) -> &'static str {
    match f {
        Failure::Refused(r) => r.code,
        Failure::Internal(_) => "internal",
    }
}
fn lib_apply(ctx: &Context, body: Value) -> Result<Value, Failure> {
    let req: ont::action::ApplyActionRequest = serde_json::from_value(body).unwrap();
    match ont::action::apply_action(ctx, &req)? {
        ont::Answer::Enveloped { result, .. } => Ok(result),
        _ => panic!("enveloped"),
    }
}

#[test]
fn b6_paged_linked_scan_still_refuses_and_snapshot_too_large() {
    let t = setup();
    let mut key_effect: Vec<(String, String)> = Vec::new();
    for effect in ["merge", "freeze", "audit"] {
        let (code, ans) = apply(
            &t,
            json!({"op":"apply_action","action_type":"set_hold","parameters":{"task":task_id(),"effect":effect,"until":"later"}}),
        );
        assert_eq!(code, 0, "{ans}");
        key_effect.push((
            ans["result"]["edits"][0]["id"]["native_key"]
                .as_str()
                .unwrap()
                .to_string(),
            effect.to_string(),
        ));
    }
    key_effect.sort();
    let deciding = key_effect.last().unwrap().1.clone();
    let ctx = Context::with_snapshot_bounds(t.store_path(), NOW, 2, 100);
    let err = lib_apply(
        &ctx,
        json!({"action_type":"set_hold","parameters":{"task":task_id(),"effect":deciding,"until":"later"}}),
    )
    .unwrap_err();
    assert_eq!(refusal_code(&err), "submission_criteria", "{err:?}");
    // Hard bound: more scanned heads than the bound refuses, writing nothing.
    let before = full_counts(&t);
    let ctx = Context::with_snapshot_bounds(t.store_path(), NOW, 2, 2);
    let err = lib_apply(
        &ctx,
        json!({"action_type":"set_hold","parameters":{"task":task_id(),"effect":"merge","until":"later"}}),
    )
    .unwrap_err();
    assert_eq!(refusal_code(&err), "snapshot_too_large", "{err:?}");
    assert_nothing_written(&t, &before);
}

#[test]
fn b4_validate_with_request_id_equals_apply_and_actor_bounds() {
    let t = setup();
    let ctx = Context::new(t.store_path(), NOW);
    let vreq = ont::action::ValidateActionRequest {
        op: None,
        action_type: "record_note".into(),
        parameters: json!({"title":"rid","task":task_id()}),
        agent: Some("agent-a".into()),
        session: None,
        expected: None,
        request_id: Some("val-1".into()),
    };
    let v = ont::action::validate_action(&ctx, &vreq).unwrap();
    let ont::Answer::Enveloped { result: vres, .. } = v else {
        panic!("enveloped")
    };
    let areq = ont::action::ApplyActionRequest {
        op: None,
        action_type: "record_note".into(),
        parameters: json!({"title":"rid","task":task_id()}),
        agent: Some("agent-a".into()),
        session: None,
        expected: None,
        request_id: Some("val-1".into()),
    };
    let a = ont::action::apply_action(&ctx, &areq).unwrap();
    let ont::Answer::Enveloped { result: ares, .. } = a else {
        panic!("enveloped")
    };
    assert_eq!(vres["edits"], ares["edits"]);
    assert_eq!(ares["outcome"], "applied");
    // Library-path actor bounds.
    let mut bad_ctx = Context::new(t.store_path(), NOW);
    bad_ctx.agent = Some("bad agent!".into());
    let mut no_agent_req = areq.clone();
    no_agent_req.agent = None;
    no_agent_req.request_id = Some("val-2".into());
    let err = ont::action::apply_action(&bad_ctx, &no_agent_req).unwrap_err();
    assert_eq!(refusal_code(&err), "invalid_request");
    let big = json!({"action_type":"record_note","parameters":{"title":"x".repeat(70_000),"task":task_id()}});
    let err = lib_apply(&ctx, big).unwrap_err();
    assert_eq!(refusal_code(&err), "invalid_request");
}

#[test]
fn n2_retry_id_length_prefixed_distinguishes_colon_fields() {
    let t = setup();
    let mut ctx_a = Context::new(t.store_path(), NOW);
    ctx_a.client = "a:b".into();
    let r_a = lib_apply(
        &ctx_a,
        json!({"action_type":"record_note","parameters":{"title":"n2","task":task_id()},"request_id":"n2-rid"}),
    )
    .unwrap();
    let mut ctx_b = Context::new(t.store_path(), NOW);
    ctx_b.client = "a".into();
    let r_b = lib_apply(
        &ctx_b,
        json!({"action_type":"record_note","parameters":{"title":"n2","task":task_id()},"agent":"b:","request_id":"n2-rid"}),
    )
    .unwrap();
    assert_eq!(r_a["outcome"], "applied", "{r_a}");
    assert_eq!(r_b["outcome"], "applied", "{r_b}");
    assert_ne!(r_a["action_id"], r_b["action_id"]);
    let mut store = Store::open_schema(&t.store_path()).unwrap();
    assert_eq!(store.action_log().unwrap().len(), 2);
}

#[test]
fn n1_interleaved_apply_sync_never_rebuilt() {
    let t = setup();
    ont::action::__reset_pause_after_commit();
    let path_a = t.store_path();
    let handle = std::thread::spawn(move || {
        ont::action::__set_pause_after_commit(true);
        let ctx = Context::new(path_a, NOW);
        lib_apply(
            &ctx,
            json!({"action_type":"record_note","parameters":{"title":"a-first","task":task_id()}}),
        )
    });
    for _ in 0..5000 {
        if ont::action::__pause_after_commit_entered() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(ont::action::__pause_after_commit_entered());
    let ctx_b = Context::new(t.store_path(), NOW);
    let r_b = lib_apply(
        &ctx_b,
        json!({"action_type":"record_note","parameters":{"title":"b-second","task":task_id()}}),
    )
    .unwrap();
    ont::action::__release_pause_after_commit();
    let r_a = handle.join().unwrap().unwrap();
    ont::action::__reset_pause_after_commit();
    for r in [&r_a, &r_b] {
        assert_eq!(r["outcome"], "applied", "{r}");
        assert!(
            matches!(r["edits_log"].as_str(), Some("written" | "pending")),
            "{r}"
        );
    }
    let mut store = Store::open_schema(&t.store_path()).unwrap();
    let log = store.action_log().unwrap();
    drop(store);
    assert_eq!(log.len(), 2);
    let text = std::fs::read_to_string(t.edits()).unwrap();
    assert_eq!(text.lines().count(), log.len(), "{text}");
    for ((seq, e), line) in log.iter().zip(text.lines()) {
        assert_eq!(line, log_line(*seq, e));
    }
    assert!(
        !std::fs::read_dir(&t.0)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e
                .file_name()
                .to_string_lossy()
                .starts_with("edits.jsonl.diverged-")),
        "no diverged aside expected"
    );
}

#[test]
fn n1b_two_diverged_repairs_leave_two_asides() {
    let t = setup();
    let (_, _) = make_note(&t, "one");
    for _ in 0..2 {
        let mut bytes = std::fs::read(t.edits()).unwrap();
        bytes.extend_from_slice(b"{\"seq\":999}\n");
        std::fs::write(t.edits(), &bytes).unwrap();
        let _ = ont::action::sync_edits_file(&t.store_path());
    }
    let asides: Vec<_> = std::fs::read_dir(&t.0)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("edits.jsonl.diverged-")
        })
        .collect();
    assert_eq!(asides.len(), 2, "{asides:?}");
}

#[test]
fn g2a_earlier_changed_line_is_left_by_a_write_and_found_by_verify() {
    let t = setup();
    for title in ["one", "two", "three"] {
        make_note(&t, title);
    }
    let text = std::fs::read_to_string(t.edits()).unwrap();
    std::fs::write(t.edits(), text.replacen("one", "eno", 1)).unwrap();
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"record_note","parameters":{"title":"four","task":task_id()}}),
    );
    assert_eq!(code, 0, "{ans}");
    assert_eq!(ans["result"]["edits_log"], "written");
    let asides = || {
        std::fs::read_dir(&t.0)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("edits.jsonl.diverged-")
            })
            .count()
    };
    assert_eq!(asides(), 0);
    let mut store = Store::open_schema(&t.store_path()).unwrap();
    let log = store.action_log().unwrap();
    let text = std::fs::read_to_string(t.edits()).unwrap();
    assert_eq!(text.lines().count(), 4);
    assert_ne!(text.lines().next().unwrap(), log_line(log[0].0, &log[0].1));
    for ((seq, e), line) in log.iter().skip(1).zip(text.lines().skip(1)) {
        assert_eq!(line, log_line(*seq, e));
    }
    assert_eq!(
        ont::action::verify_edits_file(&t.store_path()),
        ont::action::EditsLogStatus::Rebuilt
    );
    assert_eq!(asides(), 1);
    let text = std::fs::read_to_string(t.edits()).unwrap();
    assert_eq!(text.lines().count(), 4);
    for ((seq, e), line) in log.iter().zip(text.lines()) {
        assert_eq!(line, log_line(*seq, e));
    }
}
#[test]
fn g2a_rows_missing_after_the_tail_are_appended() {
    let t = setup();
    make_note(&t, "one");
    make_note(&t, "two");
    let text = std::fs::read_to_string(t.edits()).unwrap();
    std::fs::write(t.edits(), format!("{}\n", text.lines().next().unwrap())).unwrap();
    make_note(&t, "three");
    let mut store = Store::open_schema(&t.store_path()).unwrap();
    let log = store.action_log().unwrap();
    let text = std::fs::read_to_string(t.edits()).unwrap();
    assert_eq!(text.lines().count(), 3);
    for ((seq, e), line) in log.iter().zip(text.lines()) {
        assert_eq!(line, log_line(*seq, e));
    }
}

#[test]
fn backup_verifies_the_export() {
    let t = setup();
    for title in ["one", "two", "three"] {
        make_note(&t, title);
    }
    let text = std::fs::read_to_string(t.edits()).unwrap();
    let changed = text.replacen("one", "eno", 1);
    assert_ne!(changed, text);
    std::fs::write(t.edits(), changed).unwrap();
    let asides = || {
        std::fs::read_dir(&t.0)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("edits.jsonl.diverged-")
            })
            .count()
    };
    for expected in ["rebuilt", "written"] {
        let (code, ans) = run_store(&t, &["backup"], &json!({"op":"backup"}));
        assert_eq!(code, 0, "{ans}");
        assert_eq!(ans["result"]["edits_log"], expected);
        assert!(std::path::Path::new(ans["result"]["path"].as_str().unwrap()).is_file());
        assert_eq!(asides(), 1);
        let mut store = Store::open_schema(&t.store_path()).unwrap();
        let log = store.action_log().unwrap();
        let text = std::fs::read_to_string(t.edits()).unwrap();
        assert_eq!(text.lines().count(), 3);
        assert_eq!(log.len(), 3);
        for ((seq, entry), line) in log.iter().zip(text.lines()) {
            assert_eq!(line, log_line(*seq, entry));
        }
    }
}

// Use the same typed Append/coverage construction as candidates.rs::row and
// unresolved_stale_partial_and_historical_heads_remain_candidates.
fn g4_append_hold(t: &Temp, key: &str, task: Value, partial: bool) {
    let mut store = Store::open_schema(&t.store_path()).unwrap();
    let mut observation = store
        .head(&serde_json::from_value(task_id()).unwrap())
        .unwrap()
        .unwrap()
        .head
        .observation;
    observation.coverage = if partial {
        ont_core::Coverage::Partial
    } else {
        ont_core::Coverage::Complete
    };
    observation.fields = BTreeMap::from([
        ("task".into(), ont_core::Knowledge::Value(task)),
        ("effect".into(), ont_core::Knowledge::Value(json!("merge"))),
        ("until".into(), ont_core::Knowledge::Value(json!("later"))),
        ("status".into(), ont_core::Knowledge::Value(json!("active"))),
    ]);
    store
        .append_schema(
            &[ont_store::Append {
                id: serde_json::from_value(id_json("hold", key)).unwrap(),
                scope: ont_core::Scope::domain(ns(), "work", BTreeMap::new()).unwrap(),
                observation,
            }],
            NOW,
        )
        .unwrap();
}
fn g4_setup() -> Temp {
    let t = setup();
    let mut store = Store::open_schema(&t.store_path()).unwrap();
    let head = store
        .head(&serde_json::from_value(task_id()).unwrap())
        .unwrap()
        .unwrap();
    store
        .append_schema(
            &[ont_store::Append {
                id: serde_json::from_value(id_json("task", "task2")).unwrap(),
                scope: head.scope,
                observation: head.head.observation,
            }],
            NOW,
        )
        .unwrap();
    for i in 0..40 {
        g4_append_hold(&t, &format!("b{i:02}"), id_json("task", "task2"), false);
    }
    t
}
fn g4_request(task: Value) -> Value {
    json!({"action_type":"set_hold","parameters":{"task":task,"effect":"merge","until":"later"}})
}
#[test]
fn g4_linked_loads_only_the_targets_heads() {
    let t = g4_setup();
    let ctx = Context::with_snapshot_bounds(t.store_path(), NOW, 2, 3);
    let result = lib_apply(&ctx, g4_request(task_id())).unwrap();
    assert_eq!(result["outcome"], "applied");
}
#[test]
fn g4_linked_still_refuses_on_a_duplicate() {
    let t = g4_setup();
    g4_append_hold(&t, "a", task_id(), false);
    let ctx = Context::with_snapshot_bounds(t.store_path(), NOW, 2, 3);
    let err = lib_apply(&ctx, g4_request(task_id())).unwrap_err();
    assert_eq!(refusal_code(&err), "submission_criteria", "{err:?}");
}
#[test]
fn g4_partial_or_old_pin_head_is_still_counted() {
    let t = g4_setup();
    g4_append_hold(&t, "a-partial", task_id(), true);
    let connection = rusqlite::Connection::open(t.store_path()).unwrap();
    let unknown: i64 = connection.query_row(
        "SELECT count(*) FROM head_candidates WHERE object_type='hold' AND native_key='a-partial' AND property='task' AND value='?'",
        [], |row| row.get(0),
    ).unwrap();
    assert!(unknown > 0);
    drop(connection);
    let ctx = Context::with_snapshot_bounds(t.store_path(), NOW, 2, 3);
    let err = lib_apply(&ctx, g4_request(task_id())).unwrap_err();
    assert_eq!(refusal_code(&err), "submission_criteria", "{err:?}");
}
#[test]
fn g4_reference_key_order() {
    let t = g4_setup();
    g4_append_hold(&t, "a", task_id(), false);
    let ctx = Context::with_snapshot_bounds(t.store_path(), NOW, 2, 3);
    for raw in [
        r#"{"object_type":"task","namespace":{"instance":"vps://palantirkc","account":"root"},"native_key":"task1"}"#,
        r#"{"native_key":"task1","namespace":{"account":"root","instance":"vps://palantirkc"},"object_type":"task"}"#,
    ] {
        let err = lib_apply(&ctx, g4_request(serde_json::from_str(raw).unwrap())).unwrap_err();
        assert_eq!(refusal_code(&err), "submission_criteria", "{err:?}");
    }
}

#[test]
fn g4_list_reference_retains_type_scan() {
    let t = Temp::new();
    let mut schema: Value = serde_json::from_slice(FIXTURE).unwrap();
    schema["object_types"]["hold"]["properties"]["task"]["type"] = json!({"array":{"ref":"task"}});
    schema["link_types"]["hold_task"]["cardinality"] = json!("many_to_many");
    schema["action_types"]["set_hold"]["rules"][0]["set"]["task"] = json!(["$task"]);
    install_bytes(&t, &serde_json::to_vec(&schema).unwrap());
    append_task(&t);
    g4_append_hold(&t, "list", json!([task_id()]), false);
    let ctx = Context::with_snapshot_bounds(t.store_path(), NOW, 2, 3);
    let err = lib_apply(&ctx, g4_request(task_id())).unwrap_err();
    assert_eq!(refusal_code(&err), "submission_criteria", "{err:?}");
}

#[test]
fn g4_link_property_differs_from_target_type() {
    let t = Temp::new();
    let mut schema: Value = serde_json::from_slice(FIXTURE).unwrap();
    let property = schema["object_types"]["hold"]["properties"]
        .as_object_mut()
        .unwrap()
        .remove("task")
        .unwrap();
    schema["object_types"]["hold"]["properties"]["owner_task"] = property;
    schema["link_types"]["hold_task"]["via"] = json!("owner_task");
    let value = schema["action_types"]["set_hold"]["rules"][0]["set"]
        .as_object_mut()
        .unwrap()
        .remove("task")
        .unwrap();
    schema["action_types"]["set_hold"]["rules"][0]["set"]["owner_task"] = value;
    install_bytes(&t, &serde_json::to_vec(&schema).unwrap());
    append_task(&t);
    let mut store = Store::open_schema(&t.store_path()).unwrap();
    let head = store
        .head(&serde_json::from_value(task_id()).unwrap())
        .unwrap()
        .unwrap();
    store
        .append_schema(
            &[ont_store::Append {
                id: serde_json::from_value(id_json("task", "task2")).unwrap(),
                scope: head.scope.clone(),
                observation: head.head.observation.clone(),
            }],
            NOW,
        )
        .unwrap();
    for i in 0..40 {
        let mut observation = head.head.observation.clone();
        observation.fields = BTreeMap::from([
            (
                "owner_task".into(),
                ont_core::Knowledge::Value(id_json("task", "task2")),
            ),
            ("effect".into(), ont_core::Knowledge::Value(json!("merge"))),
            ("until".into(), ont_core::Knowledge::Value(json!("later"))),
            ("status".into(), ont_core::Knowledge::Value(json!("active"))),
        ]);
        store
            .append_schema(
                &[ont_store::Append {
                    id: serde_json::from_value(id_json("hold", &format!("b{i:02}"))).unwrap(),
                    scope: head.scope.clone(),
                    observation,
                }],
                NOW,
            )
            .unwrap();
    }
    drop(store);
    let ctx = Context::with_snapshot_bounds(t.store_path(), NOW, 2, 3);
    let result = lib_apply(&ctx, g4_request(task_id())).unwrap();
    assert_eq!(result["outcome"], "applied");
    let err = lib_apply(&ctx, g4_request(task_id())).unwrap_err();
    assert_eq!(refusal_code(&err), "submission_criteria", "{err:?}");
}

#[test]
fn g4_page_merge_keeps_the_smallest_ids() {
    let t = setup();
    let mut store = Store::open_schema(&t.store_path()).unwrap();
    let head = store
        .head(&serde_json::from_value(task_id()).unwrap())
        .unwrap()
        .unwrap();
    store
        .append_schema(
            &[ont_store::Append {
                id: serde_json::from_value(id_json("task", "task2")).unwrap(),
                scope: head.scope.clone(),
                observation: head.head.observation.clone(),
            }],
            NOW,
        )
        .unwrap();
    // The matching stream supplies a0/a1; merging stale s0 must retain a0/a1
    // on page one and return s0 on page two, rather than skipping a0.
    for (key, task, effect, fresh_until) in [
        ("a0", task_id(), "merge", 2000),
        ("a1", task_id(), "freeze", 2000),
        ("s0", id_json("task", "task2"), "merge", NOW - 1),
    ] {
        let mut observation = head.head.observation.clone();
        observation.fresh_until = fresh_until;
        observation.fields = BTreeMap::from([
            ("task".into(), ont_core::Knowledge::Value(task)),
            ("effect".into(), ont_core::Knowledge::Value(json!(effect))),
            ("until".into(), ont_core::Knowledge::Value(json!("later"))),
            ("status".into(), ont_core::Knowledge::Value(json!("active"))),
        ]);
        store
            .append_schema(
                &[ont_store::Append {
                    id: serde_json::from_value(id_json("hold", key)).unwrap(),
                    scope: head.scope.clone(),
                    observation,
                }],
                NOW,
            )
            .unwrap();
    }
    drop(store);
    let ctx = Context::with_snapshot_bounds(t.store_path(), NOW, 2, 100);
    let err = lib_apply(&ctx, g4_request(task_id())).unwrap_err();
    assert_eq!(refusal_code(&err), "submission_criteria", "{err:?}");
    let req = serde_json::from_value(g4_request(task_id())).unwrap();
    let answer = ont::action::validate_action(&ctx, &req).unwrap();
    let ont::Answer::Enveloped { result, .. } = answer else {
        panic!("enveloped")
    };
    assert_eq!(result["valid"], false, "{result}");
}

#[test]
fn g2a_complete_tail_with_unterminated_fragment_is_truncated() {
    let t = setup();
    make_note(&t, "one");
    let mut bytes = std::fs::read(t.edits()).unwrap();
    bytes.extend_from_slice(b"{\"seq\":");
    std::fs::write(t.edits(), bytes).unwrap();
    let (code, ans) = apply(
        &t,
        json!({"op":"apply_action","action_type":"record_note","parameters":{"title":"two","task":task_id()}}),
    );
    assert_eq!(code, 0, "{ans}");
    assert_eq!(ans["result"]["edits_log"], "written");
    let mut store = Store::open_schema(&t.store_path()).unwrap();
    let log = store.action_log().unwrap();
    let expected: String = log
        .iter()
        .map(|(seq, row)| format!("{}\n", log_line(*seq, row)))
        .collect();
    assert_eq!(
        std::fs::read_to_string(t.edits()).unwrap(),
        expected,
        "fragment must be removed before appending"
    );
}
