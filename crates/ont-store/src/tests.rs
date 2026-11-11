//! Synthetic version/domain fences; no source adapters or external data.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use ont_core::{Coverage, Lineage};
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
            "ont-store-v2-{}-{nonce}-{}",
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
fn object(mode: StoreMode, key: &str) -> Object {
    let namespace = Namespace {
        instance: "synthetic://store-fence".into(),
        account: "owner".into(),
    };
    Object {
        id: Id {
            object_type: "synthetic".into(),
            namespace: namespace.clone(),
            native_key: key.into(),
        },
        scope: match mode {
            StoreMode::Education => Scope::education(namespace, "C1", "001").unwrap(),
            StoreMode::Program => Scope::program(namespace, "program-1").unwrap(),
        },
        head: "r1".into(),
        observations: vec![Observation {
            revision: "r1".into(),
            source_revision: "synthetic-v1".into(),
            observed_at: 10,
            fresh_until: 20,
            change: Change::Initial,
            supersedes: None,
            coverage: Coverage::Partial,
            fields: Default::default(),
            lineage: Lineage {
                artifact_sha256: "a".repeat(64),
                pointer: "/synthetic".into(),
            },
        }],
    }
}
fn memory(mode: StoreMode) -> Store {
    match mode {
        StoreMode::Education => Store::memory().unwrap(),
        StoreMode::Program => Store::memory_program().unwrap(),
    }
}
fn open(path: &Path, mode: StoreMode) -> Store {
    match mode {
        StoreMode::Education => Store::open(path).unwrap(),
        StoreMode::Program => Store::open_program(path).unwrap(),
    }
}
fn correction(object: &Object, revision: &str) -> Append {
    let mut observation = object.observations.last().unwrap().clone();
    observation.revision = revision.into();
    observation.change = Change::Correction;
    observation.supersedes = Some(object.head.clone());
    Append {
        id: object.id.clone(),
        scope: object.scope.clone(),
        observation,
    }
}
fn read(store: &mut Store, object: &Object) -> Option<Object> {
    store.snapshot(|s| s.get(&object.id)).unwrap()
}
fn version(store: &Store) -> i64 {
    store
        .connection
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap()
}
fn schema(store: &Store) -> Vec<(String, String)> {
    store
        .connection
        .prepare("SELECT name,sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}
#[test]
fn explicit_program_mode_uses_same_tables_and_application_id_without_actions() {
    let education = Store::memory().unwrap();
    let mut program = Store::memory_program().unwrap();
    assert_eq!(version(&education), 1);
    assert_eq!(version(&program), 2);
    assert_eq!(schema(&education), schema(&program));
    assert_eq!(schema(&program).len(), 3);
    let app: i64 = program
        .connection
        .query_row("PRAGMA application_id", [], |r| r.get(0))
        .unwrap();
    assert_eq!(app, APPLICATION_ID);
    assert_eq!(program.prepare_local_actions(), Err(Error::StoreFailure));
    assert_eq!(program.local_journal_id(), Err(Error::StoreFailure));
    assert_eq!(schema(&program), schema(&education));
    assert_eq!(version(&program), 2);
}
#[test]
fn file_modes_reopen_their_own_version_and_never_convert_each_other() {
    let temp = Temp::new();
    for mode in [StoreMode::Education, StoreMode::Program] {
        let path = temp.0.join(format!("v{}.sqlite", mode.version()));
        let item = object(mode, "root");
        let mut store = open(&path, mode);
        store.seed(std::slice::from_ref(&item), &[]).unwrap();
        drop(store);
        let before = std::fs::read(&path).unwrap();
        let wrong = match mode {
            StoreMode::Education => Store::open_program(&path),
            StoreMode::Program => Store::open(&path),
        };
        assert!(matches!(wrong, Err(Error::StoreFailure)));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let mut reopened = open(&path, mode);
        assert_eq!(version(&reopened), mode.version());
        assert_eq!(read(&mut reopened, &item), Some(item));
    }
}
#[test]
fn incompatible_versions_refuse_before_object_tables_exist_and_preserve_file() {
    let temp = Temp::new();
    for (name, app, version) in [
        ("v2-no-tables", APPLICATION_ID, 2),
        ("v1-no-tables", APPLICATION_ID, 1),
        ("future", APPLICATION_ID, 3),
        ("foreign", 42, 2),
        ("unmarked", 0, 0),
    ] {
        let path = temp.0.join(name);
        let connection = Connection::open(&path).unwrap();
        connection
            .pragma_update(None, "application_id", app)
            .unwrap();
        connection
            .pragma_update(None, "user_version", version)
            .unwrap();
        if name == "unmarked" {
            connection
                .execute_batch("CREATE TABLE unrelated(value TEXT)")
                .unwrap();
        }
        drop(connection);
        let before = std::fs::read(&path).unwrap();
        if name != "v1-no-tables" {
            assert!(matches!(Store::open(&path), Err(Error::StoreFailure)));
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
        if name != "v2-no-tables" {
            assert!(matches!(
                Store::open_program(&path),
                Err(Error::StoreFailure)
            ));
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
    }
}
#[test]
fn seed_and_append_roll_back_domain_namespace_and_invalid_scope_failures() {
    for mode in [StoreMode::Education, StoreMode::Program] {
        let good = object(mode, "good");
        let other = if mode == StoreMode::Education {
            StoreMode::Program
        } else {
            StoreMode::Education
        };
        let mut foreign = good.scope.clone();
        match &mut foreign {
            Scope::Education { namespace, .. }
            | Scope::Program { namespace, .. }
            | Scope::Domain { namespace, .. } => namespace.account = "foreign".into(),
        }
        let mut empty = good.scope.clone();
        match &mut empty {
            Scope::Education { class_key, .. } => class_key.clear(),
            Scope::Program { program_key, .. } => program_key.clear(),
            Scope::Domain { domain, .. } => domain.clear(),
        }
        for bad_scope in [object(other, "bad").scope, foreign, empty] {
            let mut store = memory(mode);
            let mut bad = object(mode, "bad");
            bad.scope = bad_scope.clone();
            assert_eq!(
                store.seed(&[good.clone(), bad.clone()], &[]),
                Err(Error::InvalidEvidence)
            );
            assert_eq!(read(&mut store, &good), None);
            assert_eq!(read(&mut store, &bad), None);
            store.seed(std::slice::from_ref(&good), &[]).unwrap();
            let mut bad_append = correction(&good, "r3");
            bad_append.scope = bad_scope;
            assert_eq!(
                store.append_batch(&[correction(&good, "r2"), bad_append]),
                Err(Error::InvalidEvidence)
            );
            assert_eq!(read(&mut store, &good), Some(good.clone()));
        }
    }
}
#[test]
fn schema_only_change_kinds_refused_in_both_store_modes() {
    fn counts(store: &Store) -> (i64, i64) {
        let objects: i64 = store
            .connection
            .query_row("SELECT count(*) FROM objects", [], |r| r.get(0))
            .unwrap();
        let observations: i64 = store
            .connection
            .query_row("SELECT count(*) FROM observations", [], |r| r.get(0))
            .unwrap();
        (objects, observations)
    }
    for mode in [StoreMode::Education, StoreMode::Program] {
        let mut store = memory(mode);
        let item = object(mode, "root");
        store.seed(std::slice::from_ref(&item), &[]).unwrap();
        let before = counts(&store);
        for (change, revision) in [
            (Change::Update, "r-update"),
            (Change::Refresh, "r-refresh"),
            (Change::Retraction, "r-retraction"),
        ] {
            let mut append = correction(&item, revision);
            append.observation.change = change.clone();
            assert!(
                append.observation.validate().is_ok(),
                "{change:?} must be valid by Observation::validate"
            );
            assert_eq!(
                store.append_batch(std::slice::from_ref(&append)),
                Err(Error::InvalidEvidence),
                "mode version {} change {change:?}",
                mode.version()
            );
            assert_eq!(counts(&store), before);
        }
        assert_eq!(read(&mut store, &item), Some(item));
    }
}
#[test]
fn program_history_preserves_order_scope_and_revision_conflicts_atomically() {
    let mut store = Store::memory_program().unwrap();
    let item = object(StoreMode::Program, "root");
    store.seed(std::slice::from_ref(&item), &[]).unwrap();
    let mut conflicting = correction(&item, "r2");
    conflicting.scope = Scope::program(item.id.namespace.clone(), "program-2").unwrap();
    assert_eq!(
        store.append_batch(&[conflicting]),
        Err(Error::ScopeConflict)
    );
    let duplicate = correction(&item, "r1");
    assert_eq!(
        store.append_batch(&[duplicate]),
        Err(Error::RevisionConflict)
    );
    let second = correction(&item, "z-correction");
    store.append_batch(std::slice::from_ref(&second)).unwrap();
    let after = read(&mut store, &item).unwrap();
    let mut backfill = correction(&after, "a-backfill");
    backfill.observation.change = Change::Backfill;
    backfill.observation.observed_at = 1;
    store.append_batch(std::slice::from_ref(&backfill)).unwrap();
    let current = read(&mut store, &item).unwrap();
    assert_eq!(current.head, "a-backfill");
    assert_eq!(
        current.observations,
        vec![
            item.observations[0].clone(),
            second.observation,
            backfill.observation
        ]
    );
    assert_eq!(
        store.append_batch(&[correction(&item, "stale")]),
        Err(Error::RevisionConflict)
    );
    assert_eq!(read(&mut store, &item), Some(current));
}
#[test]
fn external_scope_corruption_refuses_objects_links_and_existing_scope_append() {
    let temp = Temp::new();
    for mode in [StoreMode::Education, StoreMode::Program] {
        let path = temp.0.join(format!("corrupt-v{}", mode.version()));
        let root = object(mode, "root");
        let target = object(mode, "target");
        let link = Link {
            source: root.id.clone(),
            relation: "basis".into(),
            target: target.id.clone(),
        };
        let mut store = open(&path, mode);
        store
            .seed(&[root.clone(), target.clone()], std::slice::from_ref(&link))
            .unwrap();
        let other = if mode == StoreMode::Education {
            StoreMode::Program
        } else {
            StoreMode::Education
        };
        let mut foreign = root.scope.clone();
        match &mut foreign {
            Scope::Education { namespace, .. }
            | Scope::Program { namespace, .. }
            | Scope::Domain { namespace, .. } => namespace.instance = "synthetic://foreign".into(),
        }
        let invalid = [
            serde_json::to_string(&object(other, "unused").scope).unwrap(),
            serde_json::to_string(&foreign).unwrap(),
            "{}".into(),
            "{\"domain\":\"program\",\"domain\":\"program\"}".into(),
        ];
        for raw in invalid {
            for corrupted in [&root, &target] {
                // A separate native connection bypasses trusted Store intake.
                Connection::open(&path)
                    .unwrap()
                    .execute(
                        "UPDATE objects SET scope=?1 WHERE native_key=?2",
                        params![raw, corrupted.id.native_key],
                    )
                    .unwrap();
                assert_eq!(
                    store.snapshot(|s| s.get(&corrupted.id)),
                    Err(Error::StoreFailure)
                );
                assert_eq!(
                    store.snapshot(|s| s.targets(&root.id, "basis")),
                    Err(Error::StoreFailure)
                );
                let untouched = if corrupted.id == root.id {
                    &target
                } else {
                    &root
                };
                assert_eq!(
                    store.append_batch(&[correction(untouched, "r2"), correction(corrupted, "r2")]),
                    Err(Error::StoreFailure)
                );
                assert_eq!(read(&mut store, untouched), Some(untouched.clone()));
                let new = object(mode, "new");
                let invalid_link = Link {
                    source: new.id.clone(),
                    relation: "basis".into(),
                    target: corrupted.id.clone(),
                };
                assert_eq!(
                    store.seed(std::slice::from_ref(&new), &[invalid_link]),
                    Err(Error::StoreFailure)
                );
                assert_eq!(read(&mut store, &new), None);
                Connection::open(&path)
                    .unwrap()
                    .execute(
                        "UPDATE objects SET scope=?1 WHERE native_key=?2",
                        params![
                            serde_json::to_string(&corrupted.scope).unwrap(),
                            corrupted.id.native_key
                        ],
                    )
                    .unwrap();
            }
        }
        assert_eq!(
            store.snapshot(|s| s.targets(&root.id, "basis")).unwrap(),
            vec![target.id.clone()]
        );
        assert_eq!(read(&mut store, &root), Some(root));
        assert_eq!(read(&mut store, &target), Some(target));
    }
}
