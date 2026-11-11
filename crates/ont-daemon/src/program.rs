//! Trusted, bounded snapshot/manifest intake (DESIGN §8 row 3).
//!
//! This is not a request protocol or an authenticated reader. The embedding
//! owner supplies the source contract and clock. Only `Service::render` publishes
//! objects to consumers. No source command, Git, network, refresh or action runs.
//! Retain `Capture::envelope_bytes()` before discarding a disposable projection.
//! Raw manifest paths are never copied into that envelope or the Store.
use ont_core::program::{MetricDefinition, PathAlgorithm, PathBasis, PathPopulation, Unit};
use ont_core::{
    Change, Coverage, Id, Knowledge, Lineage, Link, Namespace, Object, Observation, Scope,
};
use ont_store::Store;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
#[cfg(any(target_os = "linux", test))]
use std::io::Read;

#[cfg(target_os = "linux")]
pub(crate) mod source;
#[cfg(all(test, target_os = "linux"))]
mod source_tests;
pub(crate) mod strict;
#[cfg(test)]
mod tests;
pub mod v2;

pub const SNAPSHOTS_PATH: &str = "/home/palantirkc/work/ws-replan/preparation/kpi/snapshots.jsonl";
pub const MANIFEST_PATH: &str =
    "/home/palantirkc/work/ws-replan/preparation/plan-a-v1/MANIFEST.json";
pub const PRODUCER_PATH: &str = "/home/palantirkc/work/ws-replan/preparation/kpi/measure.py";
pub const ADAPTER_REVISION: &str = "program-snapshot-intake/v1";
const MAX_SNAPSHOTS: usize = 4 * 1024 * 1024;
const MAX_MANIFEST: usize = 8 * 1024 * 1024;
const MAX_PRODUCER: usize = 64 * 1024;
const MAX_LINE: usize = 64 * 1024;
const MAX_ROWS: usize = 4096;
const SELECTORS: [&str; 6] = [
    "kpi_legacy_removal",
    "kpi_mcp_parity",
    "guard_ci_minutes_month",
    "guard_disk_free_gb",
    "guard_escaped_defects_week",
    "guard_recall_regression",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntakeError {
    SourceUnavailable,
    Bounds,
    InvalidData,
    InvalidContract,
    Store,
}
impl IntakeError {
    pub fn code(self) -> &'static str {
        match self {
            Self::SourceUnavailable => "program_source_unavailable",
            Self::Bounds => "program_capture_bounds",
            Self::InvalidData => "invalid_program_capture",
            Self::InvalidContract => "invalid_program_intake_contract",
            Self::Store => "program_projection_failure",
        }
    }
}
impl std::fmt::Display for IntakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for IntakeError {}
type Result<T> = std::result::Result<T, IntakeError>;

/// Trusted versioned producer/definition binding; never derived from a row's
/// title or supplied by an untrusted query. Original endpoint pins are absent
/// in producer v1, regardless of manifest planning heads or later Git captures.
#[derive(Clone)]
pub struct IntakeContract {
    pub namespace: Namespace,
    pub program_key: String,
    pub legacy_definition: MetricDefinition,
    pub producer_sha256: String,
}
impl IntakeContract {
    fn validate(&self, producer: &[u8]) -> Result<()> {
        Scope::program(self.namespace.clone(), self.program_key.clone())
            .map_err(|_| IntakeError::InvalidContract)?;
        self.legacy_definition
            .validate()
            .map_err(|_| IntakeError::InvalidContract)?;
        if self.legacy_definition.unit != Unit::Fraction || self.producer_sha256 != digest(producer)
        {
            return Err(IntakeError::InvalidContract);
        }
        Ok(())
    }
}

