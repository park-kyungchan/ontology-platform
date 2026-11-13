//! Projection-only controls: synthetic stores, including old-writer rows.
#![allow(clippy::unwrap_used)]
use ont::{Answer, Context};
use ont_core::{Change, Coverage, Id, Knowledge, Lineage, Namespace, Observation, Scope};
use ont_store::{Append, Store, schema_mode::CandidateCondition};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

const NOW: u64 = 1000;
struct Fixture(PathBuf);
impl Fixture {
    fn new(list: bool) -> Self {
        let path = std::env::temp_dir().join(format!(
            "ont-absent-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        let t = Self(path);
        t.install(&schema(1, list));
        let rows = ["me", "other"].map(|key| Append {
            id: id("task", key),
            scope: scope(),
            observation: Observation {
                revision: "r0".into(),
                source_revision: "s0".into(),
                observed_at: 1,
                fresh_until: ont_store::NEVER_STALE,
                change: Change::Initial,
                supersedes: None,
                coverage: Coverage::Complete,
                fields: BTreeMap::from([
                    ("directory".into(), Knowledge::Value(json!(key))),
                    ("state_sha256".into(), Knowledge::Value(json!("abc"))),
                    ("checkpoint_line".into(), Knowledge::Value(json!("line"))),
                ]),
                lineage: Lineage {
                    artifact_sha256: "a".repeat(64),
                    pointer: "/synthetic".into(),
                },
            },
        });
        Store::open_schema_existing_writable(&t.path())
            .unwrap()
            .append_schema(&rows, NOW)
            .unwrap();
        t
    }
    fn path(&self) -> PathBuf {
        self.0.join("store.sqlite")
    }
    fn install(&self, schema: &Value) {
        Store::open_schema(&self.path())
            .unwrap()
            .install_schema(&serde_json::to_vec(schema).unwrap(), Default::default())
            .unwrap();
    }
    fn context(&self) -> Context {
        Context::with_snapshot_bounds(self.path(), NOW, 2, 3)
    }
    fn apply(&self, request: Value) -> Value {
        let answer =
            ont::action::apply_action(&self.context(), &serde_json::from_value(request).unwrap())
                .unwrap();
        result(answer)
    }
    fn create(&self, key: &str, target: &str, title: &str) -> Value {
        let answer = self.apply(json!({"action_type":"record_note","request_id":key,"parameters":{"task":id("task", target),"title":title}}));
        answer["edits"][0]["id"].clone()
    }
    fn clear(&self, note: Value) {
        self.apply(json!({"action_type":"clear_note_task","parameters":{"note":note}}));
    }
    fn closed(&self, count: usize) {
        for i in 0..count {
            let note = self.create(&format!("closed-{i}"), "other", &format!("field-{i}"));
            assert!(!note.is_null());
            self.clear(note);
        }
    }
    fn open_two(&self) {
        self.create("open-a", "me", "a");
        self.create("open-b", "me", "b");
    }
    fn validate(&self, title: &str) -> Value {
        result(
            ont::action::validate_action(
                &self.context(),
                &serde_json::from_value(request(title)).unwrap(),
            )
            .unwrap(),
        )
    }
    fn text(&self, command: &str, request: Value) -> String {
        let out = ont::run(
            &[
                "--store".into(),
                self.path().display().to_string(),
                "--now".into(),
                NOW.to_string(),
                command.into(),
            ],
            serde_json::to_vec(&request).unwrap().as_slice(),
            0,
            0,
        );
        assert_eq!(out.code, 0, "{}", out.stdout);
        assert!(out.stderr.is_empty(), "{}", out.stderr);
        out.stdout
    }
    fn candidates(&self) -> usize {
        let condition = CandidateCondition {
            property: "task".into(),
            values: vec![json!(id("task", "me"))],
            used: BTreeSet::new(),
            absent: false,
        };
        Store::open_schema_existing(&self.path())
            .unwrap()
            .candidate_access("note", Some(&condition), NOW, 2000, None, &[scope()])
            .unwrap()
            .len()
    }
    fn old_rows(&self, mixed: bool) {
        let sql = rusqlite::Connection::open(self.path()).unwrap();
        sql.execute("UPDATE head_candidates SET value='?' WHERE value='-' AND (?1=0 OR native_key=(SELECT min(native_key) FROM head_candidates WHERE value='-' AND object_type='note' AND property='task'))", [i64::from(mixed)]).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn ns() -> Namespace {
    Namespace {
        instance: "vps://palantirkc".into(),
        account: "root".into(),
    }
}
fn id(ty: &str, key: &str) -> Id {
    Id {
        object_type: ty.into(),
        namespace: ns(),
        native_key: key.into(),
    }
}
fn scope() -> Scope {
    Scope::domain(ns(), "work", BTreeMap::new()).unwrap()
}
fn result(answer: Answer) -> Value {
    match answer {
        Answer::Enveloped { result, .. } => result,
        other => panic!("{other:?}"),
    }
}
fn schema(version: u64, list: bool) -> Value {
    let mut s: Value = serde_json::from_slice(include_bytes!("fixtures/action-work.json")).unwrap();
    s["version"] = json!(version);
    s["object_types"]["note"]["properties"]["title"]["max_bytes"] = json!(170);
    s["action_types"]["record_note"]["submission_criteria"] = json!([{"code":"field_fenced","message":"The field already has an open note.","none":[{"linked":{"from":"$task","link":"notes","where":{"all":[{"eq":["title","$title"]}]}}}]}]);
    if list {
        s["object_types"]["note"]["properties"]["task"]["type"] = json!({"array":{"ref":"task"}});
        s["link_types"]["note_task"]["cardinality"] = json!("many_to_many");
        s["action_types"]["record_note"]["rules"][0]["set"]["task"] = json!(["$task"]);
    }
    s
}
fn request(title: &str) -> Value {
    json!({"action_type":"record_note","request_id":"probe","parameters":{"task":id("task", "me"),"title":title}})
}
fn timed_validate(t: &Fixture, count: usize) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let start = std::time::Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_ont"))
        .args([
            "--store",
            &t.path().display().to_string(),
            "--now",
            "1000",
            "action",
            "validate",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&request("a")).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let elapsed = start.elapsed().as_secs_f64() * 1000.0;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let answer: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(answer["result"]["valid"], false);
    eprintln!("debug action validate: cleared={count}, elapsed_ms={elapsed:.3}");
}

#[test]
fn reverse_link_skips_heads_without_a_value() {
    let mut baseline = None;
    for count in [0, 1000] {
        let t = Fixture::new(false);
        t.closed(count);
        t.open_two();
        let candidates = t.candidates();
        assert_eq!(candidates, 2, "cleared={count}");
        let verdicts: Vec<_> = ["a", "b", "free"]
            .into_iter()
            .map(|title| t.validate(title))
            .collect();
        assert_eq!(verdicts[0]["valid"], false);
        assert_eq!(verdicts[1]["valid"], false);
        assert_eq!(verdicts[2]["valid"], true);
        if let Some(baseline) = &baseline {
            assert_eq!(&verdicts, baseline);
        } else {
            baseline = Some(verdicts);
        }
        // This bound exercises the real fetch_linked path, in read and write transactions.
        let error =
            ont::action::apply_action(&t.context(), &serde_json::from_value(request("a")).unwrap())
                .unwrap_err();
        assert!(
            matches!(error, ont::Failure::Refused(ref r) if r.code == "submission_criteria"),
            "{error:?}"
        );
        eprintln!("reverse-link candidate heads: cleared={count}, returned={candidates}");
        timed_validate(&t, count);
    }
}

#[test]
fn search_answers_are_unchanged() {
    let t = Fixture::new(false);
    t.closed(4);
    t.open_two();
    // Include a never-set key alongside action-cleared Missing fields.
    let mut store = Store::open_schema_existing_writable(&t.path()).unwrap();
    let head = store.scan_heads("note", 1, None).unwrap().remove(0);
    let mut row = Append {
        id: id("note", "never-set"),
        scope: head.scope,
        observation: head.head.observation,
    };
    row.observation.fields.remove("task");
    row.observation.supersedes = None;
    row.observation.change = Change::Initial;
    store.append_schema(&[row], NOW).unwrap();
    drop(store);
    // A real copy with identical base rows; change only derived no-value postings.
    let copy = Fixture(t.0.join("copy"));
    std::fs::create_dir(&copy.0).unwrap();
    std::fs::copy(t.path(), copy.path()).unwrap();
    copy.old_rows(false);
    for filter in [
        json!({"eq":["task",id("task", "me")]}),
        json!({"ne":["task",id("task", "me")]}),
        json!({"none":[{"eq":["task",id("task", "me")]}]}),
        json!({"all":[{"eq":["title","a"]},{"eq":["task",id("task", "me")]}]}),
    ] {
        for command in ["search", "aggregate", "around"] {
            let request = match command {
                "around" => {
                    json!({"from":{"object_type":"note","where":filter},"path":["note_task"]})
                }
                "aggregate" => {
                    json!({"object_type":"note","where":filter,"group_by":["title"],"metrics":["count"]})
                }
                _ => json!({"object_type":"note","where":filter,"limit":1}),
            };
            assert_eq!(
                t.text(command, request.clone()),
                copy.text(command, request),
                "{command}/{filter}"
            );
        }
    }
}

#[test]
fn mixed_projection_same_verdicts() {
    let t = Fixture::new(false);
    t.closed(4);
    t.open_two();
    // A mixed projection is wider; use the normal bound when comparing verdicts.
    let validate = || {
        ["a", "b", "free"]
            .into_iter()
            .map(|title| {
                result(
                    ont::action::validate_action(
                        &Context::new(t.path(), NOW),
                        &serde_json::from_value(request(title)).unwrap(),
                    )
                    .unwrap(),
                )
            })
            .collect::<Vec<_>>()
    };
    let new = validate();
    t.old_rows(true);
    let sql = rusqlite::Connection::open(t.path()).unwrap();
    let mixed: (usize, usize) = sql.query_row("SELECT sum(value='?'),sum(value='-') FROM head_candidates WHERE object_type='note' AND property='task'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(mixed, (1, 3));
    assert_eq!(new, validate());
    t.old_rows(false);
    assert_eq!(new, validate());
}

#[test]
fn older_pin_value_still_fences() {
    let t = Fixture::new(true);
    t.create("old-open", "me", "a");
    t.install(&schema(2, false));
    let sql = rusqlite::Connection::open(t.path()).unwrap();
    let posting: String = sql
        .query_row(
            "SELECT value FROM head_candidates WHERE object_type='note' AND property='task'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(t.validate("a")["valid"], false);
    assert_eq!(posting, "?");
    assert_eq!(t.candidates(), 1);
    let error =
        ont::action::apply_action(&t.context(), &serde_json::from_value(request("a")).unwrap())
            .unwrap_err();
    assert!(
        matches!(error, ont::Failure::Refused(ref r) if r.code == "submission_criteria"),
        "{error:?}"
    );
}
