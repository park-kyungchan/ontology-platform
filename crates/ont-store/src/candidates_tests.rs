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
                    absent: true,
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

#[test]
fn candidate_access_read_write_parity() {
    let mut store = Store::memory_schema().unwrap();
    let schema = include_bytes!("../../ont/tests/fixtures/action-work.json");
    store
        .install_schema(schema, InstallArgs::default())
        .unwrap();
    let id: Id = serde_json::from_value(serde_json::json!({"object_type":"hold","namespace":{"instance":"vps://palantirkc","account":"root"},"native_key":"h1"})).unwrap();
    let scope = Scope::domain(id.namespace.clone(), "work", BTreeMap::new()).unwrap();
    let observation = serde_json::from_value(serde_json::json!({"revision":"r0","source_revision":"s0","observed_at":1,"fresh_until":100,
        "change":"initial","supersedes":null,"coverage":"complete","fields":{"effect":{"kind":"value","value":"merge"},"status":{"kind":"value","value":"active"},"until":{"kind":"value","value":"later"},"task":{"kind":"value","value":{"object_type":"task","namespace":{"instance":"vps://palantirkc","account":"root"},"native_key":"t1"}}},
        "lineage":{"artifact_sha256":"a".repeat(64),"pointer":""}})).unwrap();
    store
        .append_schema(
            &[Append {
                id,
                scope: scope.clone(),
                observation,
            }],
            1,
        )
        .unwrap();
    let condition = CandidateCondition {
        property: "effect".into(),
        values: vec![Value::String("merge".into())],
        used: BTreeSet::new(),
        absent: true,
    };
    let scopes = [scope];
    let read = store
        .read_schema(|tx| tx.candidate_access("hold", Some(&condition), 10, 2, None, &scopes))
        .unwrap()
        .unwrap();
    let write = store
        .write(10, |tx| {
            tx.candidate_access("hold", Some(&condition), 10, 2, None, &scopes)
        })
        .unwrap();
    let direct = store
        .candidate_access("hold", Some(&condition), 10, 2, None, &scopes)
        .unwrap();
    assert_eq!(read.len(), 1);
    assert_eq!(read, write);
    assert_eq!(read, direct);
}

fn expiry_fixture() -> (Store, Vec<HeadAccess>) {
    let mut store = Store::memory_schema().unwrap();
    let schema = serde_json::json!({"format":"ontology-schema/v1","domain":"test","version":1,
        "namespace":{"instance":"test","account":"root"},"scope_keys":["tenant"],
        "object_types":{"record":{"owner":"ontology","primary_key":{"from":"$action_id"},
            "properties":{"status":{"type":"string"},"extra":{"type":"string"}}}},
        "grants":[{"principal":"agent","scope":{"tenant":"public"},"read":["record"],"derive":[],"actions":[]},
                  {"principal":"agent","scope":{},"read":["record"],"derive":[],"actions":[]}]});
    store
        .install_schema(schema.to_string().as_bytes(), InstallArgs::default())
        .unwrap();
    let namespace = ont_core::Namespace {
        instance: "test".into(),
        account: "root".into(),
    };
    let scope = Scope::domain(
        namespace.clone(),
        "test",
        BTreeMap::from([("tenant".into(), "public".into())]),
    )
    .unwrap();
    let mut heads = Vec::new();
    for (key, expiry) in [
        ("finite", 100),
        ("never-a", crate::NEVER_STALE),
        ("never-b", u64::MAX),
    ] {
        let id = Id {
            object_type: "record".into(),
            namespace: namespace.clone(),
            native_key: key.into(),
        };
        let observation = serde_json::from_value(serde_json::json!({"revision":"r0","source_revision":"s0","observed_at":1,"fresh_until":expiry,
            "change":"initial","supersedes":null,"coverage":"complete","fields":{"status":{"kind":"value","value":"ready"}},
            "lineage":{"artifact_sha256":"a".repeat(64),"pointer":""}})).unwrap();
        store
            .append_schema(
                &[Append {
                    id: id.clone(),
                    scope: scope.clone(),
                    observation,
                }],
                1,
            )
            .unwrap();
        heads.push(HeadAccess {
            id,
            scope: scope.clone(),
            retracted: false,
        });
    }
    // Exercise the projector's no-observation branch directly; no invented row in observations.
    let absent = HeadAccess {
        id: Id {
            object_type: "record".into(),
            namespace,
            native_key: "absent".into(),
        },
        scope,
        retracted: false,
    };
    project_access(&store.connection, &store.schemas, &absent, None).unwrap();
    heads.push(absent);
    (store, heads)
}

