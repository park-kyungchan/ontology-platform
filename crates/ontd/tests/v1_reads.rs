//! External SQLite writers must not make schema-v1 readers accept Program scopes.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use ont_core::{Authority, Error, Query, Scope, Snapshot};
use ont_store::Store;
use ontd::{Service, local_fake, synthetic};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "v1-read-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
// Python's stdlib SQLite connection is deliberately independent of the Store API.
fn external_sql(path: &Path, sql: &str, args: Value) {
    let output = Command::new("python3")
        .arg("-c")
        .arg("import json,sqlite3,sys\nwith sqlite3.connect(sys.argv[1]) as c: c.execute(sys.argv[2],json.loads(sys.argv[3]))")
        .arg(path)
        .arg(sql)
        .arg(args.to_string())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "external synthetic SQLite write failed"
    );
}
fn invalid_scopes(scope: &Scope) -> Vec<Scope> {
    let mut foreign = scope.namespace().clone();
    foreign.account = "foreign-account".into();
    vec![
        Scope::program(scope.namespace().clone(), "external-program").unwrap(),
        Scope::education(foreign, "C1", "001").unwrap(),
    ]
}

#[test]
fn external_program_rows_and_namespace_mismatches_refuse_snapshot_and_service() {
    let temp = Temp::new();
    let path = temp.0.join("v1.sqlite");
    let (objects, links) = synthetic::graph().unwrap();
    let original = objects
        .iter()
        .find(|o| o.id == synthetic::student())
        .unwrap();
    let authority = Authority::new(synthetic::policy());
    let session = authority.bind_trusted(synthetic::PRINCIPAL).unwrap();
    let mut store = Store::open(&path).unwrap();
    store.seed(&objects, &links).unwrap();
    let mut service = Service::new(store, authority);
    let query = || Query::Get {
        id: original.id.clone(),
    };
    let before = service.render(&session, query(), synthetic::NOW).unwrap();
    drop(service);
    let mut inserted = original.id.clone();
    inserted.native_key = "externally-inserted".into();
    for scope in invalid_scopes(&original.scope) {
        // Insert a complete object/history outside Store, then reopen it as v1.
        external_sql(
            &path,
            "INSERT OR REPLACE INTO objects SELECT object_type,instance,account,?1,?2,head FROM objects WHERE object_type=?3 AND instance=?4 AND account=?5 AND native_key=?6",
            json!([
                inserted.native_key,
                serde_json::to_string(&scope).unwrap(),
                original.id.object_type,
                original.id.namespace.instance,
                original.id.namespace.account,
                original.id.native_key
            ]),
        );
        external_sql(
            &path,
            "INSERT OR REPLACE INTO observations SELECT object_type,instance,account,?1,revision,ordinal,payload FROM observations WHERE object_type=?2 AND instance=?3 AND account=?4 AND native_key=?5",
            json!([
                inserted.native_key,
                original.id.object_type,
                original.id.namespace.instance,
                original.id.namespace.account,
                original.id.native_key
            ]),
        );
        let mut store = Store::open(&path).unwrap();
        assert_eq!(
            store.snapshot(|s| s.get(&inserted)),
            Err(Error::StoreFailure)
        );
        assert_eq!(
            store.snapshot(|s| s.get(&original.id)).unwrap().as_ref(),
            Some(original)
        );
        let mut policy = synthetic::policy();
        let grants = policy.principals.get_mut(synthetic::PRINCIPAL).unwrap();
        let mut matching = grants[0].clone();
        matching.scope = scope;
        matching.object_types.insert(inserted.object_type.clone());
        grants.push(matching);
        let authority = Authority::new(policy);
        let session = authority.bind_trusted(synthetic::PRINCIPAL).unwrap();
        let mut service = Service::new(store, authority);
        assert_eq!(
            service.render(
                &session,
                Query::Get {
                    id: inserted.clone()
                },
                synthetic::NOW
            ),
            Err(Error::StoreFailure)
        );
        assert_eq!(
            service.render(&session, query(), synthetic::NOW).unwrap(),
            before
        );
    }
}

