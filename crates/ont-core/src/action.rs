//! Pure, authorized copy_recommendation logic. No storage, transport or host identity.
use crate::{
    Admission, Coverage, Error, Id, Knowledge, Namespace, Object, Observation, Scope, Snapshot,
};
use serde::{Deserialize, Serialize};

pub const COPY_RECOMMENDATION: &str = "copy_recommendation";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ActionInput {
    pub request_id: String,
    /// Audit context, never a physical field key or authorization credential.
    pub lesson_date: String,
    pub object: Id,
}
impl ActionInput {
    pub fn validate(&self) -> Result<(), Error> {
        self.object.validate()?;
        if self.request_id.is_empty()
            || self.request_id.len() > 128
            || self.lesson_date.is_empty()
            || self.lesson_date.len() > 32
        {
            return Err(Error::InvalidRequest);
        }
        Ok(())
    }
    pub fn key(&self) -> RequestKey {
        RequestKey {
            namespace: self.object.namespace.clone(),
            request_id: self.request_id.clone(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RequestKey {
    pub namespace: Namespace,
    pub request_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PhysicalField {
    pub namespace: Namespace,
    pub record: String,
    pub field: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Basis {
    pub id: Id,
    pub scope: Scope,
    pub observation: Observation,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TargetSnapshot {
    pub identity: String,
    pub canonical_path: String,
    pub physical: PhysicalField,
    pub scope: Scope,
    pub value: String,
    pub version: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Review {
    pub input: ActionInput,
    pub basis: Basis,
    pub target: TargetSnapshot,
    pub desired: String,
    pub owner: String,
    pub authorization_generation: u64,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Reviewed,
    PossibleDispatch,
    Persisted,
    NoEffect,
    ConflictNoEffect,
    ManualReconciliation,
}
impl State {
    pub fn code(self) -> &'static str {
        match self {
            Self::Reviewed => "reviewed",
            Self::PossibleDispatch => "possible_dispatch",
            Self::Persisted => "persisted",
            Self::NoEffect => "no_effect",
            Self::ConflictNoEffect => "conflict_no_effect",
            Self::ManualReconciliation => "manual_reconciliation",
        }
    }
    pub fn settled(self) -> bool {
        matches!(
            self,
            Self::Persisted | Self::NoEffect | Self::ConflictNoEffect
        )
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionOutput {
    pub status: String,
    pub authorization_revision: u64,
    pub request_id: String,
    pub object: Id,
    pub scope: Scope,
    pub physical: PhysicalField,
    pub state: State,
    pub desired: String,
    pub observed: Option<String>,
}

fn text(observation: &Observation, field: &str) -> Result<String, Error> {
    match observation.fields.get(field) {
        Some(Knowledge::Value(serde_json::Value::String(value)))
            if !value.is_empty() && value.len() <= 4096 =>
        {
            Ok(value.clone())
        }
        _ => Err(Error::ReviewRequired),
    }
}
pub fn physical(basis: &Basis) -> Result<PhysicalField, Error> {
    Ok(PhysicalField {
        namespace: basis.id.namespace.clone(),
        record: text(&basis.observation, "physical_record")?,
        field: "note".into(),
    })
}
impl Admission<'_> {
    fn actionable(&self, object: &Object) -> Result<(), Error> {
        if !self.permitted(object, false) {
            return Err(Error::Unavailable);
        }
        if !self.permitted(object, true) {
            return Err(Error::DeriveForbidden);
        }
        if !self
            .guard
            .policy
            .principals
            .get(self.principal)
            .is_some_and(|grants| {
                grants.iter().any(|g| {
                    g.scope == object.scope
                        && g.object_types.contains(&object.id.object_type)
                        && g.read
                        && g.derive
                        && g.actions.contains(COPY_RECOMMENDATION)
                })
            })
        {
            return Err(Error::ActionForbidden);
        }
        Ok(())
    }
    /// Function output is computed from one authorized snapshot, not caller text.
    pub fn action_basis(
        &self,
        snapshot: &impl Snapshot,
        input: &ActionInput,
        now: u64,
    ) -> Result<Basis, Error> {
        input.validate()?;
        let object = snapshot.get(&input.object)?.ok_or(Error::Unavailable)?;
        self.actionable(&object)?;
        let observation = object
            .observations
            .iter()
            .find(|o| o.revision == object.head)
            .ok_or(Error::InvalidEvidence)?
            .clone();
        if observation.coverage != Coverage::Complete
            || now < observation.observed_at
            || now > observation.fresh_until
        {
            return Err(Error::ReviewRequired);
        }
        let basis = Basis {
            id: object.id,
            scope: object.scope,
            observation,
        };
        physical(&basis)?;
        text(&basis.observation, "recommendation")?;
        Ok(basis)
    }
    pub fn review_action(
        &self,
        snapshot: &impl Snapshot,
        input: ActionInput,
        target: TargetSnapshot,
        now: u64,
    ) -> Result<Review, Error> {
        let basis = self.action_basis(snapshot, &input, now)?;
        if target.physical != physical(&basis)? || target.scope != basis.scope {
            return Err(Error::TargetConflict);
        }
        Ok(Review {
            desired: text(&basis.observation, "recommendation")?,
            input,
            basis,
            target,
            owner: self.principal.into(),
            authorization_generation: self.guard.generation,
        })
    }
    /// Narrow current reauthorization. Recovery never follows a remapped physical record.
    pub fn authorize_review(
        &self,
        snapshot: &impl Snapshot,
        review: &Review,
        now: u64,
        dispatch: bool,
    ) -> Result<(), Error> {
        if self.principal != review.owner {
            return Err(Error::Unavailable);
        }
        let object = snapshot.get(&review.basis.id)?.ok_or(Error::Unavailable)?;
        self.actionable(&object)?;
        if object.scope != review.basis.scope {
            return Err(Error::Unavailable);
        }
        let current = object
            .observations
            .iter()
            .find(|o| o.revision == object.head)
            .ok_or(Error::InvalidEvidence)?;
        let current_basis = Basis {
            id: object.id.clone(),
            scope: object.scope.clone(),
            observation: current.clone(),
        };
        if physical(&current_basis)? != review.target.physical {
            return Err(Error::ReviewStale);
        }
        if dispatch
            && (self.guard.generation != review.authorization_generation
                || self.action_basis(snapshot, &review.input, now)? != review.basis)
        {
            return Err(Error::ReviewStale);
        }
        Ok(())
    }
    /// Caller must hold this Admission through serialization after authorize_review.
    pub fn action_output(
        &self,
        snapshot: &impl Snapshot,
        review: &Review,
        state: State,
        observed: Option<String>,
        now: u64,
    ) -> Result<ActionOutput, Error> {
        self.authorize_review(snapshot, review, now, false)?;
        Ok(ActionOutput {
            status: "ok".into(),
            authorization_revision: self.guard.generation,
            request_id: review.input.request_id.clone(),
            object: review.basis.id.clone(),
            scope: review.basis.scope.clone(),
            physical: review.target.physical.clone(),
            state,
            desired: review.desired.clone(),
            observed,
        })
    }
}
