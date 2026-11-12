#![allow(clippy::unwrap_used)]
use super::*;

#[test]
fn preopened_old_statements_cannot_hide_mutations_and_repair_is_atomic() {
    let dir = std::env::temp_dir().join(format!("ont-old-generation-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("store.sqlite");
    std::fs::write(
        &path,
        include_bytes!("../../ont/tests/fixtures/o22-previous.sqlite"),
    )
    .unwrap();
    let old = Connection::open(&path).unwrap();
    old.execute_batch("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF;")
        .unwrap();
    let (key, revision, payload): (String,String,String) = old.query_row(
        "SELECT o.native_key,o.head,ob.payload FROM objects o JOIN observations ob ON ob.object_type=o.object_type AND ob.native_key=o.native_key AND ob.revision=o.head WHERE json_extract(ob.payload,'$.fields.status.value')='superseded'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    let mut change = old.prepare("UPDATE objects SET head=?5 WHERE object_type=?1 AND instance=?2 AND account=?3 AND native_key=?4").unwrap();
    let mut insert = old.prepare("INSERT INTO observations(object_type,instance,account,native_key,revision,ordinal,payload,ingested_at,schema_domain,schema_version,change_kind) SELECT ?1,?2,?3,?4,?5,COALESCE(MAX(ordinal),-1)+1,?6,?7,?8,?9,?10 FROM observations WHERE object_type=?1 AND instance=?2 AND account=?3 AND native_key=?4").unwrap();
    let mut install = old.prepare("INSERT INTO schema_installs(domain,version,sha256,bytes,installed_at,source_repository,source_commit,source_path) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)").unwrap();
    change
        .execute(params!["record", "test", "root", key, revision])
        .unwrap();
    let baseline = change.reset_status(rusqlite::StatementStatus::VmStep);
    let mut store = Store::open_schema_existing_writable(&path).unwrap();
    let bytes: Vec<u8> = old
        .query_row(
            "SELECT bytes FROM schema_installs WHERE domain='previous' AND version=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let scope = Scope::domain(
        ont_core::Namespace {
            instance: "test".into(),
            account: "root".into(),
        },
        "previous",
        BTreeMap::new(),
    )
    .unwrap();
    let condition = CandidateCondition {
        property: "status".into(),
        values: vec![Value::String("ready".into())],
        used: BTreeSet::from(["status".into()]),
    };
    let read = |store: &mut Store| {
        store
            .read_schema(|tx| {
                tx.candidate_access(
                    "record",
                    Some(&condition),
                    10,
                    100,
                    None,
                    std::slice::from_ref(&scope),
                )
            })
            .unwrap()
    };
    let mut schema: Value = serde_json::from_slice(&bytes).unwrap();
    for case in ["ready", "retract", "grant"] {
        old.execute_batch("BEGIN IMMEDIATE").unwrap();
        if case == "grant" {
            schema["version"] = Value::from(2);
            schema["grants"][0]["scope"] = serde_json::json!({});
            schema["grants"][0]["read"] = serde_json::json!([]);
            let bytes = serde_json::to_vec(&schema).unwrap();
            let pin = ont_core::schema::Schema::check_bytes(&bytes, &BTreeSet::new())
                .unwrap()
                .pin()
                .clone();
            install
                .execute(params![
                    "previous",
                    2,
                    pin.sha256,
                    bytes,
                    2,
                    None::<String>,
                    None::<String>,
                    None::<String>
                ])
                .unwrap();
        } else {
            let mut obs: ont_core::Observation = serde_json::from_str(&payload).unwrap();
            obs.revision = format!("old-{case}");
            obs.supersedes = Some(revision.clone());
            if case == "ready" {
                obs.change = ont_core::Change::Update;
                obs.fields.insert(
                    "status".into(),
                    Knowledge::Value(Value::String("ready".into())),
                );
            } else {
                obs.change = ont_core::Change::Retraction;
                obs.supersedes = Some("old-ready".into());
                obs.fields.clear();
            }
            insert
                .execute(params![
                    "record",
                    "test",
                    "root",
                    key,
                    obs.revision,
                    serde_json::to_string(&obs).unwrap(),
                    2,
                    "previous",
                    1,
                    if case == "ready" {
                        "update"
                    } else {
                        "retraction"
                    }
                ])
                .unwrap();
            change
                .execute(params!["record", "test", "root", key, obs.revision])
                .unwrap();
        }
        old.execute_batch("COMMIT").unwrap();
        assert_eq!(read(&mut store).unwrap_err().code, "projection_stale");
        let repair = if case == "grant" {
            serde_json::to_vec(&schema).unwrap()
        } else {
            bytes.clone()
        };
        if case == "ready" {
            // A partial native write must not certify earlier unprojected edits.
            assert_eq!(
                store.write(2, |_| Ok(())).unwrap_err().code,
                "projection_stale"
            );
            INTERRUPT_REBUILD.set(true);
            assert_eq!(
                store
                    .install_schema(&repair, InstallArgs::default())
                    .unwrap_err()
                    .code,
                "test_interrupted_rebuild"
            );
            assert_eq!(read(&mut store).unwrap_err().code, "projection_stale");
        }
        store
            .install_schema(&repair, InstallArgs::default())
            .unwrap();
        assert_eq!(
            read(&mut store).unwrap().len(),
            if case == "ready" {
                2
            } else if case == "retract" {
                1
            } else {
                0
            }
        );
    }
    assert!(change.get_status(rusqlite::StatementStatus::RePrepare) > 0);
    assert!(insert.get_status(rusqlite::StatementStatus::RePrepare) > 0);
    assert!(install.get_status(rusqlite::StatementStatus::RePrepare) > 0);
    change.reset_status(rusqlite::StatementStatus::VmStep);
    change
        .execute(params!["record", "test", "root", key, "old-retract"])
        .unwrap();
    let guarded = change.get_status(rusqlite::StatementStatus::VmStep);
    eprintln!("head_update_vm baseline={baseline} with_generation_trigger={guarded}");
    drop(store);
    drop(change);
    drop(insert);
    drop(install);
    drop(old);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn native_write_and_read_stay_current_without_rebuilding() {
    let dir = std::env::temp_dir().join(format!("ont-native-generation-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("store.sqlite");
    std::fs::write(
        &path,
        include_bytes!("../../ont/tests/fixtures/o22-previous.sqlite"),
    )
    .unwrap();
    let mut store = Store::open_schema_existing_writable(&path).unwrap();
    let head = store.scan_heads("record", 1, None).unwrap().remove(0);
    let mut obs = head.head.observation.clone();
    obs.supersedes = Some(obs.revision.clone());
    obs.revision = "native".into();
    obs.change = ont_core::Change::Update;
    INTERRUPT_REBUILD.set(true);
    store
        .append_schema(
            &[Append {
                id: head.id,
                scope: head.scope.clone(),
                observation: obs,
            }],
            2,
        )
        .unwrap();
    assert!(current(&store.connection, &store.candidate_triggers).unwrap());
    store
        .read_schema(|tx| {
            tx.candidate_access("record", None, 10, 100, None, &[head.scope])
                .unwrap()
        })
        .unwrap();
    assert!(INTERRUPT_REBUILD.replace(false)); // Neither write nor read rebuilt.
    let mut check = store.connection.prepare(GENERATION_CHECK).unwrap();
    assert!(check.query_row([], |r| r.get::<_, bool>(0)).unwrap());
    eprintln!(
        "generation_read_vm={}",
        check.get_status(rusqlite::StatementStatus::VmStep)
    );
    drop(check);
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

struct SemanticsFixture {
    dir: std::path::PathBuf,
    path: std::path::PathBuf,
}
impl SemanticsFixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("ont-semantics-{name}-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("store.sqlite");
        std::fs::write(
            &path,
            include_bytes!("../../ont/tests/fixtures/o22-previous.sqlite"),
        )
        .unwrap();
        drop(Store::open_schema_existing_writable(&path).unwrap());
        Self { dir, path }
    }
}
impl Drop for SemanticsFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn semantics_marker(c: &Connection) -> Option<i64> {
    let has_column: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('candidate_generation') WHERE name='semantics_version')", [], |r| r.get(0)).unwrap();
    has_column.then(|| {
        c.query_row(
            "SELECT semantics_version FROM candidate_generation WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .unwrap()
    })
}
fn projection_snapshot(c: &Connection) -> String {
    c.query_row("SELECT json_group_array(json_array(object_type,visibility,property,value,instance,account,native_key,scope_keys)) FROM (SELECT * FROM head_candidates ORDER BY object_type,visibility,property,value,instance,account,native_key)", [], |r| r.get(0)).unwrap()
}
#[test]
fn current_projection_open_does_not_rebuild() {
    let f = SemanticsFixture::new("current");
    super::super::PAYLOAD_COPIES.set(0);
    INTERRUPT_REBUILD.set(true);
    let store = Store::open_schema_existing(&f.path).unwrap();
    let armed = INTERRUPT_REBUILD.replace(false);
    let copies = super::super::PAYLOAD_COPIES.get();
    eprintln!("current_open_payload_copies={copies}, rebuild_probe_untouched={armed}");
    assert_eq!(copies, 0);
    assert!(armed, "a current projection must not enter rebuild");
    assert_eq!(semantics_marker(&store.connection), Some(1));
    let certified: bool = store.connection.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('candidate_generation') WHERE name='semantics_projected')", [], |r| r.get(0)).unwrap();
    assert!(
        certified,
        "native checkpoint must certify its projected generation"
    );
    assert!(
        store
            .connection
            .query_row(
                "SELECT semantics_projected=projected FROM candidate_generation",
                [],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
    );
    // A mutation without a checkpoint remains stale; opening must not rebuild it.
    let mutation = Connection::open(&f.path).unwrap();
    mutation
        .execute("UPDATE objects SET head=head", [])
        .unwrap();
    INTERRUPT_REBUILD.set(true);
    let reopened = Store::open_schema_existing(&f.path).unwrap();
    assert!(
        INTERRUPT_REBUILD.replace(false),
        "compare with projected, not mutation"
    );
    assert!(!current(&reopened.connection, &reopened.candidate_triggers).unwrap());
    drop(reopened);
    // A preopened reader must not consume a projection relabelled by another engine.
    let other = Connection::open(&f.path).unwrap();
    other
        .execute("UPDATE candidate_generation SET semantics_version=99", [])
        .unwrap();
    assert!(!current(&store.connection, &store.candidate_triggers).unwrap());
}
#[test]
fn semantics_rebuild_failure_preserves_projection_and_version() {
    for absent in [true, false] {
        let f = SemanticsFixture::new(if absent { "absent" } else { "different" });
        let old = Connection::open(&f.path).unwrap();
        if semantics_marker(&old).is_some() {
            old.execute_batch("ALTER TABLE candidate_generation DROP COLUMN semantics_version")
                .unwrap();
        }
        if !absent {
            old.execute_batch("ALTER TABLE candidate_generation ADD COLUMN semantics_version INTEGER NOT NULL DEFAULT 99").unwrap();
        }
        let projection = projection_snapshot(&old);
        let marker = semantics_marker(&old);
        let generation: (i64, i64) = old
            .query_row(
                "SELECT mutation,projected FROM candidate_generation",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        INTERRUPT_REBUILD.set(true);
        let result = Store::open_schema_existing_writable(&f.path);
        let unconsumed = INTERRUPT_REBUILD.replace(false);
        let failure = match result {
            Err(e) => e,
            Ok(_) => panic!("old semantics were accepted without attempting rebuild"),
        };
        assert_eq!(failure.code, "test_interrupted_rebuild");
        assert!(!unconsumed);
        assert_eq!(projection_snapshot(&old), projection);
        assert_eq!(
            semantics_marker(&old),
            marker,
            "DDL/version must roll back too"
        );
        assert_eq!(
            old.query_row(
                "SELECT mutation,projected FROM candidate_generation",
                [],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            )
            .unwrap(),
            generation
        );
        assert_eq!(
            Store::open_schema_existing(&f.path).err().unwrap().code,
            "store_upgrade_required"
        );
        assert_eq!(semantics_marker(&old), marker);
        drop(Store::open_schema_existing_writable(&f.path).unwrap());
        drop(Store::open_schema_existing(&f.path).unwrap());
        assert_eq!(semantics_marker(&old), Some(1));
        assert_eq!(projection_snapshot(&old), projection);
        assert_eq!(
            old.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            4
        );
    }
}

#[test]
fn old_engine_checkpoint_stales_preopened_reader() {
    let f = SemanticsFixture::new("old-checkpoint-reader");
    let mut reader = Store::open_schema_existing(&f.path).unwrap();
    let head = reader.scan_heads("record", 1, None).unwrap().remove(0);
    let id = head.id.clone();
    let mut obs = head.head.observation.clone();
    obs.supersedes = Some(obs.revision.clone());
    obs.revision = "old-oversized".into();
    obs.change = Change::Update;
    obs.fields.insert(
        "status".into(),
        Knowledge::Value(Value::String("x".repeat(MAX_OBSERVATION_BYTES))),
    );
    obs.validate().unwrap();
    let payload = serde_json::to_string(&obs).unwrap();
    assert!(payload.len() > MAX_OBSERVATION_BYTES);
    let resolved = format!(
        "v{}",
        match obs.fields.get("status").unwrap() {
            Knowledge::Value(v) => v,
            _ => unreachable!(),
        }
    );
    let old = Connection::open(&f.path).unwrap();
    old.execute_batch("BEGIN IMMEDIATE").unwrap();
    old.execute("INSERT INTO observations(object_type,instance,account,native_key,revision,ordinal,payload,ingested_at,schema_domain,schema_version,change_kind) SELECT ?1,?2,?3,?4,?5,COALESCE(MAX(ordinal),-1)+1,?6,2,?7,?8,'update' FROM observations WHERE object_type=?1 AND instance=?2 AND account=?3 AND native_key=?4", params![id.object_type,id.namespace.instance,id.namespace.account,id.native_key,obs.revision,payload,head.schema_pin.domain,head.schema_pin.version]).unwrap();
    old.execute("UPDATE objects SET head=?5 WHERE object_type=?1 AND instance=?2 AND account=?3 AND native_key=?4", params![id.object_type,id.namespace.instance,id.namespace.account,id.native_key,obs.revision]).unwrap();
    assert_eq!(old.execute("UPDATE head_candidates SET value=?5 WHERE object_type=?1 AND instance=?2 AND account=?3 AND native_key=?4 AND property='status'", params![id.object_type,id.namespace.instance,id.namespace.account,id.native_key,resolved]).unwrap(),1);
    old.execute("UPDATE candidate_generation SET projected=mutation", [])
        .unwrap();
    old.execute_batch("COMMIT").unwrap();
    let condition = CandidateCondition {
        property: "status".into(),
        values: vec![Value::String("not present".into())],
        used: BTreeSet::from(["status".into()]),
    };
    let result = reader
        .read_schema(|tx| {
            tx.candidate_access("record", Some(&condition), 10, 100, None, &[head.scope])
        })
        .unwrap();
    eprintln!(
        "preopened_reader_result={}",
        result.as_ref().err().map_or("ok", |e| e.code)
    );
    assert_eq!(result.unwrap_err().code, "projection_stale");
}
