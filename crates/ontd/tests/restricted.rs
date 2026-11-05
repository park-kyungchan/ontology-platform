//! Contract-level challenges, separate from the literal process oracle.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use ont_core::{Authority, Change, Error, Knowledge, Policy, Query, Scope, Snapshot};
use ont_store::{Append, Store};
use ontd::{Service, synthetic as fixture};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn seeded() -> Store {
    let (objects, links) = fixture::graph().unwrap();
    let mut store = Store::memory().unwrap();
    store.seed(&objects, &links).unwrap();
    store
}
fn get() -> Query {
    Query::Get {
        id: fixture::student(),
    }
}
fn decision() -> Query {
    Query::Decide {
        root: fixture::student(),
        relation: "learning".into(),
    }
}

#[test]
fn stale_and_foreign_sessions_never_authorize() {
    let authority = Authority::new(fixture::policy());
    assert!(matches!(
        authority.bind_trusted("root"),
        Err(Error::PrincipalUnknown)
    ));
    assert!(matches!(
        authority.bind_trusted("ap"),
        Err(Error::PrincipalUnknown)
    ));
    assert!(matches!(
        authority.bind_trusted("박경찬"),
        Err(Error::PrincipalUnknown)
    ));
    let old = authority.bind_trusted(fixture::PRINCIPAL).unwrap();
    let foreign = Authority::new(fixture::policy())
        .bind_trusted(fixture::PRINCIPAL)
        .unwrap();
    let mut service = Service::new(seeded(), authority.clone());
    assert_eq!(
        service.render(&foreign, get(), fixture::NOW),
        Err(Error::StaleAuthorization)
    );
    authority.replace_trusted(Policy::default()).unwrap();
    for query in [
        get(),
        decision(),
        Query::Traverse {
            root: fixture::student(),
            relation: "learning".into(),
        },
    ] {
        assert_eq!(
            service.render(&old, query, fixture::NOW),
            Err(Error::StaleAuthorization)
        );
    }
    assert!(matches!(
        authority.bind_trusted(fixture::PRINCIPAL),
        Err(Error::PrincipalUnknown)
    ));
}

#[test]
fn derive_is_current_and_separate_from_read() {
    let authority = Authority::new(fixture::policy());
    let old = authority.bind_trusted(fixture::PRINCIPAL).unwrap();
    let mut service = Service::new(seeded(), authority.clone());
    let mut narrowed = fixture::policy();
    for grants in narrowed.principals.values_mut() {
        for grant in grants {
            grant.derive = false;
        }
    }
    authority.replace_trusted(narrowed).unwrap();
    assert_eq!(
        service.render(&old, decision(), fixture::NOW),
        Err(Error::StaleAuthorization)
    );
    let current = authority.bind_trusted(fixture::PRINCIPAL).unwrap();
    assert!(service.render(&current, get(), fixture::NOW).is_ok());
    assert_eq!(
        service.render(&current, decision(), fixture::NOW),
        Err(Error::DeriveForbidden)
    );

    // Root derive permission cannot widen linked-object derive permission.
    let mut selective = fixture::policy();
    let grants = selective.principals.get_mut(fixture::PRINCIPAL).unwrap();
    for grant in grants.iter_mut() {
        grant.derive = false;
    }
    let mut root_only = grants[0].clone();
    root_only.object_types = ["student".to_string()].into_iter().collect();
    root_only.derive = true;
    grants.push(root_only);
    authority.replace_trusted(selective).unwrap();
    let current = authority.bind_trusted(fixture::PRINCIPAL).unwrap();
    assert_eq!(
        service.render(&current, decision(), fixture::NOW),
        Err(Error::DeriveForbidden)
    );
}

#[test]
fn forbidden_link_data_cannot_affect_full_derived_output() {
    let (objects, links) = fixture::graph().unwrap();
    let authority = Authority::new(fixture::policy());
    let session = authority.bind_trusted(fixture::PRINCIPAL).unwrap();
    let mut full = Service::new(seeded(), authority.clone());
    let mut reduced = Store::memory().unwrap();
    // Remove all objects/edges forbidden to the fixed principal. Exact literal
    // CLI expectations separately prove these are the right access boundaries.
    let permitted: Vec<_> = objects
        .into_iter()
        .filter(|o| {
            (o.scope
                == Scope::Education {
                    namespace: fixture::namespace(),
                    class_key: "C1".into(),
                    student_key: "001".into(),
                })
                || o.scope.namespace() == &fixture::recorded_namespace()
        })
        .collect();
    let links: Vec<_> = links
        .into_iter()
        .filter(|l| {
            permitted.iter().any(|o| o.id == l.source) && permitted.iter().any(|o| o.id == l.target)
        })
        .collect();
    reduced.seed(&permitted, &links).unwrap();
    let mut reduced = Service::new(reduced, authority);
    assert_eq!(
        full.render(&session, decision(), fixture::NOW).unwrap(),
        reduced.render(&session, decision(), fixture::NOW).unwrap()
    );
}

#[test]
fn revocation_waits_for_the_publication_guard() {
    let authority = Authority::new(fixture::policy());
    let session = authority.bind_trusted(fixture::PRINCIPAL).unwrap();
    let admission = authority.admit(&session).unwrap();
    let start = Arc::new(Barrier::new(2));
    let other = authority.clone();
    let gate = start.clone();
    let (sent, received) = std::sync::mpsc::channel();
    let writer = std::thread::spawn(move || {
        gate.wait();
        other.replace_trusted(Policy::default()).unwrap();
        sent.send(()).unwrap();
    });
    start.wait();
    assert!(received.recv_timeout(Duration::from_millis(50)).is_err());
    drop(admission);
    received.recv_timeout(Duration::from_secs(2)).unwrap();
    writer.join().unwrap();
    assert!(matches!(
        authority.admit(&session),
        Err(Error::StaleAuthorization)
    ));
}

