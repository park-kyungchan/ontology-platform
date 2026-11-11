//! Synthetic format-3 (Schema mode) tests; temporary files only.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::schema_mode::{ActionLogEntry, InstallArgs, StoreRefusal, append_schema_in};
use super::*;
use ont_core::{Coverage, Knowledge, Lineage, Namespace};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "ont-store-v3-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).unwrap();
        Self(dir)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn ns() -> Namespace {
    Namespace {
        instance: "vps://synthetic".into(),
        account: "root".into(),
    }
}
fn ns2() -> Namespace {
    Namespace {
        instance: "vps://other".into(),
        account: "root".into(),
    }
}
fn work_bytes(version: u64) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "format":"ontology-schema/v1","domain":"work","version":version,
        "namespace":{"instance":"vps://synthetic","account":"root"},
        "scope_keys":["tenant"],
        "object_types":{
          "thing":{"owner":"ontology","primary_key":{"from":"$action_id"},"title":"name",
            "properties":{
              "name":{"type":"string","required":true},
              "other":{"type":{"ref":"other"},"required":false},
              "score":{"type":"integer","required":false}}},
          "other":{"owner":"ontology","primary_key":{"from":"$action_id"},
            "properties":{"label":{"type":"string","required":true}}}
        },
        "grants":[{"principal":"agent","scope":{"tenant":"t1"},"read":["thing","other"],"derive":[],"actions":[]}]
    }))
    .unwrap()
}
fn academy_bytes(with_thing: bool) -> Vec<u8> {
    let types = if with_thing {
        json!({"thing":{"owner":"ontology","primary_key":{"from":"$action_id"},
            "properties":{"name":{"type":"string","required":true}}}})
    } else {
        json!({"student":{"owner":"ontology","primary_key":{"from":"$action_id"},
            "properties":{"label":{"type":"string","required":true}}}})
    };
    serde_json::to_vec(&json!({
        "format":"ontology-schema/v1","domain":"academy","version":1,
        "namespace":{"instance":"vps://synthetic2","account":"root"},
        "object_types":types
    }))
    .unwrap()
}
fn scope() -> Scope {
    Scope::domain(
        ns(),
        "work",
        BTreeMap::from([("tenant".into(), "t1".into())]),
    )
    .unwrap()
}
fn id(object_type: &str, key: &str) -> Id {
    Id {
        object_type: object_type.into(),
        namespace: ns(),
        native_key: key.into(),
    }
}
fn fields(pairs: &[(&str, Value)]) -> BTreeMap<String, Knowledge> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), Knowledge::Value(v.clone())))
        .collect()
}
fn obs(
    revision: &str,
    change: Change,
    supersedes: Option<&str>,
    fields: BTreeMap<String, Knowledge>,
) -> Observation {
    Observation {
        revision: revision.into(),
        source_revision: format!("s-{revision}"),
        observed_at: 10,
        fresh_until: 20,
        change,
        supersedes: supersedes.map(str::to_string),
        coverage: Coverage::Complete,
        fields,
        lineage: Lineage {
            artifact_sha256: "a".repeat(64),
            pointer: "/synthetic".into(),
        },
    }
}
fn append(object_id: &Id, observation: Observation) -> Append {
    Append {
        id: object_id.clone(),
        scope: scope(),
        observation,
    }
}
fn name_obs(revision: &str, change: Change, supersedes: Option<&str>, name: &str) -> Observation {
    obs(
        revision,
        change,
        supersedes,
        fields(&[("name", json!(name))]),
    )
}
fn store_with_work() -> Store {
    let mut store = Store::memory_schema().unwrap();
    store
        .install_schema(
            &work_bytes(1),
            InstallArgs {
                installed_at: 100,
                ..Default::default()
            },
        )
        .unwrap();
    store
}

#[test]
fn format3_refuses_old_files_and_old_modes_refuse_format3() {
    let temp = Temp::new();
    let v1 = temp.0.join("v1.sqlite");
    drop(Store::open(&v1).unwrap());
    let before = std::fs::read(&v1).unwrap();
    assert_eq!(Store::open_schema(&v1).err().unwrap().code, "store_failure");
    assert_eq!(std::fs::read(&v1).unwrap(), before);
    let v2 = temp.0.join("v2.sqlite");
    drop(Store::open_program(&v2).unwrap());
    let before = std::fs::read(&v2).unwrap();
    assert_eq!(Store::open_schema(&v2).err().unwrap().code, "store_failure");
    assert_eq!(std::fs::read(&v2).unwrap(), before);
    let v3 = temp.0.join("v3.sqlite");
    drop(Store::open_schema(&v3).unwrap());
    let before = std::fs::read(&v3).unwrap();
    assert!(matches!(Store::open(&v3), Err(Error::StoreFailure)));
    assert!(matches!(Store::open_program(&v3), Err(Error::StoreFailure)));
    assert_eq!(std::fs::read(&v3).unwrap(), before);
    let store = Store::open_schema(&v3).unwrap();
    let version: i64 = store
        .connection
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 3);
}