/// Opaque admitted capture. Private fields prevent substituting parsed rows or
/// a digest after validation. Retained envelope bytes bind every lineage pointer.
#[derive(Clone)]
pub struct Capture {
    envelope: Vec<u8>,
    envelope_sha256: String,
    contract: IntakeContract,
    population: PathPopulation,
    rows: Vec<Row>,
}
#[derive(Clone)]
struct Row {
    value: Value,
    line: usize,
    sha256: String,
    raw: String,
    observed_at: u64,
}

impl Capture {
    /// Explicit trusted recording input, also used by synthetic tests. These
    /// bytes must be retained by the intake owner; a hash is not authentication.
    pub fn recorded(
        snapshots: &[u8],
        manifest: &[u8],
        producer: &[u8],
        capture_start: u64,
        capture_end: u64,
        contract: IntakeContract,
    ) -> Result<Self> {
        if snapshots.is_empty()
            || snapshots.len() > MAX_SNAPSHOTS
            || manifest.len() > MAX_MANIFEST
            || producer.len() > MAX_PRODUCER
        {
            return Err(IntakeError::Bounds);
        }
        if capture_start > capture_end {
            return Err(IntakeError::InvalidData);
        }
        contract.validate(producer)?;
        let manifest_value = strict::parse(manifest)?;
        let manifest_sha256 = digest(manifest);
        let (population, sanitized_manifest) =
            manifest_population(&manifest_value, &manifest_sha256, &contract)?;
        // A final newline is the JSONL record-commit boundary. A readable partial
        // append is refused rather than silently accepting a truncated capture.
        if snapshots.last() != Some(&b'\n') {
            return Err(IntakeError::InvalidData);
        }
        let mut rows = Vec::new();
        let mut row_digests = BTreeSet::new();
        for (index, line) in snapshots.split_inclusive(|b| *b == b'\n').enumerate() {
            if line.len() > MAX_LINE || index >= MAX_ROWS {
                return Err(IntakeError::Bounds);
            }
            let value = strict::parse(line)?;
            let observed_at = validate_row(&value, &population)?;
            let sha256 = digest(line);
            if !row_digests.insert(sha256.clone()) {
                return Err(IntakeError::InvalidData);
            }
            rows.push(Row {
                value,
                line: index + 1,
                sha256,
                raw: std::str::from_utf8(line)
                    .map_err(|_| IntakeError::InvalidData)?
                    .to_string(),
                observed_at,
            });
        }
        let envelope_rows: Vec<_> = rows.iter().map(|r| r.value.clone()).collect();
        let row_sources: Vec<_> = rows
            .iter()
            .map(|r| json!({"line":r.line,"sha256":r.sha256,"raw_utf8":r.raw}))
            .collect();
        let envelope = serde_json::to_vec(&json!({
            "schema":ADAPTER_REVISION, "namespace":contract.namespace,"program_key":contract.program_key,
            "source":{"snapshots_path":SNAPSHOTS_PATH,"snapshots_sha256":digest(snapshots),
                "manifest_path":MANIFEST_PATH,"manifest_sha256":manifest_sha256,
                "producer_path":PRODUCER_PATH,"producer_sha256":digest(producer),
                "capture_start":capture_start,"capture_end":capture_end},
            "algorithm":"producer-v1-rendered-paths", "original_git_pins":"unknown",
            "definition":definition_metadata(&contract.legacy_definition),
            "manifest":sanitized_manifest,"row_sources":row_sources,"rows":envelope_rows,
        })).map_err(|_| IntakeError::InvalidData)?;
        let envelope_sha256 = digest(&envelope);
        Ok(Self {
            envelope,
            envelope_sha256,
            contract,
            population,
            rows,
        })
    }
    pub fn envelope_bytes(&self) -> &[u8] {
        &self.envelope
    }
    pub fn envelope_sha256(&self) -> &str {
        &self.envelope_sha256
    }
    /// Trusted adapter input only. This contains exact manifest path metadata;
    /// do not serialize it to consumers. PR4 can reuse PR2's exact-path rules.
    pub fn population(&self) -> &PathPopulation {
        &self.population
    }

