//! Meaningful synthetic authorization, transaction and recovery challenges.
//! Literal subprocess fault cases live in conformance/s07/cases-v1.json.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use ont_core::action::{ActionInput, COPY_RECOMMENDATION, RequestKey};
use ont_core::{Authority, Change, Error, Grant, Knowledge, Policy, Session, Snapshot};
use ont_store::{
    Append, Store,
    actions::{Boundary, Evidence},
    fake::{FakeTarget, SeedCell},
};
use ontd::{
    Service,
    local_fake::{self as fixture, ActionCommand},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "s07-rust-fake-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fixture::initialize(&root).unwrap();
        Self(root)
    }
    fn store(&self) -> Store {
        Store::open(&self.0.join("local.sqlite")).unwrap()
    }
    fn service(&self) -> (Service, Authority, Session, FakeTarget) {
        let (authority, session) = fixture::authority("normal").unwrap();
        let (store, target) = fixture::open(&self.0).unwrap();
        (
            Service::new(store, authority.clone()),
            authority,
            session,
            target,
        )
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn input() -> ActionInput {
    ActionInput {
        request_id: "test-r1".into(),
        lesson_date: "2026-11-02".into(),
        object: fixture::graph().unwrap()[0].id.clone(),
    }
}
fn key() -> RequestKey {
    input().key()
}
fn policy(read: bool, derive: bool, act: bool) -> Policy {
    Policy {
        principals: BTreeMap::from([(
            fixture::PRINCIPAL.into(),
            vec![Grant {
                scope: fixture::graph().unwrap()[0].scope.clone(),
                object_types: BTreeSet::from(["progress".into()]),
                read,
                derive,
                actions: if act {
                    BTreeSet::from([COPY_RECOMMENDATION.into()])
                } else {
                    BTreeSet::new()
                },
            }],
        )]),
    }
}
fn call(
    service: &mut Service,
    session: &Session,
    target: &mut FakeTarget,
    command: ActionCommand,
) -> Result<serde_json::Value, Error> {
    service
        .render_action(session, target, command, fixture::NOW, &mut |_| {})
        .map(|raw| serde_json::from_str(&raw).unwrap())
}
fn review(service: &mut Service, session: &Session, target: &mut FakeTarget) {
    assert_eq!(
        call(
            service,
            session,
            target,
            ActionCommand::Review { input: input() }
        )
        .unwrap()["state"],
        "reviewed"
    );
}
fn correction(temp: &Temp, record: Option<&str>) -> Append {
    let object = temp
        .store()
        .snapshot(|s| s.get(&input().object))
        .unwrap()
        .unwrap();
    let mut observation = object.observations.last().unwrap().clone();
    observation.change = Change::Correction;
    observation.supersedes = Some(object.head);
    observation.revision = "v2".into();
    if let Some(record) = record {
        observation
            .fields
            .insert("physical_record".into(), Knowledge::Value(record.into()));
    }
    Append {
        id: object.id,
        scope: object.scope,
        observation,
    }
}

#[test]
fn current_sessions_and_named_grants_gate_every_action_and_receipt_output() {
    let temp = Temp::new();
    let (mut service, authority, session, mut target) = temp.service();
    review(&mut service, &session, &mut target);
    assert_eq!(
        call(
            &mut service,
            &session,
            &mut target,
            ActionCommand::Dispatch { key: key() }
        )
        .unwrap()["state"],
        "persisted"
    );
    let (_, foreign) = fixture::authority("normal").unwrap();
    for command in [
        ActionCommand::Inspect { key: key() },
        ActionCommand::Dispatch { key: key() },
        ActionCommand::Reconcile {
            key: key(),
            evidence: Evidence::Durable,
        },
    ] {
        assert_eq!(
            call(&mut service, &foreign, &mut target, command),
            Err(Error::StaleAuthorization)
        );
    }
    authority
        .replace_trusted(policy(true, true, false))
        .unwrap();
    assert_eq!(
        call(
            &mut service,
            &session,
            &mut target,
            ActionCommand::Dispatch { key: key() }
        ),
        Err(Error::StaleAuthorization)
    );
    let current = authority.bind_trusted(fixture::PRINCIPAL).unwrap();
    for command in [
        ActionCommand::Inspect { key: key() },
        ActionCommand::Dispatch { key: key() },
        ActionCommand::Review { input: input() },
        ActionCommand::Reconcile {
            key: key(),
            evidence: Evidence::Durable,
        },
    ] {
        assert_eq!(
            call(&mut service, &current, &mut target, command),
            Err(Error::ActionForbidden)
        );
    }
    authority
        .replace_trusted(policy(true, false, true))
        .unwrap();
    let current = authority.bind_trusted(fixture::PRINCIPAL).unwrap();
    assert_eq!(
        call(
            &mut service,
            &current,
            &mut target,
            ActionCommand::Inspect { key: key() }
        ),
        Err(Error::DeriveForbidden)
    );
    authority.replace_trusted(Policy::default()).unwrap();
    assert!(matches!(
        authority.bind_trusted("root"),
        Err(Error::PrincipalUnknown)
    ));
    assert_eq!(
        call(
            &mut service,
            &current,
            &mut target,
            ActionCommand::Inspect { key: key() }
        ),
        Err(Error::StaleAuthorization)
    );
}

#[test]
fn scope_and_missingness_survive_function_and_action_boundaries() {
    let temp = Temp::new();
    let (mut service, _, session, mut target) = temp.service();
    for object in fixture::graph().unwrap().into_iter().skip(2) {
        let expected = if object.id.native_key == "partial" || object.id.native_key == "stale" {
            Error::ReviewRequired
        } else {
            Error::Unavailable
        };
        let mut request = input();
        request.object = object.id;
        assert_eq!(
            call(
                &mut service,
                &session,
                &mut target,
                ActionCommand::Review { input: request }
            ),
            Err(expected)
        );
    }
    let mut changed = correction(&temp, None);
    changed
        .observation
        .fields
        .insert("recommendation".into(), Knowledge::Unknown);
    temp.store().append_batch(&[changed]).unwrap();
    assert_eq!(
        call(
            &mut service,
            &session,
            &mut target,
            ActionCommand::Review { input: input() }
        ),
        Err(Error::ReviewRequired)
    );
}

#[test]
fn stale_review_and_remapped_source_never_dispatch() {
    for mapping in [None, Some("R-rebound")] {
        let temp = Temp::new();
        let (mut service, _, session, mut target) = temp.service();
        review(&mut service, &session, &mut target);
        temp.store()
            .append_batch(&[correction(&temp, mapping)])
            .unwrap();
        assert_eq!(
            call(
                &mut service,
                &session,
                &mut target,
                ActionCommand::Dispatch { key: key() }
            ),
            Err(Error::ReviewStale)
        );
    }
}

#[test]
fn target_identity_path_and_journal_ownership_are_rechecked() {
    let temp = Temp::new();
    let (mut service, _, session, mut target) = temp.service();
    review(&mut service, &session, &mut target);
    let review = call(
        &mut service,
        &session,
        &mut target,
        ActionCommand::Inspect { key: key() },
    )
    .unwrap();
    let cell = SeedCell {
        physical: serde_json::from_value(review["physical"].clone()).unwrap(),
        scope: serde_json::from_value(review["scope"].clone()).unwrap(),
        value: "old".into(),
    };
    let id = temp.store().local_journal_id().unwrap();
    let mut replacement =
        FakeTarget::create(&temp.0.join("replacement.sqlite"), &id, &[cell]).unwrap();
    assert_eq!(
        call(
            &mut service,
            &session,
            &mut replacement,
            ActionCommand::Dispatch { key: key() }
        ),
        Err(Error::TargetIdentity)
    );
    assert!(matches!(
        FakeTarget::open(&temp.0.join("target.sqlite"), "other-journal"),
        Err(Error::TargetIdentity)
    ));
    let another = Temp::new();
    let (mut other_service, _, other_session, _) = another.service();
    assert_eq!(
        call(
            &mut other_service,
            &other_session,
            &mut target,
            ActionCommand::Review { input: input() }
        ),
        Err(Error::TargetIdentity)
    );
}

#[test]
fn failed_local_fence_commit_cannot_reach_the_target() {
    let temp = Temp::new();
    let (mut service, _, session, mut target) = temp.service();
    review(&mut service, &session, &mut target);
    temp.store()
        .snapshot(|snapshot| {
            snapshot.get(&input().object)?; // pin DELETE-journal read transaction
            assert_eq!(
                call(
                    &mut service,
                    &session,
                    &mut target,
                    ActionCommand::Dispatch { key: key() }
                ),
                Err(Error::StoreFailure)
            );
            Ok(())
        })
        .unwrap();
    let view = call(
        &mut service,
        &session,
        &mut target,
        ActionCommand::Inspect { key: key() },
    )
    .unwrap();
    assert_eq!(view["state"], "reviewed");
    assert_eq!(view["observed"], "old");
    assert_eq!(
        call(
            &mut service,
            &session,
            &mut target,
            ActionCommand::Dispatch { key: key() }
        )
        .unwrap()["state"],
        "persisted"
    );
}

#[test]
fn pending_basis_hold_and_no_effect_recovery_survive_a_real_child_exit() {
    let temp = Temp::new();
    let (mut service, authority, mut session, mut target) = temp.service();
    review(&mut service, &session, &mut target);
    let mut child = Command::new(env!("CARGO_BIN_EXE_ont-local-fake"))
        .args([
            "--root",
            temp.0.to_str().unwrap(),
            "--fault",
            "after_fence_commit",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            serde_json::to_string(&ActionCommand::Dispatch { key: key() })
                .unwrap()
                .as_bytes(),
        )
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert_eq!(result.status.code(), Some(84));
    assert!(result.stdout.is_empty());
    assert!(result.stderr.is_empty());
    let mut narrowed = policy(true, true, true);
    let ont_core::Scope::Education { student_key, .. } =
        &mut narrowed.principals.get_mut(fixture::PRINCIPAL).unwrap()[0].scope
    else {
        panic!("Education fixture required");
    };
    *student_key = "other-student".into();
    authority.replace_trusted(narrowed).unwrap();
    session = authority.bind_trusted(fixture::PRINCIPAL).unwrap();
    assert_eq!(
        call(
            &mut service,
            &session,
            &mut target,
            ActionCommand::Reconcile {
                key: key(),
                evidence: Evidence::Durable
            }
        ),
        Err(Error::Unavailable)
    );
    let mut other_owner = policy(true, true, true);
    let grants = other_owner.principals.remove(fixture::PRINCIPAL).unwrap();
    other_owner.principals.insert("other-owner".into(), grants);
    authority.replace_trusted(other_owner).unwrap();
    session = authority.bind_trusted("other-owner").unwrap();
    assert_eq!(
        call(
            &mut service,
            &session,
            &mut target,
            ActionCommand::Inspect { key: key() }
        ),
        Err(Error::Unavailable)
    );
    authority.replace_trusted(policy(true, true, true)).unwrap();
    session = authority.bind_trusted(fixture::PRINCIPAL).unwrap();
    let pending = call(
        &mut service,
        &session,
        &mut target,
        ActionCommand::Inspect { key: key() },
    )
    .unwrap();
    let cell = SeedCell {
        physical: serde_json::from_value(pending["physical"].clone()).unwrap(),
        scope: serde_json::from_value(pending["scope"].clone()).unwrap(),
        value: "old".into(),
    };
    let mut replacement = FakeTarget::create(
        &temp.0.join("wrong-recovery-target.sqlite"),
        &temp.store().local_journal_id().unwrap(),
        &[cell],
    )
    .unwrap();
    assert_eq!(
        call(
            &mut service,
            &session,
            &mut replacement,
            ActionCommand::Reconcile {
                key: key(),
                evidence: Evidence::Durable
            }
        ),
        Err(Error::TargetIdentity)
    );
    let change = correction(&temp, None);
    assert_eq!(
        temp.store().append_batch(std::slice::from_ref(&change)),
        Err(Error::StoreFailure)
    );
    assert_eq!(
        call(
            &mut service,
            &session,
            &mut target,
            ActionCommand::Dispatch { key: key() }
        ),
        Err(Error::RetryForbidden)
    );
    assert_eq!(
        call(
            &mut service,
            &session,
            &mut target,
            ActionCommand::Reconcile {
                key: key(),
                evidence: Evidence::Durable
            }
        )
        .unwrap()["state"],
        "no_effect"
    );
    temp.store().append_batch(&[change]).unwrap();
    // No duplicate target application: same request remains settled after a correction.
    assert_eq!(
        call(
            &mut service,
            &session,
            &mut target,
            ActionCommand::Dispatch { key: key() }
        )
        .unwrap()["state"],
        "no_effect"
    );
    let mut next = input();
    next.request_id = "test-r2".into();
    next.lesson_date = "2026-11-03".into();
    next.object.native_key = "occurrence-2".into();
    let next_key = next.key();
    assert_eq!(
        call(
            &mut service,
            &session,
            &mut target,
            ActionCommand::Review { input: next }
        )
        .unwrap()["state"],
        "reviewed"
    );
    assert_eq!(
        call(
            &mut service,
            &session,
            &mut target,
            ActionCommand::Dispatch { key: next_key }
        )
        .unwrap()["state"],
        "persisted"
    );
}

#[test]
fn revocation_linearizes_after_actual_effect_and_output_serialization() {
    let temp = Temp::new();
    let (mut service, authority, session, mut target) = temp.service();
    review(&mut service, &session, &mut target);
    let other = authority.clone();
    let (start_tx, start_rx) = std::sync::mpsc::channel();
    let (attempt_tx, attempt_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let writer = std::thread::spawn(move || {
        start_rx.recv().unwrap();
        attempt_tx.send(()).unwrap();
        other.replace_trusted(Policy::default()).unwrap();
        done_tx.send(()).unwrap();
    });
    let mut hook = |boundary| {
        if boundary == Boundary::AfterFenceCommit {
            start_tx.send(()).unwrap();
            attempt_rx.recv().unwrap();
            assert!(done_rx.recv_timeout(Duration::from_millis(50)).is_err());
        }
    };
    let raw = service
        .render_action(
            &session,
            &mut target,
            ActionCommand::Dispatch { key: key() },
            fixture::NOW,
            &mut hook,
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&raw).unwrap()["state"],
        "persisted"
    );
    done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    writer.join().unwrap();
    assert_eq!(
        call(
            &mut service,
            &session,
            &mut target,
            ActionCommand::Inspect { key: key() }
        ),
        Err(Error::StaleAuthorization)
    );
}

#[test]
fn additive_extension_keeps_existing_s06_graph_and_serialized_reads_unchanged() {
    let temp = Temp::new();
    let path = temp.0.join("existing-s06.sqlite");
    let (objects, links) = ontd::synthetic::graph().unwrap();
    let mut legacy = Store::open(&path).unwrap();
    legacy.seed(&objects, &links).unwrap();
    drop(legacy);
    let authority = Authority::new(ontd::synthetic::policy());
    let session = authority.bind_trusted(ontd::synthetic::PRINCIPAL).unwrap();
    let query = ont_core::Query::Decide {
        root: ontd::synthetic::student(),
        relation: "learning".into(),
    };
    let before = Service::new(Store::open(&path).unwrap(), authority.clone())
        .render(&session, query.clone(), ontd::synthetic::NOW)
        .unwrap();
    let mut upgraded = Store::open(&path).unwrap();
    upgraded.prepare_local_actions().unwrap();
    let identity = upgraded.local_journal_id().unwrap();
    upgraded.prepare_local_actions().unwrap();
    assert_eq!(upgraded.local_journal_id().unwrap(), identity);
    drop(upgraded);
    let after = Service::new(Store::open(&path).unwrap(), authority)
        .render(&session, query, ontd::synthetic::NOW)
        .unwrap();
    assert_eq!(before, after);
}
