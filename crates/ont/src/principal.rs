//! The principal `agent`: one grant view of the installed schemas, from
//! which both the admission `Policy` and `describe` are derived, so the
//! two cannot differ.
use crate::{Failure, internal};
use ont_core::schema::SchemaSet;
use ont_core::{Authority, Grant, Policy, Scope, Session};
use std::collections::{BTreeMap, BTreeSet};

pub const PRINCIPAL: &str = "agent";

/// What one grant of the principal allows, with its scope built.
#[derive(Debug, Clone)]
pub struct GrantEntry {
    pub domain: String,
    pub scope: Scope,
    pub read: BTreeSet<String>,
    pub derive: BTreeSet<String>,
    pub actions: BTreeSet<String>,
}

/// The principal's grants over all installed schemas.
#[derive(Debug, Clone, Default)]
pub struct GrantView {
    pub entries: Vec<GrantEntry>,
}
impl GrantView {
    /// Fails (internal) on a grant whose scope cannot be built: schema check
    /// refuses such a grant, so an installed schema never has one.
    pub fn from_set(set: &SchemaSet) -> Result<Self, Failure> {
        let mut entries = Vec::new();
        for domain in set.pins().keys() {
            let Some(schema) = set.schema(domain) else {
                continue;
            };
            for g in schema.grants.iter().filter(|g| g.principal == PRINCIPAL) {
                let Ok(scope) = Scope::domain(
                    schema.namespace.clone(),
                    schema.domain.clone(),
                    g.scope.clone(),
                ) else {
                    return Err(Failure::Internal(internal!("grant_scope_invalid")));
                };
                entries.push(GrantEntry {
                    domain: schema.domain.clone(),
                    scope,
                    read: g.read.iter().cloned().collect(),
                    derive: g.derive.iter().cloned().collect(),
                    actions: g.actions.iter().cloned().collect(),
                });
            }
        }
        Ok(Self { entries })
    }
    /// Object types of `domain` the principal may read.
    pub fn read_types(&self, domain: &str) -> BTreeSet<String> {
        self.entries
            .iter()
            .filter(|e| e.domain == domain)
            .flat_map(|e| e.read.iter().cloned())
            .collect()
    }
    /// Action types of `domain` the principal may apply.
    pub fn action_types(&self, domain: &str) -> BTreeSet<String> {
        self.entries
            .iter()
            .filter(|e| e.domain == domain)
            .flat_map(|e| e.actions.iter().cloned())
            .collect()
    }
    /// Object types readable in any domain.
    pub fn all_read_types(&self) -> BTreeSet<String> {
        self.entries
            .iter()
            .flat_map(|e| e.read.iter().cloned())
            .collect()
    }
    /// Domains with at least one readable object type or applicable action.
    pub fn domains(&self) -> BTreeSet<String> {
        self.entries
            .iter()
            .filter(|e| !e.read.is_empty() || !e.actions.is_empty())
            .map(|e| e.domain.clone())
            .collect()
    }
    /// The admission policy. Each read/derive type gets its own grant so
    /// neither flag leaks to the other.
    pub(crate) fn policy(&self) -> Policy {
        let mut grants: Vec<Grant> = Vec::new();
        for e in &self.entries {
            let types: BTreeSet<&String> = e.read.iter().chain(e.derive.iter()).collect();
            for t in types {
                grants.push(Grant {
                    scope: e.scope.clone(),
                    object_types: BTreeSet::from([t.clone()]),
                    read: e.read.contains(t),
                    derive: e.derive.contains(t),
                    actions: e.actions.iter().cloned().collect(),
                });
            }
        }
        Policy {
            principals: BTreeMap::from([(PRINCIPAL.to_string(), grants)]),
        }
    }
}

/// The policy generation of a fresh `Authority`, read from the `Authority`
/// (never a literal). Operations that bind no principal (`backup`, `index`,
/// `schema install`) take their `authorization_revision` from here.
pub fn fresh_revision() -> Result<u64, Failure> {
    Authority::new(Policy::default())
        .generation()
        .map_err(|_| Failure::Internal(internal!("authority_failure")))
}

/// The bound principal of one request.
pub struct Principal {
    authority: Authority,
    session: Session,
    view: GrantView,
}
impl Principal {
    /// Build the `Authority` from the installed grants and bind `agent`
    /// through `bind_trusted` (the caller checked uid 0 first).
    pub fn bind(set: &SchemaSet) -> Result<Self, Failure> {
        let view = GrantView::from_set(set)?;
        let authority = Authority::new(view.policy());
        let session = authority
            .bind_trusted(PRINCIPAL)
            .map_err(|_| Failure::Internal(internal!("authority_failure")))?;
        Ok(Self {
            authority,
            session,
            view,
        })
    }
    pub fn view(&self) -> &GrantView {
        &self.view
    }
    pub fn authority(&self) -> &Authority {
        &self.authority
    }
    pub fn session(&self) -> &Session {
        &self.session
    }
    /// `authorization_revision` of an answer: the policy generation of the
    /// `Authority` built for this request, read through
    /// `Authority::generation` (a poisoned lock is `authority_failure`).
    pub fn revision(&self) -> Result<u64, Failure> {
        self.authority
            .generation()
            .map_err(|_| Failure::Internal(internal!("authority_failure")))
    }
}
