//! Durable action journal for the closed fake target. No replay loop or expiration.
use crate::fake::{FakeTarget, Outcome, Proof};
use crate::{ReadSnapshot, Store, db, decode, v1_scope};
use ont_core::action::{
    ActionInput, ActionOutput, PhysicalField, RequestKey, Review, State, physical,
};
use ont_core::{Admission, Error};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

pub const ACTION_SCHEMA: &str = include_str!("../actions.sql");
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    Durable,
    ValueOnly,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Boundary {
    BeforeReviewCommit,
    AfterReviewCommit,
    BeforeFenceCommit,
    AfterFenceCommit,
    BeforeTargetCommit,
    AfterTargetCommit,
    BeforeReadback,
    BeforeReceiptCommit,
    AfterReceiptCommit,
}
impl Boundary {
    pub fn name(self) -> &'static str {
        match self {
            Self::BeforeReviewCommit => "before_review_commit",
            Self::AfterReviewCommit => "after_review_commit",
            Self::BeforeFenceCommit => "before_fence_commit",
            Self::AfterFenceCommit => "after_fence_commit",
            Self::BeforeTargetCommit => "before_target_commit",
            Self::AfterTargetCommit => "after_target_commit",
            Self::BeforeReadback => "before_readback",
            Self::BeforeReceiptCommit => "before_receipt_commit",
            Self::AfterReceiptCommit => "after_receipt_commit",
        }
    }
    pub fn exit_status(self) -> i32 {
        match self {
            Self::BeforeReviewCommit => 81,
            Self::AfterReviewCommit => 82,
            Self::BeforeFenceCommit => 83,
            Self::AfterFenceCommit => 84,
            Self::BeforeTargetCommit => 85,
            Self::AfterTargetCommit => 86,
            Self::BeforeReceiptCommit => 87,
            Self::AfterReceiptCommit => 88,
            Self::BeforeReadback => 89,
        }
    }
}
struct Saved {
    token: String,
    review: Review,
    state: State,
    observed: Option<String>,
}
fn json<T: Serialize>(value: &T) -> Result<String, Error> {
    serde_json::to_string(value).map_err(|_| Error::StoreFailure)
}
fn snapshot<'a>(tx: &'a Transaction<'_>) -> ReadSnapshot<'a> {
    ReadSnapshot { connection: tx }
}
pub(crate) fn decode_v1_review(raw: &str) -> Result<Review, Error> {
    let review: Review = decode(raw)?;
    v1_scope(&review.basis.scope, &review.basis.id.namespace)?;
    v1_scope(&review.target.scope, &review.target.physical.namespace)?;
    if review.input.object != review.basis.id || review.target.scope != review.basis.scope {
        return Err(Error::StoreFailure);
    }
    Ok(review)
}
fn load(tx: &Transaction<'_>, key: &RequestKey) -> Result<Option<Saved>, Error> {
    let row: Option<(String,String,String,Option<String>)> = db(tx.query_row(
        "SELECT token,review,state,observed FROM local_actions WHERE instance=?1 AND account=?2 AND request_id=?3",
        params![key.namespace.instance,key.namespace.account,key.request_id],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional())?;
    row.map(|(token, review, state, observed)| {
        let review = decode_v1_review(&review)?;
        if review.input.key() != *key {
            return Err(Error::StoreFailure);
        }
        Ok(Saved {
            token,
            review,
            state: decode(&json(&state)?)?,
            observed,
        })
    })
    .transpose()
}
fn unfenced(tx: &Transaction<'_>, p: &PhysicalField) -> Result<(), Error> {
    let exists: bool = db(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM local_fences WHERE instance=?1 AND account=?2 AND record=?3 AND field=?4)",
        params![p.namespace.instance,p.namespace.account,p.record,p.field], |r| r.get(0)))?;
    if exists {
        Err(Error::FieldFenced)
    } else {
        Ok(())
    }
}
fn output(
    tx: &Transaction<'_>,
    admission: &Admission<'_>,
    saved: &Saved,
    now: u64,
) -> Result<ActionOutput, Error> {
    admission.action_output(
        &snapshot(tx),
        &saved.review,
        saved.state,
        saved.observed.clone(),
        now,
    )
}
impl Store {
    /// Explicit additive installation on this trusted synthetic S06 store only.
    /// S06 readers keep their version-1 object tables and ignore this extension.
    pub fn prepare_local_actions(&mut self) -> Result<(), Error> {
        let tx = db(self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate))?;
        db(tx.execute_batch(ACTION_SCHEMA))?;
        let version: i64 = db(tx.query_row(
            "SELECT version FROM local_action_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        ))?;
        if version != 1 {
            return Err(Error::StoreFailure);
        }
        db(tx.commit())
    }
    pub fn local_journal_id(&self) -> Result<String, Error> {
        db(self.connection.query_row(
            "SELECT journal_id FROM local_action_meta WHERE singleton=1 AND version=1",
            [],
            |r| r.get(0),
        ))
    }
    pub fn review_local(
        &mut self,
        admission: &Admission<'_>,
        target: &mut FakeTarget,
        input: ActionInput,
        now: u64,
        hook: &mut impl FnMut(Boundary),
    ) -> Result<ActionOutput, Error> {
        target.check_journal(&self.local_journal_id()?)?;
        let tx = db(self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate))?;
        let basis = admission.action_basis(&snapshot(&tx), &input, now)?;
        if let Some(saved) = load(&tx, &input.key())? {
            admission.authorize_review(&snapshot(&tx), &saved.review, now, false)?;
            if saved.review.input != input {
                return Err(Error::ReviewStale);
            }
            return output(&tx, admission, &saved, now);
        }
        let p = physical(&basis)?;
        unfenced(&tx, &p)?;
        let target_tx = target.transaction()?;
        let review =
            admission.review_action(&snapshot(&tx), input, target_tx.snapshot(&p)?, now)?;
        let token: String =
            db(tx.query_row("SELECT lower(hex(randomblob(16)))", [], |r| r.get(0)))?;
        let key = review.input.key();
        db(tx.execute(
            "INSERT INTO local_actions VALUES(?1,?2,?3,?4,?5,?6,?7,'reviewed',?8,NULL)",
            params![
                token,
                key.namespace.instance,
                key.namespace.account,
                key.request_id,
                review.basis.id.object_type,
                review.basis.id.native_key,
                json(&review)?,
                review.target.value
            ],
        ))?;
        let saved = Saved {
            token,
            observed: Some(review.target.value.clone()),
            review,
            state: State::Reviewed,
        };
        let result = output(&tx, admission, &saved, now)?;
        hook(Boundary::BeforeReviewCommit);
        db(tx.commit())?;
        hook(Boundary::AfterReviewCommit);
        target_tx.commit()?;
        Ok(result)
    }
    pub fn inspect_local(
        &mut self,
        admission: &Admission<'_>,
        key: &RequestKey,
        now: u64,
    ) -> Result<ActionOutput, Error> {
        let tx = db(self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate))?;
        let saved = load(&tx, key)?.ok_or(Error::Unavailable)?;
        let result = output(&tx, admission, &saved, now)?;
        db(tx.commit())?;
        Ok(result)
    }
    pub fn dispatch_local(
        &mut self,
        admission: &Admission<'_>,
        target: &mut FakeTarget,
        key: &RequestKey,
        now: u64,
        hook: &mut impl FnMut(Boundary),
    ) -> Result<ActionOutput, Error> {
        target.check_journal(&self.local_journal_id()?)?;
        // Never acquire local SQLite while holding an unaccompanied target lock.
        let tx = db(self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate))?;
        let saved = load(&tx, key)?.ok_or(Error::Unavailable)?;
        admission.authorize_review(&snapshot(&tx), &saved.review, now, false)?;
        if saved.state.settled() {
            return output(&tx, admission, &saved, now);
        }
        if saved.state != State::Reviewed {
            return Err(Error::RetryForbidden);
        }
        admission.authorize_review(&snapshot(&tx), &saved.review, now, true)?;
        let p = &saved.review.target.physical;
        unfenced(&tx, p)?;
        let target_tx = target.transaction()?;
        if target_tx.identity(&saved.review)? != saved.review.target {
            return Err(Error::TargetConflict);
        }
        db(tx.execute(
            "INSERT INTO local_fences VALUES(?1,?2,?3,?4,?5)",
            params![
                p.namespace.instance,
                p.namespace.account,
                p.record,
                p.field,
                saved.token
            ],
        ))?;
        db(tx.execute(
            "UPDATE local_actions SET state='possible_dispatch',observed=NULL WHERE token=?1",
            [&saved.token],
        ))?;
        hook(Boundary::BeforeFenceCommit);
        // MUST succeed before even calling apply; ambiguous commit => return without effect.
        db(tx.commit())?;
        hook(Boundary::AfterFenceCommit);
        target_tx.apply(&saved.token, &saved.review)?;
        hook(Boundary::BeforeTargetCommit);
        target_tx.commit()?;
        hook(Boundary::AfterTargetCommit);
        hook(Boundary::BeforeReadback);
        // Target lock is released before reacquiring the local writer transaction.
        self.reconcile_local(admission, target, key, now, Evidence::Durable, hook)
    }
    pub fn reconcile_local(
        &mut self,
        admission: &Admission<'_>,
        target: &mut FakeTarget,
        key: &RequestKey,
        now: u64,
        evidence: Evidence,
        hook: &mut impl FnMut(Boundary),
    ) -> Result<ActionOutput, Error> {
        target.check_journal(&self.local_journal_id()?)?;
        let tx = db(self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate))?;
        let mut saved = load(&tx, key)?.ok_or(Error::Unavailable)?;
        admission.authorize_review(&snapshot(&tx), &saved.review, now, false)?;
        if saved.state.settled() || saved.state == State::Reviewed {
            return output(&tx, admission, &saved, now);
        }
        // A corrupt/missing fence is never repaired by dispatching or assumed settled.
        let fenced: bool = db(tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM local_fences WHERE token=?1)",
            [&saved.token],
            |r| r.get(0),
        ))?;
        if !fenced {
            return Err(Error::StoreFailure);
        }
        let target_tx = target.transaction()?;
        target_tx.identity(&saved.review)?;
        if evidence == Evidence::ValueOnly {
            // A value, even desired, cannot establish no effect or causation.
            target_tx.commit()?;
            return output(&tx, admission, &saved, now);
        }
        let (proof, current) = target_tx.reconcile(&saved.token, &saved.review)?;
        saved.state = match proof.outcome {
            Outcome::Applied
                if proof.version == current.version
                    && proof.value == current.value
                    && current.value == saved.review.desired =>
            {
                State::Persisted
            }
            Outcome::Applied => State::ManualReconciliation,
            Outcome::SealedNoEffect => State::NoEffect,
            Outcome::ConflictNoEffect => State::ConflictNoEffect,
        };
        saved.observed = Some(current.value);
        // Seal/idempotency proof must be durable BEFORE a local fence is released.
        target_tx.commit()?;
        persist(&tx, &saved, &proof)?;
        let result = output(&tx, admission, &saved, now)?;
        hook(Boundary::BeforeReceiptCommit);
        db(tx.commit())?;
        hook(Boundary::AfterReceiptCommit);
        Ok(result)
    }
}
fn persist(tx: &Transaction<'_>, saved: &Saved, proof: &Proof) -> Result<(), Error> {
    db(tx.execute(
        "UPDATE local_actions SET state=?2,observed=?3,proof=?4 WHERE token=?1",
        params![
            saved.token,
            saved.state.code(),
            saved.observed,
            json(proof)?
        ],
    ))?;
    if saved.state.settled() {
        db(tx.execute("DELETE FROM local_fences WHERE token=?1", [&saved.token]))?;
    }
    Ok(())
}