#[test]
fn linked_scope_reads_refuse_invalid_source_and_target_rows() {
    let temp = Temp::new();
    let path = temp.0.join("links.sqlite");
    let (objects, links) = synthetic::graph().unwrap();
    let link = &links[0];
    let mut store = Store::open(&path).unwrap();
    store.seed(&objects, &links).unwrap();
    let expected = store
        .snapshot(|s| s.targets(&link.source, &link.relation))
        .unwrap();
    drop(store);
    for id in [&link.source, &link.target] {
        let object = objects.iter().find(|o| &o.id == id).unwrap();
        for scope in invalid_scopes(&object.scope) {
            external_sql(
                &path,
                "UPDATE objects SET scope=?1 WHERE object_type=?2 AND instance=?3 AND account=?4 AND native_key=?5",
                json!([
                    serde_json::to_string(&scope).unwrap(),
                    id.object_type,
                    id.namespace.instance,
                    id.namespace.account,
                    id.native_key
                ]),
            );
            let mut store = Store::open(&path).unwrap();
            assert_eq!(
                store.snapshot(|s| s.targets(&link.source, &link.relation)),
                Err(Error::StoreFailure)
            );
            drop(store);
        }
        external_sql(
            &path,
            "UPDATE objects SET scope=?1 WHERE object_type=?2 AND instance=?3 AND account=?4 AND native_key=?5",
            json!([
                serde_json::to_string(&object.scope).unwrap(),
                id.object_type,
                id.namespace.instance,
                id.namespace.account,
                id.native_key
            ]),
        );
    }
    assert_eq!(
        Store::open(&path)
            .unwrap()
            .snapshot(|s| s.targets(&link.source, &link.relation))
            .unwrap(),
        expected
    );
}

fn action_input() -> ont_core::action::ActionInput {
    ont_core::action::ActionInput {
        request_id: "v1-read-request".into(),
        lesson_date: "2026-11-05".into(),
        object: local_fake::graph().unwrap()[0].id.clone(),
    }
}
fn action_call(root: &Path, command: local_fake::ActionCommand) -> Result<String, Error> {
    let (store, mut target) = local_fake::open(root).unwrap();
    let (authority, session) = local_fake::authority("normal").unwrap();
    Service::new(store, authority).render_action(
        &session,
        &mut target,
        command,
        local_fake::NOW,
        &mut |_| {},
    )
}

#[test]
fn reopened_fake_target_refuses_external_program_and_foreign_namespace_cells() {
    let temp = Temp::new();
    let root = temp.0.join("fake");
    local_fake::initialize(&root).unwrap();
    let command = || local_fake::ActionCommand::Review {
        input: action_input(),
    };
    let before = action_call(&root, command()).unwrap();
    for scope in invalid_scopes(&local_fake::graph().unwrap()[0].scope) {
        external_sql(
            &root.join("target.sqlite"),
            "UPDATE cells SET scope=?1",
            json!([serde_json::to_string(&scope).unwrap()]),
        );
        // Use a fresh request so review actually reads the target cell.
        let mut input = action_input();
        input.request_id = "new-target-read".into();
        assert_eq!(
            action_call(&root, local_fake::ActionCommand::Review { input }),
            Err(Error::StoreFailure)
        );
    }
    external_sql(
        &root.join("target.sqlite"),
        "UPDATE cells SET scope=?1",
        json!([serde_json::to_string(&local_fake::graph().unwrap()[0].scope).unwrap()]),
    );
    assert_eq!(action_call(&root, command()).unwrap(), before);
}

#[test]
fn reopened_journal_refuses_program_or_foreign_scopes_before_all_action_outputs() {
    let temp = Temp::new();
    let root = temp.0.join("journal");
    local_fake::initialize(&root).unwrap();
    let review_command = || local_fake::ActionCommand::Review {
        input: action_input(),
    };
    let before = action_call(&root, review_command()).unwrap();
    for member in ["basis", "target"] {
        for scope in invalid_scopes(&local_fake::graph().unwrap()[0].scope) {
            external_sql(
                &root.join("local.sqlite"),
                "UPDATE local_actions SET review=json_set(review,?1,json(?2))",
                json!([
                    format!("$.{member}.scope"),
                    serde_json::to_string(&scope).unwrap()
                ]),
            );
            let key = action_input().key();
            for command in [
                review_command(),
                local_fake::ActionCommand::Inspect { key: key.clone() },
                local_fake::ActionCommand::Dispatch { key: key.clone() },
                local_fake::ActionCommand::Reconcile {
                    key,
                    evidence: ont_store::actions::Evidence::Durable,
                },
            ] {
                assert_eq!(action_call(&root, command), Err(Error::StoreFailure));
            }
        }
        external_sql(
            &root.join("local.sqlite"),
            "UPDATE local_actions SET review=json_set(review,?1,json(?2))",
            json!([
                format!("$.{member}.scope"),
                serde_json::to_string(&local_fake::graph().unwrap()[0].scope).unwrap()
            ]),
        );
    }
    assert_eq!(action_call(&root, review_command()).unwrap(), before);
}
