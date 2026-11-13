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
            "properties":{"status":{"type":"string","max_bytes":170},"extra":{"type":"string"},"edge":{"type":{"ref":"record"}}}}},
        "link_types":{"edge":{"from":"record","via":"edge","to":"record","cardinality":"many_to_one","names":{"forward":"next","reverse":"previous"}}},"action_types":{},"functions":{},
        "views":{"ready_records":{"object_type":"record","order":[{"property":"status","direction":"asc"}],"filter":{"eq":["status","ready"]},"limit":1000,"columns":[{"header":"Key","property":"$key"}]},"all_records":{"object_type":"record","order":[{"property":"status","direction":"asc"}],"limit":1000,"columns":[{"header":"Key","property":"$key"}]}},
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
        id: id.clone(),
        observation: Observation {
            revision: "r0".into(),
            source_revision: "s0".into(),
            observed_at: 1,
            fresh_until: if stale { 1 } else { 100 },
            change: Change::Initial,
            supersedes: None,
            coverage: Coverage::Complete,
            fields: BTreeMap::from([
                ("status".into(), status),
                ("edge".into(), Knowledge::Value(json!(id))),
            ]),
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
    let mut boundary = row("e-fresh-boundary", status("superseded"), false, false);
    boundary.observation.fresh_until = 10;
    rows.push(boundary);
    f.append(rows);
    let ans = f.search(json!({"eq":["status","ready"]}), Value::Null);
    assert_eq!(ans["status"], "ok", "{ans}");
    assert_eq!(
        ans["result"]["basis"],
        json!({"scanned":4,"matched":1,"unresolved_excluded":1,"oversized_excluded":0,"stale":1,"partial":1,"qualified":false,"stale_sources":[]})
    );
    let mut upgraded = schema(2);
    upgraded["object_types"]["record"]["properties"]["status"]["type"] = json!("integer");
    upgraded["views"]["ready_records"]["filter"] = json!({"eq":["status",1]});
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
fn previous_binary_store_migrates_once_and_preserves_existing_answer_fields() {
    let f = Fixture::new();
    std::fs::write(f.path(), include_bytes!("fixtures/o22-previous.sqlite")).unwrap();
    let before = std::fs::read(f.path()).unwrap();
    assert_eq!(
        f.search(json!({"eq":["status","ready"]}), Value::Null)["code"],
        "store_upgrade_required"
    );
    assert_eq!(before, std::fs::read(f.path()).unwrap());
    drop(Store::open_schema_existing_writable(&f.path()).unwrap());
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
    let mut old: Value =
        serde_json::from_str(include_str!("fixtures/o22-previous-answer.json")).unwrap();
    // The retained old-engine fixture stays immutable; O21 adds one zero counter.
    old["result"]["basis"]["oversized_excluded"] = json!(0);
    assert_eq!(answer.to_string(), old.to_string());
}

#[test]
fn hidden_population_has_byte_equal_search_answers() {
    let clean = Fixture::new();
    clean.append(vec![row("visible", status("ready"), false, false)]);
    let expected = clean.search(current(), Value::Null).to_string();
    for count in [512, 1024, 2048] {
        let f = Fixture::new();
        let mut rows = vec![row("visible", status("ready"), false, false)];
        rows.extend(
            (0..count).map(|i| row(&format!("hidden-{i:05}"), status("ready"), true, true)),
        );
        f.append(rows);
        assert_eq!(f.search(current(), Value::Null).to_string(), expected);
        rusqlite::Connection::open(f.path())
            .unwrap()
            .execute(
                "UPDATE observations SET payload='invalid' WHERE native_key LIKE 'hidden-%'",
                [],
            )
            .unwrap();
        let refused = f.search(current(), Value::Null);
        assert_eq!(refused["code"], "projection_stale");
        assert!(refused["hint"].as_str().unwrap().contains("schema install"));
    }
}

#[test]
fn update_retraction_resurrection_and_action_apply_maintain_postings() {
    let f = Fixture::new();
    f.append(vec![row("mut", status("superseded"), false, false)]);
    assert_eq!(
        f.search(current(), Value::Null)["result"]["basis"]["matched"],
        0
    );
    let mut item = row("mut", status("ready"), false, false);
    item.observation.change = Change::Update;
    item.observation.revision = "r1".into();
    item.observation.supersedes = Some("r0".into());
    f.append(vec![item.clone()]);
    assert_eq!(
        f.search(current(), Value::Null)["result"]["basis"]["matched"],
        1
    );
    item.observation.fields.clear();
    item.observation.change = Change::Retraction;
    item.observation.revision = "r2".into();
    item.observation.supersedes = Some("r1".into());
    f.append(vec![item.clone()]);
    assert_eq!(
        f.search(current(), Value::Null)["result"]["basis"]["matched"],
        0
    );
    item.observation
        .fields
        .insert("status".into(), status("ready"));
    item.observation.change = Change::Initial;
    item.observation.revision = "r3".into();
    item.observation.supersedes = None;
    f.append(vec![item]);
    assert_eq!(
        f.search(current(), Value::Null)["result"]["basis"]["matched"],
        1
    );
    let g = Fixture::new();
    std::fs::write(g.path(), include_bytes!("fixtures/o22-previous.sqlite")).unwrap();
    let out = ont::run(&["--store".into(),g.path().display().to_string(),"action".into(),"apply".into()],
        json!({"action_type":"create_record","parameters":{"status":"ready"},"request_id":"r1-maintenance"}).to_string().as_bytes(),0,0);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(
        g.search(current(), Value::Null)["result"]["basis"]["matched"],
        2
    );
}

#[test]
fn preopened_connection_stales_search_until_identical_schema_install() {
    preopened_case(false);
}
#[test]
fn dropped_trigger_invalidates_cached_protection_until_repaired() {
    preopened_case(true);
}
fn preopened_case(drop_trigger: bool) {
    let f = Fixture::new();
    std::fs::write(f.path(), include_bytes!("fixtures/o22-previous.sqlite")).unwrap();
    let old = rusqlite::Connection::open(f.path()).unwrap();
    let mut change=old.prepare("UPDATE observations SET payload=json_set(payload,'$.fields.status.value','ready') WHERE json_extract(payload,'$.fields.status.value')='superseded'").unwrap();
    drop(Store::open_schema_existing_writable(&f.path()).unwrap());
    assert_eq!(
        f.search(current(), Value::Null)["result"]["basis"]["matched"],
        1
    );
    let mut held = Store::open_schema_existing_writable(&f.path()).unwrap();
    if drop_trigger {
        old.execute_batch("DROP TRIGGER candidate_observations_UPDATE")
            .unwrap();
    }
    change.execute([]).unwrap();
    assert_eq!(
        held.read_schema(|tx| tx.candidate_access("record", None, 10, 100, None, &[]))
            .unwrap()
            .unwrap_err()
            .code,
        "projection_stale"
    );
    assert!(change.get_status(rusqlite::StatementStatus::RePrepare) > 0);
    let refused = f.search(current(), Value::Null);
    assert_eq!(refused["code"], "projection_stale");
    let bytes: Vec<u8> = old
        .query_row(
            "SELECT bytes FROM schema_installs WHERE version=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let file = f.0.join("installed.json");
    std::fs::write(&file, bytes).unwrap();
    let out = ont::run(
        &[
            "--store".into(),
            f.path().display().to_string(),
            "schema".into(),
            "install".into(),
            file.display().to_string(),
        ],
        &b""[..],
        0,
        0,
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(
        f.search(current(), Value::Null)["result"]["basis"]["matched"],
        2
    );
}

fn reader_request(command: &str, filtered: bool) -> Value {
    let filter = if filtered {
        json!({"eq":["status","ready"]})
    } else {
        Value::Null
    };
    match command {
        "around" => {
            json!({"from":{"object_type":"record","where":filter},"path":["next"],"limit":1000})
        }
        "aggregate" => {
            json!({"object_type":"record","where":filter,"group_by":["status"],"metrics":["count"]})
        }
        _ => json!({"view":if filtered {"ready_records"} else {"all_records"}}),
    }
}
fn reader_text(f: &Fixture, command: &str, filtered: bool) -> String {
    let out = ont::run(
        &[
            "--store".into(),
            f.path().display().to_string(),
            "--now".into(),
            "10".into(),
            command.into(),
        ],
        reader_request(command, filtered).to_string().as_bytes(),
        0,
        0,
    );
    assert!(out.stderr.is_empty(), "{}", out.stderr);
    out.stdout
}
fn reader_parity(command: &str) {
    // A static OnceLock never drops its value at process exit. Own the fixture
    // in the test so both successful returns and unwinding remove its store.
    let large = {
        let f = Fixture::new();
        let mut rows: Vec<_> = (0..10001)
            .map(|i| row(&format!("z-{i:05}"), status("superseded"), false, false))
            .collect();
        rows.extend((0..201).map(|i| row(&format!("a-{i:05}"), status("ready"), false, false)));
        rows.extend(
            (0..10001).map(|i| row(&format!("hidden-{i:05}"), status("ready"), true, true)),
        );
        f.append(rows);
        f
    };
    let small = Fixture::new();
    small.append(
        (0..201)
            .map(|i| row(&format!("a-{i:05}"), status("ready"), false, false))
            .collect(),
    );
    let expected = reader_text(&small, command, true);
    let answer: Value = serde_json::from_str(&expected).unwrap();
    assert_eq!(answer["status"], "ok", "{answer}");
    assert_eq!(reader_text(&large, command, true), expected);
    let raw: Value = serde_json::from_str(&reader_text(&large, command, false)).unwrap();
    assert_eq!(raw["code"], "result_too_large", "{raw}");
    if command == "render" {
        assert_eq!(answer["rows"], 201);
    } else {
        assert_eq!(answer["result"]["basis"]["scanned"], 201);
    }
}
#[test]
fn around_candidates_match_small_store_and_keep_raw_bound() {
    reader_parity("around");
}
#[test]
fn aggregate_candidates_match_small_store_and_keep_raw_bound() {
    reader_parity("aggregate");
}
#[test]
fn render_candidates_match_small_store_and_keep_raw_bound() {
    reader_parity("render");
}
#[test]
fn stale_projection_refuses_all_three_readers() {
    let f = Fixture::new();
    f.append(vec![row("a", status("ready"), false, false)]);
    let sql = rusqlite::Connection::open(f.path()).unwrap();
    sql.execute("UPDATE objects SET head=head", []).unwrap();
    for command in ["around", "aggregate", "render"] {
        let answer: Value = serde_json::from_str(&reader_text(&f, command, true)).unwrap();
        assert_eq!(answer["code"], "projection_stale", "{command}: {answer}");
    }
}

fn store_hashes(f: &Fixture) -> BTreeMap<String, String> {
    use sha2::{Digest, Sha256};
    ["", "-journal", "-wal", "-shm"]
        .into_iter()
        .filter_map(|suffix| {
            let path = f.0.join(format!("store.sqlite{suffix}"));
            path.exists().then(|| {
                (
                    suffix.to_string(),
                    format!("{:x}", Sha256::digest(std::fs::read(path).unwrap())),
                )
            })
        })
        .collect()
}
fn stored_heads(f: &Fixture) -> Vec<String> {
    rusqlite::Connection::open_with_flags(f.path(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
        .prepare("SELECT json_array(object_type,instance,account,native_key,scope,head) FROM objects ORDER BY object_type,instance,account,native_key")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn format3_upgrade_refusal_preserves_bytes_and_owner_write_preserves_heads() {
    upgrade_refusal_case("format3");
}
#[test]
fn old_projection_upgrade_refusal_preserves_bytes_and_owner_write_preserves_heads() {
    for mode in ["missing", "different", "uncertified"] {
        upgrade_refusal_case(mode);
    }
}
fn upgrade_refusal_case(mode: &str) {
    let f = Fixture::new();
    std::fs::write(f.path(), include_bytes!("fixtures/o22-previous.sqlite")).unwrap();
    if mode != "format3" {
        drop(Store::open_schema_existing_writable(&f.path()).unwrap());
        let old = rusqlite::Connection::open(f.path()).unwrap();
        old.execute_batch(match mode {
            "missing" => "ALTER TABLE candidate_generation DROP COLUMN semantics_version",
            "different" => "UPDATE candidate_generation SET semantics_version=99",
            _ => "UPDATE candidate_generation SET semantics_projected=projected+1",
        })
        .unwrap();
    }
    let heads = stored_heads(&f);
    std::fs::write(f.0.join("store.sqlite-journal"), b"").unwrap();
    let before = store_hashes(&f);
    assert_eq!(
        Store::open_schema_existing(&f.path()).err().unwrap().code,
        "store_upgrade_required",
        "{mode}"
    );
    assert_eq!(store_hashes(&f), before, "read changed {mode}");
    let request = json!({"object_type":"record","where":{"eq":["status","ready"]}});
    let run = |command: &str, body: &Value| {
        ont::run(
            &[
                "--store".into(),
                f.path().display().to_string(),
                "--now".into(),
                "10".into(),
                command.into(),
            ],
            body.to_string().as_bytes(),
            0,
            0,
        )
    };
    let out = run("search", &request);
    assert_eq!(out.code, 2, "{mode}: {}", out.stderr);
    assert!(out.stderr.is_empty());
    let refusal: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(refusal["code"], "store_upgrade_required");
    assert_eq!(refusal["field"], "--store");
    assert_eq!(
        refusal["hint"],
        "This store needs a one-time upgrade by a write of its owner: `ont index` on the work store (its Lead), or the start of the service that owns a private store."
    );
    let config = ont::mcp::Config {
        store: f.path(),
        now: Some(10),
        client: "cli".into(),
    };
    let exec = ont::cli::execute;
    let mut server = ont::mcp::Server::new(config, &exec);
    server.handle_line(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}).to_string().as_bytes()).unwrap();
    let mcp = server.handle_line(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"ont_search","arguments":request}}).to_string().as_bytes()).unwrap();
    assert_eq!(mcp["result"]["isError"], true, "{mcp}");
    let text = mcp["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(serde_json::from_str::<Value>(text).unwrap(), refusal);
    assert_eq!(store_hashes(&f), before, "command/MCP changed {mode}");
    let upgraded = run("index", &json!({}));
    assert_eq!(upgraded.code, 0, "{} {}", upgraded.stdout, upgraded.stderr);
    let mut reader = Store::open_schema_existing(&f.path()).unwrap();
    assert_eq!(
        reader.scan_heads("record", 10, None).unwrap().len(),
        heads.len()
    );
    assert_eq!(stored_heads(&f), heads);
    let answer = run("search", &request);
    assert_eq!(answer.code, 0, "{} {}", answer.stdout, answer.stderr);
    assert_eq!(
        serde_json::from_str::<Value>(&answer.stdout).unwrap()["result"]["basis"]["matched"],
        1
    );
}
