//! One admitted snapshot, finite Program navigation, and source-qualified reporting.
//! Projection fields are trusted intake contracts, never request payloads. Native
//! program rules own arithmetic/qualification; adapters own attribution and bounds.
use crate::program::{self, Fraction};
use crate::{Admission, Error, Id, Knowledge, Object, Observation, Scope, Snapshot, View};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Inclusive source-time selection. This never supplies the trusted current time.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReportWindow {
    pub start: u64,
    pub end: u64,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Assessment {
    ProgressObserved,
    ReviewRequired,
    InsufficientEvidence,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Limitation {
    AuthorizedBasisOnly,
    NotAvailableInAuthorizedResult,
    StaleSource,
    FutureSource,
    PartialSource,
    UnresolvedSource,
    IncompatibleDefinition,
    UnresolvedReconciliation,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProgramReport {
    pub window: ReportWindow,
    pub assessment: Assessment,
    pub basis: Vec<View>,
    pub metrics: Vec<MetricStatement>,
    pub attributions: Vec<AttributionStatement>,
    pub guardrails: Vec<GuardrailStatement>,
    /// Availability describes this admitted result; no hidden inventory/count.
    pub guardrail_availability: Knowledge,
    pub limitations: BTreeSet<Limitation>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExactFraction {
    pub numerator: String,
    pub denominator: u64,
}
impl From<Fraction> for ExactFraction {
    fn from(value: Fraction) -> Self {
        Self {
            numerator: value.numerator().to_string(),
            denominator: value.denominator(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetricStatement {
    pub metric: Id,
    pub definition: Knowledge,
    pub samples: Vec<MeasurementStatement>,
    /// Recorded arithmetic can remain known while source qualification is false.
    pub delta: Option<ExactFraction>,
    pub percentage_point_delta: Option<ExactFraction>,
    pub source_qualified: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MeasurementStatement {
    pub id: Id,
    pub revision: String,
    pub observed_at: u64,
    pub fresh_until: u64,
    pub source_unit: Knowledge,
    pub definition_revision: Knowledge,
    pub manifest_sha256: Knowledge,
    pub algorithm: Knowledge,
    pub fraction: Option<ExactFraction>,
    pub source_qualified: bool,
}
/// PR5's ordinary Observation interface. Values/bounds/provenance remain in basis.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GuardrailStatement {
    pub id: Id,
    pub revision: String,
    pub guardrail: Knowledge,
    pub unit: Knowledge,
    pub value: Knowledge,
    pub bound_satisfied: Knowledge,
    pub source_qualified: bool,
    pub qualification: Knowledge,
}
/// Typed publication of PR4's attribute_v1 projection; no second attribution rule.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AttributionStatement {
    pub id: Id,
    pub revision: String,
    pub source_qualified: bool,
    pub attribution: AttributionInput,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DefinitionPin {
    pub id: Id,
    pub revision: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AttributionInput {
    pub rule: String,
    pub adapter: String,
    pub definition: DefinitionPin,
    pub manifest_sha256: String,
    pub population_adapter_revision: String,
    pub algorithm: String,
    pub denominator: u64,
    pub delta_bound: Option<String>,
    pub residual: Option<String>,
    pub qualification: String,
    pub gaps: Vec<String>,
    pub contributions: Vec<ContributionInput>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ContributionInput {
    pub repository: Id,
    pub commit: String,
    pub first_parent: String,
    pub newly_absent_count: u64,
    pub restored_count: u64,
    pub path_set_sha256: String,
    pub paths: String,
    pub association: String,
    pub changes: Vec<Id>,
}
fn head(object: &Object) -> Result<&Observation, Error> {
    object
        .observations
        .iter()
        .find(|o| o.revision == object.head)
        .ok_or(Error::InvalidEvidence)
}
fn field(observation: &Observation, name: &str) -> Knowledge {
    observation
        .fields
        .get(name)
        .cloned()
        .unwrap_or(Knowledge::Missing)
}
fn value<'a>(observation: &'a Observation, name: &str) -> Option<&'a Value> {
    match observation.fields.get(name) {
        Some(Knowledge::Value(v)) => Some(v),
        _ => None,
    }
}
fn qualify(
    o: &Observation,
    now: u64,
    limitations: &mut BTreeSet<Limitation>,
) -> Result<bool, Error> {
    let q = program::qualify_basis(o, now).map_err(|_| Error::InvalidEvidence)?;
    if q.future {
        limitations.insert(Limitation::FutureSource);
    }
    if q.stale {
        limitations.insert(Limitation::StaleSource);
    }
    if q.partial {
        limitations.insert(Limitation::PartialSource);
    }
    if q.unresolved {
        limitations.insert(Limitation::UnresolvedSource);
    }
    let source_unqualified = value(o, "source_qualified") == Some(&Value::Bool(false))
        || value(o, "qualification")
            .and_then(Value::as_str)
            .is_some_and(|q| q != "qualified");
    if source_unqualified {
        limitations.insert(Limitation::UnresolvedSource);
    }
    Ok(q.is_current() && !source_unqualified)
}
fn fraction(o: &Observation) -> Result<Option<Fraction>, Error> {
    for (n, d) in [
        ("removed", "total"),
        ("qualified", "total"),
        ("supported", "total"),
    ] {
        if let (Some(n), Some(d)) = (
            value(o, n).and_then(Value::as_u64),
            value(o, d).and_then(Value::as_u64),
        ) {
            if n > d {
                return Err(Error::InvalidEvidence);
            }
            return Fraction::new(i128::from(n), d)
                .map(Some)
                .map_err(|_| Error::InvalidEvidence);
        }
    }
    Ok(None)
}
fn definition(v: &Value) -> Option<program::MetricDefinition> {
    let pin = |v: &Value| {
        Some(program::DefinitionRef {
            id: serde_json::from_value(v.get("id")?.clone()).ok()?,
            revision: v.get("revision")?.as_str()?.into(),
        })
    };
    let role = match v.get("role")?.get("kind")?.as_str()? {
        "indicator" => program::MetricRole::Indicator,
        "kpi" => program::MetricRole::Kpi {
            strategy: pin(v.get("role")?.get("strategy")?)?,
            adoption: serde_json::from_value(v.get("role")?.get("adoption")?.clone()).ok()?,
        },
        _ => return None,
    };
    let d = program::MetricDefinition {
        reference: pin(v)?,
        formula: v.get("formula")?.as_str()?.into(),
        population: v.get("population")?.as_str()?.into(),
        unit: match v.get("unit")?.as_str()? {
            "fraction" => program::Unit::Fraction,
            "count" => program::Unit::Count,
            "paths" => program::Unit::Paths,
            "linux_minutes" => program::Unit::LinuxMinutes,
            "bytes" => program::Unit::Bytes,
            _ => return None,
        },
        direction: match v.get("direction")?.as_str()? {
            "increase" => program::Direction::Increase,
            "decrease" => program::Direction::Decrease,
            _ => return None,
        },
        role,
    };
    d.validate().ok()?;
    Some(d)
}
fn relations(kind: &str) -> &'static [&'static str] {
    match kind {
        "program" => &[
            "adopts_kpi",
            "tracks_indicator",
            "has_measurement",
            "guardrail",
            "target",
            "repository",
        ],
        "metric" => &["has_measurement"],
        "measurement" => &["supported_by"],
        "change" => &["in_repository"],
        _ => &[],
    }
}
impl Admission<'_> {
    pub(crate) fn program_report(
        &self,
        snapshot: &impl Snapshot,
        root: Id,
        window: ReportWindow,
        now: u64,
    ) -> Result<ProgramReport, Error> {
        root.validate()?;
        if root.object_type != "program" || window.start > window.end {
            return Err(Error::InvalidRequest);
        }
        let root = snapshot
            .get(&root)?
            .filter(|o| self.permitted(o, false))
            .ok_or(Error::Unavailable)?;
        let Scope::Program { program_key, .. } = &root.scope else {
            return Err(Error::InvalidRequest);
        };
        let program_key = program_key.clone();
        let mut selected = BTreeMap::new();
        let mut pending = vec![root];
        while let Some(object) = pending.pop() {
            if selected.contains_key(&object.id) {
                continue;
            }
            if !self.permitted(&object, true) {
                return Err(Error::DeriveForbidden);
            }
            // Limits depend only on included visible basis, never hidden counts.
            if selected.len() >= 256 {
                return Err(Error::InvalidRequest);
            }
            for relation in relations(&object.id.object_type) {
                for id in snapshot.targets(&object.id, relation)? {
                    if selected.contains_key(&id) {
                        continue;
                    }
                    let Some(target) = snapshot.get(&id)?.filter(|o| self.permitted(o, false))
                    else {
                        continue;
                    };
                    if !matches!(&target.scope, Scope::Program { program_key:key,.. } if key == &program_key)
                    {
                        continue;
                    }
                    let h = head(&target)?;
                    if matches!(id.object_type.as_str(), "measurement" | "guardrail")
                        && !(window.start..=window.end).contains(&h.observed_at)
                    {
                        continue;
                    }
                    pending.push(target);
                }
            }
            selected.insert(object.id.clone(), object);
        }
        let mut limitations = BTreeSet::from([Limitation::AuthorizedBasisOnly]);
        let mut metrics = vec![];
        let mut attributions = vec![];
        let mut guardrails = vec![];
        let mut progress = false;
        let mut review = false;
        let mut unresolved = false;
        for object in selected.values() {
            let h = head(object)?;
            let head_qualified = qualify(h, now, &mut limitations)?;
            if !head_qualified {
                unresolved = true;
            }
            if object.id.object_type == "metric" {
                let parsed = value(h, "definition").and_then(definition);
                let mut samples = vec![];
                // Read only already admitted targets; no second Store pass/snapshot.
                for id in snapshot
                    .targets(&object.id, "has_measurement")?
                    .into_iter()
                    .collect::<BTreeSet<_>>()
                {
                    let Some(sample) = selected.get(&id) else {
                        continue;
                    };
                    let s = head(sample)?;
                    let qualified = qualify(s, now, &mut limitations)?;
                    samples.push(MeasurementStatement {
                        id: id.clone(),
                        revision: s.revision.clone(),
                        observed_at: s.observed_at,
                        fresh_until: s.fresh_until,
                        source_unit: field(s, "source_unit"),
                        definition_revision: field(s, "definition_revision"),
                        manifest_sha256: field(s, "manifest_sha256"),
                        algorithm: field(s, "algorithm"),
                        fraction: fraction(s)?.map(Into::into),
                        source_qualified: qualified,
                    });
                }
                samples.sort_by(|a, b| (a.observed_at, &a.id).cmp(&(b.observed_at, &b.id)));
                let mut delta = None;
                let mut pp = None;
                let mut qualified = head_qualified
                    && parsed.is_some()
                    && !samples.is_empty()
                    && samples.iter().all(|s| s.source_qualified);
                if let (Some(first), Some(last)) = (samples.first(), samples.last()) {
                    let same = |a: &Knowledge, b: &Knowledge| matches!((a,b),(Knowledge::Value(x),Knowledge::Value(y)) if x==y);
                    let comparable = samples.iter().all(|s| {
                        same(&s.source_unit, &first.source_unit)
                            && same(&s.definition_revision, &first.definition_revision)
                            && same(&s.manifest_sha256, &first.manifest_sha256)
                            && same(&s.algorithm, &first.algorithm)
                            && matches!((&s.fraction, &first.fraction), (Some(a),Some(b)) if a.denominator == b.denominator)
                    });
                    let known = samples.iter().all(|s| {
                        [
                            &s.source_unit,
                            &s.definition_revision,
                            &s.manifest_sha256,
                            &s.algorithm,
                        ]
                        .iter()
                        .all(|k| matches!(k, Knowledge::Value(_)))
                            && s.fraction.is_some()
                    });
                    let definition_matches = parsed.as_ref().is_some_and(|d| {
                        first.definition_revision
                            == Knowledge::Value(d.reference.revision.clone().into())
                            && d.unit == program::Unit::Fraction
                            && first.source_unit == Knowledge::Value("fraction".into())
                    });
                    let strictly_ordered = samples
                        .windows(2)
                        .all(|pair| pair[0].observed_at < pair[1].observed_at);
                    if samples.len() >= 2 && comparable && definition_matches && strictly_ordered {
                        if let (Some(a), Some(b)) = (&first.fraction, &last.fraction) {
                            if a.denominator == b.denominator {
                                let n = b
                                    .numerator
                                    .parse::<i128>()
                                    .map_err(|_| Error::InvalidEvidence)?
                                    - a.numerator
                                        .parse::<i128>()
                                        .map_err(|_| Error::InvalidEvidence)?;
                                let exact = Fraction::new(n, a.denominator)
                                    .map_err(|_| Error::InvalidEvidence)?;
                                delta = Some(exact.into());
                                pp = Some(
                                    Fraction::new(
                                        n.checked_mul(100).ok_or(Error::InvalidEvidence)?,
                                        a.denominator,
                                    )
                                    .map_err(|_| Error::InvalidEvidence)?
                                    .into(),
                                );
                                if qualified
                                    && parsed.as_ref().is_some_and(|d| match d.direction {
                                        program::Direction::Increase => n > 0,
                                        program::Direction::Decrease => n < 0,
                                    })
                                {
                                    progress = true;
                                }
                            } else {
                                qualified = false;
                                review = true;
                                limitations.insert(Limitation::IncompatibleDefinition);
                            }
                        } else {
                            qualified = false;
                        }
                    } else {
                        qualified = false;
                        if samples.len() >= 2 && parsed.is_some() && known {
                            review = true;
                            limitations.insert(Limitation::IncompatibleDefinition);
                        }
                    }
                }
                if !qualified {
                    unresolved = true;
                    if samples.is_empty() {
                        limitations.insert(Limitation::NotAvailableInAuthorizedResult);
                    }
                }
                metrics.push(MetricStatement {
                    metric: object.id.clone(),
                    definition: field(h, "definition"),
                    samples,
                    delta,
                    percentage_point_delta: pp,
                    source_qualified: qualified,
                });
            }
            if let Some(input) = value(h, "attribution") {
                let input: AttributionInput =
                    serde_json::from_value(input.clone()).map_err(|_| Error::InvalidEvidence)?;
                let qualified = qualify(h, now, &mut limitations)? && input.gaps.is_empty();
                if !qualified {
                    unresolved = true;
                    limitations.insert(Limitation::UnresolvedReconciliation);
                }
                attributions.push(AttributionStatement {
                    id: object.id.clone(),
                    revision: h.revision.clone(),
                    source_qualified: qualified,
                    attribution: input,
                });
            }
            if object.id.object_type == "guardrail" {
                let qualified =
                    head_qualified && value(h, "source_qualified") == Some(&Value::Bool(true));
                // PR5 can qualify the disk measurement while the GB/GiB bound
                // meaning remains unresolved (Partial with Unknown bound_value).
                // Retain its exact comparison without qualifying the statement
                // or recomputing the adapter's candidate thresholds here.
                let temporal =
                    program::qualify_basis(h, now).map_err(|_| Error::InvalidEvidence)?;
                let unresolved_disk_unit = !temporal.future
                    && !temporal.stale
                    && value(h, "source_qualified") == Some(&Value::Bool(true))
                    && value(h, "guardrail").and_then(Value::as_str) == Some("disk")
                    && value(h, "qualification").and_then(Value::as_str)
                        == Some("unresolved_disk_unit")
                    && value(h, "value").and_then(Value::as_u64).is_some();
                if unresolved_disk_unit {
                    review = true;
                }
                let bound = if qualified || unresolved_disk_unit {
                    field(h, "bound_satisfied")
                } else if matches!(field(h, "value"), Knowledge::NotPerformed) {
                    Knowledge::NotPerformed
                } else {
                    Knowledge::Unknown
                };
                if bound == Knowledge::Value(false.into()) {
                    review = true;
                }
                if !qualified || !matches!(bound, Knowledge::Value(Value::Bool(_))) {
                    unresolved = true;
                }
                guardrails.push(GuardrailStatement {
                    id: object.id.clone(),
                    revision: h.revision.clone(),
                    guardrail: field(h, "guardrail"),
                    unit: field(h, "unit"),
                    value: if qualified || unresolved_disk_unit {
                        field(h, "value")
                    } else if matches!(bound, Knowledge::NotPerformed) {
                        Knowledge::NotPerformed
                    } else {
                        Knowledge::Unknown
                    },
                    bound_satisfied: bound,
                    source_qualified: qualified,
                    qualification: field(h, "qualification"),
                });
            }
        }
        if metrics.is_empty() || guardrails.is_empty() {
            unresolved = true;
            limitations.insert(Limitation::NotAvailableInAuthorizedResult);
        }
        let guardrail_availability = if guardrails.is_empty() {
            Knowledge::Unknown
        } else {
            Knowledge::Value("visible_basis_only".into())
        };
        Ok(ProgramReport {
            window,
            assessment: if review || (!unresolved && !progress) {
                Assessment::ReviewRequired
            } else if unresolved || !progress {
                Assessment::InsufficientEvidence
            } else {
                Assessment::ProgressObserved
            },
            basis: selected.into_values().map(|o| o.view(now)).collect(),
            metrics,
            attributions,
            guardrails,
            guardrail_availability,
            limitations,
        })
    }
}
