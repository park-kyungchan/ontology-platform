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
    assert!(current(&store.connection).unwrap());
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