#[test]
fn schema_install_rules() {
    let mut store = Store::memory_schema().unwrap();
    let pin = store
        .install_schema(
            &work_bytes(1),
            InstallArgs {
                installed_at: 100,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(store.schema_pins().unwrap().get("work"), Some(&pin));
    // Same bytes again: no-op, still one row.
    assert_eq!(
        store
            .install_schema(
                &work_bytes(1),
                InstallArgs {
                    installed_at: 101,
                    ..Default::default()
                }
            )
            .unwrap(),
        pin
    );
    let rows: i64 = store
        .connection
        .query_row("SELECT count(*) FROM schema_installs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1);
    // Same (domain, version) with different bytes refuses.
    let mut changed = serde_json::from_slice::<Value>(&work_bytes(1)).unwrap();
    changed["object_types"]["thing"]["description"] = json!("changed");
    let changed = serde_json::to_vec(&changed).unwrap();
    assert_eq!(
        store
            .install_schema(
                &changed,
                InstallArgs {
                    installed_at: 102,
                    ..Default::default()
                }
            )
            .unwrap_err()
            .code,
        "schema_version_conflict"
    );
    // A type name already declared by another installed domain refuses.
    let err = store
        .install_schema(
            &academy_bytes(true),
            InstallArgs {
                installed_at: 103,
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(err.code, "schema_violation");
    assert_eq!(err.violation.unwrap().code, "duplicate_type_name");
    // The version only grows.
    let pin2 = store
        .install_schema(
            &work_bytes(2),
            InstallArgs {
                installed_at: 104,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(pin2.version, 2);
    assert_eq!(store.schema_pins().unwrap().get("work"), Some(&pin2));
    // Only the exact bytes of the latest version are a no-op (A10); old
    // bytes and different old-version bytes refuse.
    assert_eq!(
        store
            .install_schema(
                &work_bytes(1),
                InstallArgs {
                    installed_at: 105,
                    ..Default::default()
                }
            )
            .unwrap_err()
            .code,
        "schema_version_conflict"
    );
    assert_eq!(
        store
            .install_schema(
                &changed,
                InstallArgs {
                    installed_at: 106,
                    ..Default::default()
                }
            )
            .unwrap_err()
            .code,
        "schema_version_conflict"
    );
    assert_eq!(
        store
            .install_schema(
                &work_bytes(2),
                InstallArgs {
                    installed_at: 108,
                    ..Default::default()
                }
            )
            .unwrap(),
        pin2
    );
    // A second domain with fresh type names installs fine.
    store
        .install_schema(
            &academy_bytes(false),
            InstallArgs {
                installed_at: 107,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(store.schema_pins().unwrap().contains_key("academy"));
}

#[test]
fn tampered_stored_bytes_refuse_the_open() {
    let temp = Temp::new();
    let path = temp.0.join("v3.sqlite");
    let mut store = Store::open_schema(&path).unwrap();
    store
        .install_schema(
            &work_bytes(1),
            InstallArgs {
                installed_at: 100,
                ..Default::default()
            },
        )
        .unwrap();
    drop(store);
    let conn = rusqlite::Connection::open(&path).unwrap();
    // Same domain, changed bytes: the stored sha no longer matches.
    let mut changed = serde_json::from_slice::<Value>(&work_bytes(1)).unwrap();
    changed["object_types"]["thing"]["description"] = serde_json::json!("tampered");
    conn.execute(
        "UPDATE schema_installs SET bytes=?1",
        rusqlite::params![serde_json::to_vec(&changed).unwrap()],
    )
    .unwrap();
    drop(conn);
    assert_eq!(
        Store::open_schema(&path).err().unwrap().code,
        "schema_pin_mismatch"
    );
}

#[test]
fn typed_ingest_refusals_have_exact_codes() {
    let mut store = store_with_work();
    let thing = id("thing", "k1");
    store
        .append_schema(
            &[append(&thing, name_obs("r1", Change::Initial, None, "a"))],
            7,
        )
        .unwrap();
    let check = |item: Append, code: &str, vcode: &str, pointer: &str| {
        let mut store = store_with_work();
        let thing = id("thing", "k1");
        store
            .append_schema(
                &[append(&thing, name_obs("r1", Change::Initial, None, "a"))],
                7,
            )
            .unwrap();
        let err = store.append_schema(&[item], 8).unwrap_err();
        assert_eq!(err.code, code, "{err:?}");
        let v = err.violation.expect("violation");
        assert_eq!((v.code, v.pointer.as_str()), (vcode, pointer));
    };
    // Undeclared type.
    check(
        append(
            &id("ghost", "k9"),
            name_obs("r1", Change::Initial, None, "a"),
        ),
        "schema_violation",
        "unknown_type",
        "/object_types/ghost",
    );
    // Wrong-typed value.
    check(
        append(
            &id("thing", "k2"),
            obs("r1", Change::Initial, None, fields(&[("name", json!(3))])),
        ),
        "schema_violation",
        "wrong_value_type",
        "/fields/name",
    );
    // Non-Domain scope, wrong domain, wrong namespace, wrong key set.
    let mut item = append(
        &id("thing", "k2"),
        name_obs("r1", Change::Initial, None, "a"),
    );
    item.scope = Scope::education(ns(), "C1", "001").unwrap();
    check(item, "schema_violation", "scope_domain_mismatch", "/scope");
    let mut item = append(
        &id("thing", "k2"),
        name_obs("r1", Change::Initial, None, "a"),
    );
    item.scope = Scope::domain(
        ns(),
        "academy",
        BTreeMap::from([("tenant".into(), "t1".into())]),
    )
    .unwrap();
    check(
        item,
        "schema_violation",
        "scope_domain_mismatch",
        "/scope/domain",
    );
    let mut item = append(
        &id("thing", "k2"),
        name_obs("r1", Change::Initial, None, "a"),
    );
    item.scope = Scope::domain(
        ns2(),
        "work",
        BTreeMap::from([("tenant".into(), "t1".into())]),
    )
    .unwrap();
    check(
        item,
        "schema_violation",
        "scope_namespace_mismatch",
        "/scope/namespace",
    );
    let mut item = append(
        &id("thing", "k2"),
        name_obs("r1", Change::Initial, None, "a"),
    );
    item.scope = Scope::domain(ns(), "work", BTreeMap::new()).unwrap();
    check(
        item,
        "schema_violation",
        "scope_keys_mismatch",
        "/scope/keys",
    );
    // Object Id in a foreign namespace.
    let mut foreign = id("thing", "k2");
    foreign.namespace = ns2();
    check(
        append(&foreign, name_obs("r1", Change::Initial, None, "a")),
        "schema_violation",
        "id_namespace_mismatch",
        "/id/namespace",
    );
    // Ref value whose Id is in a foreign namespace.
    let ref_id = Id {
        object_type: "other".into(),
        namespace: ns2(),
        native_key: "x".into(),
    };
    check(
        append(
            &id("thing", "k2"),
            obs(
                "r1",
                Change::Initial,
                None,
                fields(&[
                    ("name", json!("a")),
                    ("other", serde_json::to_value(&ref_id).unwrap()),
                ]),
            ),
        ),
        "schema_violation",
        "ref_namespace_mismatch",
        "/fields/other",
    );
    // Backfill is not a schema-mode change kind.
    check(
        append(&thing, name_obs("r2", Change::Backfill, Some("r1"), "a")),
        "schema_violation",
        "invalid_change_kind",
        "/change",
    );
    // A ref to an object that does not exist is accepted.
    let ghost_ref = Id {
        object_type: "other".into(),
        namespace: ns(),
        native_key: "ghost".into(),
    };
    let mut store = store_with_work();
    store
        .append_schema(
            &[append(
                &id("thing", "k2"),
                obs(
                    "r1",
                    Change::Initial,
                    None,
                    fields(&[
                        ("name", json!("a")),
                        ("other", serde_json::to_value(&ghost_ref).unwrap()),
                    ]),
                ),
            )],
            7,
        )
        .unwrap();
    // Whole batch or nothing.
    let mut store = store_with_work();
    let good = append(
        &id("thing", "good"),
        name_obs("r1", Change::Initial, None, "a"),
    );
    let bad = append(
        &id("thing", "bad"),
        obs("r1", Change::Initial, None, fields(&[("name", json!(3))])),
    );
    assert!(store.append_schema(&[good, bad], 7).is_err());
    assert_eq!(store.scan_heads("thing", 10, None).unwrap().len(), 0);
}

#[test]
fn change_kinds_and_retraction_lifecycle() {
    let mut store = store_with_work();
    let thing = id("thing", "k1");
    let heads = |store: &mut Store| store.scan_heads("thing", 10, None).unwrap().len();
    store
        .append_schema(
            &[append(&thing, name_obs("r1", Change::Initial, None, "a"))],
            7,
        )
        .unwrap();
    assert_eq!(
        store
            .append_schema(
                &[append(&thing, name_obs("rx", Change::Initial, None, "a"))],
                8
            )
            .unwrap_err()
            .code,
        "revision_conflict"
    );
    assert_eq!(
        store
            .append_schema(
                &[append(
                    &thing,
                    name_obs("r2", Change::Update, Some("nope"), "b")
                )],
                8
            )
            .unwrap_err()
            .code,
        "revision_conflict"
    );
    store
        .append_schema(
            &[append(
                &thing,
                name_obs("r2", Change::Update, Some("r1"), "b"),
            )],
            8,
        )
        .unwrap();
    let err = store
        .append_schema(
            &[append(
                &thing,
                name_obs("r3", Change::Refresh, Some("r2"), "c"),
            )],
            9,
        )
        .unwrap_err();
    assert_eq!(err.code, "schema_violation");
    assert_eq!(err.violation.unwrap().code, "refresh_fields_differ");
    store
        .append_schema(
            &[append(
                &thing,
                name_obs("r3", Change::Refresh, Some("r2"), "b"),
            )],
            9,
        )
        .unwrap();
    store
        .append_schema(
            &[append(
                &thing,
                name_obs("r4", Change::Correction, Some("r3"), "d"),
            )],
            10,
        )
        .unwrap();
    store
        .append_schema(
            &[append(
                &thing,
                obs("r5", Change::Retraction, Some("r4"), BTreeMap::new()),
            )],
            11,
        )
        .unwrap();
    let head = store.head(&thing).unwrap().unwrap();
    assert!(head.retracted);
    assert_eq!(head.head.observation.revision, "r5");
    assert_eq!(head.head.ingested_at, 11);
    assert_eq!(heads(&mut store), 0);
    // Full history is kept, and the unchanged Snapshot get still returns it.
    let history = store.history(&thing).unwrap().unwrap();
    assert_eq!(history.observations.len(), 5);
    assert!(history.retracted);
    let object = store
        .snapshot(|s| ont_core::Snapshot::get(s, &thing))
        .unwrap()
        .unwrap();
    assert_eq!(object.observations.len(), 5);
    // After retraction an update refuses; a fresh initial revives the object.
    assert_eq!(
        store
            .append_schema(
                &[append(
                    &thing,
                    name_obs("r6", Change::Update, Some("r5"), "e")
                )],
                12
            )
            .unwrap_err()
            .code,
        "revision_conflict"
    );
    store
        .append_schema(
            &[append(&thing, name_obs("r6", Change::Initial, None, "e"))],
            12,
        )
        .unwrap();
    assert_eq!(heads(&mut store), 1);
    assert_eq!(
        store.history(&thing).unwrap().unwrap().observations.len(),
        6
    );
    let head = store.head(&thing).unwrap().unwrap();
    assert!(!head.retracted);
    assert_eq!(head.schema_pin.domain, "work");
    assert_eq!(
        head.head.schema_pin.sha256,
        store.schema_pins().unwrap().get("work").unwrap().sha256
    );
}

#[test]
fn reopen_keeps_installs_rows_and_heads() {
    let temp = Temp::new();
    let path = temp.0.join("v3.sqlite");
    let thing = id("thing", "k1");
    let pin;
    {
        let mut store = Store::open_schema(&path).unwrap();
        pin = store
            .install_schema(
                &work_bytes(1),
                InstallArgs {
                    installed_at: 100,
                    ..Default::default()
                },
            )
            .unwrap();
        store
            .append_schema(
                &[append(&thing, name_obs("r1", Change::Initial, None, "a"))],
                7,
            )
            .unwrap();
    }
    let mut store = Store::open_schema(&path).unwrap();
    assert_eq!(store.schema_pins().unwrap().get("work"), Some(&pin));
    let head = store.head(&thing).unwrap().unwrap();
    assert_eq!(head.head.observation.revision, "r1");
    assert_eq!(
        store.history(&thing).unwrap().unwrap().observations.len(),
        1
    );
}

#[test]
fn head_scan_orders_limits_and_continues() {
    let mut store = store_with_work();
    let items: Vec<Append> = (0..25)
        .map(|i| {
            append(
                &id("thing", &format!("k{i:02}")),
                name_obs("r1", Change::Initial, None, "a"),
            )
        })
        .collect();
    store.append_schema(&items, 7).unwrap();
    let page1 = store.scan_heads("thing", 10, None).unwrap();
    assert_eq!(page1.len(), 10);
    assert_eq!(page1[0].id.native_key, "k00");
    assert_eq!(page1[9].id.native_key, "k09");
    let after = page1.last().unwrap().id.clone();
    let page2 = store.scan_heads("thing", 10, Some(&after)).unwrap();
    assert_eq!(page2.len(), 10);
    assert_eq!(page2[0].id.native_key, "k10");
    let after = page2.last().unwrap().id.clone();
    let page3 = store.scan_heads("thing", 10, Some(&after)).unwrap();
    assert_eq!(page3.len(), 5);
    // Retracted heads are hidden from the scan.
    let k07 = id("thing", "k07");
    store
        .append_schema(
            &[append(
                &k07,
                obs("r2", Change::Retraction, Some("r1"), BTreeMap::new()),
            )],
            8,
        )
        .unwrap();
    let all = store.scan_heads("thing", 100, None).unwrap();
    assert_eq!(all.len(), 24);
    assert!(all.iter().all(|h| h.id.native_key != "k07"));
    assert_eq!(
        store
            .scan_heads("ghost", 10, None)
            .unwrap_err()
            .violation
            .unwrap()
            .code,
        "unknown_type"
    );
}

#[test]
fn action_log_assigns_seq_and_reads_back() {
    let mut store = store_with_work();
    let entry = ActionLogEntry {
        action_id: "act-1".into(),
        action_type: "set_hold".into(),
        domain: "work".into(),
        schema_domain: "work".into(),
        schema_version: 1,
        actor: json!({"principal":"agent","agent":"lead"}),
        submitted_at: 42,
        parameters: json!({"effect":"x"}),
        basis: json!([]),
        edits: json!([{"id":"hold/1","revision":"r1"}]),
        outcome: "applied".into(),
    };
    assert_eq!(store.append_action_log(&entry).unwrap(), 1);
    let mut second = entry.clone();
    second.action_id = "act-2".into();
    assert_eq!(store.append_action_log(&second).unwrap(), 2);
    let log = store.action_log().unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].1, entry);
    assert_eq!(log[1].1.action_id, "act-2");
}

#[test]
fn reader_sees_committed_state_and_second_writer_gets_store_busy() {
    let temp = Temp::new();
    let path = temp.0.join("v3.sqlite");
    let mut first =
        Store::open_schema_with_busy_timeout(&path, std::time::Duration::from_secs(10)).unwrap();
    first
        .install_schema(
            &work_bytes(1),
            InstallArgs {
                installed_at: 100,
                ..Default::default()
            },
        )
        .unwrap();
    let thing = id("thing", "held");
    let schemas = first.schemas.clone();
    let tx = first
        .connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    append_schema_in(
        &tx,
        &schemas,
        &append(&thing, name_obs("r1", Change::Initial, None, "a")),
        5,
    )
    .unwrap();
    // A reader on a second connection still sees the previous committed
    // state (the held object is not there yet).
    let mut reader =
        Store::open_schema_with_busy_timeout(&path, std::time::Duration::from_millis(100)).unwrap();
    assert_eq!(reader.head(&thing).unwrap(), None);
    // A second writer cannot take the lock within its short timeout.
    let err = reader
        .append_schema(
            &[append(
                &id("thing", "other"),
                name_obs("r1", Change::Initial, None, "a"),
            )],
            6,
        )
        .unwrap_err();
    assert_eq!(err.code, "store_busy");
    drop(tx);
    assert_eq!(reader.head(&thing).unwrap(), None);
}

#[test]
fn schema_apis_refuse_old_modes_and_legacy_paths_refuse_schema_mode() {
    let mut legacy = Store::memory().unwrap();
    assert_eq!(
        legacy
            .install_schema(
                &work_bytes(1),
                InstallArgs {
                    installed_at: 1,
                    ..Default::default()
                }
            )
            .unwrap_err()
            .code,
        "store_failure"
    );
    assert_eq!(
        legacy
            .append_schema(
                &[append(
                    &id("thing", "k"),
                    name_obs("r1", Change::Initial, None, "a")
                )],
                1
            )
            .unwrap_err()
            .code,
        "store_failure"
    );
    let mut store = store_with_work();
    assert_eq!(
        store.append_batch(&[append(
            &id("thing", "k"),
            name_obs("r1", Change::Initial, None, "a")
        )]),
        Err(Error::SchemaViolation)
    );
}

fn install(store: &mut Store, bytes: &[u8], at: u64) -> ont_core::schema::SchemaPin {
    store
        .install_schema(
            bytes,
            InstallArgs {
                installed_at: at,
                ..Default::default()
            },
        )
        .unwrap()
}
fn work_value() -> Value {
    serde_json::from_slice(&work_bytes(1)).unwrap()
}
fn log_entry(action_id: &str) -> ActionLogEntry {
    ActionLogEntry {
        action_id: action_id.into(),
        action_type: "set_hold".into(),
        domain: "work".into(),
        schema_domain: "work".into(),
        schema_version: 1,
        actor: json!({"principal":"agent"}),
        submitted_at: 42,
        parameters: json!({}),
        basis: json!([]),
        edits: json!([]),
        outcome: "applied".into(),
    }
}

#[test]
fn second_connection_install_refreshes_first_connections_next_append() {
    let temp = Temp::new();
    let path = temp.0.join("v3.sqlite");
    let mut a = Store::open_schema(&path).unwrap();
    install(&mut a, &work_bytes(1), 100);
    let mut b = Store::open_schema(&path).unwrap();
    let mut v2 = work_value();
    v2["version"] = json!(2);
    v2["object_types"]["thing"]["properties"]["score"]["required"] = json!(true);
    install(&mut b, &serde_json::to_vec(&v2).unwrap(), 101);
    // A's cache still holds v1, but its next append validates against v2.
    let err = a
        .append_schema(
            &[append(
                &id("thing", "k1"),
                name_obs("r1", Change::Initial, None, "a"),
            )],
            7,
        )
        .unwrap_err();
    assert_eq!(err.code, "schema_violation");
    assert_eq!(err.violation.unwrap().code, "missing_required_property");
    let with_score = obs(
        "r1",
        Change::Initial,
        None,
        fields(&[("name", json!("a")), ("score", json!(1))]),
    );
    a.append_schema(&[append(&id("thing", "k1"), with_score)], 7)
        .unwrap();
    assert_eq!(
        a.head(&id("thing", "k1"))
            .unwrap()
            .unwrap()
            .schema_pin
            .version,
        2
    );
}

#[test]
fn two_connections_installing_clashing_types_second_refuses_and_store_opens() {
    let temp = Temp::new();
    let path = temp.0.join("v3.sqlite");
    let mut a = Store::open_schema(&path).unwrap();
    let mut b = Store::open_schema(&path).unwrap();
    install(&mut a, &work_bytes(1), 100);
    let err = b
        .install_schema(
            &academy_bytes(true),
            InstallArgs {
                installed_at: 101,
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(err.code, "schema_violation");
    assert_eq!(err.violation.unwrap().code, "duplicate_type_name");
    let mut reopened = Store::open_schema(&path).unwrap();
    assert!(reopened.schema_pins().unwrap().contains_key("work"));
    assert!(!reopened.schema_pins().unwrap().contains_key("academy"));
}

#[test]
fn older_pin_stays_readable_after_v2_and_removed_type_refuses_new_writes() {
    let mut store = Store::memory_schema().unwrap();
    let pin1 = install(&mut store, &work_bytes(1), 100);
    let other = id("other", "o1");
    store
        .append_schema(
            &[append(
                &other,
                obs(
                    "r1",
                    Change::Initial,
                    None,
                    fields(&[("label", json!("x"))]),
                ),
            )],
            7,
        )
        .unwrap();
    let mut v2 = work_value();
    v2["version"] = json!(2);
    v2["object_types"].as_object_mut().unwrap().remove("other");
    v2["grants"][0]["read"] = serde_json::json!(["thing"]);
    v2["object_types"]["thing"]["properties"]
        .as_object_mut()
        .unwrap()
        .remove("other");
    let pin2 = install(&mut store, &serde_json::to_vec(&v2).unwrap(), 101);
    assert_ne!(pin1.sha256, pin2.sha256);
    let head = store.head(&other).unwrap().unwrap();
    assert_eq!(head.schema_pin, pin1);
    assert_eq!(store.history(&other).unwrap().unwrap().schema_pin, pin1);
    let err = store
        .append_schema(
            &[append(
                &id("other", "o2"),
                obs(
                    "r1",
                    Change::Initial,
                    None,
                    fields(&[("label", json!("y"))]),
                ),
            )],
            8,
        )
        .unwrap_err();
    assert_eq!(err.violation.unwrap().code, "unknown_type");
    assert_eq!(
        store
            .scan_heads("other", 10, None)
            .unwrap_err()
            .violation
            .unwrap()
            .code,
        "unknown_type"
    );
    store
        .append_schema(
            &[append(
                &id("thing", "k1"),
                name_obs("r1", Change::Initial, None, "a"),
            )],
            8,
        )
        .unwrap();
    assert_eq!(
        store.head(&id("thing", "k1")).unwrap().unwrap().schema_pin,
        pin2
    );
}

#[test]
fn write_transaction_is_atomic_and_sees_its_own_appends() {
    let mut store = store_with_work();
    let thing = id("thing", "k1");
    store
        .write(20, |tx| {
            tx.append(&append(&thing, name_obs("r1", Change::Initial, None, "a")))?;
            tx.append(&append(
                &thing,
                name_obs("r2", Change::Update, Some("r1"), "b"),
            ))?;
            assert_eq!(tx.log_action(&log_entry("act-1"))?, 1);
            let head = tx.head(&thing)?.unwrap();
            assert_eq!(head.head.observation.revision, "r2");
            Ok(())
        })
        .unwrap();
    assert_eq!(
        store.history(&thing).unwrap().unwrap().observations.len(),
        2
    );
    assert_eq!(store.action_log().unwrap().len(), 1);
    let err = store
        .write(21, |tx| {
            tx.append(&append(
                &id("thing", "k2"),
                name_obs("r1", Change::Initial, None, "a"),
            ))?;
            tx.log_action(&log_entry("act-2"))?;
            Err::<(), _>(StoreRefusal::new("test_abort"))
        })
        .unwrap_err();
    assert_eq!(err.code, "test_abort");
    assert_eq!(store.head(&id("thing", "k2")).unwrap(), None);
    assert_eq!(store.action_log().unwrap().len(), 1);
}

#[test]
fn corrupt_head_or_history_is_store_failure_not_absent() {
    let temp = Temp::new();
    let path = temp.0.join("v3.sqlite");
    let thing = id("thing", "k1");
    let mut store = Store::open_schema(&path).unwrap();
    install(&mut store, &work_bytes(1), 100);
    store
        .append_schema(
            &[append(&thing, name_obs("r1", Change::Initial, None, "a"))],
            7,
        )
        .unwrap();
    drop(store);
    let raw = rusqlite::Connection::open(&path).unwrap();
    raw.execute("DELETE FROM observations WHERE revision='r1'", [])
        .unwrap();
    drop(raw);
    let mut store = Store::open_schema(&path).unwrap();
    assert_eq!(store.head(&thing).unwrap_err().code, "store_failure");
    assert_eq!(store.history(&thing).unwrap_err().code, "store_failure");
    // A head pointer that no observation carries is also corruption.
    let raw = rusqlite::Connection::open(&path).unwrap();
    raw.execute_batch(
        "INSERT INTO observations(object_type,instance,account,native_key,revision,ordinal,payload,ingested_at,schema_domain,schema_version,change_kind) SELECT object_type,instance,account,native_key,'r1',0,'{}',7,'work',1,'initial' FROM objects",
    )
    .unwrap();
    drop(raw);
    assert_eq!(store.history(&thing).unwrap_err().code, "store_failure");
}

#[test]
fn scan_continues_by_primary_key_across_namespaces_and_special_keys() {
    let mut store = Store::memory_schema().unwrap();
    install(&mut store, &work_bytes(1), 100);
    for key in ["k%", "k_", "ké", "k~"] {
        store
            .append_schema(
                &[append(
                    &id("thing", key),
                    name_obs("r1", Change::Initial, None, "a"),
                )],
                7,
            )
            .unwrap();
    }
    // A v2 schema for the same domain in another namespace.
    let mut v2 = work_value();
    v2["version"] = json!(2);
    v2["namespace"] = json!({"instance":"vps://other","account":"root"});
    install(&mut store, &serde_json::to_vec(&v2).unwrap(), 101);
    let scope2 = Scope::domain(
        ns2(),
        "work",
        BTreeMap::from([("tenant".into(), "t1".into())]),
    )
    .unwrap();
    for key in ["a1", "k%"] {
        let mut item = append(
            &id("thing", key),
            name_obs("r1", Change::Initial, None, "a"),
        );
        item.id.namespace = ns2();
        item.scope = scope2.clone();
        store.append_schema(&[item], 8).unwrap();
    }
    let mut seen: Vec<(String, String)> = Vec::new();
    let mut after: Option<Id> = None;
    loop {
        let page = store.scan_heads("thing", 2, after.as_ref()).unwrap();
        if page.is_empty() {
            break;
        }
        after = page.last().map(|h| h.id.clone());
        seen.extend(
            page.iter()
                .map(|h| (h.id.namespace.instance.clone(), h.id.native_key.clone())),
        );
    }
    assert_eq!(seen.len(), 6, "{seen:?}");
    let unique: std::collections::BTreeSet<_> = seen.iter().collect();
    assert_eq!(unique.len(), 6);
    let mut sorted = seen.clone();
    sorted.sort();
    assert_eq!(seen, sorted, "primary-key order");
    assert_eq!(
        store.scan_heads("thing", 10_001, None).unwrap_err().code,
        "limit_too_large"
    );
    let mut big = append(
        &id("thing", "big"),
        name_obs("r1", Change::Initial, None, "a"),
    );
    big.id.namespace = ns2();
    big.scope = scope2.clone();
    assert_eq!(
        store.append_schema(&[big], u64::MAX).unwrap_err().code,
        "value_out_of_range"
    );
}

#[test]
fn busy_open_with_exclusive_lock_is_store_busy() {
    let temp = Temp::new();
    let path = temp.0.join("v3.sqlite");
    drop(Store::open_schema(&path).unwrap());
    let raw = rusqlite::Connection::open(&path).unwrap();
    raw.execute_batch("BEGIN EXCLUSIVE").unwrap();
    let err = Store::open_schema_with_busy_timeout(&path, std::time::Duration::from_millis(100))
        .err()
        .unwrap();
    assert_eq!(err.code, "store_busy");
    raw.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn duplicate_action_id_outcome_check_and_install_source_are_recorded() {
    let mut store = Store::memory_schema().unwrap();
    let pin = store
        .install_schema(
            &work_bytes(1),
            InstallArgs {
                installed_at: 5,
                source_repository: Some("park-kyungchan/ontology-platform".into()),
                source_commit: Some("abc123".into()),
            },
        )
        .unwrap();
    assert_eq!(pin.domain, "work");
    let (repo, commit): (Option<String>, Option<String>) = store
        .connection
        .query_row(
            "SELECT source_repository, source_commit FROM schema_installs WHERE domain='work'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(repo.as_deref(), Some("park-kyungchan/ontology-platform"));
    assert_eq!(commit.as_deref(), Some("abc123"));
    assert_eq!(store.append_action_log(&log_entry("act-1")).unwrap(), 1);
    assert_eq!(
        store
            .append_action_log(&log_entry("act-1"))
            .unwrap_err()
            .code,
        "duplicate_action_id"
    );
    let mut bad = log_entry("act-2");
    bad.outcome = "bogus".into();
    assert_eq!(
        store.append_action_log(&bad).unwrap_err().code,
        "store_failure"
    );
}

#[test]
fn scope_conflict_duplicate_revision_seed_and_nested_refs() {
    let mut store = store_with_work();
    let thing = id("thing", "k1");
    store
        .append_schema(
            &[append(&thing, name_obs("r1", Change::Initial, None, "a"))],
            7,
        )
        .unwrap();
    // Same object under a different scope value: scope_conflict.
    let mut item = append(&thing, name_obs("r2", Change::Update, Some("r1"), "b"));
    item.scope = Scope::domain(
        ns(),
        "work",
        BTreeMap::from([("tenant".into(), "t2".into())]),
    )
    .unwrap();
    assert_eq!(
        store.append_schema(&[item], 8).unwrap_err().code,
        "scope_conflict"
    );
    // A repeated revision is distinct from a moved head.
    assert_eq!(
        store
            .append_schema(
                &[append(
                    &thing,
                    name_obs("r1", Change::Update, Some("r1"), "b")
                )],
                8
            )
            .unwrap_err()
            .code,
        "duplicate_revision"
    );
    // seed on a format 3 store keeps the legacy-path refusal.
    let object = Object {
        id: thing.clone(),
        scope: scope(),
        head: "r1".into(),
        observations: vec![name_obs("r1", Change::Initial, None, "a")],
    };
    assert_eq!(store.seed(&[object], &[]), Err(Error::SchemaViolation));
}

#[test]
fn refs_nested_in_array_and_struct_are_checked() {
    let mut v = work_value();
    v["object_types"]["thing"]["properties"]["refs"] =
        json!({"type":{"array":{"ref":"other"}},"required":false});
    v["object_types"]["thing"]["properties"]["wrapped"] =
        json!({"type":{"struct":{"inner":{"ref":"other"}}},"required":false});
    let mut store = Store::memory_schema().unwrap();
    install(&mut store, &serde_json::to_vec(&v).unwrap(), 100);
    let foreign = serde_json::to_value(Id {
        object_type: "other".into(),
        namespace: ns2(),
        native_key: "x".into(),
    })
    .unwrap();
    for (key, extra) in [
        ("arr", ("refs", json!([foreign.clone()]))),
        ("str", ("wrapped", json!({"inner": foreign.clone()}))),
    ] {
        let item = append(
            &id("thing", key),
            obs(
                "r1",
                Change::Initial,
                None,
                fields(&[("name", json!("a")), extra]),
            ),
        );
        let err = store.append_schema(&[item], 7).unwrap_err();
        assert_eq!(
            err.violation.unwrap().code,
            "ref_namespace_mismatch",
            "{key}"
        );
    }
    let good_ref = serde_json::to_value(Id {
        object_type: "other".into(),
        namespace: ns(),
        native_key: "ghost".into(),
    })
    .unwrap();
    store
        .append_schema(
            &[append(
                &id("thing", "ok"),
                obs(
                    "r1",
                    Change::Initial,
                    None,
                    fields(&[
                        ("name", json!("a")),
                        ("refs", json!([good_ref.clone()])),
                        ("wrapped", json!({"inner": good_ref})),
                    ]),
                ),
            )],
            7,
        )
        .unwrap();
}

#[test]
fn two_items_for_one_object_in_one_batch_see_each_other() {
    let mut store = store_with_work();
    let thing = id("thing", "k1");
    store
        .append_schema(
            &[
                append(&thing, name_obs("r1", Change::Initial, None, "a")),
                append(&thing, name_obs("r2", Change::Update, Some("r1"), "b")),
            ],
            7,
        )
        .unwrap();
    let head = store.head(&thing).unwrap().unwrap();
    assert_eq!(head.head.observation.revision, "r2");
    assert_eq!(
        store.history(&thing).unwrap().unwrap().observations.len(),
        2
    );
}

#[test]
fn busy_writer_holds_transaction_while_reader_reads_committed_row() {
    let temp = Temp::new();
    let path = temp.0.join("v3.sqlite");
    let mut first =
        Store::open_schema_with_busy_timeout(&path, std::time::Duration::from_secs(10)).unwrap();
    install(&mut first, &work_bytes(1), 100);
    let committed = id("thing", "committed");
    first
        .append_schema(
            &[append(
                &committed,
                name_obs("r1", Change::Initial, None, "a"),
            )],
            7,
        )
        .unwrap();
    let held = id("thing", "held");
    first
        .write(30, |tx| {
            tx.append(&append(&held, name_obs("r1", Change::Initial, None, "a")))?;
            let mut reader =
                Store::open_schema_with_busy_timeout(&path, std::time::Duration::from_millis(200))
                    .unwrap();
            // The reader sees the previously committed row through the
            // public path, and the held append is not visible.
            assert_eq!(
                reader
                    .head(&committed)
                    .unwrap()
                    .unwrap()
                    .head
                    .observation
                    .revision,
                "r1"
            );
            assert_eq!(reader.head(&held).unwrap(), None);
            let err = reader
                .append_schema(
                    &[append(
                        &id("thing", "other"),
                        name_obs("r1", Change::Initial, None, "a"),
                    )],
                    31,
                )
                .unwrap_err();
            assert_eq!(err.code, "store_busy");
            Ok(())
        })
        .unwrap();
    assert!(first.head(&held).unwrap().is_some());
}
