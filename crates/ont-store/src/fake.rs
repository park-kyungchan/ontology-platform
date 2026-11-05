//! Closed LOCAL FAKE target with transactional idempotency AND durable absence sealing.
//! This capability is not a claim about Academy or any HTTP adapter.
use crate::{db, decode};
use ont_core::action::{PhysicalField, Review, TargetSnapshot};
use ont_core::{Error, Scope};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

pub const FAKE_SCHEMA: &str = include_str!("../fake.sql");

pub struct FakeTarget {
    connection: Connection,
    identity: String,
    canonical_path: String,
    journal_id: String,
}
pub struct SeedCell {
    pub physical: PhysicalField,
    pub scope: Scope,
    pub value: String,
}
pub(crate) struct FakeTransaction<'a> {
    transaction: Transaction<'a>,
    identity: &'a str,
    path: &'a str,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Outcome {
    Applied,
    SealedNoEffect,
    ConflictNoEffect,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct Proof {
    pub token: String,
    pub review: Review,
    pub outcome: Outcome,
    pub version: u64,
    pub value: String,
}

fn json<T: Serialize>(value: &T) -> Result<String, Error> {
    serde_json::to_string(value).map_err(|_| Error::StoreFailure)
}
impl FakeTarget {
    pub(crate) fn check_journal(&self, journal_id: &str) -> Result<(), Error> {
        if self.journal_id == journal_id {
            Ok(())
        } else {
            Err(Error::TargetIdentity)
        }
    }
    /// Explicit synthetic initialization, refusing all existing paths.
    pub fn create(path: &Path, journal_id: &str, cells: &[SeedCell]) -> Result<Self, Error> {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|_| Error::InvalidRequest)?;
        let mut connection = db(Connection::open(path))?;
        db(connection.execute_batch("PRAGMA synchronous=FULL; PRAGMA journal_mode=DELETE;"))?;
        let tx = db(connection.transaction_with_behavior(TransactionBehavior::Immediate))?;
        db(tx.execute_batch(FAKE_SCHEMA))?;
        db(tx.execute(
            "INSERT INTO meta VALUES(1,lower(hex(randomblob(16))),?1)",
            [journal_id],
        ))?;
        for cell in cells {
            if !matches!(cell.scope, Scope::Education { .. })
                || cell.scope.validate().is_err()
                || cell.scope.namespace() != &cell.physical.namespace
            {
                return Err(Error::InvalidEvidence);
            }
            db(tx.execute(
                "INSERT OR IGNORE INTO cells VALUES(?1,?2,?3,?4,?5,?6,0)",
                params![
                    cell.physical.namespace.instance,
                    cell.physical.namespace.account,
                    cell.physical.record,
                    cell.physical.field,
                    json(&cell.scope)?,
                    cell.value
                ],
            ))?;
            let (saved_scope, saved_value): (String, String) = db(tx.query_row(
                "SELECT scope,value FROM cells WHERE instance=?1 AND account=?2 AND record=?3 AND field=?4",
                params![cell.physical.namespace.instance,cell.physical.namespace.account,cell.physical.record,cell.physical.field],
                |r| Ok((r.get(0)?,r.get(1)?)),
            ))?;
            if decode::<Scope>(&saved_scope)? != cell.scope || saved_value != cell.value {
                return Err(Error::ScopeConflict);
            }
        }
        db(tx.commit())?;
        drop(connection);
        Self::open(path, journal_id)
    }
    pub fn open(path: &Path, journal_id: &str) -> Result<Self, Error> {
        let connection = db(Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ))?;
        db(connection.busy_timeout(Duration::from_millis(250)))?;
        let app: i64 = db(connection.query_row("PRAGMA application_id", [], |r| r.get(0)))?;
        let version: i64 = db(connection.query_row("PRAGMA user_version", [], |r| r.get(0)))?;
        if app != 1_330_533_431 || version != 1 {
            return Err(Error::TargetIdentity);
        }
        db(connection.execute_batch(
            "PRAGMA synchronous=FULL; PRAGMA journal_mode=DELETE; PRAGMA trusted_schema=OFF;",
        ))?;
        let (identity, owner): (String, String) = db(connection.query_row(
            "SELECT identity,journal_id FROM meta WHERE singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ))?;
        if owner != journal_id {
            return Err(Error::TargetIdentity);
        }
        let canonical_path = path
            .canonicalize()
            .map_err(|_| Error::TargetIdentity)?
            .to_str()
            .ok_or(Error::TargetIdentity)?
            .to_owned();
        Ok(Self {
            connection,
            identity,
            canonical_path,
            journal_id: journal_id.into(),
        })
    }
    pub(crate) fn transaction(&mut self) -> Result<FakeTransaction<'_>, Error> {
        let transaction = db(self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate))?;
        let (identity, owner): (String, String) = db(transaction.query_row(
            "SELECT identity,journal_id FROM meta WHERE singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ))?;
        if identity != self.identity || owner != self.journal_id {
            return Err(Error::TargetIdentity);
        }
        Ok(FakeTransaction {
            transaction,
            identity: &self.identity,
            path: &self.canonical_path,
        })
    }
    /// Trusted test perturbation, not an end-user domain action or service API.
    pub fn perturb_trusted(&mut self, physical: &PhysicalField, value: &str) -> Result<(), Error> {
        let tx = self.transaction()?;
        let changed = db(tx.transaction.execute("UPDATE cells SET value=?5,version=version+1 WHERE instance=?1 AND account=?2 AND record=?3 AND field=?4", params![physical.namespace.instance,physical.namespace.account,physical.record,physical.field,value]))?;
        if changed != 1 {
            return Err(Error::TargetConflict);
        }
        tx.commit()
    }
}
impl FakeTransaction<'_> {
    pub(crate) fn snapshot(&self, p: &PhysicalField) -> Result<TargetSnapshot, Error> {
        let row: Option<(String,String,u64)> = db(self.transaction.query_row(
            "SELECT scope,value,version FROM cells WHERE instance=?1 AND account=?2 AND record=?3 AND field=?4",
            params![p.namespace.instance,p.namespace.account,p.record,p.field],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional())?;
        let (scope, value, version) = row.ok_or(Error::TargetConflict)?;
        Ok(TargetSnapshot {
            identity: self.identity.into(),
            canonical_path: self.path.into(),
            physical: p.clone(),
            scope: decode(&scope)?,
            value,
            version,
        })
    }
    pub(crate) fn identity(&self, review: &Review) -> Result<TargetSnapshot, Error> {
        let current = self.snapshot(&review.target.physical)?;
        if current.identity != review.target.identity
            || current.canonical_path != review.target.canonical_path
            || current.scope != review.basis.scope
        {
            return Err(Error::TargetIdentity);
        }
        Ok(current)
    }
    pub(crate) fn proof(&self, token: &str, review: &Review) -> Result<Option<Proof>, Error> {
        let row: Option<(String, String, u64, String)> = db(self
            .transaction
            .query_row(
                "SELECT review,outcome,version,value FROM dispatches WHERE token=?1",
                [token],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional())?;
        row.map(|(saved, outcome, version, value)| {
            if decode::<Review>(&saved)? != *review {
                return Err(Error::TargetIdentity);
            }
            Ok(Proof {
                token: token.into(),
                review: review.clone(),
                outcome: decode(&format!("\"{outcome}\""))?,
                version,
                value,
            })
        })
        .transpose()
    }
    fn record(
        &self,
        token: &str,
        review: &Review,
        outcome: Outcome,
        snapshot: &TargetSnapshot,
    ) -> Result<Proof, Error> {
        let code = match outcome {
            Outcome::Applied => "applied",
            Outcome::SealedNoEffect => "sealed_no_effect",
            Outcome::ConflictNoEffect => "conflict_no_effect",
        };
        db(self.transaction.execute(
            "INSERT INTO dispatches VALUES(?1,?2,?3,?4,?5)",
            params![token, json(review)?, code, snapshot.version, snapshot.value],
        ))?;
        Ok(Proof {
            token: token.into(),
            review: review.clone(),
            outcome,
            version: snapshot.version,
            value: snapshot.value.clone(),
        })
    }
    /// Requires the SAME target transaction that validated the review pre-fence.
    pub(crate) fn apply(&self, token: &str, review: &Review) -> Result<(), Error> {
        if self.proof(token, review)?.is_some() {
            return Ok(());
        }
        let current = self.identity(review)?;
        if current != review.target {
            self.record(token, review, Outcome::ConflictNoEffect, &current)?;
            return Ok(());
        }
        let p = &current.physical;
        let changed = db(self.transaction.execute("UPDATE cells SET value=?5,version=version+1 WHERE instance=?1 AND account=?2 AND record=?3 AND field=?4 AND version=?6 AND value=?7", params![p.namespace.instance,p.namespace.account,p.record,p.field,review.desired,current.version,current.value]))?;
        if changed != 1 {
            return Err(Error::TargetConflict);
        }
        self.record(token, review, Outcome::Applied, &self.snapshot(p)?)?;
        Ok(())
    }
    /// Fake-only capability: seal absence while excluding any in-flight dispatch.
    /// Once sealed, apply(token) can never mutate. Current value alone is NOT proof.
    pub(crate) fn reconcile(
        &self,
        token: &str,
        review: &Review,
    ) -> Result<(Proof, TargetSnapshot), Error> {
        let current = self.identity(review)?;
        let proof = match self.proof(token, review)? {
            Some(proof) => proof,
            None => self.record(token, review, Outcome::SealedNoEffect, &current)?,
        };
        Ok((proof, current))
    }
    pub(crate) fn commit(self) -> Result<(), Error> {
        db(self.transaction.commit())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use ont_core::action::{ActionInput, Basis};
    use ont_core::{Id, Namespace, Object};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "s07-target-unit-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
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
    fn setup() -> (Temp, FakeTarget, Review) {
        let temp = Temp::new();
        let namespace = Namespace {
            instance: "synthetic://target-unit".into(),
            account: "001|A".into(),
        };
        let scope = Scope::Education {
            namespace: namespace.clone(),
            class_key: "C1".into(),
            student_key: "001".into(),
        };
        let physical = PhysicalField {
            namespace: namespace.clone(),
            record: "001|record".into(),
            field: "note".into(),
        };
        let mut target = FakeTarget::create(
            &temp.0.join("target.sqlite"),
            "unit-journal",
            &[SeedCell {
                physical: physical.clone(),
                scope: scope.clone(),
                value: "old".into(),
            }],
        )
        .unwrap();
        let snapshot = target.transaction().unwrap().snapshot(&physical).unwrap();
        // Private target primitive tests only. This is not authentication/admission.
        let mut source: serde_json::Value =
            serde_json::from_str(include_str!("../../../conformance/s07/source-v1.json")).unwrap();
        let observation = &mut source["objects"][0]["observations"][0];
        observation["lineage"] = serde_json::json!({"artifact_sha256":"0".repeat(64),"pointer":"/private-unit-primitive"});
        let object: Object = serde_json::from_value(source["objects"][0].clone()).unwrap();
        let id = Id {
            object_type: "progress".into(),
            namespace,
            native_key: "logical-unit".into(),
        };
        let review = Review {
            input: ActionInput {
                request_id: "unit-request".into(),
                lesson_date: "unit-date".into(),
                object: id.clone(),
            },
            basis: Basis {
                id,
                scope,
                observation: object.observations[0].clone(),
            },
            target: snapshot,
            desired: "practice fractions".into(),
            owner: "unit-owner".into(),
            authorization_generation: 1,
        };
        (temp, target, review)
    }
    #[test]
    fn sealed_absence_prevents_even_a_late_explicit_apply() {
        let (_temp, mut target, review) = setup();
        let tx = target.transaction().unwrap();
        assert_eq!(
            tx.reconcile("token", &review).unwrap().0.outcome,
            Outcome::SealedNoEffect
        );
        tx.commit().unwrap();
        let tx = target.transaction().unwrap();
        tx.apply("token", &review).unwrap();
        assert_eq!(tx.snapshot(&review.target.physical).unwrap(), review.target);
        assert_eq!(
            tx.proof("token", &review).unwrap().unwrap().outcome,
            Outcome::SealedNoEffect
        );
        tx.commit().unwrap();
    }
    #[test]
    fn atomic_target_receipt_is_idempotent_and_conflict_proof_is_no_effect() {
        let (_temp, mut target, review) = setup();
        let tx = target.transaction().unwrap();
        tx.apply("applied", &review).unwrap();
        tx.commit().unwrap();
        let tx = target.transaction().unwrap();
        tx.apply("applied", &review).unwrap();
        let (proof, current) = tx.reconcile("applied", &review).unwrap();
        assert_eq!(proof.outcome, Outcome::Applied);
        assert_eq!(current.version, 1);
        assert_eq!(current.value, review.desired);
        tx.commit().unwrap();
        // A new stale token receives durable rejection without incrementing version.
        let tx = target.transaction().unwrap();
        tx.apply("stale-token", &review).unwrap();
        let proof = tx.proof("stale-token", &review).unwrap().unwrap();
        assert_eq!(proof.outcome, Outcome::ConflictNoEffect);
        assert_eq!(proof.version, 1);
        tx.commit().unwrap();
    }
    #[test]
    fn identity_is_not_only_checked_at_open() {
        let (_temp, mut target, _review) = setup();
        target
            .connection
            .execute("UPDATE meta SET identity='replaced'", [])
            .unwrap();
        assert!(matches!(target.transaction(), Err(Error::TargetIdentity)));
    }
}