#[test]
fn never_stale_head_has_no_stale_postings() {
    let (store, heads) = expiry_fixture();
    for head in heads {
        let count: u64 = store
            .connection
            .query_row(
                "SELECT count(*) FROM head_candidates WHERE property='$stale' AND native_key=?1",
                [&head.id.native_key],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            count,
            if head.id.native_key == "finite" {
                32
            } else {
                0
            },
            "{}: sixteen per visibility only for finite expiry",
            head.id.native_key
        );
    }
}

#[test]
fn candidates_equal_on_old_new_and_mixed_postings() {
    let (store, heads) = expiry_fixture();
    store
        .connection
        .execute_batch("CREATE TEMP TABLE new_projection AS SELECT * FROM head_candidates")
        .unwrap();
    let matching = CandidateCondition {
        property: "status".into(),
        values: vec![Value::String("ready".into())],
        used: BTreeSet::new(),
        absent: true,
    };
    let missing = CandidateCondition {
        property: "status".into(),
        values: vec![Value::String("other".into())],
        used: BTreeSet::new(),
        absent: true,
    };
    let uncertain = CandidateCondition {
        property: "extra".into(),
        values: vec![Value::String("other".into())],
        used: BTreeSet::new(),
        absent: true,
    };
    let wildcard = Scope::domain(heads[0].id.namespace.clone(), "test", BTreeMap::new()).unwrap();
    let cases = [None, Some(&matching), Some(&missing), Some(&uncertain)];
    let scopes = [
        vec![heads[0].scope.clone()],
        vec![heads[0].scope.clone(), wildcard],
    ];
    let collect = || {
        let mut results = Vec::new();
        for now in [0, 50, 101, crate::NEVER_STALE] {
            for condition in cases {
                for scope in &scopes {
                    results.push(
                        candidate_access_in(
                            &store.connection,
                            &store.schemas,
                            "record",
                            condition,
                            now,
                            100,
                            None,
                            scope,
                        )
                        .unwrap(),
                    );
                }
            }
        }
        results
    };
    let new = collect();
    for keys in [vec!["never-a", "never-b", "absent"], vec!["never-a"]] {
        store.connection.execute_batch("DELETE FROM head_candidates; INSERT INTO head_candidates SELECT * FROM new_projection").unwrap();
        for key in keys {
            let expiry = format!(
                "{:016x}",
                if key == "never-a" {
                    crate::NEVER_STALE
                } else {
                    u64::MAX
                }
            );
            for n in 1..=16 {
                store.connection.execute("INSERT INTO head_candidates SELECT object_type,visibility,'$stale',?1,instance,account,native_key,scope_keys FROM new_projection WHERE property='' AND native_key=?2", params![&expiry[..n], key]).unwrap();
            }
        }
        assert_eq!(
            new,
            collect(),
            "old/new/mixed projections must return identical heads"
        );
    }
}

#[test]
fn clock_above_never_stale_is_refused() {
    let (mut store, heads) = expiry_fixture();
    for now in [crate::NEVER_STALE + 1, u64::MAX] {
        assert_eq!(
            store
                .candidate_access(
                    "record",
                    None,
                    now,
                    100,
                    None,
                    std::slice::from_ref(&heads[0].scope)
                )
                .unwrap_err()
                .code,
            "clock_out_of_range"
        );
        // Also precedes schema lookup (and any database access) inside the shared reader.
        assert_eq!(
            candidate_access_in(
                &store.connection,
                &store.schemas,
                "missing",
                None,
                now,
                1,
                None,
                &[]
            )
            .unwrap_err()
            .code,
            "clock_out_of_range"
        );
    }
}

