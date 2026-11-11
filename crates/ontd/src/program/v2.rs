//! Separate, owner-pinned v2 intake. No source execution or journal mutation.
//! Wire types mirror the merged producer at agent-platform 6e252fd. Retain the
//! envelope and artifact bytes before dropping this disposable projection.
use super::{IntakeError, Replay, Result, digest, strict};
use crate::{program_git, program_guardrails as guards};
use ont_core::program::{MetricDefinition, MetricRole, PathPopulation, Unit};
use ont_core::report::ReportWindow;
use ont_core::{
    Change, Coverage, Id, Knowledge, Lineage, Link, Namespace, Object, Observation, Scope,
};
use ont_store::Store;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub mod model;
#[cfg(target_os = "linux")]
mod source;
#[cfg(test)]
pub(crate) mod tests;
pub mod wire;
use model::*;
pub const ADAPTER_REVISION: &str = "program-snapshot-intake/v2";
pub const SNAPSHOTS_PATH: &str =
    "/home/palantirkc/work/ws-replan/preparation/kpi/snapshots-v2.jsonl";
pub const EVIDENCE_ROOT: &str = "/home/palantirkc/work/ws-replan/preparation/kpi/v2-captures";
const EVIDENCE_LIMIT: usize = 32 * 1024 * 1024;
const TOTAL_LIMIT: usize = 64 * 1024 * 1024;
const SELECTORS: [&str; 2] = ["kpi_legacy_removal", "kpi_mcp_parity"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProducerPin {
    pub source_commit: String,
    pub source_tree: String,
    pub binary_sha256: String,
    pub build_evidence_sha256: String,
}
/// Trusted owner binding. Definition and guardrail contracts are adopted outside
/// the journal; row-provided thresholds never grant themselves authority.
#[derive(Clone)]
pub struct IntakeContract {
    pub namespace: Namespace,
    pub guardrail_namespace: Namespace,
    pub program_key: String,
    /// Trusted acquisition owner, supplied independently of row bytes.
    pub source_owner_uid: u32,
    pub definition: MetricDefinition,
    pub producer: ProducerPin,
    pub definition_sha256: String,
    pub manifest_sha256: String,
    pub repositories: BTreeMap<String, Identity>,
    pub guardrails: BTreeMap<String, wire::Contract>,
}
impl IntakeContract {
    fn metadata(&self) -> Value {
        json!({"namespace":self.namespace,"guardrail_namespace":self.guardrail_namespace,
            "program_key":self.program_key,"source_owner_uid":self.source_owner_uid,"definition":super::definition_metadata(&self.definition),
            "producer":self.producer,"definition_sha256":self.definition_sha256,
            "manifest_sha256":self.manifest_sha256,"repositories":self.repositories,"guardrails":self.guardrails})
    }
    fn validate(&self) -> Result<()> {
        self.definition
            .validate()
            .map_err(|_| IntakeError::InvalidContract)?;
        for ns in [&self.namespace, &self.guardrail_namespace] {
            Scope::program(ns.clone(), self.program_key.clone())
                .map_err(|_| IntakeError::InvalidContract)?;
        }
        if self.definition.unit != Unit::Fraction
            || self.definition.reference.revision != DEFINITION
            || self.definition.reference.id.namespace != self.namespace
            || self.guardrail_namespace == self.namespace
            || !hex(&self.producer.source_commit, 40)
            || !hex(&self.producer.source_tree, 40)
            || !hex(&self.producer.binary_sha256, 64)
            || !hex(&self.producer.build_evidence_sha256, 64)
            || !hex(&self.definition_sha256, 64)
            || !hex(&self.manifest_sha256, 64)
            || self.repositories.is_empty()
            || self.repositories.len() > 32
            || self
                .guardrails
                .keys()
                .map(String::as_str)
                .ne(["billing", "defects", "disk", "recall"])
        {
            return Err(IntakeError::InvalidContract);
        }
        let mut ids = BTreeSet::new();
        let mut names = BTreeSet::new();
        for (key, r) in &self.repositories {
            if !text(key) || !r.valid() || !ids.insert(r.github_id) || !names.insert(&r.full_name) {
                return Err(IntakeError::InvalidContract);
            }
        }
        for (kind, c) in &self.guardrails {
            c.validate()?;
            if c.kind() != kind {
                return Err(IntakeError::InvalidContract);
            }
            guards::qualify_recording(None, &c.adopted(), 0)
                .map_err(|_| IntakeError::InvalidContract)?;
        }
        // The Lead's adopted policy is exact 15 GiB. Keep unresolved wire support
        // for parsing compatibility, without silently adopting it for this task.
        if !matches!(
            self.guardrails.get("disk"),
            Some(wire::Contract::Disk {
                minimum: wire::DiskMinimum::Exact {
                    quantity: 15,
                    unit: wire::DiskUnit::Gib
                }
            })
        ) {
            return Err(IntakeError::InvalidContract);
        }
        Ok(())
    }
}
fn hex(s: &str, n: usize) -> bool {
    s.len() == n
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn text(s: &str) -> bool {
    !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control)
}
fn closed_json(v: &Value) -> bool {
    match v {
        Value::Number(n) => n.as_u64().is_some(),
        Value::Array(a) => a.iter().all(closed_json),
        Value::Object(m) => {
            // serde accepts a null content field for an adjacent-tagged unit
            // variant. The v2 grammar permits only {kind} for these states.
            let shape = match m.get("kind").and_then(Value::as_str) {
                Some("unknown" | "not_performed") => m.len() == 1,
                Some("value") => m.len() == 2 && m.contains_key("value"),
                _ => true,
            };
            shape && m.values().all(closed_json)
        }
        _ => true,
    }
}
fn parse<T: DeserializeOwned>(bytes: &[u8], limit: usize) -> Result<T> {
    if bytes.len() > limit {
        return Err(IntakeError::Bounds);
    }
    let v = strict::parse(bytes)?;
    if !closed_json(&v) {
        return Err(IntakeError::InvalidData);
    }
    serde_json::from_value(v).map_err(|_| IntakeError::InvalidData)
}
fn value<T: Serialize>(v: &T) -> Result<Value> {
    serde_json::to_value(v).map_err(|_| IntakeError::InvalidData)
}
fn knowledge<T: Serialize>(v: &K<T>) -> Result<Knowledge> {
    serde_json::from_value(value(v)?).map_err(|_| IntakeError::InvalidData)
}
fn path_hash(paths: &BTreeSet<String>) -> String {
    let mut hash = Sha256::new();
    hash.update(b"kpi-v2-path-set-v1\0");
    for p in paths {
        hash.update((p.len() as u64).to_be_bytes());
        hash.update(p.as_bytes());
    }
    format!("{:x}", hash.finalize())
}
fn repository_id(r: &Identity) -> Result<Id> {
    let (owner, name) = r
        .full_name
        .split_once('/')
        .ok_or(IntakeError::InvalidContract)?;
    program_git::Repository::github(owner, name, r.github_id)
        .map(|r| r.id().clone())
        .map_err(|_| IntakeError::InvalidContract)
}
/// Artifact bytes indexed by measurement ID then validated basename. These are
/// explicit trusted recording inputs, never paths received from a query.
pub type Evidence = BTreeMap<String, BTreeMap<String, Vec<u8>>>;
#[derive(Clone)]
struct Row {
    snapshot: Snapshot,
    value: Value,
    line: usize,
    sha256: String,
    raw: String,
}
#[derive(Clone)]
pub struct Capture {
    envelope: Vec<u8>,
    hash: String,
    contract: IntakeContract,
    population: PathPopulation,
    rows: Vec<Row>,
    evidence: Evidence,
}
impl Capture {
    pub fn recorded(
        journal: &[u8],
        manifest: &[u8],
        evidence: Evidence,
        start: u64,
        end: u64,
        contract: IntakeContract,
    ) -> Result<Self> {
        contract.validate()?;
        if start > end || journal.last() != Some(&b'\n') {
            return Err(IntakeError::InvalidData);
        }
        if journal.is_empty()
            || journal.len() > JOURNAL_LIMIT
            || manifest.len() > super::MAX_MANIFEST
        {
            return Err(IntakeError::Bounds);
        }
        if digest(manifest) != contract.manifest_sha256 {
            return Err(IntakeError::InvalidContract);
        }
        let dummy = super::IntakeContract {
            namespace: contract.namespace.clone(),
            program_key: contract.program_key.clone(),
            legacy_definition: contract.definition.clone(),
            producer_sha256: digest(b""),
        };
        let (mut population, manifest_summary) = super::manifest_population(
            &strict::parse(manifest)?,
            &contract.manifest_sha256,
            &dummy,
        )?;
        let old = std::mem::take(&mut population.repositories);
        if old.values().flatten().any(|p| {
            p.starts_with('/') || p.split('/').any(|s| s.is_empty() || s == "." || s == "..")
        }) {
            return Err(IntakeError::InvalidData);
        }
        if old
            .keys()
            .map(|id| id.native_key.as_str())
            .ne(contract.repositories.keys().map(String::as_str))
        {
            return Err(IntakeError::InvalidContract);
        }
        for (key, r) in &contract.repositories {
            population.repositories.insert(
                repository_id(r)?,
                old.iter()
                    .find(|(id, _)| &id.native_key == key)
                    .ok_or(IntakeError::InvalidData)?
                    .1
                    .clone(),
            );
        }
        population.basis.adapter_revision = ADAPTER_REVISION.into();
        population
            .validate()
            .map_err(|_| IntakeError::InvalidData)?;
        let total_bytes = evidence
            .values()
            .flat_map(|m| m.values())
            .try_fold(0usize, |n, b| {
                n.checked_add(b.len()).ok_or(IntakeError::Bounds)
            })?;
        if total_bytes > TOTAL_LIMIT {
            return Err(IntakeError::Bounds);
        }
        let mut rows = Vec::new();
        let mut seen = BTreeMap::new();
        for (index, raw) in journal.split_inclusive(|b| *b == b'\n').enumerate() {
            if index >= super::MAX_ROWS {
                return Err(IntakeError::Bounds);
            }
            let row_value: Value = parse(raw, LINE_LIMIT)?;
            fn bounded_text(v: &Value) -> bool {
                match v {
                    Value::String(s) => s.len() <= 256,
                    Value::Object(m) => m.values().all(bounded_text),
                    Value::Array(a) => a.iter().all(bounded_text),
                    _ => true,
                }
            }
            if !bounded_text(&row_value) {
                return Err(IntakeError::InvalidData);
            }
            let snapshot: Snapshot =
                serde_json::from_value(row_value).map_err(|_| IntakeError::InvalidData)?;
            snapshot.validate()?;
            if snapshot.capture.end > start {
                return Err(IntakeError::InvalidData);
            }
            let bytes = evidence
                .get(&snapshot.measurement_id)
                .ok_or(IntakeError::InvalidData)?;
            verify_artifacts(&snapshot, bytes)?;
            verify_pins(&snapshot, bytes, manifest, &contract)?;
            verify_repositories(&snapshot, bytes, &population, &contract)?;
            verify_parity(&snapshot, bytes, &contract)?;
            for g in guardrails(&snapshot) {
                qualify_guardrail(g, bytes, &contract, snapshot.capture.end, true)?;
            }
            let hash = digest(raw);
            if seen
                .insert(snapshot.measurement_id.clone(), hash.clone())
                .is_some_and(|previous| previous != hash)
            {
                return Err(IntakeError::InvalidData);
            }
            rows.push(Row {
                value: parse(raw, LINE_LIMIT)?,
                snapshot,
                line: index + 1,
                sha256: hash,
                raw: std::str::from_utf8(raw)
                    .map_err(|_| IntakeError::InvalidData)?
                    .into(),
            });
        }
        if seen.keys().ne(evidence.keys()) {
            return Err(IntakeError::InvalidData);
        }
        let row_sources:Vec<_>=rows.iter().map(|r|json!({"measurement_id":r.snapshot.measurement_id,"line":r.line,"sha256":r.sha256,"raw_utf8":r.raw})).collect();
        let values: Vec<_> = rows.iter().map(|r| r.value.clone()).collect();
        let envelope=serde_json::to_vec(&json!({"schema":ADAPTER_REVISION,"source":{"snapshots_path":SNAPSHOTS_PATH,"snapshots_sha256":digest(journal),"manifest_path":super::MANIFEST_PATH,"manifest_sha256":digest(manifest),"capture_start":start,"capture_end":end},"pins":contract.metadata(),"manifest":manifest_summary,"rows":values,"row_sources":row_sources})).map_err(|_|IntakeError::InvalidData)?;
        Ok(Self {
            hash: digest(&envelope),
            envelope,
            contract,
            population,
            rows,
            evidence,
        })
    }
    pub fn envelope_bytes(&self) -> &[u8] {
        &self.envelope
    }
    pub fn envelope_sha256(&self) -> &str {
        &self.hash
    }
    pub fn population(&self) -> &PathPopulation {
        &self.population
    }
    /// Eligible (repository identity, commit, measurement id) triples only;
    /// no paths, artifacts or row bytes leave the capture through here.
    pub(crate) fn endpoint_bindings(&self) -> Vec<(Identity, String, String)> {
        // Latest capture end wins per (repository, commit); ties by measurement id.
        let mut chosen: BTreeMap<(u64, String, String), (u64, String)> = BTreeMap::new();
        for row in &self.rows {
            if row.snapshot.capture_status == "failed"
                || !row.snapshot.kpi_legacy_removal.source_qualified
            {
                continue;
            }
            for repo in &row.snapshot.repositories {
                if !repo.source_qualified {
                    continue;
                }
                if let Some(commit) = repo.commit_sha.get() {
                    let key = (
                        repo.identity.github_id,
                        repo.identity.full_name.clone(),
                        commit.clone(),
                    );
                    let candidate = (
                        row.snapshot.capture.end,
                        row.snapshot.measurement_id.clone(),
                    );
                    if chosen.get(&key).is_none_or(|old| candidate > *old) {
                        chosen.insert(key, candidate);
                    }
                }
            }
        }
        chosen
            .into_iter()
            .map(|((github_id, full_name, commit), (_, measurement_id))| {
                (
                    Identity {
                        github_id,
                        full_name,
                    },
                    commit,
                    measurement_id,
                )
            })
            .collect()
    }
    pub fn artifacts(&self) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::from([(self.hash.clone(), self.envelope.clone())]);
        for b in self.evidence.values().flat_map(|m| m.values()) {
            out.insert(digest(b), b.clone());
        }
        out
    }
}
pub(crate) fn measurement_object_id(
    namespace: &Namespace,
    measurement_id: &str,
    selector: &str,
) -> Id {
    Id {
        object_type: "measurement".into(),
        namespace: namespace.clone(),
        native_key: format!("v2:{measurement_id}:{selector}"),
    }
}
pub(crate) fn legacy_measurement_id(namespace: &Namespace, measurement_id: &str) -> Id {
    measurement_object_id(namespace, measurement_id, SELECTORS[0])
}
fn artifact<'a>(a: &ArtifactRef, bytes: &'a BTreeMap<String, Vec<u8>>) -> Result<&'a [u8]> {
    let b = bytes.get(&a.path).ok_or(IntakeError::InvalidData)?;
    if !a.valid() || a.bytes != b.len() as u64 || a.sha256 != digest(b) {
        return Err(IntakeError::InvalidData);
    }
    Ok(b)
}
fn verify_artifacts(s: &Snapshot, bytes: &BTreeMap<String, Vec<u8>>) -> Result<()> {
    if s.artifacts.len() != bytes.len()
        || bytes.values().map(Vec::len).sum::<usize>() > EVIDENCE_LIMIT
    {
        return Err(IntakeError::Bounds);
    }
    for a in &s.artifacts {
        artifact(a, bytes)?;
    }
    Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DefinitionContract {
    revision: String,
    algorithm_revision: String,
    manifest_sha256: String,
    repositories: Vec<Identity>,
    parity_tool_ids: BTreeSet<String>,
    billing: wire::Contract,
    disk: wire::Contract,
    recall: wire::Contract,
    defects: wire::Contract,
}
fn verify_pins(
    s: &Snapshot,
    b: &BTreeMap<String, Vec<u8>>,
    manifest: &[u8],
    c: &IntakeContract,
) -> Result<()> {
    let p = &s.producer;
    if p.source_commit != c.producer.source_commit
        || p.source_tree != c.producer.source_tree
        || p.binary_sha256 != c.producer.binary_sha256
        || p.build_evidence.sha256 != c.producer.build_evidence_sha256
        || s.definition.contract.sha256 != c.definition_sha256
        || s.definition.manifest.sha256 != c.manifest_sha256
        || s.definition.manifest.bytes != manifest.len() as u64
    {
        return Err(IntakeError::InvalidContract);
    }
    let build: BuildEvidence = parse(artifact(&p.build_evidence, b)?, LINE_LIMIT)?;
    build.validate()?;
    if build.source_commit != p.source_commit
        || build.source_tree != p.source_tree
        || build.binary_sha256 != p.binary_sha256
    {
        return Err(IntakeError::InvalidData);
    }
    let d: DefinitionContract = parse(artifact(&s.definition.contract, b)?, LINE_LIMIT)?;
    if d.revision != DEFINITION
        || d.algorithm_revision != ALGORITHM
        || d.manifest_sha256 != c.manifest_sha256
        || d.repositories != c.repositories.values().cloned().collect::<Vec<_>>()
        || d.parity_tool_ids.is_empty()
        || d.parity_tool_ids.len() > 256
        || d.parity_tool_ids.iter().any(|s| !text(s))
    {
        return Err(IntakeError::InvalidData);
    }
    for (kind, contract) in [
        ("billing", &d.billing),
        ("disk", &d.disk),
        ("recall", &d.recall),
        ("defects", &d.defects),
    ] {
        if value(contract)? != value(c.guardrails.get(kind).ok_or(IntakeError::InvalidContract)?)? {
            return Err(IntakeError::InvalidContract);
        }
    }
    let r: Request = parse(artifact(&s.definition.request, b)?, LINE_LIMIT)?;
    r.validate(s, c)?;
    for (input, g) in [
        (&r.billing_recording_path, &s.guard_ci_minutes_month),
        (&r.recall_recording_path, &s.guard_recall_regression),
        (&r.defects_recording_path, &s.guard_escaped_defects_week),
    ] {
        if matches!(input, RecordingInput::State(_))
            && g.qualification != Qualification::NotPerformed
        {
            return Err(IntakeError::InvalidData);
        }
    }
    if matches!(r.parity_recording_path, RecordingInput::State(_))
        && s.kpi_mcp_parity.qualification != Qualification::NotPerformed
    {
        return Err(IntakeError::InvalidData);
    }
    Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    schema_version: String,
    measurement_id: String,
    manifest_path: String,
    definition_contract_path: String,
    build_evidence_path: String,
    journal_path: String,
    evidence_root: String,
    repositories: Vec<RepositoryRequest>,
    billing_recording_path: RecordingInput,
    parity_recording_path: RecordingInput,
    recall_recording_path: RecordingInput,
    defects_recording_path: RecordingInput,
    disk_target: String,
    limits: Limits,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepositoryRequest {
    manifest_key: String,
    github_id: u64,
    full_name: String,
    local_root: K<String>,
    backend: String,
}
#[derive(Deserialize)]
#[serde(untagged)]
enum RecordingInput {
    Path(String),
    State(Unattempted),
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Unattempted {
    kind: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Limits {
    process_timeout_seconds: u32,
    capture_timeout_seconds: u32,
    process_output_bytes: u64,
    evidence_bytes: u64,
}
fn absolute(s: &str) -> bool {
    s.starts_with('/') && !s.contains('\0') && !s.split('/').any(|s| s == ".." || s == ".")
}
impl Request {
    fn validate(&self, s: &Snapshot, c: &IntakeContract) -> Result<()> {
        if self.schema_version != "ws-replan-kpi-capture-request/v2"
            || self.measurement_id != s.measurement_id
            || self.manifest_path != super::MANIFEST_PATH
            || self.journal_path != SNAPSHOTS_PATH
            || self.evidence_root != EVIDENCE_ROOT
            || self.disk_target != "/"
            || !absolute(&self.definition_contract_path)
            || !absolute(&self.build_evidence_path)
            || self.repositories.len() != c.repositories.len()
            || !(1..=60).contains(&self.limits.process_timeout_seconds)
            || !(1..=300).contains(&self.limits.capture_timeout_seconds)
            || !(1..=16 * 1024 * 1024).contains(&self.limits.process_output_bytes)
            || !(1..=32 * 1024 * 1024).contains(&self.limits.evidence_bytes)
            || self
                .repositories
                .iter()
                .map(|r| r.manifest_key.as_str())
                .ne(c.repositories.keys().map(String::as_str))
        {
            return Err(IntakeError::InvalidData);
        }
        for r in &self.repositories {
            let adopted = &c.repositories[&r.manifest_key];
            if r.github_id != adopted.github_id
                || r.full_name != adopted.full_name
                || !matches!(r.backend.as_str(), "api_only" | "local_then_api")
                || r.local_root.get().is_some_and(|p| !absolute(p))
            {
                return Err(IntakeError::InvalidData);
            }
        }
        for r in [
            &self.billing_recording_path,
            &self.parity_recording_path,
            &self.recall_recording_path,
            &self.defects_recording_path,
        ] {
            match r {
                RecordingInput::Path(p) if absolute(p) => {}
                RecordingInput::State(s) if s.kind == "not_performed" => {}
                _ => return Err(IntakeError::InvalidData),
            }
        }
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RefEvidence {
    identity: Identity,
    #[serde(rename = "ref")]
    reference: String,
    commit_sha: String,
    identity_response_sha256: String,
    ref_response_sha256: String,
    capture: Interval,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TreeEvidence {
    identity: Identity,
    commit_sha: String,
    tree_sha: String,
    backend: String,
    capture: Interval,
    commit_response_sha256: String,
    tree_response_sha256: Option<String>,
    leaf_path_set_sha256: String,
    absent_path_set_sha256: String,
    complete: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Membership {
    identity: Identity,
    commit_sha: String,
    tree_sha: String,
    members: Vec<Member>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Member {
    path_sha256: String,
    present: bool,
}
fn verify_repositories(
    s: &Snapshot,
    b: &BTreeMap<String, Vec<u8>>,
    population: &PathPopulation,
    c: &IntakeContract,
) -> Result<()> {
    if s.repositories
        .iter()
        .map(|r| r.manifest_key.as_str())
        .ne(c.repositories.keys().map(String::as_str))
    {
        return Err(IntakeError::InvalidData);
    }
    let mut total = 0u64;
    let mut removed = 0u64;
    let mut all = true;
    let mut earliest = u64::MAX;
    for r in &s.repositories {
        if c.repositories[&r.manifest_key] != r.identity {
            return Err(IntakeError::InvalidData);
        }
        let paths = &population.repositories[&repository_id(&r.identity)?];
        if c.repositories[&r.manifest_key] != r.identity
            || r.population.total != paths.len() as u64
            || r.population.path_set_sha256 != path_hash(paths)
        {
            return Err(IntakeError::InvalidData);
        }
        let complete = r.ref_read.status == Status::Captured
            && r.tree_read.status == Status::Captured
            && r.membership.get().is_some();
        let eligible = complete && s.capture.end - r.ref_read.capture.end <= 900;
        if r.source_qualified != eligible {
            return Err(IntakeError::InvalidData);
        }
        if complete {
            if !matches!(r.backend.as_str(), "local_git" | "github_api")
                || r.ref_read.reason != "none"
                || r.tree_read.reason != "none"
                || r.ref_read.exit_code != K::Value(0)
                || r.tree_read.exit_code != K::Value(0)
                || !matches!(r.ref_read.signal, K::Unknown)
                || !matches!(r.tree_read.signal, K::Unknown)
            {
                return Err(IntakeError::InvalidData);
            }
            let commit = r.commit_sha.get().ok_or(IntakeError::InvalidData)?;
            let tree = r.tree_sha.get().ok_or(IntakeError::InvalidData)?;
            let reference: RefEvidence = parse(
                artifact(
                    r.ref_read.artifact.get().ok_or(IntakeError::InvalidData)?,
                    b,
                )?,
                LINE_LIMIT,
            )?;
            let tree_e: TreeEvidence = parse(
                artifact(
                    r.tree_read.artifact.get().ok_or(IntakeError::InvalidData)?,
                    b,
                )?,
                LINE_LIMIT,
            )?;
            let membership: Membership = parse(
                artifact(r.membership.get().ok_or(IntakeError::InvalidData)?, b)?,
                EVIDENCE_LIMIT,
            )?;
            if reference.identity != r.identity
                || reference.reference != r.ref_name
                || reference.commit_sha != *commit
                || reference.capture != r.ref_read.capture
                || !hex(&reference.identity_response_sha256, 64)
                || !hex(&reference.ref_response_sha256, 64)
                || r.ref_read.observed_at != K::Value(r.ref_read.capture.end)
                || r.tree_read.observed_at != r.ref_read.observed_at
                || r.tree_read.capture.start < r.ref_read.capture.end
                || tree_e.identity != r.identity
                || tree_e.commit_sha != *commit
                || tree_e.tree_sha != *tree
                || tree_e.backend != r.backend
                || tree_e.capture != r.tree_read.capture
                || !tree_e.complete
                || !hex(&tree_e.commit_response_sha256, 64)
                || tree_e
                    .tree_response_sha256
                    .as_ref()
                    .is_some_and(|h| !hex(h, 64))
                || (r.backend == "github_api" && tree_e.tree_response_sha256.is_none())
                || r.leaf_path_set_sha256 != K::Value(tree_e.leaf_path_set_sha256)
                || r.absent_path_set_sha256 != K::Value(tree_e.absent_path_set_sha256.clone())
                || membership.identity != r.identity
                || membership.commit_sha != *commit
                || membership.tree_sha != *tree
            {
                return Err(IntakeError::InvalidData);
            }
            let path_by_hash: BTreeMap<_, _> =
                paths.iter().map(|p| (digest(p.as_bytes()), p)).collect();
            if membership.members.len() != paths.len()
                || membership
                    .members
                    .windows(2)
                    .any(|w| w[0].path_sha256 >= w[1].path_sha256)
            {
                return Err(IntakeError::InvalidData);
            }
            let mut absent = BTreeSet::new();
            for member in membership.members {
                let path = path_by_hash
                    .get(&member.path_sha256)
                    .ok_or(IntakeError::InvalidData)?;
                if !member.present {
                    absent.insert((*path).clone());
                }
            }
            if path_hash(&absent) != tree_e.absent_path_set_sha256
                || (eligible && r.removed != K::Value(absent.len() as u64))
            {
                return Err(IntakeError::InvalidData);
            }
            if eligible {
                removed = removed
                    .checked_add(absent.len() as u64)
                    .ok_or(IntakeError::Bounds)?;
                earliest = earliest.min(r.ref_read.capture.end);
            }
        }
        total = total
            .checked_add(paths.len() as u64)
            .ok_or(IntakeError::Bounds)?;
        all &= eligible;
    }
    let aggregate = &s.kpi_legacy_removal;
    if aggregate.source_qualified != all
        || (all
            && (aggregate.source.observed_at != K::Value(earliest)
                || aggregate.source.status != Status::Captured
                || aggregate.source.reason != "none"))
    {
        return Err(IntakeError::InvalidData);
    }
    if all {
        let v = aggregate.value.get().ok_or(IntakeError::InvalidData)?;
        if v.removed != removed || v.total != total {
            return Err(IntakeError::InvalidData);
        }
    } else if aggregate.qualification != Qualification::IncompleteSource
        || aggregate.source.status != Status::Incomplete
        || aggregate.source.reason != "incomplete_repository_population"
        || !matches!(aggregate.value, K::Unknown)
    {
        return Err(IntakeError::InvalidData);
    }
    Ok(())
}
fn guardrails(s: &Snapshot) -> [&GuardrailSample; 4] {
    [
        &s.guard_ci_minutes_month,
        &s.guard_disk_free_gib,
        &s.guard_escaped_defects_week,
        &s.guard_recall_regression,
    ]
}
fn qualify_guardrail(
    g: &GuardrailSample,
    b: &BTreeMap<String, Vec<u8>>,
    c: &IntakeContract,
    now: u64,
    cross_check: bool,
) -> Result<guards::QualifiedCapture> {
    let trusted = &c.guardrails[&g.kind];
    let contract: wire::Contract = parse(artifact(&g.contract, b)?, LINE_LIMIT)?;
    if value(&contract)? != value(trusted)? || g.source.artifact != g.recording {
        return Err(IntakeError::InvalidContract);
    }
    let recording = g.recording.get().map(|a| artifact(a, b)).transpose()?;
    if let Some(bytes) = recording {
        let v: Value = parse(bytes, 1024 * 1024)?;
        // The producer deliberately retains {} as its sanitized invalid sentinel.
        if v != json!({}) {
            let _: wire::Recording =
                serde_json::from_value(v).map_err(|_| IntakeError::InvalidData)?;
        }
    }
    let qualified = guards::qualify_recording(recording, &trusted.adopted(), now)
        .map_err(|_| IntakeError::InvalidData)?;
    if cross_check {
        let o = qualified.observation();
        if o.fields.get("value") != Some(&knowledge(&g.value)?)
            || o.fields.get("source_qualified")
                != Some(&Knowledge::Value(g.source_qualified.into()))
            || o.fields.get("qualification") != Some(&Knowledge::Value(value(&g.qualification)?))
            || (g.kind == "recall" && o.fields.get("total") != Some(&knowledge(&g.total)?))
            || g.source
                .observed_at
                .get()
                .is_some_and(|t| *t != o.observed_at)
        {
            return Err(IntakeError::InvalidData);
        }
        let expected = match g.qualification {
            Qualification::NotPerformed => Status::NotPerformed,
            Qualification::MissingSource => Status::Missing,
            Qualification::FailedSource => Status::Failed,
            Qualification::InvalidSource => Status::Invalid,
            Qualification::IncompleteSource => Status::Incomplete,
            _ => Status::Captured,
        };
        let reason = if g.qualification == Qualification::Qualified {
            "none".to_string()
        } else {
            value(&g.qualification)?
                .as_str()
                .ok_or(IntakeError::InvalidData)?
                .into()
        };
        if g.source.status != expected || g.source.reason != reason {
            return Err(IntakeError::InvalidData);
        }
    }
    Ok(qualified)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolRecording {
    source_revision: String,
    observed_at: u64,
    population_complete: bool,
    tools: Vec<ToolStatus>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolStatus {
    id: String,
    bounded_implemented: bool,
    production_qualified: bool,
    parity: K<ArtifactRef>,
    synthetic_tests: K<ArtifactRef>,
    independent_review: K<ArtifactRef>,
    ci: K<ArtifactRef>,
    rollback: K<ArtifactRef>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Gate {
    gate: String,
    candidate_commit: String,
    passed: bool,
    owner_revision: String,
}
fn verify_parity(s: &Snapshot, b: &BTreeMap<String, Vec<u8>>, _c: &IntakeContract) -> Result<()> {
    let p = &s.kpi_mcp_parity;
    let Some(a) = p.source.artifact.get() else {
        let expected = match (&p.source.status, &p.qualification) {
            (Status::NotPerformed, Qualification::NotPerformed) => {
                Some((Knowledge::NotPerformed, "not_performed"))
            }
            (Status::Missing, Qualification::MissingSource) => {
                Some((Knowledge::Unknown, "missing_source"))
            }
            (Status::Failed, Qualification::FailedSource) => {
                Some((Knowledge::Unknown, "failed_source"))
            }
            (Status::Invalid, Qualification::InvalidSource) => {
                Some((Knowledge::Unknown, "invalid_source"))
            }
            (Status::Incomplete, Qualification::IncompleteSource) => {
                Some((Knowledge::Unknown, "incomplete_source"))
            }
            _ => None,
        };
        let Some((expected_value, reason)) = expected else {
            return Err(IntakeError::InvalidData);
        };
        if p.source_qualified
            || knowledge(&p.value)? != expected_value
            || p.source.reason != reason
            || (p.source.status == Status::NotPerformed
                && (matches!(p.source.exit_code, K::Value(_))
                    || matches!(p.source.signal, K::Value(_))))
        {
            return Err(IntakeError::InvalidData);
        }
        return Ok(());
    };
    let d: DefinitionContract = parse(artifact(&s.definition.contract, b)?, LINE_LIMIT)?;
    let r: ToolRecording = parse(artifact(a, b)?, 1024 * 1024)?;
    if p.source.status != Status::Captured || p.source.reason != "none" {
        return Err(IntakeError::InvalidData);
    }
    let ids: BTreeSet<_> = r.tools.iter().map(|t| t.id.clone()).collect();
    if !text(&r.source_revision)
        || !r.population_complete
        || ids != d.parity_tool_ids
        || ids.len() != r.tools.len()
        || p.source.observed_at != K::Value(r.observed_at)
    {
        return Err(IntakeError::InvalidData);
    }
    for t in &r.tools {
        let mut candidate = None;
        for (kind, ref_) in [
            ("parity", &t.parity),
            ("synthetic_tests", &t.synthetic_tests),
            ("independent_review", &t.independent_review),
            ("ci", &t.ci),
            ("rollback", &t.rollback),
        ] {
            if let Some(a) = ref_.get() {
                if !s.artifacts.contains(a) {
                    return Err(IntakeError::InvalidData);
                }
                let g: Gate = parse(artifact(a, b)?, LINE_LIMIT)?;
                if g.gate != kind || !hex(&g.candidate_commit, 40) || !text(&g.owner_revision) {
                    return Err(IntakeError::InvalidData);
                }
                if t.production_qualified {
                    if !g.passed || candidate.as_ref().is_some_and(|c| c != &g.candidate_commit) {
                        return Err(IntakeError::InvalidData);
                    }
                    candidate = Some(g.candidate_commit);
                }
            } else if t.production_qualified {
                return Err(IntakeError::InvalidData);
            }
        }
    }
    let reason = if r.observed_at > s.capture.end {
        Qualification::FutureSource
    } else if s.capture.end - r.observed_at > 3600 {
        Qualification::StaleSource
    } else {
        Qualification::Qualified
    };
    if p.qualification != reason || p.source_qualified != (reason == Qualification::Qualified) {
        return Err(IntakeError::InvalidData);
    }
    if let Some(v) = p.value.get()
        && (v.qualified != r.tools.iter().filter(|t| t.production_qualified).count() as u64
            || v.total != r.tools.len() as u64
            || v.bounded_implemented
                != K::Value(r.tools.iter().filter(|t| t.bounded_implemented).count() as u64)
            || v.tool_set_sha256 != path_hash(&ids)
            || v.result_revision != r.source_revision)
    {
        return Err(IntakeError::InvalidData);
    }
    Ok(())
}
/// Fixed Linux journal and measurement-relative evidence; no arbitrary read API.
#[cfg(target_os = "linux")]
pub fn capture_fixed(contract: IntakeContract, mut clock: impl FnMut() -> u64) -> Result<Capture> {
    let start = clock();
    let journal = source::read_journal(contract.source_owner_uid)?;
    let manifest = super::bounded_file(super::MANIFEST_PATH, super::MAX_MANIFEST)?;
    let mut evidence = Evidence::new();
    let mut total = 0usize;
    for (i, row) in journal.split_inclusive(|b| *b == b'\n').enumerate() {
        if i >= super::MAX_ROWS {
            return Err(IntakeError::Bounds);
        }
        let s: Snapshot = parse(row, LINE_LIMIT)?;
        s.validate()?;
        if evidence.contains_key(&s.measurement_id) {
            continue;
        }
        let bytes =
            source::read_evidence(&s.measurement_id, &s.artifacts, contract.source_owner_uid)?;
        total = total
            .checked_add(bytes.values().map(Vec::len).sum::<usize>())
            .ok_or(IntakeError::Bounds)?;
        if total > TOTAL_LIMIT {
            return Err(IntakeError::Bounds);
        }
        evidence.insert(s.measurement_id, bytes);
    }
    Capture::recorded(&journal, &manifest, evidence, start, clock(), contract)
}

#[cfg(not(target_os = "linux"))]
pub fn capture_fixed(_contract: IntakeContract, _clock: impl FnMut() -> u64) -> Result<Capture> {
    Err(IntakeError::SourceUnavailable)
}

pub(crate) struct Graph {
    pub objects: Vec<Object>,
    pub links: Vec<Link>,
    pub artifacts: BTreeMap<String, Vec<u8>>,
}
fn edge(source: Id, relation: &str, target: Id) -> Link {
    Link {
        source,
        relation: relation.into(),
        target,
    }
}
fn object(id: Id, ns: &Namespace, key: &str, o: Observation) -> Result<Object> {
    Ok(Object {
        id,
        scope: Scope::program(ns.clone(), key).map_err(|_| IntakeError::InvalidContract)?,
        head: o.revision.clone(),
        observations: vec![o],
    })
}
fn sample_observation(
    row: &Row,
    selector: &str,
    index: usize,
    hash: &str,
    fields: BTreeMap<String, Knowledge>,
    quality: (u64, Coverage, u64),
) -> Result<Observation> {
    let (at, coverage, budget) = quality;
    Ok(Observation {
        revision: row.sha256.clone(),
        source_revision: row.snapshot.producer.binary_sha256.clone(),
        observed_at: at,
        fresh_until: at.checked_add(budget).ok_or(IntakeError::InvalidData)?,
        change: Change::Initial,
        supersedes: None,
        coverage,
        fields,
        lineage: Lineage {
            artifact_sha256: hash.into(),
            pointer: format!("/rows/{index}/{selector}/value"),
        },
    })
}
fn disclosure_envelope(
    graph: &mut Graph,
    observation: &mut Observation,
    selector: &str,
    fields: Value,
) -> Result<()> {
    // Exact journal bytes/digest live only in the program_source record. A
    // consumer allowed one metric must not observe a hidden sibling's digest.
    let selected = json!({"schema":ADAPTER_REVISION,"rows":[{selector:{"value":fields}}]});
    let bytes = serde_json::to_vec(&selected).map_err(|_| IntakeError::InvalidData)?;
    let hash = digest(&bytes);
    graph.artifacts.insert(hash.clone(), bytes);
    observation.revision = hash.clone();
    observation.lineage = Lineage {
        artifact_sha256: hash,
        pointer: format!("/rows/0/{selector}/value"),
    };
    Ok(())
}
impl Capture {
    pub(crate) fn selected_graph(&self, window: ReportWindow, now: u64) -> Result<Graph> {
        if window.start > window.end {
            return Err(IntakeError::InvalidData);
        }
        let c = &self.contract;
        let root = Id {
            object_type: "program".into(),
            namespace: c.namespace.clone(),
            native_key: c.program_key.clone(),
        };
        let mut graph = Graph {
            objects: Vec::new(),
            links: Vec::new(),
            artifacts: self.artifacts(),
        };
        let identity = json!({"program_key":c.program_key});
        let bytes = serde_json::to_vec(&identity).map_err(|_| IntakeError::InvalidData)?;
        let hash = digest(&bytes);
        graph.artifacts.insert(hash.clone(), bytes);
        let base = Observation {
            revision: hash.clone(),
            source_revision: ADAPTER_REVISION.into(),
            observed_at: 0,
            fresh_until: u64::MAX,
            change: Change::Initial,
            supersedes: None,
            coverage: Coverage::Complete,
            fields: BTreeMap::from([(
                "program_key".into(),
                Knowledge::Value(c.program_key.clone().into()),
            )]),
            lineage: Lineage {
                artifact_sha256: hash,
                pointer: "".into(),
            },
        };
        graph.objects.push(object(
            root.clone(),
            &c.namespace,
            &c.program_key,
            base.clone(),
        )?);
        for selector in SELECTORS {
            let metric = Id {
                object_type: "metric".into(),
                namespace: c.namespace.clone(),
                native_key: format!("v2:{selector}"),
            };
            let adopted =
                selector == SELECTORS[0] && matches!(c.definition.role, MetricRole::Kpi { .. });
            let mut o = base.clone();
            o.fields = BTreeMap::from([
                ("selector".into(), Knowledge::Value(selector.into())),
                (
                    "role".into(),
                    Knowledge::Value(if adopted { "kpi" } else { "indicator" }.into()),
                ),
                ("source_unit".into(), Knowledge::Value("fraction".into())),
            ]);
            if selector == SELECTORS[0] {
                o.fields.insert(
                    "definition".into(),
                    Knowledge::Value(super::definition_metadata(&c.definition)),
                );
            }
            let bytes = serde_json::to_vec(&json!({"fields":o.fields}))
                .map_err(|_| IntakeError::InvalidData)?;
            let hash = digest(&bytes);
            graph.artifacts.insert(hash.clone(), bytes);
            o.revision = hash.clone();
            o.lineage = Lineage {
                artifact_sha256: hash,
                pointer: "/fields".into(),
            };
            graph
                .objects
                .push(object(metric.clone(), &c.namespace, &c.program_key, o)?);
            graph.links.push(edge(
                root.clone(),
                if adopted {
                    "adopts_kpi"
                } else {
                    "tracks_indicator"
                },
                metric.clone(),
            ));
            let mut seen = BTreeSet::new();
            for (index, r) in self.rows.iter().enumerate() {
                if !seen.insert(&r.snapshot.measurement_id) {
                    continue;
                }
                let (v, q, qualification, source, budget) = if selector == SELECTORS[0] {
                    let s = &r.snapshot.kpi_legacy_removal;
                    (
                        value(&s.value)?,
                        s.source_qualified,
                        &s.qualification,
                        &s.source,
                        900,
                    )
                } else {
                    let s = &r.snapshot.kpi_mcp_parity;
                    (
                        value(&s.value)?,
                        s.source_qualified,
                        &s.qualification,
                        &s.source,
                        3600,
                    )
                };
                let at = source
                    .observed_at
                    .get()
                    .copied()
                    .unwrap_or(r.snapshot.capture.end);
                if !(window.start..=window.end).contains(&at) {
                    continue;
                }
                // Qualified at observation does not decay with render age;
                // freshness lives only in observed_at/fresh_until.
                let qualified = q && at <= now;
                let mut fields = BTreeMap::from([
                    (
                        "value".into(),
                        if qualified {
                            serde_json::from_value(v.clone())
                                .map_err(|_| IntakeError::InvalidData)?
                        } else if v.get("kind").and_then(Value::as_str) == Some("not_performed") {
                            Knowledge::NotPerformed
                        } else {
                            Knowledge::Unknown
                        },
                    ),
                    (
                        "source_qualified".into(),
                        Knowledge::Value(qualified.into()),
                    ),
                    (
                        "qualification".into(),
                        Knowledge::Value(if q && at > now {
                            "future_source".into()
                        } else {
                            value(qualification)?
                        }),
                    ),
                    ("source_unit".into(), Knowledge::Value("fraction".into())),
                ]);
                if qualified {
                    let obj = v
                        .get("value")
                        .and_then(Value::as_object)
                        .ok_or(IntakeError::InvalidData)?;
                    for (key, v) in obj {
                        fields.insert(key.clone(), Knowledge::Value(v.clone()));
                    }
                    fields.insert(
                        "value".into(),
                        Knowledge::Value(
                            obj.get(if selector == SELECTORS[0] {
                                "removed"
                            } else {
                                "qualified"
                            })
                            .ok_or(IntakeError::InvalidData)?
                            .clone(),
                        ),
                    );
                } else {
                    let state = fields["value"].clone();
                    fields.insert("total".into(), state.clone());
                    fields.insert(
                        if selector == SELECTORS[0] {
                            "removed"
                        } else {
                            "qualified"
                        }
                        .into(),
                        state,
                    );
                }
                if selector == SELECTORS[0] {
                    fields.insert("algorithm".into(), Knowledge::Value(ALGORITHM.into()));
                    fields.insert(
                        "definition_revision".into(),
                        Knowledge::Value(DEFINITION.into()),
                    );
                    fields.insert(
                        "manifest_sha256".into(),
                        Knowledge::Value(c.manifest_sha256.clone().into()),
                    );
                    fields.insert(
                        "repository_pins".into(),
                        Knowledge::Value(Value::Array(r.snapshot.repositories.iter().map(|repo|json!({"identity":repo.identity,"commit_sha":repo.commit_sha,"tree_sha":repo.tree_sha})).collect())),
                    );
                }
                let disclosed = json!({"measurement_id":r.snapshot.measurement_id,"source_revision":r.snapshot.producer.binary_sha256,"observed_at":at,"fields":fields});
                let mut o = sample_observation(
                    r,
                    selector,
                    index,
                    &self.hash,
                    fields,
                    (
                        at,
                        if qualified {
                            Coverage::Complete
                        } else {
                            Coverage::Partial
                        },
                        budget,
                    ),
                )?;
                disclosure_envelope(&mut graph, &mut o, selector, disclosed)?;
                let id = measurement_object_id(&c.namespace, &r.snapshot.measurement_id, selector);
                graph
                    .links
                    .push(edge(metric.clone(), "has_measurement", id.clone()));
                graph
                    .objects
                    .push(object(id, &c.namespace, &c.program_key, o)?);
            }
        }
        let mut seen = BTreeSet::new();
        for (index, r) in self.rows.iter().enumerate() {
            if !seen.insert(&r.snapshot.measurement_id) {
                continue;
            }
            let bytes = &self.evidence[&r.snapshot.measurement_id];
            for g in guardrails(&r.snapshot) {
                let q = qualify_guardrail(g, bytes, c, now, false)?;
                let id = Id {
                    object_type: "guardrail".into(),
                    namespace: c.guardrail_namespace.clone(),
                    native_key: format!("v2:{}:{}", r.snapshot.measurement_id, g.kind),
                };
                let mut object = q
                    .object(
                        id.clone(),
                        Scope::program(c.guardrail_namespace.clone(), c.program_key.clone())
                            .map_err(|_| IntakeError::InvalidContract)?,
                    )
                    .map_err(|_| IntakeError::InvalidData)?;
                let observation = &mut object.observations[0];
                let mut envelope = q.envelope_bytes().to_vec();
                // PR5 has no source observation time for these diagnostics.
                // Index their availability by the retained source capture end,
                // without refreshing or qualifying the original source value.
                if observation.observed_at == 0
                    && matches!(
                        observation.fields.get("qualification"),
                        Some(Knowledge::Value(Value::String(reason)))
                        if matches!(reason.as_str(), "not_performed" | "failed_source" | "missing_source" | "invalid_source" | "future_source")
                    )
                {
                    observation.observed_at = g.source.capture.end;
                    observation.fresh_until = g.source.capture.end;
                    let mut retained: Value =
                        serde_json::from_slice(&envelope).map_err(|_| IntakeError::InvalidData)?;
                    retained["v2_reporting_time"] = json!({
                        "observed_at": observation.observed_at,
                        "source_capture": g.source.capture,
                    });
                    envelope =
                        serde_json::to_vec(&retained).map_err(|_| IntakeError::InvalidData)?;
                    observation.revision = digest(&envelope);
                    observation.lineage.artifact_sha256 = observation.revision.clone();
                    object.head = observation.revision.clone();
                }
                if !(window.start..=window.end).contains(&observation.observed_at) {
                    continue;
                }
                graph.artifacts.insert(digest(&envelope), envelope);
                graph.objects.push(object);
                graph.links.push(edge(root.clone(), "guardrail", id));
            }
            // Retained producer assertions are restricted source-only records.
            let id = Id {
                object_type: "program_source".into(),
                namespace: c.guardrail_namespace.clone(),
                native_key: format!("v2:{}", r.snapshot.measurement_id),
            };
            let o = Observation {
                revision: r.sha256.clone(),
                source_revision: ADAPTER_REVISION.into(),
                observed_at: r.snapshot.capture.end,
                fresh_until: r.snapshot.capture.end,
                change: Change::Initial,
                supersedes: None,
                coverage: Coverage::Partial,
                fields: BTreeMap::from([(
                    "capture_status".into(),
                    Knowledge::Value(r.snapshot.capture_status.clone().into()),
                )]),
                lineage: Lineage {
                    artifact_sha256: self.hash.clone(),
                    pointer: format!("/rows/{index}"),
                },
            };
            graph
                .objects
                .push(object(id, &c.guardrail_namespace, &c.program_key, o)?);
            // Each qualified repository component keeps its own identity/grant,
            // even when another repository makes the aggregate Unknown.
            for repo in &r.snapshot.repositories {
                if !repo.source_qualified {
                    continue;
                }
                let repository = repository_id(&repo.identity)?;
                let at = repo.ref_read.capture.end;
                if !(window.start..=window.end).contains(&at) {
                    continue;
                }
                let current = at <= now && now - at <= 900;
                let mut o = sample_observation(
                    r,
                    "kpi_legacy_removal",
                    index,
                    &self.hash,
                    BTreeMap::from([
                        (
                            "removed".into(),
                            if current {
                                knowledge(&repo.removed)?
                            } else {
                                Knowledge::Unknown
                            },
                        ),
                        (
                            "total".into(),
                            Knowledge::Value(repo.population.total.into()),
                        ),
                        ("source_qualified".into(), Knowledge::Value(current.into())),
                        ("commit_sha".into(), knowledge(&repo.commit_sha)?),
                        ("tree_sha".into(), knowledge(&repo.tree_sha)?),
                    ]),
                    (
                        at,
                        if current {
                            Coverage::Complete
                        } else {
                            Coverage::Partial
                        },
                        900,
                    ),
                )?;
                let disclosed = json!({"measurement_id":r.snapshot.measurement_id,"repository":repo.identity,"observed_at":at,"fields":o.fields});
                disclosure_envelope(&mut graph, &mut o, "repository", disclosed)?;
                let id = Id {
                    object_type: "measurement".into(),
                    namespace: repository.namespace.clone(),
                    native_key: format!(
                        "v2:{}:{}",
                        r.snapshot.measurement_id, repo.identity.github_id
                    ),
                };
                graph.objects.push(object(
                    id.clone(),
                    &repository.namespace,
                    &c.program_key,
                    o,
                )?);
                graph.links.push(edge(root.clone(), "has_measurement", id));
            }
        }
        Ok(graph)
    }
}
/// Rebuild disposable graphs only after a whole capture and ID registry validate.
/// Identical IDs/bytes keep first lineage. Conflicts refuse without mutation.
pub struct Projection {
    store: Store,
    captures: Vec<Capture>,
    ids: BTreeMap<String, String>,
    contract: Value,
    now: u64,
}
impl Projection {
    pub fn new(capture: &Capture, now: u64) -> Result<Self> {
        let mut p = Self {
            store: Store::memory_program().map_err(|_| IntakeError::Store)?,
            captures: Vec::new(),
            ids: BTreeMap::new(),
            contract: capture.contract.metadata(),
            now,
        };
        p.ingest(capture)?;
        Ok(p)
    }
    pub fn ingest(&mut self, capture: &Capture) -> Result<Replay> {
        if capture.contract.metadata() != self.contract {
            return Err(IntakeError::InvalidContract);
        }
        let mut ids = self.ids.clone();
        let mut added = false;
        for r in &capture.rows {
            if let Some(old) = ids.insert(r.snapshot.measurement_id.clone(), r.sha256.clone()) {
                if old != r.sha256 {
                    return Err(IntakeError::InvalidData);
                }
            } else {
                added = true;
            }
        }
        if !added {
            return Ok(Replay::Unchanged);
        }
        let mut captures = self.captures.clone();
        captures.push(capture.clone());
        let mut objects = BTreeMap::new();
        let mut links = BTreeSet::new();
        for c in &captures {
            let g = c.selected_graph(
                ReportWindow {
                    start: 0,
                    end: u64::MAX,
                },
                self.now,
            )?;
            for o in g.objects {
                objects.entry(o.id.clone()).or_insert(o);
            }
            for l in g.links {
                links.insert((l.source, l.relation, l.target));
            }
        }
        let mut store = Store::memory_program().map_err(|_| IntakeError::Store)?;
        store
            .seed(
                &objects.into_values().collect::<Vec<_>>(),
                &links
                    .into_iter()
                    .map(|(source, relation, target)| Link {
                        source,
                        relation,
                        target,
                    })
                    .collect::<Vec<_>>(),
            )
            .map_err(|_| IntakeError::Store)?;
        self.store = store;
        self.ids = ids;
        self.captures = captures;
        Ok(Replay::Added)
    }
    pub fn into_store(self) -> Store {
        self.store
    }
}