    fn objects(&self) -> Result<Vec<Object>> {
        let mut result = Vec::new();
        for (index, row) in self.rows.iter().enumerate() {
            for selector in SELECTORS {
                let mut fields = fields_for(&row.value, selector)?;
                fields.insert("original_git_pins".into(), Knowledge::Unknown);
                fields.insert(
                    "manifest_sha256".into(),
                    Knowledge::Value(self.population.basis.manifest_sha256.clone().into()),
                );
                fields.insert(
                    "source_unit".into(),
                    Knowledge::Value(unit(selector).into()),
                );
                fields.insert(
                    "qualification".into(),
                    Knowledge::Value("recorded_unverified".into()),
                );
                if selector == "kpi_legacy_removal" {
                    fields.insert(
                        "algorithm".into(),
                        Knowledge::Value("producer-v1-rendered-paths".into()),
                    );
                    fields.insert(
                        "definition_revision".into(),
                        Knowledge::Value(
                            self.contract
                                .legacy_definition
                                .reference
                                .revision
                                .clone()
                                .into(),
                        ),
                    );
                }
                let revision = format!("sha256:{}", row.sha256);
                let observation = Observation {
                    revision: revision.clone(),
                    source_revision: self.contract.producer_sha256.clone(),
                    observed_at: row.observed_at,
                    fresh_until: row
                        .observed_at
                        .checked_add(budget(selector))
                        .ok_or(IntakeError::InvalidData)?,
                    change: Change::Initial,
                    supersedes: None,
                    coverage: Coverage::Partial,
                    fields,
                    lineage: Lineage {
                        artifact_sha256: self.envelope_sha256.clone(),
                        pointer: format!("/rows/{index}"),
                    },
                };
                // Qualification semantics are owned by PR2; importing never
                // replaces source time with capture time or upgrades coverage.
                ont_core::program::qualify_basis(&observation, row.observed_at)
                    .map_err(|_| IntakeError::InvalidData)?;
                result.push(Object {
                    id: Id {
                        object_type: "measurement".into(),
                        namespace: self.contract.namespace.clone(),
                        native_key: format!("{}:{selector}", row.sha256),
                    },
                    scope: Scope::program(
                        self.contract.namespace.clone(),
                        self.contract.program_key.clone(),
                    )
                    .map_err(|_| IntakeError::InvalidContract)?,
                    head: revision,
                    observations: vec![observation],
                });
            }
        }
        Ok(result)
    }
}

