#![allow(clippy::unwrap_used)]
use super::*;

#[test]
fn format_barrier_restore_and_two_openers() {
    let dir = std::env::temp_dir().join(format!("ont-format4-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("store.sqlite");
    let original = include_bytes!("../../ont/tests/fixtures/o22-previous.sqlite");
    for _ in 0..2 {
        std::fs::write(&path, original).unwrap();
        assert_eq!(
            Store::open_schema_existing(&path).err().unwrap().code,
            "store_upgrade_required"
        );
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|s| {
            for _ in 0..2 {
                s.spawn(|| {
                    barrier.wait();
                    Store::open_schema_existing_writable(&path).unwrap();
                });
            }
        });
        let store = Store::open_schema_existing(&path).unwrap();
        assert_eq!(
            store
                .connection
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            4
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn visible_keyset_work_is_bounded_with_hidden_and_retracted_heads() {
    let mut counts = Vec::new();
    for hidden in [512, 1024, 2048] {
        let mut store = Store::memory_schema().unwrap();
        let schema = serde_json::json!({"format":"ontology-schema/v1","domain":"test","version":1,
            "namespace":{"instance":"test","account":"root"},"scope_keys":["tenant"],"sources":{},"value_types":{},
            "object_types":{"record":{"owner":"ontology","primary_key":{"from":"$action_id"},"properties":{"status":{"type":"string"}}}},
            "link_types":{},"action_types":{},"functions":{},
            "grants":[{"principal":"agent","scope":{"tenant":"public"},"read":["record"],"derive":[],"actions":[]},
                      {"principal":"other","scope":{"tenant":"private"},"read":["record"],"derive":[],"actions":[]}]});
        store
            .install_schema(schema.to_string().as_bytes(), InstallArgs::default())
            .unwrap();
        let rows: Vec<_> = (0..hidden+3).map(|i| {
            let namespace = ont_core::Namespace{instance:"test".into(),account:"root".into()};
            Append { id:Id{object_type:"record".into(),namespace:namespace.clone(),native_key:format!("{i:05}")},
                scope:Scope::domain(namespace,"test",BTreeMap::from([("tenant".into(),if i<3 {"public"} else {"private"}.into())])).unwrap(),
                observation:serde_json::from_value(serde_json::json!({"revision":"r0","source_revision":"s0","observed_at":1,"fresh_until":100,
                    "change":"initial","supersedes":null,"coverage":"complete","fields":{"status":{"kind":"value","value":"ready"}},
                    "lineage":{"artifact_sha256":"a".repeat(64),"pointer":""}})).unwrap() }
        }).collect();
        let mut retracted = vec![];
        for i in 0..hidden {
            let mut initial = rows[0].clone();
            initial.id.native_key = format!("retracted{i:05}");
            let mut deleted = initial.clone();
            deleted.observation.revision = "r1".into();
            deleted.observation.supersedes = Some("r0".into());
            deleted.observation.change = Change::Retraction;
            deleted.observation.fields.clear();
            retracted.extend([initial, deleted]);
        }
        store.append_schema(&rows, 1).unwrap();
        store.append_schema(&retracted, 1).unwrap();
        store
            .read_schema(|tx| {
                let scope = rows[0].scope.clone();
                let condition = CandidateCondition {
                    property: "status".into(),
                    values: vec![Value::String("ready".into())],
                    used: BTreeSet::from(["status".into()]),
                };
                for condition in [Some(&condition), None] {
                    STEPS.set(0);
                    let mut after = None;
                    let mut ids = Vec::new();
                    loop {
                        let page = tx
                            .candidate_access(
                                "record",
                                condition,
                                10,
                                2,
                                after.as_ref(),
                                std::slice::from_ref(&scope),
                            )
                            .unwrap();
                        if page.is_empty() {
                            break;
                        }
                        after = page.last().map(|a| a.id.clone());
                        ids.extend(page.into_iter().map(|a| a.id.native_key));
                    }
                    assert_eq!(ids, ["00000", "00001", "00002"]);
                    assert!(STEPS.get() > 0, "VM-work probe must be active");
                    counts.push(STEPS.get());
                }
                let plan: Vec<String> = tx
                    .tx
                    .prepare(&format!("EXPLAIN QUERY PLAN {POSTING_SQL}"))
                    .unwrap()
                    .query_map(
                        params![
                            "record",
                            json_text(&scope).unwrap(),
                            "status",
                            "v\"ready\"",
                            "",
                            "",
                            "",
                            2
                        ],
                        |r| r.get(3),
                    )
                    .unwrap()
                    .map(Result::unwrap)
                    .collect();
                assert!(plan.iter().any(|p| p.contains("USING PRIMARY KEY")));
                assert!(
                    !plan
                        .iter()
                        .any(|p| p.contains("TEMP B-TREE") || p.contains("SCAN "))
                );
                eprintln!("hidden={hidden}, vm_steps={}, plan={plan:?}", STEPS.get());
            })
            .unwrap();
        // Corrupt a hidden payload after indexing: selection must never load it.
        store
            .connection
            .execute(
                "UPDATE observations SET payload='invalid' WHERE native_key='00003'",
                [],
            )
            .unwrap();
        assert!(
            !store
                .read_schema(|tx| current(tx.tx, tx.trigger_cache))
                .unwrap()
                .unwrap()
        );
    }
    // Equality and all-head streams both seek only admitted, live partitions.
    // A visibility-unfiltered scan or paging through retracted objects grows
    // VM work with the fixture size and fails these exact-work assertions.
    assert_eq!(counts[0], counts[2]);
    assert_eq!(counts[2], counts[4]);
    assert_eq!(counts[1], counts[3]);
    assert_eq!(counts[3], counts[5]);
}
