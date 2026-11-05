//! Owner-supplied sanitized guardrail recordings (DESIGN §8 row 5).
//!
//! No source acquisition, credentials, actions or consumer read bypass. The
//! embedding owner authenticates captures, supplies Contract and trusted time,
//! retains envelope_bytes(), and grants the resulting object independently of
//! the program root. Hashes bind bytes, not source authenticity. Failed/missing
//! sources never produce a qualified zero. See program_guardrails/INTERFACE.md.
use ont_core::program::{Fraction, qualify_basis};
use ont_core::{Change, Coverage, Id, Knowledge, Lineage, Object, Observation, Scope};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const ADAPTER_REVISION: &str = "program-guardrails/v1";
const MAX_BYTES: usize = 1024 * 1024;
const MAX_ITEMS: usize = 4096;
const WEEK: u64 = 7 * 86400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntakeError {
    InvalidContract,
    InvalidObject,
    InvalidEvidence,
}
impl IntakeError {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidContract => "invalid_guardrail_contract",
            Self::InvalidObject => "invalid_guardrail_object",
            Self::InvalidEvidence => "invalid_guardrail_evidence",
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

/// UTC calendar month, not a rolling 30-day window or local timezone.
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UtcMonth {
    pub year: u16,
    pub month: u8,
}
impl UtcMonth {
    pub fn bounds(self) -> Result<(u64, u64)> {
        if !(1970..=9999).contains(&self.year) || !(1..=12).contains(&self.month) {
            return Err(IntakeError::InvalidContract);
        }
        let leap =
            |y: u64| y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400));
        let days_before = |y: u64| 365 * (y - 1) + (y - 1) / 4 - (y - 1) / 100 + (y - 1) / 400;
        let lengths = [
            31,
            28 + u64::from(leap(u64::from(self.year))),
            31,
            30,
            31,
            30,
            31,
            31,
            30,
            31,
            30,
            31,
        ];
        let days = days_before(u64::from(self.year)) - days_before(1970)
            + lengths[..usize::from(self.month - 1)].iter().sum::<u64>();
        Ok((
            days * 86400,
            (days + lengths[usize::from(self.month - 1)]) * 86400,
        ))
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskUnit {
    Bytes,
    Gb,
    Gib,
}
impl DiskUnit {
    fn multiplier(self) -> u64 {
        match self {
            Self::Bytes => 1,
            Self::Gb => 1_000_000_000,
            Self::Gib => 1 << 30,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Bytes => "bytes",
            Self::Gb => "gb",
            Self::Gib => "gib",
        }
    }
}
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiskMinimum {
    Exact {
        quantity: u64,
        unit: DiskUnit,
    },
    /// Preserve the strategy/producer GB/GiB mismatch. Never silently adopt one.
    UnresolvedGbOrGib {
        quantity: u64,
    },
}
impl DiskMinimum {
    fn bounds(&self) -> Result<(u64, u64)> {
        let mul = |n: u64, by: u64| n.checked_mul(by).ok_or(IntakeError::InvalidContract);
        match *self {
            Self::Exact { quantity, unit } => {
                let n = mul(quantity, unit.multiplier())?;
                Ok((n, n))
            }
            Self::UnresolvedGbOrGib { quantity } => {
                Ok((mul(quantity, 1_000_000_000)?, mul(quantity, 1 << 30)?))
            }
        }
    }
}
#[derive(Clone, Serialize)]
pub struct RecallBaseline {
    pub supported: u64,
    pub total: u64,
    pub evaluation_set_sha256: String,
    pub result_revision: String,
    pub artifact_sha256: String,
}

/// Trusted configuration, never accepted from a consumer query. Linux SKUs are
/// explicit adopted identities; the recording must cover this account's entire
/// declared Linux-minute population, not an arbitrary successful subset.
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Contract {
    Billing {
        account: String,
        month: UtcMonth,
        linux_skus: BTreeSet<String>,
        limit: u64,
    },
    Disk {
        minimum: DiskMinimum,
    },
    Recall {
        baseline: RecallBaseline,
    },
    Defects {
        week_start: u64,
        limit: u64,
    },
}
impl Contract {
    fn kind(&self) -> &'static str {
        match self {
            Self::Billing { .. } => "billing",
            Self::Disk { .. } => "disk",
            Self::Recall { .. } => "recall",
            Self::Defects { .. } => "defects",
        }
    }
    fn unit(&self) -> &'static str {
        match self {
            Self::Billing { .. } => "linux_minutes",
            Self::Disk { .. } => "bytes",
            Self::Recall { .. } => "fraction",
            Self::Defects { .. } => "count",
        }
    }
    fn budget(&self) -> u64 {
        match self {
            Self::Billing { .. } => 86400,
            Self::Disk { .. } => 900,
            _ => 3600,
        }
    }
    fn fields(&self) -> Result<BTreeMap<String, Knowledge>> {
        let mut f = BTreeMap::new();
        put(&mut f, "guardrail", self.kind());
        put(&mut f, "unit", self.unit());
        put(
            &mut f,
            "bound_direction",
            if matches!(self, Self::Billing { .. } | Self::Defects { .. }) {
                "at_most"
            } else {
                "at_least"
            },
        );
        match self {
            Self::Billing {
                account,
                month,
                linux_skus,
                limit,
            } => {
                if !text(account)
                    || linux_skus.is_empty()
                    || linux_skus.len() > 64
                    || linux_skus.iter().any(|s| !text(s))
                {
                    return Err(IntakeError::InvalidContract);
                }
                let (start, end) = month.bounds()?;
                put(&mut f, "account", account.as_str());
                put(&mut f, "window_start", start);
                put(&mut f, "window_end", end);
                put(&mut f, "bound_value", *limit);
            }
            Self::Disk { minimum } => {
                let (low, high) = minimum.bounds()?;
                if matches!(minimum, DiskMinimum::UnresolvedGbOrGib { .. }) {
                    f.insert("bound_value".into(), Knowledge::Unknown);
                    put(&mut f, "bound_lower_bytes", low);
                    put(&mut f, "bound_upper_bytes", high);
                } else {
                    put(&mut f, "bound_value", low);
                }
            }
            Self::Recall { baseline: b } => {
                fraction(b.supported, b.total).map_err(|_| IntakeError::InvalidContract)?;
                if !pin(&b.evaluation_set_sha256)
                    || !pin(&b.artifact_sha256)
                    || !text(&b.result_revision)
                {
                    return Err(IntakeError::InvalidContract);
                }
                put(&mut f, "bound_value", b.supported);
                put(&mut f, "bound_total", b.total);
                put(
                    &mut f,
                    "evaluation_set_sha256",
                    b.evaluation_set_sha256.as_str(),
                );
                put(
                    &mut f,
                    "baseline_result_revision",
                    b.result_revision.as_str(),
                );
                put(
                    &mut f,
                    "baseline_artifact_sha256",
                    b.artifact_sha256.as_str(),
                );
            }
            Self::Defects { week_start, limit } => {
                // Unix epoch was Thursday; the first UTC Monday was day four.
                if *week_start % WEEK != 4 * 86400 {
                    return Err(IntakeError::InvalidContract);
                }
                put(&mut f, "window_start", *week_start);
                put(
                    &mut f,
                    "window_end",
                    week_start
                        .checked_add(WEEK)
                        .ok_or(IntakeError::InvalidContract)?,
                );
                put(&mut f, "bound_value", *limit);
            }
        }
        Ok(f)
    }
}