/// This Linux host reader has exactly three fixed paths; recording input is
/// separate and portable. Other platforms refuse fixed-path capture. Limits
/// apply during the read, including files that grow while read.
pub fn capture_fixed(contract: IntakeContract, mut clock: impl FnMut() -> u64) -> Result<Capture> {
    let start = clock();
    let snapshots = bounded_file(SNAPSHOTS_PATH, MAX_SNAPSHOTS)?;
    let manifest = bounded_file(MANIFEST_PATH, MAX_MANIFEST)?;
    let producer = bounded_file(PRODUCER_PATH, MAX_PRODUCER)?;
    Capture::recorded(&snapshots, &manifest, &producer, start, clock(), contract)
}
#[cfg(target_os = "linux")]
fn bounded_file(path: &str, limit: usize) -> Result<Vec<u8>> {
    let file = source::open_file(path)?;
    bounded_read(file, limit)
}
#[cfg(not(target_os = "linux"))]
fn bounded_file(_path: &str, _limit: usize) -> Result<Vec<u8>> {
    Err(IntakeError::SourceUnavailable)
}
#[cfg(any(target_os = "linux", test))]
fn bounded_read(reader: impl Read, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| IntakeError::SourceUnavailable)?;
    if bytes.len() > limit {
        return Err(IntakeError::Bounds);
    }
    Ok(bytes)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Replay {
    Added,
    Unchanged,
}
/// Disposable Program-v2 graph. Rebuild through Store::seed, then swap only on
/// success; no append_batch graph-refresh claim and no live database migration.
pub struct Projection {
    store: Store,
    objects: BTreeMap<Id, Object>,
    root: Id,
    contract: IntakeContract,
}
impl Projection {
    pub fn new(capture: &Capture) -> Result<Self> {
        let root = Id {
            object_type: "program".into(),
            namespace: capture.contract.namespace.clone(),
            native_key: capture.contract.program_key.clone(),
        };
        let mut projection = Self {
            store: Store::memory_program().map_err(|_| IntakeError::Store)?,
            objects: BTreeMap::new(),
            root,
            contract: capture.contract.clone(),
        };
        projection.ingest(capture)?;
        Ok(projection)
    }
    pub fn root(&self) -> &Id {
        &self.root
    }
    /// Trusted embedding handoff; do not expose Store administration to clients.
    pub fn into_store(self) -> Store {
        self.store
    }
    pub fn ingest(&mut self, capture: &Capture) -> Result<Replay> {
        if capture.contract.namespace != self.contract.namespace
            || capture.contract.program_key != self.contract.program_key
            || capture.contract.legacy_definition != self.contract.legacy_definition
            || capture.contract.producer_sha256 != self.contract.producer_sha256
        {
            return Err(IntakeError::InvalidContract);
        }
        let incoming = capture.objects()?;
        let mut objects = self.objects.clone();
        let mut added = false;
        for object in incoming {
            if let Some(previous) = objects.get(&object.id) {
                // A re-capture may have a new file hash, envelope index and
                // capture clock. Verify semantic identity; retain first lineage.
                let old = &previous.observations[0];
                let new = &object.observations[0];
                if old.fields != new.fields
                    || old.observed_at != new.observed_at
                    || old.source_revision != new.source_revision
                    || previous.scope != object.scope
                {
                    return Err(IntakeError::InvalidData);
                }
            } else {
                objects.insert(object.id.clone(), object);
                added = true;
            }
        }
        if !added {
            return Ok(Replay::Unchanged);
        }
        let (graph, links) = projection_graph(capture, &self.root, &objects)?;
        let mut store = Store::memory_program().map_err(|_| IntakeError::Store)?;
        store.seed(&graph, &links).map_err(|_| IntakeError::Store)?;
        self.store = store;
        self.objects = objects;
        Ok(Replay::Added)
    }
}

/// Crate-internal graph composition. Selected samples retain their exact v1
/// IDs, revisions, source time and original full-envelope lineage.
pub(crate) fn selected_graph(
    capture: &Capture,
    window: ont_core::report::ReportWindow,
) -> Result<(Vec<Object>, Vec<Link>)> {
    if window.start > window.end {
        return Err(IntakeError::InvalidData);
    }
    let objects = capture
        .objects()?
        .into_iter()
        .filter(|o| (window.start..=window.end).contains(&o.observations[0].observed_at))
        .map(|o| (o.id.clone(), o))
        .collect();
    let root = Id {
        object_type: "program".into(),
        namespace: capture.contract.namespace.clone(),
        native_key: capture.contract.program_key.clone(),
    };
    projection_graph(capture, &root, &objects)
}

