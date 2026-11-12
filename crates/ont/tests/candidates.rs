//! O22 part A: large synthetic fixtures use only the typed store API.
#![allow(clippy::unwrap_used)]
use ont_core::{Change, Coverage, Id, Knowledge, Lineage, Observation, Scope};
use ont_store::{Append, Store, schema_mode::InstallArgs};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ont-candidates-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).unwrap();
        let f = Self(dir);
        Store::open_schema(&f.path())
            .unwrap()
            .install_schema(&schema(1).to_string().into_bytes(), InstallArgs::default())
            .unwrap();
        f
    }
    fn path(&self) -> PathBuf {
        self.0.join("store.sqlite")
    }
    fn append(&self, rows: Vec<Append>) {
        Store::open_schema_existing_writable(&self.path())
            .unwrap()
            .append_schema(&rows, 1)
            .unwrap();
    }
    fn search(&self, filter: Value, continuation: Value) -> Value {
        let request = json!({"object_type":"record", "where":filter,"limit":1000,"continuation":continuation});
        let out = ont::run(
            &[
                "--store".into(),
                self.path().display().to_string(),
                "--now".into(),
                "10".into(),
                "search".into(),
            ],
            request.to_string().as_bytes(),
            0,
            0,
        );
        assert!(out.stderr.is_empty(), "{}", out.stderr);
        serde_json::from_str(&out.stdout).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn schema(version: u64) -> Value {
    json!({"format":"ontology-schema/v1","domain":"test","version":version,
        "namespace":{"instance":"test","account":"root"},"scope_keys":["tenant"],
        "value_types":{},"sources":{},"object_types":{"record":{"owner":"ontology","primary_key":{"from":"$action_id"},
            "properties":{"status":{"type":"string","max_bytes":170},"extra":{"type":"string"}}}},
        "link_types":{},"action_types":{},"functions":{},
        "grants":[{"principal":"agent","scope":{"tenant":"public"},"read":["record"],"derive":["record"],"actions":[]}]})
}
fn row(key: &str, status: Knowledge, hidden: bool, stale: bool) -> Append {
    let id: Id = serde_json::from_value(json!({"object_type":"record","namespace":{"instance":"test","account":"root"},"native_key":key})).unwrap();
    Append {
        scope: Scope::domain(
            id.namespace.clone(),
            "test",
            BTreeMap::from([(
                "tenant".into(),
                if hidden { "private" } else { "public" }.into(),
            )]),
        )
        .unwrap(),
        id,
        observation: Observation {
            revision: "r0".into(),
            source_revision: "s0".into(),
            observed_at: 1,
            fresh_until: if stale { 1 } else { 100 },
            change: Change::Initial,
            supersedes: None,
            coverage: Coverage::Complete,
            fields: BTreeMap::from([("status".into(), status)]),
            lineage: Lineage {
                artifact_sha256: "a".repeat(64),
                pointer: "/synthetic".into(),
            },
        },
    }
}
fn status(s: &str) -> Knowledge {
    Knowledge::Value(json!(s))
}
fn current() -> Value {
    json!({"in":["status",["pending","ready"]]})
}

#[test]
fn history_does_not_consume_candidates_or_make_a_tail_continuation() {
    for count in [201, 1000, 1001] {
        for mode in ["visible", "hidden", "stale"] {
            let f = Fixture::new();
            let mut rows: Vec<_> = (0..10001)
                .map(|i| {
                    row(
                        &format!("z-history-{i:05}"),
                        status("superseded"),
                        false,
                        false,
                    )
                })
                .collect();
            // Some history sorts before the current rows as well.
            rows.push(row("0-history", status("superseded"), false, false));
            rows.extend((0..count).map(|i| {
                row(
                    &format!("a-current-{i:05}"),
                    status("ready"),
                    false,
                    mode == "stale",
                )
            }));
            if mode == "hidden" {
                rows.extend(
                    (0..10001)
                        .map(|i| row(&format!("b-hidden-{i:05}"), status("ready"), true, true)),
                );
            }
            f.append(rows);
            if count == 201 && mode == "visible" {
                let out = ont::run(
                    &[
                        "--store".into(),
                        f.path().display().to_string(),
                        "aggregate".into(),
                    ],
                    &br#"{"object_type":"record","group_by":["status"],"metrics":["count"]}"#[..],
                    0,
                    0,
                );
                assert_eq!(
                    serde_json::from_str::<Value>(&out.stdout).unwrap()["code"],
                    "result_too_large"
                );
            }
            let first = f.search(current(), Value::Null);
            assert_eq!(first["status"], "ok", "{mode}/{count}: {first}");
            let r = &first["result"];
            assert_eq!(r["objects"].as_array().unwrap().len(), count.min(1000));
            assert_eq!(r["basis"]["scanned"], count.min(1000));
            assert_eq!(
                r["basis"]["stale"],
                if mode == "stale" { count.min(1000) } else { 0 }
            );
            if count <= 1000 {
                assert!(
                    r["continuation"].is_null(),
                    "{mode}/{count}: unexpected continuation"
                );
            } else {
                assert!(r["continuation"].is_string());
                let next = f.search(current(), r["continuation"].clone());
                assert_eq!(next["result"]["objects"].as_array().unwrap().len(), 1);
                assert!(next["result"]["continuation"].is_null());
            }
        }
    }
}

#[test]
fn unresolved_stale_partial_and_historical_heads_remain_candidates() {
    let f = Fixture::new();
    let mut rows: Vec<_> = (0..10001)
        .map(|i| row(&format!("z-{i:05}"), status("superseded"), false, false))
        .collect();
    rows.extend([
        row("a-current", status("ready"), false, false),
        row("b-unresolved", Knowledge::Missing, false, false),
        row("c-stale", status("superseded"), false, true),
        row("d-partial", status("superseded"), false, false),
    ]);
    rows.last_mut().unwrap().observation.coverage = Coverage::Partial;
    f.append(rows);
    let ans = f.search(json!({"eq":["status","ready"]}), Value::Null);
    assert_eq!(ans["status"], "ok", "{ans}");
    assert_eq!(
        ans["result"]["basis"],
        json!({"scanned":4,"matched":1,"unresolved_excluded":1,"stale":1,"partial":1,"qualified":false,"stale_sources":[]})
    );
    let mut upgraded = schema(2);
    upgraded["object_types"]["record"]["properties"]["status"]["type"] = json!("integer");
    Store::open_schema_existing_writable(&f.path())
        .unwrap()
        .install_schema(upgraded.to_string().as_bytes(), InstallArgs::default())
        .unwrap();
    // Historical incompatible values still consume the bound, rather than disappearing.
    let ans = f.search(json!({"eq":["status",1]}), Value::Null);
    assert_eq!(ans["code"], "result_too_large", "{ans}");
}

#[test]
fn all_preserves_other_unresolved_properties_and_unsupported_forms_stay_raw() {
    let f = Fixture::new();
    f.append(vec![
        row("a", status("superseded"), false, false),
        row("b", status("ready"), false, false),
    ]);
    let filter = json!({"all":[{"eq":["status","ready"]},{"eq":["extra","x"]}]});
    let ans = f.search(filter, Value::Null);
    assert_eq!(ans["result"]["basis"]["scanned"], 2);
    assert_eq!(ans["result"]["basis"]["unresolved_excluded"], 1);
    for filter in [
        json!({"any":[{"eq":["status","ready"]}]}),
        json!({"none":[{"eq":["status","ready"]}]}),
        json!({"ne":["status","ready"]}),
    ] {
        let ans = f.search(filter, Value::Null);
        assert_eq!(ans["result"]["basis"]["scanned"], 2);
    }
    let ans = f.search(
        json!({"all":[{"eq":["status","ready"]},{"ne":["status","pending"]}]}),
        Value::Null,
    );
    assert_eq!(ans["result"]["basis"]["scanned"], 1);
    // No condition for aggregation: the history is still included.
    let out = ont::run(
        &[
            "--store".into(),
            f.path().display().to_string(),
            "--now".into(),
            "10".into(),
            "aggregate".into(),
        ],
        &br#"{"object_type":"record","group_by":["status"],"metrics":["count"]}"#[..],
        0,
        0,
    );
    let ans: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(
        ans["result"]["groups"],
        json!([{"key":{"status":"ready"},"count":1},{"key":{"status":"superseded"},"count":1}])
    );
}

#[test]
fn previous_binary_store_migrates_once_and_complete_answer_is_unchanged() {
    let f = Fixture::new();
    std::fs::write(f.path(), include_bytes!("fixtures/o22-previous.sqlite")).unwrap();
    let before = std::fs::read(f.path()).unwrap();
    let mut answer = f.search(json!({"eq":["status","ready"]}), Value::Null);
    let migrated = std::fs::read(f.path()).unwrap();
    assert_ne!(before, migrated);
    assert_eq!(
        answer,
        f.search(json!({"eq":["status","ready"]}), Value::Null)
    );
    assert_eq!(migrated, std::fs::read(f.path()).unwrap());
    assert_eq!(answer["result"]["basis"]["scanned"], 1);
    answer["result"]["basis"]["scanned"] = json!(2);
    let old: Value =
        serde_json::from_str(include_str!("fixtures/o22-previous-answer.json")).unwrap();
    assert_eq!(answer.to_string(), old.to_string());
}