#[test]
fn postings_tell_no_value_from_unposted_value() {
    let (mut store, heads) = expiry_fixture();
    let head = head_in(&store.connection, &heads[0].id).unwrap().unwrap();
    let bytes: Vec<u8> = store
        .connection
        .query_row(
            "SELECT bytes FROM schema_installs WHERE domain='test'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let mut schema: Value = serde_json::from_slice(&bytes).unwrap();
    schema["version"] = Value::from(2);
    store
        .install_schema(
            &serde_json::to_vec(&schema).unwrap(),
            InstallArgs::default(),
        )
        .unwrap();
    // Reuse a real observation and both installed pins, varying projection inputs.
    for mode in ["current", "older", "partial"] {
        for (state, field) in [
            (
                "value",
                Some(Knowledge::Value(Value::String("ready".into()))),
            ),
            ("absent", None),
            ("missing", Some(Knowledge::Missing)),
            ("null", Some(Knowledge::Null)),
            ("blank", Some(Knowledge::Blank)),
            ("unknown", Some(Knowledge::Unknown)),
            ("not_performed", Some(Knowledge::NotPerformed)),
        ] {
            let mut obs = head.head.observation.clone();
            let mut pin = store.schemas.pin("test").unwrap().clone();
            obs.fields.clear();
            if let Some(field) = field {
                obs.fields.insert("status".into(), field);
            }
            if mode == "older" {
                pin = head.schema_pin.clone();
            }
            if mode == "partial" {
                obs.coverage = ont_core::Coverage::Partial;
            }
            project_access(
                &store.connection,
                &store.schemas,
                &heads[0],
                Some((&obs, &pin)),
            )
            .unwrap();
            let posting: String = store.connection.query_row(
                "SELECT value FROM head_candidates WHERE native_key='finite' AND property='status' LIMIT 1", [], |r| r.get(0)).unwrap();
            let expected = if state != "value" {
                "-"
            } else if mode == "current" {
                "v\"ready\""
            } else {
                "?"
            };
            assert_eq!(posting, expected, "{mode}/{state}");
        }
    }
    project_access(&store.connection, &store.schemas, &heads[0], None).unwrap();
    let postings: Vec<String> = store
        .connection
        .prepare("SELECT value FROM head_candidates WHERE native_key='finite' AND property<>''")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(!postings.is_empty());
    assert!(postings.iter().all(|v| v == "-"));
}

#[test]
fn readers_see_the_same_candidates() {
    let (store, heads) = expiry_fixture();
    // Reconstruct the old writer's rows, record its answers, then rebuild.
    store
        .connection
        .execute("UPDATE head_candidates SET value='?' WHERE value='-'", [])
        .unwrap();
    let scope = std::slice::from_ref(&heads[0].scope);
    let conditions: Vec<_> = ["status", "extra"]
        .into_iter()
        .map(|p| CandidateCondition {
            property: p.into(),
            values: vec![Value::String("other".into())],
            used: BTreeSet::from(["extra".into()]),
            absent: true,
        })
        .collect();
    let collect = || {
        conditions
            .iter()
            .map(|condition| {
                candidate_access_in(
                    &store.connection,
                    &store.schemas,
                    "record",
                    Some(condition),
                    10,
                    100,
                    None,
                    scope,
                )
                .unwrap()
            })
            .collect::<Vec<_>>()
    };
    let before = collect();
    rebuild(&store.connection, &store.schemas).unwrap();
    // The synthetic no-observation head has no authoritative objects row.
    project_access(
        &store.connection,
        &store.schemas,
        heads.last().unwrap(),
        None,
    )
    .unwrap();
    assert_eq!(before, collect());
}