fn projection_graph(
    capture: &Capture,
    root: &Id,
    objects: &BTreeMap<Id, Object>,
) -> Result<(Vec<Object>, Vec<Link>)> {
    let observation = Observation {
        revision: ADAPTER_REVISION.into(),
        source_revision: capture.contract.producer_sha256.clone(),
        observed_at: 0,
        fresh_until: 0,
        change: Change::Initial,
        supersedes: None,
        coverage: Coverage::Partial,
        fields: BTreeMap::from([(
            "source_contract".into(),
            Knowledge::Value(ADAPTER_REVISION.into()),
        )]),
        lineage: Lineage {
            artifact_sha256: capture.envelope_sha256.clone(),
            pointer: "/schema".into(),
        },
    };
    let root = Object {
        id: root.clone(),
        scope: Scope::program(
            capture.contract.namespace.clone(),
            capture.contract.program_key.clone(),
        )
        .map_err(|_| IntakeError::InvalidContract)?,
        head: observation.revision.clone(),
        observations: vec![observation],
    };
    let mut links = Vec::new();
    let mut graph: Vec<_> = objects.values().cloned().collect();
    // Only an explicitly adopted definition can create an adopts_kpi edge.
    // Source field names beginning with kpi_ do not grant that status.
    for selector in SELECTORS {
        let adopted = selector == "kpi_legacy_removal"
            && matches!(
                capture.contract.legacy_definition.role,
                ont_core::program::MetricRole::Kpi { .. }
            );
        let mut metric = root.clone();
        metric.id = Id {
            object_type: "metric".into(),
            namespace: capture.contract.namespace.clone(),
            native_key: selector.into(),
        };
        metric.observations[0].fields = BTreeMap::from([
            ("selector".into(), Knowledge::Value(selector.into())),
            (
                "role".into(),
                Knowledge::Value(if adopted { "kpi" } else { "indicator" }.into()),
            ),
            (
                "source_unit".into(),
                Knowledge::Value(unit(selector).into()),
            ),
        ]);
        if selector == "kpi_legacy_removal" {
            metric.observations[0].fields.insert(
                "definition".into(),
                Knowledge::Value(definition_metadata(&capture.contract.legacy_definition)),
            );
            metric.observations[0].lineage.pointer = "/definition".into();
        }
        links.push(Link {
            source: root.id.clone(),
            relation: if adopted {
                "adopts_kpi"
            } else {
                "tracks_indicator"
            }
            .into(),
            target: metric.id.clone(),
        });
        for id in objects
            .keys()
            .filter(|id| id.native_key.ends_with(&format!(":{selector}")))
        {
            links.push(Link {
                source: metric.id.clone(),
                relation: "has_measurement".into(),
                target: id.clone(),
            });
        }
        graph.push(metric);
    }
    graph.push(root);
    Ok((graph, links))
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn unit(selector: &str) -> &'static str {
    match selector {
        "guard_ci_minutes_month" => "Linux minutes (source-unverified)",
        "guard_disk_free_gb" => "GiB (source field named gb; strategy GB unresolved)",
        "guard_escaped_defects_week" => "count",
        _ => "fraction",
    }
}
fn budget(selector: &str) -> u64 {
    match selector {
        "guard_ci_minutes_month" => 86400,
        "guard_disk_free_gb" => 900,
        _ => 3600,
    }
}