#[test]
fn revisions_are_atomic_and_scope_is_immutable() {
    let mut store = seeded();
    let id = fixture::student();
    let original = store.snapshot(|s| s.get(&id)).unwrap().unwrap();
    let mut next = original.observations[0].clone();
    next.change = Change::Correction;
    next.supersedes = Some("s1".into());
    next.revision = "s2".into();
    let good = Append {
        id: id.clone(),
        scope: original.scope.clone(),
        observation: next.clone(),
    };
    let mut wrong = good.clone();
    wrong.observation.revision = "s3".into(); // stale s1 predecessor after s2
    assert_eq!(
        store.append_batch(&[good.clone(), wrong]),
        Err(Error::RevisionConflict)
    );
    assert_eq!(store.snapshot(|s| s.get(&id)).unwrap().unwrap(), original);
    let mut changed_scope = good.clone();
    let Scope::Education { student_key, .. } = &mut changed_scope.scope else {
        panic!("Education fixture required");
    };
    *student_key = "002".into();
    assert_eq!(
        store.append_batch(&[changed_scope]),
        Err(Error::ScopeConflict)
    );
    store.append_batch(std::slice::from_ref(&good)).unwrap();
    assert_eq!(store.append_batch(&[good]), Err(Error::RevisionConflict));
    let current = store.snapshot(|s| s.get(&id)).unwrap().unwrap();
    assert_eq!(current.head, "s2");
    assert_eq!(current.observations.len(), 2);
}

struct Disposable(PathBuf);
impl Disposable {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("s06-rust-synthetic-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn db(&self) -> PathBuf {
        self.0.join("synthetic.sqlite")
    }
}
impl Drop for Disposable {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn reopen_preserves_full_views_and_snapshot_blocks_a_writer_commit() {
    let temp = Disposable::new();
    let path = temp.db();
    let (objects, links) = fixture::graph().unwrap();
    let mut store = Store::open(&path).unwrap();
    store.seed(&objects, &links).unwrap();
    let before = store
        .snapshot(|s| s.get(&fixture::student()))
        .unwrap()
        .unwrap();
    drop(store);
    let mut reader = Store::open(&path).unwrap();
    let mut writer = Store::open(&path).unwrap();
    assert_eq!(
        reader
            .snapshot(|s| s.get(&fixture::student()))
            .unwrap()
            .unwrap(),
        before
    );
    let mut next = before.observations[0].clone();
    next.change = Change::Correction;
    next.supersedes = Some("s1".into());
    next.revision = "s2".into();
    next.fields = BTreeMap::from([("new".into(), Knowledge::Value(true.into()))]);
    let append = Append {
        id: before.id.clone(),
        scope: before.scope.clone(),
        observation: next,
    };
    reader
        .snapshot(|s| {
            assert_eq!(s.get(&fixture::student())?.unwrap(), before);
            // DELETE journal: writer must roll back after its bounded commit failure.
            assert_eq!(
                writer.append_batch(std::slice::from_ref(&append)),
                Err(Error::StoreFailure)
            );
            assert_eq!(s.get(&fixture::student())?.unwrap(), before);
            Ok(())
        })
        .unwrap();
    writer.append_batch(&[append]).unwrap();
    let after = reader
        .snapshot(|s| s.get(&fixture::student()))
        .unwrap()
        .unwrap();
    assert_eq!(after.head, "s2");
    assert_eq!(after.observations.len(), 2);
}

#[test]
fn v1_memory_and_file_store_refuse_program_intake_atomically() {
    let temp = Disposable::new();
    for mut store in [Store::memory().unwrap(), Store::open(&temp.db()).unwrap()] {
        let (education, links) = fixture::graph().unwrap();
        let original = education
            .iter()
            .find(|o| o.id == fixture::student())
            .unwrap()
            .clone();
        let mut program = original.clone();
        program.id.native_key = "synthetic-program".into();
        program.scope = Scope::program(program.id.namespace.clone(), "ws-replan").unwrap();
        // A preceding valid object must roll back with the refused Program row.
        assert_eq!(
            store.seed(&[original.clone(), program.clone()], &[]),
            Err(Error::InvalidEvidence)
        );
        assert!(store.snapshot(|s| s.get(&original.id)).unwrap().is_none());
        assert!(store.snapshot(|s| s.get(&program.id)).unwrap().is_none());
        store.seed(&education, &links).unwrap();
        let mut correction = original.observations[0].clone();
        correction.revision = "s2".into();
        correction.change = Change::Correction;
        correction.supersedes = Some(original.head.clone());
        let good = Append {
            id: original.id.clone(),
            scope: original.scope.clone(),
            observation: correction.clone(),
        };
        let refused = Append {
            id: original.id.clone(),
            scope: program.scope.clone(),
            observation: correction,
        };
        assert_eq!(
            store.append_batch(&[good, refused]),
            Err(Error::InvalidEvidence)
        );
        assert_eq!(
            store.snapshot(|s| s.get(&original.id)).unwrap().unwrap(),
            original
        );
        assert_eq!(
            store.append_batch(&[Append {
                id: program.id.clone(),
                scope: program.scope,
                observation: program.observations[0].clone(),
            }]),
            Err(Error::InvalidEvidence)
        );
        assert!(store.snapshot(|s| s.get(&program.id)).unwrap().is_none());
    }
    let mut reopened = Store::open(&temp.db()).unwrap();
    assert_eq!(
        reopened
            .snapshot(|s| s.get(&fixture::student()))
            .unwrap()
            .unwrap()
            .head,
        "s1"
    );
}