/// Strict sanitized recording schema. No arbitrary JSON, header, URL, raw
/// response or incident-description fields. The owner must sanitize identity
/// and revision strings before supplying bytes; field allowlists are not DLP.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Recording {
    source_revision: String,
    capture_start: u64,
    capture_end: u64,
    outcome: Outcome,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Outcome {
    NotPerformed,
    Failed,
    /// An old hardcoded assertion is retained without becoming a new evaluation.
    Baseline {
        observed_at: u64,
        value: u64,
        total: Option<u64>,
    },
    Billing {
        account: String,
        month: UtcMonth,
        period_start: u64,
        period_end: u64,
        cutoff: u64,
        settlement_lag_seconds: Option<u64>,
        population_complete: bool,
        pages: Vec<Page>,
    },
    Disk {
        observed_at: u64,
        numerator: u64,
        denominator: u64,
        unit: DiskUnit,
    },
    Recall {
        observed_at: u64,
        evaluation_set_sha256: String,
        result_revision: String,
        supported: u64,
        total: u64,
    },
    Defects {
        week_start: u64,
        week_end: u64,
        cutoff: u64,
        population_complete: bool,
        incidents: Vec<Incident>,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    number: u32,
    has_more: bool,
    items: Vec<Usage>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Usage {
    id: String,
    sku: String,
    unit: String,
    quantity: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Incident {
    id: String,
    escaped_at: u64,
    fixed_at: Option<u64>,
}

/// Immutable admitted result. The envelope binds configuration, source digest,
/// sanitized parsed evidence and the exact fields; no payload Debug output.
pub struct QualifiedCapture {
    envelope: Vec<u8>,
    observation: Observation,
}
impl QualifiedCapture {
    pub fn envelope_bytes(&self) -> &[u8] {
        &self.envelope
    }
    pub fn observation(&self) -> &Observation {
        &self.observation
    }
    /// Use a separate explicitly granted guardrail object, never a public root.
    pub fn object(&self, id: Id, scope: Scope) -> Result<Object> {
        id.validate().map_err(|_| IntakeError::InvalidObject)?;
        scope.validate().map_err(|_| IntakeError::InvalidObject)?;
        if !matches!(scope, Scope::Program { .. }) || id.namespace != *scope.namespace() {
            return Err(IntakeError::InvalidObject);
        }
        Ok(Object {
            id,
            scope,
            head: self.observation.revision.clone(),
            observations: vec![self.observation.clone()],
        })
    }
}

struct Evaluation {
    fields: BTreeMap<String, Knowledge>,
    at: u64,
    qualified: bool,
    reason: &'static str,
}
impl Evaluation {
    fn unresolved(&mut self, reason: &'static str) {
        self.qualified = false;
        self.reason = reason;
        let state = if reason == "not_performed" {
            Knowledge::NotPerformed
        } else {
            Knowledge::Unknown
        };
        self.fields.insert("value".into(), state.clone());
        self.fields.insert("bound_satisfied".into(), state.clone());
        if self.fields.contains_key("total") {
            self.fields.insert("total".into(), state);
        }
    }
}

/// Bounded owner recording input. None means source absent, not no usage.
/// Bad source data becomes a Partial/Unknown observation with stable diagnostics;
/// invalid trusted configuration refuses. Tests and source intake use no network.
pub fn qualify_recording(
    bytes: Option<&[u8]>,
    contract: &Contract,
    now: u64,
) -> Result<QualifiedCapture> {
    let mut e = Evaluation {
        fields: contract.fields()?,
        at: 0,
        qualified: false,
        reason: "missing_source",
    };
    if matches!(contract, Contract::Recall { .. }) {
        e.fields.insert("total".into(), Knowledge::Unknown);
    }
    e.unresolved("missing_source");
    let parsed: Option<Recording> = bytes
        .filter(|b| b.len() <= MAX_BYTES)
        .and_then(|b| serde_json::from_slice(b).ok());
    if let Some(r) = &parsed {
        if !text(&r.source_revision) || r.capture_start > r.capture_end {
            e.unresolved("invalid_source");
        } else if r.capture_end > now {
            e.unresolved("future_source");
        } else if evaluate(&r.outcome, contract, &mut e).is_err() {
            e.unresolved("invalid_source");
        } else if e.at > r.capture_end
            && !matches!(r.outcome, Outcome::NotPerformed | Outcome::Failed)
        {
            e.unresolved("future_source");
        }
    } else if bytes.is_some() {
        e.unresolved("invalid_source");
    }
    let fresh_until = match e.at.checked_add(contract.budget()) {
        Some(until) => until,
        None => {
            e.unresolved("invalid_source");
            e.at = 0;
            contract.budget()
        }
    };
    // Use the existing Program clock/expiry rule; do not change S06 semantics.
    let temporal = Observation {
        revision: "temporal".into(),
        source_revision: "temporal".into(),
        observed_at: e.at,
        fresh_until,
        change: Change::Initial,
        supersedes: None,
        coverage: Coverage::Complete,
        fields: e.fields.clone(),
        lineage: Lineage {
            artifact_sha256: "0".repeat(64),
            pointer: String::new(),
        },
    };
    let q = qualify_basis(&temporal, now).map_err(|_| IntakeError::InvalidEvidence)?;
    if e.qualified && q.future {
        e.unresolved("future_source");
    }
    if e.qualified && q.stale {
        e.unresolved("stale_source");
    }
    put(&mut e.fields, "source_qualified", e.qualified);
    put(&mut e.fields, "qualification", e.reason);
    let complete = e.qualified && e.reason == "qualified";
    let envelope = serde_json::to_vec(&json!({"adapter_revision":ADAPTER_REVISION,
        "qualified_at":now,"contract":contract,"source_sha256":bytes.filter(|b| b.len() <= MAX_BYTES).map(digest),
        "source":parsed,"qualification":e.fields}))
    .map_err(|_| IntakeError::InvalidEvidence)?;
    let hash = digest(&envelope);
    let observation = Observation {
        revision: hash.clone(),
        source_revision: parsed
            .as_ref()
            .filter(|r| text(&r.source_revision))
            .map_or_else(|| ADAPTER_REVISION.into(), |r| r.source_revision.clone()),
        observed_at: e.at,
        fresh_until,
        change: Change::Initial,
        supersedes: None,
        coverage: if complete {
            Coverage::Complete
        } else {
            Coverage::Partial
        },
        fields: e.fields,
        lineage: Lineage {
            artifact_sha256: hash,
            pointer: "/qualification".into(),
        },
    };
    observation
        .validate()
        .map_err(|_| IntakeError::InvalidEvidence)?;
    Ok(QualifiedCapture {
        envelope,
        observation,
    })
}

fn evaluate(
    source: &Outcome,
    contract: &Contract,
    e: &mut Evaluation,
) -> std::result::Result<(), ()> {
    match (source, contract) {
        (Outcome::NotPerformed, _) => {
            e.unresolved("not_performed");
            return Ok(());
        }
        (Outcome::Failed, _) => {
            e.unresolved("failed_source");
            return Ok(());
        }
        (
            Outcome::Baseline {
                observed_at,
                value,
                total,
            },
            _,
        ) => {
            if let Some(total) = total {
                fraction(*value, *total)?;
                put(&mut e.fields, "recorded_total", *total);
            }
            e.at = *observed_at;
            put(&mut e.fields, "recorded_value", *value);
            e.unresolved("unverified_baseline");
            return Ok(());
        }
        (
            Outcome::Billing {
                account,
                month,
                period_start,
                period_end,
                cutoff,
                settlement_lag_seconds,
                population_complete,
                pages,
            },
            Contract::Billing {
                account: expected,
                month: expected_month,
                linux_skus,
                limit,
            },
        ) => {
            let (start, end) = expected_month.bounds().map_err(|_| ())?;
            if account != expected
                || month != expected_month
                || (*period_start, *period_end) != (start, end)
                || *cutoff < start
                || *cutoff > end
                || pages.len() > 128
            {
                return Err(());
            }
            e.at = *cutoff;
            put(&mut e.fields, "source_cutoff", *cutoff);
            e.fields.insert(
                "settlement_lag_seconds".into(),
                settlement_lag_seconds.map_or(Knowledge::Unknown, |n| Knowledge::Value(n.into())),
            );
            if pages.is_empty() {
                e.unresolved("incomplete_source");
                return Ok(());
            }
            let mut ids = BTreeSet::new();
            let mut sum = 0u64;
            let mut complete = *population_complete;
            for (i, page) in pages.iter().enumerate() {
                if page.number != (i + 1) as u32
                    || page.items.len() > MAX_ITEMS
                    || (i + 1 < pages.len() && !page.has_more)
                {
                    return Err(());
                }
                for item in &page.items {
                    if !text(&item.id)
                        || !ids.insert(&item.id)
                        || !linux_skus.contains(&item.sku)
                        || item.unit != "linux_minutes"
                    {
                        return Err(());
                    }
                    sum = sum.checked_add(item.quantity).ok_or(())?;
                }
                if i + 1 == pages.len() && page.has_more {
                    complete = false;
                }
            }
            if !complete {
                e.unresolved("incomplete_source");
                return Ok(());
            }
            put(&mut e.fields, "recorded_value", sum);
            if *settlement_lag_seconds != Some(0) {
                e.unresolved("settlement_lag");
                return Ok(());
            }
            put(&mut e.fields, "value", sum);
            put(&mut e.fields, "bound_satisfied", sum <= *limit);
        }
        (
            Outcome::Disk {
                observed_at,
                numerator,
                denominator,
                unit,
            },
            Contract::Disk { minimum },
        ) => {
            // Exact rational source conversion. No rounded float at the threshold.
            Fraction::new(i128::from(*numerator), *denominator).map_err(|_| ())?;
            let scaled = u128::from(*numerator) * u128::from(unit.multiplier());
            if scaled % u128::from(*denominator) != 0 {
                return Err(());
            }
            let bytes = u64::try_from(scaled / u128::from(*denominator)).map_err(|_| ())?;
            let (low, high) = minimum.bounds().map_err(|_| ())?;
            e.at = *observed_at;
            put(&mut e.fields, "value", bytes);
            put(&mut e.fields, "source_unit", unit.name());
            if low != high {
                e.reason = "unresolved_disk_unit";
                e.fields.insert(
                    "bound_satisfied".into(),
                    if bytes >= high {
                        Knowledge::Value(true.into())
                    } else if bytes < low {
                        Knowledge::Value(false.into())
                    } else {
                        Knowledge::Unknown
                    },
                );
            } else {
                put(&mut e.fields, "bound_satisfied", bytes >= low);
            }
        }
        (
            Outcome::Recall {
                observed_at,
                evaluation_set_sha256,
                result_revision,
                supported,
                total,
            },
            Contract::Recall { baseline },
        ) => {
            fraction(*supported, *total)?;
            if evaluation_set_sha256 != &baseline.evaluation_set_sha256
                || *total != baseline.total
                || !text(result_revision)
            {
                return Err(());
            }
            e.at = *observed_at;
            put(&mut e.fields, "value", *supported);
            put(&mut e.fields, "total", *total);
            put(&mut e.fields, "result_revision", result_revision.as_str());
            put(
                &mut e.fields,
                "bound_satisfied",
                u128::from(*supported) * u128::from(baseline.total)
                    >= u128::from(baseline.supported) * u128::from(*total),
            );
        }
        (
            Outcome::Defects {
                week_start,
                week_end,
                cutoff,
                population_complete,
                incidents,
            },
            Contract::Defects {
                week_start: expected,
                limit,
            },
        ) => {
            if week_start != expected
                || *week_end != expected.checked_add(WEEK).ok_or(())?
                || *cutoff < *week_start
                || *cutoff > *week_end
                || incidents.len() > MAX_ITEMS
            {
                return Err(());
            }
            e.at = *cutoff;
            put(&mut e.fields, "source_cutoff", *cutoff);
            let mut ids = BTreeSet::new();
            let mut fixed = 0u64;
            for incident in incidents {
                if !text(&incident.id)
                    || !ids.insert(&incident.id)
                    || incident.escaped_at < *week_start
                    || incident.escaped_at >= *cutoff
                    || incident
                        .fixed_at
                        .is_some_and(|at| at < incident.escaped_at || at > *cutoff)
                {
                    return Err(());
                }
                fixed += u64::from(incident.fixed_at.is_some());
            }
            if !population_complete {
                e.unresolved("incomplete_source");
                return Ok(());
            }
            put(&mut e.fields, "fixed_count", fixed);
            put(&mut e.fields, "value", incidents.len() as u64);
            put(
                &mut e.fields,
                "bound_satisfied",
                incidents.len() as u64 <= *limit,
            );
        }
        _ => return Err(()),
    }
    e.qualified = true;
    if e.reason != "unresolved_disk_unit" {
        e.reason = "qualified";
    }
    Ok(())
}

fn put(fields: &mut BTreeMap<String, Knowledge>, name: &str, value: impl Into<Value>) {
    fields.insert(name.into(), Knowledge::Value(value.into()));
}
fn fraction(n: u64, d: u64) -> std::result::Result<(), ()> {
    Fraction::new(i128::from(n), d).map_err(|_| ())?;
    if n > d {
        return Err(());
    }
    Ok(())
}
fn text(s: &str) -> bool {
    !s.trim().is_empty() && s.len() <= 512
}
fn pin(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