fn object(value: &Value) -> Result<&serde_json::Map<String, Value>> {
    value.as_object().ok_or(IntakeError::InvalidData)
}
fn required<'a>(value: &'a Value, key: &str) -> Result<&'a Value> {
    value.get(key).ok_or(IntakeError::InvalidData)
}
fn uint(value: &Value) -> Result<u64> {
    value.as_u64().ok_or(IntakeError::InvalidData)
}
fn text(value: &Value) -> Result<&str> {
    value
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or(IntakeError::InvalidData)
}
fn only(value: &Value, allowed: &[&str]) -> Result<()> {
    if object(value)?
        .keys()
        .any(|k| !allowed.contains(&k.as_str()))
    {
        return Err(IntakeError::InvalidData);
    }
    Ok(())
}
fn manifest_population(
    value: &Value,
    hash: &str,
    contract: &IntakeContract,
) -> Result<(PathPopulation, Value)> {
    if required(value, "schema_version")?.as_u64() != Some(1) {
        return Err(IntakeError::InvalidData);
    }
    let mut repositories = BTreeMap::new();
    let mut summaries = Vec::new();
    let mut total = 0_u64;
    for (name, repo) in object(required(value, "repos")?)? {
        if name.is_empty() || name.len() > 128 {
            return Err(IntakeError::InvalidData);
        }
        let mut paths = BTreeSet::new();
        let mut all_paths = BTreeSet::new();
        for group in required(repo, "groups")?
            .as_array()
            .ok_or(IntakeError::InvalidData)?
        {
            let disposition = text(required(group, "disposition")?)?;
            if ![
                "KEEP",
                "DELETE_NOW",
                "PORT_TO_RUST_THEN_DELETE",
                "DELETE_AFTER_CUTOVER",
            ]
            .contains(&disposition)
            {
                return Err(IntakeError::InvalidData);
            }
            let files = required(group, "files")?
                .as_array()
                .ok_or(IntakeError::InvalidData)?;
            if uint(required(group, "count")?)? != files.len() as u64 {
                return Err(IntakeError::InvalidData);
            }
            for file in files {
                let path = text(file)?;
                if path.contains('\0') || !all_paths.insert(path.to_string()) {
                    return Err(IntakeError::InvalidData);
                }
                if disposition != "KEEP" {
                    paths.insert(path.to_string());
                }
            }
        }
        if uint(required(repo, "tracked_file_count")?)? != all_paths.len() as u64 {
            return Err(IntakeError::InvalidData);
        }
        total = total
            .checked_add(paths.len() as u64)
            .ok_or(IntakeError::Bounds)?;
        let path_hashes: Vec<_> = paths.iter().map(|p| digest(p.as_bytes())).collect();
        summaries.push(
            json!({"repository":name,"non_keep_count":paths.len(),"path_sha256":path_hashes}),
        );
        let id = Id {
            object_type: "repository".into(),
            namespace: contract.namespace.clone(),
            native_key: name.clone(),
        };
        repositories.insert(id, paths);
    }
    let declared = uint(required(
        required(value, "verification")?,
        "exact_non_keep_removal_paths",
    )?)?;
    if declared != total {
        return Err(IntakeError::InvalidData);
    }
    let population = PathPopulation {
        basis: PathBasis {
            definition: contract.legacy_definition.reference.clone(),
            manifest_sha256: hash.into(),
            adapter_revision: ADAPTER_REVISION.into(),
            algorithm: PathAlgorithm::ExactUtf8PathsV1,
            denominator: total,
        },
        repositories,
    };
    population
        .validate()
        .map_err(|_| IntakeError::InvalidData)?;
    Ok((
        population,
        json!({"repositories":summaries,"non_keep_total":total}),
    ))
}
fn state(value: Option<&Value>) -> Result<Option<Knowledge>> {
    Ok(match value {
        None => Some(Knowledge::Missing),
        Some(Value::Null) => Some(Knowledge::Null),
        Some(Value::String(s)) if s.is_empty() => Some(Knowledge::Blank),
        Some(Value::Object(m)) if m.contains_key("kind") => {
            let parsed: Knowledge = serde_json::from_value(Value::Object(m.clone()))
                .map_err(|_| IntakeError::InvalidData)?;
            if matches!(parsed, Knowledge::Value(_)) {
                return Err(IntakeError::InvalidData);
            }
            Some(parsed)
        }
        _ => None,
    })
}
fn numeric(value: Option<&Value>, integer: bool) -> Result<Knowledge> {
    if let Some(state) = state(value)? {
        return Ok(state);
    }
    let value = value.ok_or(IntakeError::InvalidData)?;
    let valid = if integer {
        value.as_u64().is_some()
    } else {
        value.as_f64().is_some_and(|n| n.is_finite() && n >= 0.0)
    };
    if !valid {
        return Err(IntakeError::InvalidData);
    }
    Ok(Knowledge::Value(value.clone()))
}
fn fields_for(row: &Value, selector: &str) -> Result<BTreeMap<String, Knowledge>> {
    let mut fields = BTreeMap::new();
    let group = row.get(selector);
    if let Some(state) = state(group)? {
        fields.insert("value".into(), state);
        return Ok(fields);
    }
    let group = group.ok_or(IntakeError::InvalidData)?;
    let (numbers, allowed): (&[&str], &[&str]) = match selector {
        "kpi_legacy_removal" => (
            &["removed", "total", "pct"],
            &["removed", "total", "pct", "per_repo", "unit"],
        ),
        "kpi_mcp_parity" => (
            &["qualified", "total", "bounded_implemented"],
            &[
                "qualified",
                "total",
                "bounded_implemented",
                "source",
                "unit",
            ],
        ),
        "guard_ci_minutes_month" => (&["value", "limit"], &["value", "limit", "unit"]),
        "guard_disk_free_gb" => (&["value", "min"], &["value", "min", "unit"]),
        "guard_escaped_defects_week" => (&["value"], &["value", "note", "unit"]),
        _ => (
            &["supported", "total"],
            &["supported", "total", "source", "unit"],
        ),
    };
    only(group, allowed)?;
    if let Some(unit_value) = group.get("unit") {
        let expected = match selector {
            "guard_disk_free_gb" => "GiB",
            "guard_ci_minutes_month" => "linux_minutes",
            "guard_escaped_defects_week" => "count",
            _ => "fraction",
        };
        if unit_value.as_str() != Some(expected) {
            return Err(IntakeError::InvalidData);
        }
    }
    for key in numbers {
        let integer = !matches!(
            (selector, *key),
            ("kpi_legacy_removal", "pct")
                | ("guard_ci_minutes_month", "value")
                | ("guard_disk_free_gb", "value")
                | ("guard_disk_free_gb", "min")
        );
        fields.insert((*key).into(), numeric(group.get(*key), integer)?);
    }
    if let Some(per_repo) = group.get("per_repo") {
        fields.insert("per_repo".into(), Knowledge::Value(per_repo.clone()));
    }
    for key in ["source", "note"] {
        if let Some(v) = group.get(key) {
            text(v)?;
            fields.insert(key.into(), Knowledge::Value(v.clone()));
        }
    }
    let pair = match selector {
        "kpi_legacy_removal" => Some(("removed", "total")),
        "kpi_mcp_parity" => Some(("qualified", "total")),
        "guard_recall_regression" => Some(("supported", "total")),
        _ => None,
    };
    if let Some((numerator, denominator)) = pair
        && let Some(d) = group.get(denominator).and_then(Value::as_u64)
    {
        ont_core::program::Fraction::new(0, d).map_err(|_| IntakeError::InvalidData)?;
        if group
            .get(numerator)
            .and_then(Value::as_u64)
            .is_some_and(|n| n > d)
            || (selector == "kpi_mcp_parity"
                && group
                    .get("bounded_implemented")
                    .and_then(Value::as_u64)
                    .is_some_and(|n| n > d))
        {
            return Err(IntakeError::InvalidData);
        }
    }
    // Display percentages remain original assertions, never arithmetic inputs.
    if selector == "kpi_legacy_removal"
        && group
            .get("pct")
            .and_then(Value::as_f64)
            .is_some_and(|p| p > 100.0)
    {
        return Err(IntakeError::InvalidData);
    }
    Ok(fields)
}
fn validate_row(value: &Value, population: &PathPopulation) -> Result<u64> {
    only(
        value,
        &[
            "measured_at",
            "kpi_legacy_removal",
            "kpi_mcp_parity",
            "guard_ci_minutes_month",
            "guard_disk_free_gb",
            "guard_escaped_defects_week",
            "guard_recall_regression",
        ],
    )?;
    let at = timestamp(text(required(value, "measured_at")?)?)?;
    for selector in SELECTORS {
        fields_for(value, selector)?;
    }
    if let Some(legacy) = value
        .get("kpi_legacy_removal")
        .filter(|v| v.get("kind").is_none() && v.is_object())
    {
        // The adopted population remains known even when the source row's
        // own total is Missing/Null/Blank/Unknown/NotPerformed. Preserve that
        // state, but never admit a numeric removal beyond the known population.
        if legacy
            .get("removed")
            .and_then(Value::as_u64)
            .is_some_and(|n| n > population.basis.denominator)
        {
            return Err(IntakeError::InvalidData);
        }
        if legacy
            .get("total")
            .and_then(Value::as_u64)
            .is_some_and(|n| n != population.basis.denominator)
        {
            return Err(IntakeError::InvalidData);
        }
        if let Some(per_repo) = legacy.get("per_repo") {
            let components = object(per_repo)?;
            if components.len() != population.repositories.len() {
                return Err(IntakeError::InvalidData);
            }
            let mut sum = 0_u64;
            for (id, paths) in &population.repositories {
                let pair = components
                    .get(&id.native_key)
                    .and_then(Value::as_array)
                    .filter(|p| p.len() == 2)
                    .ok_or(IntakeError::InvalidData)?;
                let removed = uint(&pair[0])?;
                let denominator = uint(&pair[1])?;
                if denominator != paths.len() as u64 || removed > denominator {
                    return Err(IntakeError::InvalidData);
                }
                sum = sum.checked_add(removed).ok_or(IntakeError::InvalidData)?;
            }
            if legacy
                .get("removed")
                .and_then(Value::as_u64)
                .is_some_and(|n| n != sum)
            {
                return Err(IntakeError::InvalidData);
            }
        }
    }
    Ok(at)
}
// This producer's UTC minute/second syntax only; no local timezone, implicit
// offset, calendar normalization or dependency on the machine's wall clock.
fn timestamp(s: &str) -> Result<u64> {
    let b = s.as_bytes();
    if !matches!(b.len(), 17 | 20)
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[b.len() - 1] != b'Z'
        || (b.len() == 20 && b[16] != b':')
    {
        return Err(IntakeError::InvalidData);
    }
    let part = |start: usize, end: usize| -> Result<i64> {
        if !b[start..end].iter().all(u8::is_ascii_digit) {
            return Err(IntakeError::InvalidData);
        }
        std::str::from_utf8(&b[start..end])
            .map_err(|_| IntakeError::InvalidData)?
            .parse()
            .map_err(|_| IntakeError::InvalidData)
    };
    let (year, month, day, hour, minute) = (
        part(0, 4)?,
        part(5, 7)?,
        part(8, 10)?,
        part(11, 13)?,
        part(14, 16)?,
    );
    let second = if b.len() == 20 { part(17, 19)? } else { 0 };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => 0,
    };
    if year < 1970 || day < 1 || day > days || hour > 23 || minute > 59 || second > 59 {
        return Err(IntakeError::InvalidData);
    }
    let y = year - i64::from(month <= 2);
    let era = y / 400;
    let yoe = y - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let unix_days = era * 146097 + doe - 719468;
    u64::try_from(unix_days * 86400 + hour * 3600 + minute * 60 + second)
        .map_err(|_| IntakeError::InvalidData)
}

fn definition_metadata(definition: &MetricDefinition) -> Value {
    let role = match &definition.role {
        ont_core::program::MetricRole::Indicator => json!({"kind":"indicator"}),
        ont_core::program::MetricRole::Kpi { strategy, adoption } => json!({"kind":"kpi",
            "strategy":{"id":strategy.id,"revision":strategy.revision},"adoption":adoption}),
    };
    json!({"id":definition.reference.id,"revision":definition.reference.revision,
        "formula":definition.formula,"population":definition.population,"unit":"fraction",
        "direction":match definition.direction { ont_core::program::Direction::Increase=>"increase",ont_core::program::Direction::Decrease=>"decrease" },"role":role})
}
