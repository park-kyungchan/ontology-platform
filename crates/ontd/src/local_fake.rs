//! Explicitly synthetic local demonstration and authenticated service composition.
//! Profiles/fixture sessions are not production authentication.
use crate::Service;
use ont_core::action::{ActionInput, Basis, COPY_RECOMMENDATION, RequestKey, physical};
use ont_core::{Authority, Error, Object, Policy, Session};
use ont_store::{
    Store,
    actions::{Boundary, Evidence},
    fake::{FakeTarget, SeedCell},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub const NOW: u64 = 100;
pub const PRINCIPAL: &str = "s07-synthetic-authenticated";
const SOURCE: &str = include_str!("../../../conformance/s07/source-v1.json");

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActionCommand {
    Review { input: ActionInput },
    Dispatch { key: RequestKey },
    Inspect { key: RequestKey },
    Reconcile { key: RequestKey, evidence: Evidence },
}
impl Service {
    /// Current Session admission spans the coherent data decision, target effect,
    /// readback and serialized output. Hooks are trusted finite test injection only.
    pub fn render_action(
        &mut self,
        session: &Session,
        target: &mut FakeTarget,
        command: ActionCommand,
        now: u64,
        hook: &mut impl FnMut(Boundary),
    ) -> Result<String, Error> {
        let admission = self.authority.admit(session)?;
        let result = match command {
            ActionCommand::Review { input } => self
                .store
                .review_local(&admission, target, input, now, hook)?,
            ActionCommand::Dispatch { key } => self
                .store
                .dispatch_local(&admission, target, &key, now, hook)?,
            ActionCommand::Inspect { key } => self.store.inspect_local(&admission, &key, now)?,
            ActionCommand::Reconcile { key, evidence } => self
                .store
                .reconcile_local(&admission, target, &key, now, evidence, hook)?,
        };
        serde_json::to_string(&result).map_err(|_| Error::StoreFailure)
    }
}
/// Trusted synthetic intake: lineage binds the exact included source bytes and
/// JSON pointer BEFORE admission; S06 generic store ingestion remains trusted.
pub fn graph() -> Result<Vec<Object>, Error> {
    let mut source: Value = serde_json::from_str(SOURCE).map_err(|_| Error::InvalidEvidence)?;
    let digest = format!("{:x}", Sha256::digest(SOURCE.as_bytes()));
    let rows = source
        .get_mut("objects")
        .and_then(Value::as_array_mut)
        .ok_or(Error::InvalidEvidence)?;
    for (index, row) in rows.iter_mut().enumerate() {
        let observations = row
            .get_mut("observations")
            .and_then(Value::as_array_mut)
            .ok_or(Error::InvalidEvidence)?;
        for (ordinal, observation) in observations.iter_mut().enumerate() {
            observation.as_object_mut().ok_or(Error::InvalidEvidence)?.insert("lineage".into(),
                serde_json::json!({"artifact_sha256":digest,"pointer":format!("/objects/{index}/observations/{ordinal}")}));
        }
    }
    rows.iter()
        .cloned()
        .map(|value| serde_json::from_value(value).map_err(|_| Error::InvalidEvidence))
        .collect()
}
pub fn authority(profile: &str) -> Result<(Authority, Session), Error> {
    let mut scope = graph()?
        .into_iter()
        .next()
        .ok_or(Error::InvalidEvidence)?
        .scope;
    if profile == "account_b" {
        scope.namespace.account = "B".into();
    }
    let grant = ont_core::Grant {
        scope,
        object_types: BTreeSet::from(["progress".into()]),
        read: true,
        derive: profile != "no_derive",
        actions: if profile == "read_only" {
            BTreeSet::new()
        } else {
            BTreeSet::from([COPY_RECOMMENDATION.into()])
        },
    };
    let policy = if profile == "revoked" {
        Policy::default()
    } else {
        Policy {
            principals: BTreeMap::from([(PRINCIPAL.into(), vec![grant])]),
        }
    };
    if ![
        "normal",
        "account_b",
        "read_only",
        "no_derive",
        "host",
        "revoked",
        "renewed",
    ]
    .contains(&profile)
    {
        return Err(Error::InvalidRequest);
    }
    let authority = Authority::new(policy.clone());
    if profile == "renewed" {
        authority.replace_trusted(policy)?;
    }
    let session = authority.bind_trusted(if profile == "host" { "root" } else { PRINCIPAL })?;
    Ok((authority, session))
}
pub fn initialize(root: &Path) -> Result<(), Error> {
    std::fs::create_dir(root).map_err(|_| Error::InvalidRequest)?;
    let objects = graph()?;
    let mut store = Store::open(&root.join("local.sqlite"))?;
    store.seed(&objects, &[])?;
    store.prepare_local_actions()?;
    let cells = objects
        .into_iter()
        .map(|o| {
            let observation = o.observations.last().ok_or(Error::InvalidEvidence)?.clone();
            let p = physical(&Basis {
                id: o.id,
                scope: o.scope.clone(),
                observation,
            })?;
            Ok(SeedCell {
                physical: p,
                scope: o.scope,
                value: "old".into(),
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    FakeTarget::create(
        &root.join("target.sqlite"),
        &store.local_journal_id()?,
        &cells,
    )?;
    Ok(())
}
pub fn open(root: &Path) -> Result<(Store, FakeTarget), Error> {
    if !root.join("local.sqlite").is_file() || !root.join("target.sqlite").is_file() {
        return Err(Error::InvalidRequest);
    }
    let store = Store::open(&root.join("local.sqlite"))?;
    let target = FakeTarget::open(&root.join("target.sqlite"), &store.local_journal_id()?)?;
    Ok((store, target))
}
