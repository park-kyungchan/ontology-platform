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
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|s| {
            for _ in 0..2 {
                s.spawn(|| {
                    barrier.wait();
                    Store::open_schema_existing(&path).unwrap();
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
fn visible_keyset_steps_do_not_grow_with_hidden_postings() {
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
        store.append_schema(&rows, 1).unwrap();
        // Corrupt a hidden payload after indexing: selection must never load it.
        store
            .connection
            .execute(
                "UPDATE observations SET payload='invalid' WHERE native_key='00003'",
                [],
            )
            .unwrap();
        store
            .read_schema(|tx| {
                let scope = rows[0].scope.clone();
                let condition = CandidateCondition {
                    property: "status".into(),
                    values: vec![Value::String("ready".into())],
                    used: BTreeSet::from(["status".into()]),
                };
                STEPS.set(0);
                let mut after = None;
                let mut ids = Vec::new();
                loop {
                    let page = tx
                        .candidate_access(
                            "record",
                            Some(&condition),
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
                counts.push(STEPS.get());
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
    }
    assert!(counts[1] <= counts[0] * 2 && counts[2] <= counts[1] * 2);
    assert_eq!(counts[0], counts[2]);
}
