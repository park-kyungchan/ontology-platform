//! Generic identity, evidence, local authority and deterministic restricted reads.
//! No host identity, Academy parser or transport.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock, RwLockReadGuard, Weak};

pub mod action;
pub mod answer;
pub mod input;
pub mod program;
pub mod report;
pub mod schema;
mod scope;
pub use scope::Scope;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    InvalidRequest,
    InvalidEvidence,
    Unavailable,
    PrincipalUnknown,
    StaleAuthorization,
    DeriveForbidden,
    RevisionConflict,
    ScopeConflict,
    StoreFailure,
    AuthorityFailure,
    ActionForbidden,
    ReviewRequired,
    ReviewStale,
    TargetConflict,
    TargetIdentity,
    FieldFenced,
    RetryForbidden,
    SchemaViolation,
    StoreBusy,
    ObservationTooLarge,
}
impl Error {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::InvalidEvidence => "invalid_evidence",
            Self::Unavailable => "unavailable",
            Self::PrincipalUnknown => "principal_unknown",
            Self::StaleAuthorization => "stale_authorization",
            Self::DeriveForbidden => "derive_forbidden",
            Self::RevisionConflict => "revision_conflict",
            Self::ScopeConflict => "scope_conflict",
            Self::StoreFailure => "store_failure",
            Self::AuthorityFailure => "authority_failure",
            Self::ActionForbidden => "action_forbidden",
            Self::ReviewRequired => "review_required",
            Self::ReviewStale => "review_stale",
            Self::TargetConflict => "target_conflict",
            Self::TargetIdentity => "target_identity",
            Self::FieldFenced => "field_fenced",
            Self::RetryForbidden => "retry_forbidden",
            Self::SchemaViolation => "schema_violation",
            Self::StoreBusy => "store_busy",
            Self::ObservationTooLarge => "observation_too_large",
        }
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for Error {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct Namespace {
    pub instance: String,
    pub account: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct Id {
    pub object_type: String,
    pub namespace: Namespace,
    pub native_key: String,
}
impl Id {
    pub fn validate(&self) -> Result<(), Error> {
        if [
            &self.object_type,
            &self.namespace.instance,
            &self.namespace.account,
            &self.native_key,
        ]
        .iter()
        .any(|x| x.is_empty() || x.len() > 512)
        {
            return Err(Error::InvalidRequest);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Knowledge {
    Missing,
    Null,
    Blank,
    Unknown,
    NotPerformed,
    Value(Value),
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    Partial,
    Complete,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    Initial,
    Correction,
    Backfill,
    Update,
    Refresh,
    Retraction,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Lineage {
    pub artifact_sha256: String,
    pub pointer: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub revision: String,
    pub source_revision: String,
    pub observed_at: u64,
    pub fresh_until: u64,
    pub change: Change,
    pub supersedes: Option<String>,
    pub coverage: Coverage,
    pub fields: BTreeMap<String, Knowledge>,
    pub lineage: Lineage,
}
/// Maximum canonical UTF-8 JSON bytes in one persisted observation.
pub const MAX_OBSERVATION_BYTES: usize = 65_536;
/// Maximum items in any array, including nested arrays, in an observation.
pub const MAX_OBSERVATION_ARRAY_ITEMS: usize = 1_024;

/// The shared write boundary: produce exactly the bytes to persist, with a
/// bounded allocation, and check every array before a writer changes any row.
pub fn observation_payload(observation: &Observation) -> Result<String, Error> {
    struct Payload(Vec<u8>);
    impl std::io::Write for Payload {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > MAX_OBSERVATION_BYTES - self.0.len() {
                return Err(std::io::ErrorKind::FileTooLarge.into());
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut payload = Payload(Vec::new());
    serde_json::to_writer(&mut payload, observation).map_err(|_| Error::ObservationTooLarge)?;
    // JSON arrays can occur only inside value-state fields. The serialized
    // byte bound above also bounds this walk and its stack; no values are cloned.
    let mut values: Vec<_> = observation
        .fields
        .values()
        .filter_map(|k| match k {
            Knowledge::Value(v) => Some(v),
            _ => None,
        })
        .collect();
    while let Some(value) = values.pop() {
        match value {
            Value::Array(a) => {
                if a.len() > MAX_OBSERVATION_ARRAY_ITEMS {
                    return Err(Error::ObservationTooLarge);
                }
                values.extend(a);
            }
            Value::Object(o) => values.extend(o.values()),
            _ => {}
        }
    }
    String::from_utf8(payload.0).map_err(|_| Error::InvalidEvidence)
}
impl Observation {
    pub fn validate(&self) -> Result<(), Error> {
        if self.revision.is_empty()
            || self.source_revision.is_empty()
            || self.fresh_until < self.observed_at
            || self.lineage.artifact_sha256.len() != 64
            || !self
                .lineage
                .artifact_sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
            || (!self.lineage.pointer.is_empty() && !self.lineage.pointer.starts_with('/'))
            || self.fields.keys().any(String::is_empty)
        {
            return Err(Error::InvalidEvidence);
        }
        match (&self.change, &self.supersedes) {
            (Change::Initial, None) => Ok(()),
            (Change::Correction | Change::Backfill | Change::Update | Change::Refresh, Some(s))
                if !s.is_empty() =>
            {
                Ok(())
            }
            (Change::Retraction, Some(s)) if !s.is_empty() && self.fields.is_empty() => Ok(()),
            _ => Err(Error::InvalidEvidence),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Object {
    pub id: Id,
    pub scope: Scope,
    pub head: String,
    pub observations: Vec<Observation>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Link {
    pub source: Id,
    pub relation: String,
    pub target: Id,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Observed {
    #[serde(flatten)]
    pub observation: Observation,
    pub stale: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct View {
    pub id: Id,
    pub scope: Scope,
    pub head: String,
    pub observations: Vec<Observed>,
}
impl Object {
    pub fn view(self, now: u64) -> View {
        View {
            id: self.id,
            scope: self.scope,
            head: self.head,
            observations: self
                .observations
                .into_iter()
                .map(|o| Observed {
                    stale: now > o.fresh_until,
                    observation: o,
                })
                .collect(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Query {
    Get {
        id: Id,
    },
    Traverse {
        root: Id,
        relation: String,
    },
    Decide {
        root: Id,
        relation: String,
    },
    ProgramReport {
        root: Id,
        window: report::ReportWindow,
    },
    ProgramReportV2 {
        root: Id,
        window: report::ReportWindow,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Output {
    Objects {
        objects: Vec<View>,
    },
    Decision {
        decision: String,
        basis: Vec<View>,
    },
    ProgramReport {
        report: report::ProgramReport,
    },
    ProgramReportV2 {
        report: Box<report::derived::DerivedReport>,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Ok {
        authorization_revision: u64,
        result: Output,
    },
    Refused {
        code: String,
    },
}
impl Response {
    pub fn refused(e: Error) -> Self {
        Self::Refused {
            code: e.code().into(),
        }
    }
}

/// Internal persistence interface. Never hand this raw interface to an untrusted caller.
pub trait Snapshot {
    fn get(&self, id: &Id) -> Result<Option<Object>, Error>;
    fn targets(&self, root: &Id, relation: &str) -> Result<Vec<Id>, Error>;
}
#[derive(Debug, Clone)]
pub struct Grant {
    pub scope: Scope,
    pub object_types: BTreeSet<String>,
    pub read: bool,
    pub derive: bool,
    /// Explicit named actions; read/derive alone never authorizes an effect.
    pub actions: BTreeSet<String>,
}
#[derive(Debug, Clone, Default)]
pub struct Policy {
    pub principals: BTreeMap<String, Vec<Grant>>,
}
struct CurrentPolicy {
    generation: u64,
    policy: Policy,
}
#[derive(Clone)]
pub struct Authority {
    current: Arc<RwLock<CurrentPolicy>>,
}
/// Opaque local session, created only after the embedding application authenticates.
#[derive(Clone)]
pub struct Session {
    authority: Weak<RwLock<CurrentPolicy>>,
    principal: String,
    generation: u64,
}
pub struct Admission<'a> {
    guard: RwLockReadGuard<'a, CurrentPolicy>,
    principal: &'a str,
}
impl Authority {
    pub fn new(policy: Policy) -> Self {
        Self {
            current: Arc::new(RwLock::new(CurrentPolicy {
                generation: 1,
                policy,
            })),
        }
    }
    /// TRUSTED EMBEDDER ONLY. A string is NOT a credential or authentication proof.
    pub fn bind_trusted(&self, authenticated_principal: &str) -> Result<Session, Error> {
        let guard = self.current.read().map_err(|_| Error::AuthorityFailure)?;
        if !guard
            .policy
            .principals
            .contains_key(authenticated_principal)
        {
            return Err(Error::PrincipalUnknown);
        }
        Ok(Session {
            authority: Arc::downgrade(&self.current),
            principal: authenticated_principal.into(),
            generation: guard.generation,
        })
    }
    /// The current policy generation (read lock; a poisoned lock is
    /// `AuthorityFailure`). Exposes nothing else.
    pub fn generation(&self) -> Result<u64, Error> {
        let guard = self.current.read().map_err(|_| Error::AuthorityFailure)?;
        Ok(guard.generation)
    }
    /// Revocation linearizes when the write guard is acquired. In-flight publication completes first.
    pub fn replace_trusted(&self, policy: Policy) -> Result<(), Error> {
        let mut guard = self.current.write().map_err(|_| Error::AuthorityFailure)?;
        guard.generation = guard
            .generation
            .checked_add(1)
            .ok_or(Error::AuthorityFailure)?;
        guard.policy = policy;
        Ok(())
    }
    pub fn admit<'a>(&'a self, session: &'a Session) -> Result<Admission<'a>, Error> {
        if !Weak::ptr_eq(&session.authority, &Arc::downgrade(&self.current)) {
            return Err(Error::StaleAuthorization);
        }
        let guard = self.current.read().map_err(|_| Error::AuthorityFailure)?;
        if session.generation != guard.generation
            || !guard.policy.principals.contains_key(&session.principal)
        {
            return Err(Error::StaleAuthorization);
        }
        Ok(Admission {
            guard,
            principal: &session.principal,
        })
    }
}
impl Admission<'_> {
    fn permitted(&self, object: &Object, derive: bool) -> bool {
        self.guard
            .policy
            .principals
            .get(self.principal)
            .is_some_and(|grants| {
                grants.iter().any(|g| {
                    g.scope.matches(&object.scope)
                        && g.object_types.contains(&object.id.object_type)
                        && g.read
                        && (!derive || g.derive)
                })
            })
    }
    /// Keep Admission alive until serialized bytes are built. Service enforces that boundary.
    pub fn query(
        &self,
        snapshot: &impl Snapshot,
        query: Query,
        now: u64,
    ) -> Result<Response, Error> {
        if let Query::ProgramReport { root, window } = query {
            return Ok(Response::Ok {
                authorization_revision: self.guard.generation,
                result: Output::ProgramReport {
                    report: self.program_report(snapshot, root, window, now)?,
                },
            });
        }
        if let Query::ProgramReportV2 { root, window } = query {
            return Ok(Response::Ok {
                authorization_revision: self.guard.generation,
                result: Output::ProgramReportV2 {
                    report: Box::new(self.program_report_v2(snapshot, root, window, now)?),
                },
            });
        }
        let (root, relation, derived) = match &query {
            Query::Get { id } => (id, None, false),
            Query::Traverse { root, relation } => (root, Some(relation), false),
            Query::Decide { root, relation } => (root, Some(relation), true),
            Query::ProgramReport { .. } | Query::ProgramReportV2 { .. } => {
                return Err(Error::InvalidRequest);
            }
        };
        root.validate()?;
        if relation.is_some_and(|r| r.is_empty() || r.len() > 64) {
            return Err(Error::InvalidRequest);
        }
        let object = snapshot
            .get(root)?
            .filter(|o| self.permitted(o, false))
            .ok_or(Error::Unavailable)?;
        if derived && !self.permitted(&object, true) {
            return Err(Error::DeriveForbidden);
        }
        let mut selected = BTreeMap::new();
        if relation.is_none() || derived {
            selected.insert(object.id.clone(), object);
        }
        if let Some(relation) = relation {
            for id in snapshot.targets(root, relation)? {
                if let Some(target) = snapshot.get(&id)?.filter(|o| self.permitted(o, false)) {
                    if derived && !self.permitted(&target, true) {
                        return Err(Error::DeriveForbidden);
                    }
                    selected.insert(target.id.clone(), target);
                }
            }
        }
        let result = if derived {
            let review = selected.values().any(|o| {
                o.observations
                    .iter()
                    .find(|x| x.revision == o.head)
                    .is_none_or(|head| {
                        head.coverage == Coverage::Partial
                            || now > head.fresh_until
                            || head.fields.is_empty()
                            || head
                                .fields
                                .values()
                                .any(|v| !matches!(v, Knowledge::Value(_)))
                    })
            });
            Output::Decision {
                decision: if review {
                    "review_required"
                } else {
                    "evidence_available"
                }
                .into(),
                basis: selected.into_values().map(|o| o.view(now)).collect(),
            }
        } else {
            Output::Objects {
                objects: selected.into_values().map(|o| o.view(now)).collect(),
            }
        };
        Ok(Response::Ok {
            authorization_revision: self.guard.generation,
            result,
        })
    }
}

#[cfg(test)]
mod generation_tests {
    use super::*;

    #[test]
    fn generation_is_read_only_and_follows_replacement() {
        let authority = Authority::new(Policy::default());
        assert_eq!(authority.generation(), Ok(1));
        assert_eq!(authority.replace_trusted(Policy::default()), Ok(()));
        assert_eq!(authority.generation(), Ok(2));
    }
}
