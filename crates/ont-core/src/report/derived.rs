//! program-report-derivation/v1. Pure derivation from an already admitted report;
//! no Snapshot, authority lookup, source acquisition or hidden inventory access.
use super::{ExactFraction, ProgramReport, ReportWindow};
use crate::{Id, Knowledge, Observation, Scope, View};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const SCHEMA: &str = "ontology-program-report/v2";
pub const RULE: &str = "program-report-derivation/v1";
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Available<T> {
    Known { value: T },
    NotAvailable { reason: Reason },
}
impl<T> Available<T> {
    fn known(value: T) -> Self {
        Self::Known { value }
    }
    fn missing(reason: Reason) -> Self {
        Self::NotAvailable { reason }
    }
    pub fn get(&self) -> Option<&T> {
        if let Self::Known { value } = self {
            Some(value)
        } else {
            None
        }
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    MissingValue,
    MissingFractionBinding,
    ConflictingFractionBinding,
    UnsupportedFractionBinding,
    InvalidValue,
    MissingDefinition,
    MissingEndpoints,
    IncomparableComponents,
    AmbiguousTimestamp,
    EmptyDenominator,
    ArithmeticOverflow,
    NotApplicable,
    UnboundEndpoints,
    UnboundAggregateChange,
    MissingContributionEvidence,
    AmbiguousAssociation,
    MissingAssociation,
    InvalidAttribution,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BasisPin {
    pub id: Id,
    pub revision: String,
    pub observed_at: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Sample {
    pub basis: BasisPin,
    pub value: Available<ExactFraction>,
    pub source_qualified: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Endpoints {
    pub start: Sample,
    pub end: Sample,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Metric {
    pub metric: Option<Id>,
    pub scope: Scope,
    pub kind: String,
    pub definition: Available<Value>,
    pub unit: String,
    pub samples: Vec<Sample>,
    pub endpoints: Available<Endpoints>,
    pub population: Available<u64>,
    pub change: Available<ExactFraction>,
    pub percentage_point_change: Available<ExactFraction>,
    pub source_qualified: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Contribution {
    pub repository: Id,
    pub changes: Vec<Id>,
    pub commit: String,
    pub first_parent: String,
    pub change: Available<ExactFraction>,
    pub assignment: Available<Id>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Attribution {
    pub basis: BasisPin,
    pub definition: Available<Value>,
    pub population: Available<u64>,
    pub endpoints: Available<Endpoints>,
    pub contributions: Vec<Contribution>,
    pub change: Available<ExactFraction>,
    pub component_contribution_sum: Available<ExactFraction>,
    pub reconciliation_residual: Available<ExactFraction>,
    pub source_qualified: bool,
    pub qualification: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DerivedReport {
    pub schema: String,
    pub derivation_rule: String,
    pub window: ReportWindow,
    /// Assessment is retained from the native admitted report; derivation does
    /// not turn recorded arithmetic into source qualification or operational approval.
    pub assessment: super::Assessment,
    pub basis: Vec<View>,
    pub metrics: Vec<Metric>,
    pub attributions: Vec<Attribution>,
    pub guardrails: Vec<super::GuardrailStatement>,
    pub guardrail_availability: Knowledge,
    pub limitations: BTreeSet<super::Limitation>,
}
fn head(v: &View) -> Option<&Observation> {
    v.observations
        .iter()
        .find(|o| o.observation.revision == v.head)
        .map(|o| &o.observation)
}
fn val<'a>(o: &'a Observation, k: &str) -> Option<&'a Value> {
    super::value(o, k)
}
fn text<'a>(o: &'a Observation, k: &str) -> Option<&'a str> {
    val(o, k)?.as_str()
}
fn pin(v: &View) -> BasisPin {
    BasisPin {
        id: v.id.clone(),
        revision: v.head.clone(),
        observed_at: head(v).map_or(0, |h| h.observed_at),
    }
}
fn qualified(o: &Observation, now: u64) -> bool {
    super::qualification_facts(o, now).is_ok_and(|f| f.current())
}
fn qualified_at_observation(o: &Observation, now: u64) -> bool {
    super::qualification_facts(o, now).is_ok_and(|f| f.qualified_at_observation)
}
fn rational(n: i128, d: u64) -> Available<ExactFraction> {
    if d == 0 {
        Available::missing(Reason::EmptyDenominator)
    } else {
        Available::known(ExactFraction {
            numerator: n.to_string(),
            denominator: d,
        })
    }
}
/// Only native integer JSON values are exact count inputs. Decimal JSON numbers
/// have already lost their original lexeme at the Value boundary; do not infer a
/// rational by round-tripping their floating representation.
fn number(v: &Value) -> Option<ExactFraction> {
    let n = v
        .as_u64()
        .map(i128::from)
        .or_else(|| v.as_i64().map(i128::from))?;
    Some(ExactFraction {
        numerator: n.to_string(),
        denominator: 1,
    })
}
// This closed rule binds meaning once per series, never by field precedence.
fn fraction_binding(definition: &Available<Value>, kind: &str) -> Result<&'static str, Reason> {
    if let Available::NotAvailable { reason } = definition {
        return Err(*reason);
    }
    let formula = definition
        .get()
        .and_then(|d| d.get("formula"))
        .and_then(Value::as_str)
        .ok_or(Reason::MissingFractionBinding)?;
    let numerator = match formula {
        "removed / total" => "removed",
        "qualified / total" | "qualified / total adopted tools" => "qualified",
        "supported / total" | "supported / total evaluated answers" => "supported",
        _ => return Err(Reason::UnsupportedFractionBinding),
    };
    let selector_numerator = match kind {
        "kpi_mcp_parity" => Some("qualified"),
        "guard_recall_regression" => Some("supported"),
        _ => None,
    };
    if selector_numerator.is_some_and(|n| n != numerator) {
        return Err(Reason::ConflictingFractionBinding);
    }
    Ok(numerator)
}
fn flat_numerator(o: &Observation) -> Result<&'static str, Reason> {
    let mut names = ["removed", "qualified", "supported"]
        .into_iter()
        .filter(|k| o.fields.contains_key(*k));
    let first = names.next().ok_or(Reason::MissingFractionBinding)?;
    if names.next().is_some() {
        return Err(Reason::ConflictingFractionBinding);
    }
    Ok(first)
}
fn sample(v: &View, unit: &str, binding: Result<&str, Reason>, billing: bool, now: u64) -> Sample {
    let value = if let Some(o) = head(v) {
        if unit == "fraction" {
            match binding {
                Ok(n) => match (
                    val(o, n).and_then(Value::as_u64),
                    val(o, "total").and_then(Value::as_u64),
                ) {
                    (Some(n), Some(d)) if n <= d => rational(i128::from(n), d),
                    (Some(_), Some(0)) => Available::missing(Reason::EmptyDenominator),
                    _ => Available::missing(Reason::MissingValue),
                },
                Err(reason) => Available::missing(reason),
            }
        } else if let Some(n) = val(o, "value") {
            number(n)
                .map(Available::known)
                .unwrap_or_else(|| Available::missing(Reason::InvalidValue))
        } else {
            Available::missing(Reason::MissingValue)
        }
    } else {
        Available::missing(Reason::MissingValue)
    };
    Sample {
        basis: pin(v),
        value,
        source_qualified: head(v).is_some_and(|o| {
            qualified_at_observation(o, now)
                && (!billing || val(o, "source_qualified") == Some(&Value::Bool(true)))
        }),
    }
}
fn subtract(a: &ExactFraction, b: &ExactFraction) -> Available<ExactFraction> {
    let calc = || {
        let a_n = a.numerator.parse::<i128>().ok()?;
        let b_n = b.numerator.parse::<i128>().ok()?;
        if a.denominator == b.denominator {
            return Some((b_n.checked_sub(a_n)?, a.denominator));
        }
        Some((
            b_n.checked_mul(i128::from(a.denominator))?
                .checked_sub(a_n.checked_mul(i128::from(b.denominator))?)?,
            a.denominator.checked_mul(b.denominator)?,
        ))
    };
    calc()
        .map(|(n, d)| rational(n, d))
        .unwrap_or_else(|| Available::missing(Reason::ArithmeticOverflow))
}
fn pp(v: &Available<ExactFraction>) -> Available<ExactFraction> {
    match v {
        Available::Known { value: f } => f
            .numerator
            .parse::<i128>()
            .ok()
            .and_then(|n| n.checked_mul(100))
            .map(|n| rational(n, f.denominator))
            .unwrap_or_else(|| Available::missing(Reason::ArithmeticOverflow)),
        Available::NotAvailable { reason } => Available::missing(*reason),
    }
}
fn rule(selector: &str) -> Option<(&'static str, &'static str)> {
    match selector {
        "kpi_mcp_parity" => Some(("qualified / total adopted tools", "fraction")),
        "guard_ci_minutes_month" => Some(("recorded cumulative monthly CI usage", "linux_minutes")),
        "guard_disk_free_gb" => Some(("recorded free disk space", "GiB")),
        "guard_escaped_defects_week" => Some(("recorded escaped incidents in week", "count")),
        "guard_recall_regression" => Some(("supported / total evaluated answers", "fraction")),
        _ => None,
    }
}
fn metric(
    view: &View,
    inputs: Vec<&View>,
    kind: &str,
    unit: &str,
    definition: Available<Value>,
    now: u64,
) -> Metric {
    let mut inputs = inputs;
    inputs.sort_by(|a, b| (pin(a).observed_at, &a.id).cmp(&(pin(b).observed_at, &b.id)));
    let binding = fraction_binding(&definition, kind);
    let samples: Vec<_> = inputs
        .iter()
        .map(|v| sample(v, unit, binding, kind == "billing", now))
        .collect();
    let mut reason = if unit == "fraction" {
        binding.err()
    } else {
        None
    };
    if reason.is_some() {
        // Preserve the semantic binding failure across every sample and aggregate.
    } else if samples.len() < 2 {
        reason = Some(Reason::MissingEndpoints);
    } else if samples
        .windows(2)
        .any(|s| s[0].basis.observed_at == s[1].basis.observed_at)
    {
        reason = Some(Reason::AmbiguousTimestamp);
    } else if definition.get().is_none() {
        reason = Some(Reason::MissingDefinition);
    } else if samples.iter().any(|s| s.value.get().is_none()) {
        reason = Some(Reason::MissingValue);
    }
    // Scope and known contract pins define comparability. A selector alone never
    // makes a recorded legacy population or an unbound CI period comparable.
    if reason.is_none() {
        let first = head(inputs[0]);
        let explicit = definition.get().and_then(|v| v.get("id")).is_some();
        let required: &[&str] = if kind == "billing" {
            &["account", "window_start", "window_end", "unit"]
        } else if unit == "fraction" && kind == "kpi_mcp_parity" && !explicit {
            &["tool_set_sha256", "result_revision"]
        } else if unit == "fraction" {
            &[
                "definition_revision",
                "manifest_sha256",
                "algorithm",
                "source_unit",
            ]
        } else if explicit {
            &["definition_revision", "source_unit"]
        } else {
            &["source_unit"]
        };
        let same = first.is_some_and(|a| {
            inputs.iter().all(|v| {
                head(v).is_some_and(|b| {
                    let typed = required.iter().all(|k| {
                        if matches!(*k, "window_start" | "window_end") {
                            val(b, k).and_then(Value::as_u64).is_some()
                        } else {
                            text(b, k).is_some_and(|v| !v.is_empty())
                        }
                    });
                    let correct_unit = text(
                        b,
                        if kind == "billing" {
                            "unit"
                        } else {
                            "source_unit"
                        },
                    ) == Some(unit);
                    let period = kind != "billing"
                        || val(b, "window_start")
                            .and_then(Value::as_u64)
                            .zip(val(b, "window_end").and_then(Value::as_u64))
                            .is_some_and(|(start, end)| {
                                start < end && start <= b.observed_at && b.observed_at < end
                            });
                    typed
                        && correct_unit
                        && period
                        && required
                            .iter()
                            .all(|k| matches!((val(a,k),val(b,k)),(Some(x),Some(y)) if x==y))
                })
            })
        });
        let denominators = unit != "fraction"
            || samples.iter().all(|s| {
                s.value.get().map(|f| f.denominator)
                    == samples[0].value.get().map(|f| f.denominator)
            });
        let explicit_matches = !explicit
            || first.is_some_and(|o| {
                definition.get().is_some_and(|d| {
                    d.get("unit").and_then(Value::as_str) == Some(unit)
                        && d.get("revision") == val(o, "definition_revision")
                })
            });
        // Legacy guardrail selectors without period identity expose samples,
        // but their temporal difference is not a billing/weekly comparison.
        if !same
            || !denominators
            || !explicit_matches
            || matches!(
                kind,
                "guard_ci_minutes_month" | "guard_escaped_defects_week"
            )
        {
            reason = Some(Reason::IncomparableComponents);
        }
    }
    let endpoints = match (samples.first(), samples.last()) {
        _ if samples.len() < 2 => Available::missing(Reason::MissingEndpoints),
        _ if reason == Some(Reason::AmbiguousTimestamp) => {
            Available::missing(Reason::AmbiguousTimestamp)
        }
        (Some(a), Some(b)) => Available::known(Endpoints {
            start: a.clone(),
            end: b.clone(),
        }),
        _ => Available::missing(Reason::MissingEndpoints),
    };
    // No number without qualification: a stale latest sample publishes
    // no change, unless a specific missing reason already applies.
    let latest_stale = inputs.last().is_some_and(|v| {
        head(v).is_some_and(|o| super::qualification_facts(o, now).is_ok_and(|f| f.stale))
    });
    let change = if let Some(reason) = reason {
        Available::missing(reason)
    } else if latest_stale {
        Available::missing(Reason::MissingValue)
    } else if let (Some(a), Some(b)) = (
        samples.first().and_then(|s| s.value.get()),
        samples.last().and_then(|s| s.value.get()),
    ) {
        subtract(a, b)
    } else {
        Available::missing(Reason::MissingValue)
    };
    let population = if unit == "fraction"
        && samples.windows(2).any(|s| {
            s[0].value.get().map(|f| f.denominator) != s[1].value.get().map(|f| f.denominator)
        }) {
        Available::missing(Reason::IncomparableComponents)
    } else if unit == "fraction" {
        samples
            .first()
            .and_then(|s| s.value.get())
            .map(|f| Available::known(f.denominator))
            .unwrap_or_else(|| Available::missing(Reason::MissingValue))
    } else {
        Available::missing(Reason::NotApplicable)
    };
    let percentage_point_change = if unit == "fraction" {
        pp(&change)
    } else {
        Available::missing(Reason::NotApplicable)
    };
    Metric {
        metric: (view.id.object_type == "metric").then(|| view.id.clone()),
        scope: view.scope.clone(),
        kind: kind.into(),
        definition,
        unit: unit.into(),
        // With a metric object its own head must be current; without one
        // the head condition comes from the latest sample by the sort key.
        source_qualified: (if view.id.object_type == "metric" {
            head(view).is_some_and(|o| qualified(o, now))
        } else {
            inputs
                .last()
                .is_some_and(|v| head(v).is_some_and(|o| qualified(o, now)))
        }) && !samples.is_empty()
            && samples.iter().all(|s| s.source_qualified)
            && !latest_stale
            && change.get().is_some(),
        samples,
        endpoints,
        population,
        change,
        percentage_point_change,
    }
}
/// Called only after native selection, under the caller's original Admission.
pub(super) fn derive(report: ProgramReport, now: u64) -> DerivedReport {
    let by_id: BTreeMap<_, _> = report.basis.iter().map(|v| (&v.id, v)).collect();
    let mut metrics = vec![];
    let mut used = BTreeSet::new();
    for m in &report.metrics {
        let Some(view) = by_id.get(&m.metric) else {
            continue;
        };
        let Some(o) = head(view) else { continue };
        let selector = text(o, "selector").unwrap_or("defined_metric");
        let explicit = val(o, "definition").filter(|d| super::definition(d).is_some());
        let fallback = rule(selector);
        let definition=explicit.cloned().or_else(||fallback.map(|(formula,unit)|serde_json::json!({"rule":RULE,"selector":selector,"formula":formula,"unit":unit,"role":"indicator"}))).map(Available::known).unwrap_or_else(||Available::missing(Reason::MissingDefinition));
        let unit = explicit
            .and_then(|d| d.get("unit"))
            .and_then(Value::as_str)
            .or_else(|| fallback.map(|(_, u)| u))
            .unwrap_or("unknown");
        let inputs = m
            .samples
            .iter()
            .filter_map(|s| by_id.get(&s.id).copied())
            .collect::<Vec<_>>();
        for v in &inputs {
            used.insert(v.id.clone());
        }
        metrics.push(metric(view, inputs, selector, unit, definition, now));
    }
    // Billing has its own explicit period contract. Different accounts/months
    // remain different series, even when source values happen to match.
    let mut billing: BTreeMap<String, Vec<&View>> = BTreeMap::new();
    let mut direct: BTreeMap<String, Vec<&View>> = BTreeMap::new();
    for v in &report.basis {
        let Some(o) = head(v) else { continue };
        if v.id.object_type == "guardrail" && text(o, "guardrail") == Some("billing") {
            let key = serde_json::json!([
                v.scope,
                val(o, "account"),
                val(o, "window_start"),
                val(o, "window_end")
            ])
            .to_string();
            billing.entry(key).or_default().push(v);
        } else if v.id.object_type == "measurement"
            && !used.contains(&v.id)
            && val(o, "attribution").is_none()
            && ["removed", "qualified", "supported"]
                .iter()
                .any(|k| o.fields.contains_key(*k))
            && o.fields.contains_key("total")
        {
            // Flat snapshot rows can be self describing even without a metric
            // handle. Group only exact declared population/definition identities.
            // Incomplete component pins stay separate and cannot imply a total.
            let complete = [
                "definition_revision",
                "manifest_sha256",
                "algorithm",
                "source_unit",
            ]
            .iter()
            .all(|k| val(o, k).is_some());
            let kind = flat_numerator(o);
            let key = if complete && kind.is_ok() {
                serde_json::json!([
                    v.scope,
                    kind.ok(),
                    val(o, "definition_revision"),
                    val(o, "manifest_sha256"),
                    val(o, "algorithm"),
                    val(o, "source_unit")
                ])
                .to_string()
            } else {
                serde_json::json!([v.id]).to_string()
            };
            direct.entry(key).or_default().push(v);
        }
    }
    for inputs in direct.into_values() {
        let view = inputs[0];
        let definition = head(view).filter(|o|["definition_revision","manifest_sha256","algorithm","source_unit"].iter().all(|k|val(o,k).is_some())).map(|o| {
            match flat_numerator(o) {
                Ok(n) => Available::known(serde_json::json!({"rule":RULE,"formula":format!("{n} / total"),"unit":"fraction","role":"indicator","definition_revision":val(o,"definition_revision"),"manifest_sha256":val(o,"manifest_sha256"),"algorithm":val(o,"algorithm")})),
                Err(reason) => Available::missing(reason),
            }
        }).unwrap_or_else(||Available::missing(Reason::MissingDefinition));
        metrics.push(metric(
            view,
            inputs,
            "snapshot_component",
            "fraction",
            definition,
            now,
        ));
    }
    for inputs in billing.into_values() {
        let view = inputs[0];
        metrics.push(metric(view,inputs,"billing","linux_minutes",Available::known(serde_json::json!({"rule":RULE,"formula":"cumulative Linux minutes in exact account/month","unit":"linux_minutes","role":"indicator"})),now));
    }
    let mut attributions = vec![];
    let mut assigned = BTreeSet::new();
    for a in &report.attributions {
        let input = &a.attribution;
        if !input.contributions.iter().all(|c| {
            by_id.contains_key(&c.repository) && c.changes.iter().all(|id| by_id.contains_key(id))
        }) {
            continue;
        }
        let Some(v) = by_id.get(&a.id) else { continue };
        let mut seen = BTreeSet::new();
        let mut seen_assignments = BTreeSet::new();
        let mut sum = Some(0_i128);
        let mut valid = input.rule == "attribution-v1";
        let mut contributions: Vec<Contribution> = input
            .contributions
            .iter()
            .map(|c| {
                assigned.extend(c.changes.iter().cloned());
                valid &= seen.insert((c.repository.clone(), c.commit.clone()));
                let n = i128::from(c.newly_absent_count) - i128::from(c.restored_count);
                valid &= c.paths.parse::<i128>().ok() == Some(n);
                sum = sum.and_then(|s| s.checked_add(n));
                let assignment = if c.association == "unique" && c.changes.len() == 1 {
                    valid &= seen_assignments.insert(c.changes[0].clone());
                    Available::known(c.changes[0].clone())
                } else {
                    Available::missing(if c.association == "ambiguous" {
                        Reason::AmbiguousAssociation
                    } else {
                        Reason::MissingAssociation
                    })
                };
                Contribution {
                    repository: c.repository.clone(),
                    changes: c.changes.clone(),
                    commit: c.commit.clone(),
                    first_parent: c.first_parent.clone(),
                    change: rational(n, input.denominator),
                    assignment,
                }
            })
            .collect();
        let bound = input
            .delta_bound
            .as_ref()
            .and_then(|n| n.parse::<i128>().ok());
        let residual = bound.zip(sum).and_then(|(n, s)| n.checked_sub(s));
        valid &= input
            .residual
            .as_ref()
            .is_none_or(|n| n.parse::<i128>().ok() == residual);
        if !valid {
            for contribution in &mut contributions {
                contribution.change = Available::missing(Reason::InvalidAttribution);
                contribution.assignment = Available::missing(Reason::InvalidAttribution);
            }
        }
        let associations_resolved = contributions.iter().all(|c| c.assignment.get().is_some());
        let missing = || Available::missing(Reason::InvalidAttribution);
        let producer_qualified = matches!(
            input.qualification.as_str(),
            crate::program::QUALIFIED_PR_ATTRIBUTION
                | crate::program::QUALIFIED_EMPTY_INTERVAL
                | "qualified"
        );
        let source_qualified =
            a.source_qualified && valid && input.denominator > 0 && producer_qualified;
        let reconciled = valid
            && associations_resolved
            && residual == Some(0)
            && input.gaps.is_empty()
            && producer_qualified;
        attributions.push(Attribution {
            basis: pin(v),
            definition: Available::known(serde_json::json!({
                "id": input.definition.id, "revision": input.definition.revision,
                "manifest_sha256": input.manifest_sha256, "algorithm": input.algorithm
            })),
            population: if input.denominator == 0 {
                Available::missing(Reason::EmptyDenominator)
            } else {
                Available::known(input.denominator)
            },
            endpoints: Available::missing(Reason::UnboundEndpoints),
            contributions,
            change: if !valid {
                missing()
            } else {
                bound
                    .map(|n| rational(n, input.denominator))
                    .unwrap_or_else(|| Available::missing(Reason::UnboundAggregateChange))
            },
            component_contribution_sum: if !valid {
                missing()
            } else {
                sum.map(|n| rational(n, input.denominator))
                    .unwrap_or_else(|| Available::missing(Reason::ArithmeticOverflow))
            },
            reconciliation_residual: if !valid {
                missing()
            } else {
                residual
                    .map(|n| rational(n, input.denominator))
                    .unwrap_or_else(|| Available::missing(Reason::UnboundAggregateChange))
            },
            source_qualified,
            qualification: if reconciled {
                "qualified".into()
            } else {
                "unresolved_reconciliation".into()
            },
        });
    }
    for v in &report.basis {
        let Some(o) = head(v) else { continue };
        if v.id.object_type != "change"
            || assigned.contains(&v.id)
            || val(o, "merged") != Some(&Value::Bool(true))
        {
            continue;
        }
        attributions.push(Attribution {
            basis: pin(v),
            definition: Available::missing(Reason::MissingDefinition),
            population: Available::missing(Reason::MissingDefinition),
            endpoints: Available::missing(Reason::UnboundEndpoints),
            contributions: vec![],
            change: Available::missing(Reason::MissingContributionEvidence),
            component_contribution_sum: Available::missing(Reason::MissingContributionEvidence),
            reconciliation_residual: Available::missing(Reason::UnboundAggregateChange),
            source_qualified: false,
            qualification: "unresolved_reconciliation".into(),
        });
    }
    DerivedReport {
        schema: SCHEMA.into(),
        derivation_rule: RULE.into(),
        window: report.window,
        assessment: report.assessment,
        basis: report.basis,
        metrics,
        attributions,
        guardrails: report.guardrails,
        guardrail_availability: report.guardrail_availability,
        limitations: report.limitations,
    }
}
