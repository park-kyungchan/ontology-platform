//! Slice A's trusted, pure program contract. No store, source retrieval or authority.
//!
//! Adapters must authenticate source identities and content-bind declared pins to
//! actual bytes/Git objects before calling these functions. A syntactically valid
//! digest, declared first parent or complete flag does not prove source retrieval.
//! Report callers must authorize every input before derivation/publication.
use crate::{Coverage, Id, Knowledge, Lineage, Observation};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleError {
    InvalidDefinition,
    InvalidWindow,
    IncompatibleUnit,
    InvalidEvidence,
    IncompleteEvidence,
    UnsupportedAlgorithm,
    IntegrityFailure,
    ArithmeticOverflow,
}
impl RuleError {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidDefinition => "invalid_program_definition",
            Self::InvalidWindow => "invalid_program_window",
            Self::IncompatibleUnit => "incompatible_program_unit",
            Self::InvalidEvidence => "invalid_program_evidence",
            Self::IncompleteEvidence => "incomplete_program_evidence",
            Self::UnsupportedAlgorithm => "unsupported_path_algorithm",
            Self::IntegrityFailure => "attribution_integrity_failure",
            Self::ArithmeticOverflow => "program_arithmetic_overflow",
        }
    }
}
impl std::fmt::Display for RuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for RuleError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionRef {
    pub id: Id,
    pub revision: String,
}
impl DefinitionRef {
    pub fn validate(&self) -> Result<(), RuleError> {
        self.id
            .validate()
            .map_err(|_| RuleError::InvalidDefinition)?;
        if self.revision.is_empty() {
            return Err(RuleError::InvalidDefinition);
        }
        Ok(())
    }
}

/// Only the units needed by this slice; these carry no conversion/coercion rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Count,
    Paths,
    Fraction,
    LinuxMinutes,
    Bytes,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Increase,
    Decrease,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetricRole {
    Indicator,
    /// Adoption must cite the user-owned strategy and its exact decision artifact.
    Kpi {
        strategy: DefinitionRef,
        adoption: Lineage,
    },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricDefinition {
    pub reference: DefinitionRef,
    pub formula: String,
    pub population: String,
    pub unit: Unit,
    pub direction: Direction,
    pub role: MetricRole,
}
impl MetricDefinition {
    pub fn validate(&self) -> Result<(), RuleError> {
        self.reference.validate()?;
        if self.formula.is_empty() || self.population.is_empty() {
            return Err(RuleError::InvalidDefinition);
        }
        if let MetricRole::Kpi { strategy, adoption } = &self.role {
            strategy.validate()?;
            if !hex_pin(&adoption.artifact_sha256, &[64])
                || (!adoption.pointer.is_empty() && !adoption.pointer.starts_with('/'))
            {
                return Err(RuleError::InvalidDefinition);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Window {
    Instant(u64),
    /// UTC Unix seconds; no inferred calendar/month or sampling simultaneity.
    Interval {
        start: u64,
        end: u64,
    },
    /// An accepted relative deadline remains relative until the trigger is observed.
    AfterTrigger {
        trigger: Id,
        within_seconds: u64,
    },
}
impl Window {
    pub fn validate(&self) -> Result<(), RuleError> {
        match self {
            Self::Interval { start, end } if start >= end => Err(RuleError::InvalidWindow),
            Self::AfterTrigger {
                trigger,
                within_seconds,
            } => {
                trigger.validate().map_err(|_| RuleError::InvalidWindow)?;
                if *within_seconds == 0 {
                    return Err(RuleError::InvalidWindow);
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
    /// `None` means no observed trigger, never an inferred date. The adapter must
    /// bind any supplied trigger time to this window's exact trigger identity.
    pub fn deadline(&self, observed_trigger: Option<u64>) -> Result<Option<u64>, RuleError> {
        self.validate()?;
        match self {
            Self::Instant(at) => Ok(Some(*at)),
            Self::Interval { end, .. } => Ok(Some(*end)),
            Self::AfterTrigger { within_seconds, .. } => observed_trigger
                .map(|at| {
                    at.checked_add(*within_seconds)
                        .ok_or(RuleError::ArithmeticOverflow)
                })
                .transpose(),
        }
    }
}

/// Exact, signed fraction. Denominator is retained rather than normalized away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fraction {
    numerator: i128,
    denominator: u64,
}
impl Fraction {
    pub fn new(numerator: i128, denominator: u64) -> Result<Self, RuleError> {
        if denominator == 0 {
            return Err(RuleError::InvalidDefinition);
        }
        Ok(Self {
            numerator,
            denominator,
        })
    }
    pub fn numerator(self) -> i128 {
        self.numerator
    }
    pub fn denominator(self) -> u64 {
        self.denominator
    }

    /// Display only: nearest, ties away from zero, explicitly labelled; no floats.
    pub fn rounded_percent(self, decimal_places: u8) -> Result<RoundedPercent, RuleError> {
        if decimal_places > 9 {
            return Err(RuleError::InvalidDefinition);
        }
        let scale = 10_i128.pow(u32::from(decimal_places));
        let scaled = self
            .numerator
            .checked_mul(100)
            .and_then(|v| v.checked_mul(scale))
            .ok_or(RuleError::ArithmeticOverflow)?;
        let magnitude = scaled.checked_abs().ok_or(RuleError::ArithmeticOverflow)?;
        let denominator = i128::from(self.denominator);
        let mut rounded = magnitude / denominator;
        if (magnitude % denominator) * 2 >= denominator {
            rounded = rounded
                .checked_add(1)
                .ok_or(RuleError::ArithmeticOverflow)?;
        }
        let sign = if scaled < 0 && rounded != 0 { "-" } else { "" };
        let text = if decimal_places == 0 {
            format!("{sign}{rounded}%")
        } else {
            format!(
                "{sign}{}.{:0width$}%",
                rounded / scale,
                rounded % scale,
                width = usize::from(decimal_places)
            )
        };
        Ok(RoundedPercent {
            exact: self,
            text,
            decimal_places,
            rule: "nearest_ties_away_from_zero",
        })
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoundedPercent {
    pub exact: Fraction,
    pub text: String,
    pub decimal_places: u8,
    pub rule: &'static str,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundDirection {
    AtMost,
    AtLeast,
    Exactly,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub definition: DefinitionRef,
    pub unit: Unit,
    pub window: Window,
    pub direction: BoundDirection,
    pub threshold: Fraction,
}
impl Target {
    pub fn validate(&self, definition: &MetricDefinition) -> Result<(), RuleError> {
        definition.validate()?;
        self.window.validate()?;
        if self.definition != definition.reference {
            return Err(RuleError::InvalidDefinition);
        }
        if self.unit != definition.unit {
            return Err(RuleError::IncompatibleUnit);
        }
        Ok(())
    }
    /// A comparison needs the exact evaluation window, not an adjacent sample.
    pub fn validate_measurement(
        &self,
        definition: &DefinitionRef,
        unit: Unit,
        window: &Window,
    ) -> Result<(), RuleError> {
        self.definition.validate()?;
        definition.validate()?;
        self.window.validate()?;
        window.validate()?;
        if definition != &self.definition {
            return Err(RuleError::InvalidDefinition);
        }
        if unit != self.unit {
            return Err(RuleError::IncompatibleUnit);
        }
        if window != &self.window {
            return Err(RuleError::InvalidWindow);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guardrail {
    pub bound: Target,
    /// In the bound's exact unit/definition; satisfying non-regression alone does
    /// not imply satisfying the target. Assessment belongs to the report layer.
    pub non_regression_baseline: Option<Fraction>,
}
impl Guardrail {
    pub fn validate(&self, definition: &MetricDefinition) -> Result<(), RuleError> {
        self.bound.validate(definition)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BasisQualification {
    pub future: bool,
    pub stale: bool,
    pub partial: bool,
    pub unresolved: bool,
}
impl BasisQualification {
    pub fn is_current(self) -> bool {
        !self.future && !self.stale && !self.partial && !self.unresolved
    }
}
/// Does not change S06's expiry-only semantics; this is Program qualification.
pub fn qualify_basis(observation: &Observation, now: u64) -> Result<BasisQualification, RuleError> {
    observation
        .validate()
        .map_err(|_| RuleError::InvalidEvidence)?;
    Ok(BasisQualification {
        future: now < observation.observed_at,
        stale: now > observation.fresh_until,
        partial: observation.coverage != Coverage::Complete,
        unresolved: observation.fields.is_empty()
            || observation
                .fields
                .values()
                .any(|v| !matches!(v, Knowledge::Value(_))),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathAlgorithm {
    ExactUtf8PathsV1,
    /// Historical identity only; this module never reproduces the unsafe producer.
    ProducerV1RenderedPaths,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathBasis {
    pub definition: DefinitionRef,
    pub manifest_sha256: String,
    pub adapter_revision: String,
    pub algorithm: PathAlgorithm,
    pub denominator: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathPopulation {
    pub basis: PathBasis,
    /// Non-KEEP paths, exact UTF-8; repository identity is independent of checkout.
    pub repositories: BTreeMap<Id, BTreeSet<String>>,
}
impl PathPopulation {
    pub fn validate(&self) -> Result<(), RuleError> {
        self.basis.definition.validate()?;
        if !hex_pin(&self.basis.manifest_sha256, &[64])
            || self.basis.adapter_revision.is_empty()
            || self.basis.denominator == 0
            || self.repositories.is_empty()
        {
            return Err(RuleError::InvalidDefinition);
        }
        let mut count = 0_u64;
        for (repository, paths) in &self.repositories {
            repository
                .validate()
                .map_err(|_| RuleError::InvalidDefinition)?;
            validate_paths(paths)?;
            count = count
                .checked_add(u64::try_from(paths.len()).map_err(|_| RuleError::ArithmeticOverflow)?)
                .ok_or(RuleError::ArithmeticOverflow)?;
        }
        if count != self.basis.denominator {
            return Err(RuleError::InvalidDefinition);
        }
        if self.basis.algorithm != PathAlgorithm::ExactUtf8PathsV1 {
            return Err(RuleError::UnsupportedAlgorithm);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tree {
    pub commit: String,
    pub tree: String,
    pub paths: BTreeSet<String>,
    pub coverage: Coverage,
}
impl Tree {
    fn validate_pins(&self) -> Result<(), RuleError> {
        if !hex_pin(&self.commit, &[40, 64]) || !hex_pin(&self.tree, &[self.commit.len()]) {
            return Err(RuleError::InvalidEvidence);
        }
        validate_paths(&self.paths)
    }
    fn validate(&self) -> Result<(), RuleError> {
        self.validate_pins()?;
        if self.coverage != Coverage::Complete {
            return Err(RuleError::IncompleteEvidence);
        }
        Ok(())
    }
}
fn validate_paths(paths: &BTreeSet<String>) -> Result<(), RuleError> {
    if paths.iter().any(|p| p.is_empty() || p.contains('\0')) {
        return Err(RuleError::InvalidEvidence);
    }
    Ok(())
}
fn hex_pin(pin: &str, lengths: &[usize]) -> bool {
    lengths.contains(&pin.len()) && pin.bytes().all(|b| b.is_ascii_hexdigit())
}
fn absent(population: &BTreeSet<String>, tree: &Tree) -> BTreeSet<String> {
    population.difference(&tree.paths).cloned().collect()
}
/// All adopted repositories are required. Extra/missing repositories cannot
/// quietly alter the denominator; incomplete retrieval never counts as absence.
pub fn path_absence(
    population: &PathPopulation,
    trees: &BTreeMap<Id, Tree>,
) -> Result<Fraction, RuleError> {
    population.validate()?;
    if population.repositories.keys().ne(trees.keys()) {
        return Err(RuleError::IncompleteEvidence);
    }
    let mut numerator = 0_i128;
    for (repository, paths) in &population.repositories {
        let tree = &trees[repository];
        tree.validate()?;
        numerator = numerator
            .checked_add(absent(paths, tree).len() as i128)
            .ok_or(RuleError::ArithmeticOverflow)?;
    }
    Fraction::new(numerator, population.basis.denominator)
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PrId {
    pub repository: Id,
    pub number: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrCandidate {
    pub id: PrId,
    pub merged: bool,
    pub merge_commit: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrSearch {
    pub coverage: Coverage,
    pub candidates: Vec<PrCandidate>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrAssociation {
    Unique(PrId),
    Ambiguous(BTreeSet<PrId>),
    Missing,
    Unknown { eligible: BTreeSet<PrId> },
}
fn associate(repository: &Id, commit: &str, search: &PrSearch) -> Result<PrAssociation, RuleError> {
    let mut seen = BTreeMap::new();
    let mut eligible = BTreeSet::new();
    for candidate in &search.candidates {
        candidate
            .id
            .repository
            .validate()
            .map_err(|_| RuleError::InvalidEvidence)?;
        if (candidate.merged && candidate.merge_commit.is_none())
            || candidate.id.number == 0
            || candidate
                .merge_commit
                .as_ref()
                .is_some_and(|p| !hex_pin(p, &[40, 64]))
        {
            return Err(RuleError::InvalidEvidence);
        }
        if let Some(previous) = seen.insert(&candidate.id, candidate)
            && previous != candidate
        {
            return Err(RuleError::IntegrityFailure);
        }
        if candidate.id.repository == *repository
            && candidate.merged
            && candidate.merge_commit.as_deref() == Some(commit)
        {
            eligible.insert(candidate.id.clone());
        }
    }
    if search.coverage != Coverage::Complete {
        return Ok(PrAssociation::Unknown { eligible });
    }
    match eligible.len() {
        0 => Ok(PrAssociation::Missing),
        1 => Ok(PrAssociation::Unique(
            eligible
                .into_iter()
                .next()
                .ok_or(RuleError::IntegrityFailure)?,
        )),
        _ => Ok(PrAssociation::Ambiguous(eligible)),
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureInterval {
    pub start: u64,
    pub end: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub basis: PathBasis,
    pub tree: Tree,
    /// None means the original source measurement is not bound to this endpoint.
    pub source_measurement: Option<Id>,
    pub observed_at: u64,
    pub capture: CaptureInterval,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainlineChange {
    /// Actual first parent and accepted commit; never a proposal/PR-head diff.
    pub first_parent: Tree,
    pub accepted: Tree,
    pub prs: PrSearch,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryInterval {
    pub repository: Id,
    pub before: Endpoint,
    pub after: Endpoint,
    /// Full ordered first-parent metadata chain. Individual internal tree path
    /// captures may be partial; those steps never yield a contribution.
    pub changes: Vec<MainlineChange>,
    pub coverage: Coverage,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeContribution {
    pub repository: Id,
    pub commit: String,
    pub first_parent: String,
    pub newly_absent: BTreeSet<String>,
    pub restored: BTreeSet<String>,
    pub paths: i128,
    /// Signed exact percentage-point delta is `fraction * 100`, not percent change.
    pub fraction: Fraction,
    pub association: PrAssociation,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReconciliationGap {
    EndpointBinding,
    DefinitionMismatch,
    FirstParentCoverage,
    PrAssociation,
    FutureEndpoint,
}
pub const UNRESOLVED_RECONCILIATION: &str =
    "qualified merge contributions + unresolved reconciliation";
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribution {
    pub basis: PathBasis,
    pub contributions: Vec<MergeContribution>,
    pub delta_bound: Option<i128>,
    pub residual: Option<i128>,
    pub gaps: BTreeSet<ReconciliationGap>,
}
impl Attribution {
    pub fn has_unique_pr_explanation(&self) -> bool {
        self.gaps.is_empty() && !self.contributions.is_empty()
    }
    pub fn qualification(&self) -> &'static str {
        if self.gaps.is_empty() && self.contributions.is_empty() {
            "qualified empty interval; no PR attribution"
        } else if self.has_unique_pr_explanation() {
            "qualified PR attribution"
        } else {
            UNRESOLVED_RECONCILIATION
        }
    }
}

/// Attribution rule v1. Requires the full first-parent metadata chain to prove
/// interval membership. Partial tree captures retain only independently complete
/// steps and an unresolved residual. Missing/disconnected metadata refuses; the
/// adapter must keep the interval unresolved, never invent a chain or an empty tree.
/// Component Git facts survive an unbound/incompatible measured interval. The
/// measured interval's delta and residual then stay unknown. No nearest-PR guess.
pub fn attribute_v1(
    population: &PathPopulation,
    intervals: &[RepositoryInterval],
    now: u64,
) -> Result<Attribution, RuleError> {
    population.validate()?;
    let mut result = Attribution {
        basis: population.basis.clone(),
        contributions: vec![],
        delta_bound: None,
        residual: None,
        gaps: BTreeSet::new(),
    };
    let mut repositories = BTreeSet::new();
    let mut total_delta = 0_i128;
    let mut total_contribution = 0_i128;
    for interval in intervals {
        if !repositories.insert(interval.repository.clone()) {
            return Err(RuleError::IntegrityFailure);
        }
        let paths = population
            .repositories
            .get(&interval.repository)
            .ok_or(RuleError::InvalidEvidence)?;
        let mut pins = BTreeMap::new();
        for endpoint in [&interval.before, &interval.after] {
            endpoint.tree.validate()?;
            record_tree(&mut pins, &endpoint.tree)?;
            if endpoint.capture.start > endpoint.capture.end {
                return Err(RuleError::InvalidWindow);
            }
            if now < endpoint.observed_at || now < endpoint.capture.end {
                result.gaps.insert(ReconciliationGap::FutureEndpoint);
            }
            if let Some(id) = &endpoint.source_measurement {
                id.validate().map_err(|_| RuleError::InvalidEvidence)?;
            } else {
                result.gaps.insert(ReconciliationGap::EndpointBinding);
            }
            if endpoint.basis != population.basis {
                result.gaps.insert(ReconciliationGap::DefinitionMismatch);
            }
        }
        total_delta = total_delta
            .checked_add(
                absent(paths, &interval.after.tree).len() as i128
                    - absent(paths, &interval.before.tree).len() as i128,
            )
            .ok_or(RuleError::ArithmeticOverflow)?;
        let mut preceding = &interval.before.tree;
        let mut seen = BTreeSet::from([preceding.commit.clone()]);
        for change in &interval.changes {
            record_tree(&mut pins, &change.first_parent)?;
            record_tree(&mut pins, &change.accepted)?;
            if &change.first_parent != preceding
                || !seen.insert(change.accepted.commit.clone())
                || preceding.commit == interval.after.tree.commit
            {
                return Err(RuleError::IntegrityFailure);
            }
            if change.first_parent.coverage != Coverage::Complete
                || change.accepted.coverage != Coverage::Complete
            {
                if interval.coverage == Coverage::Complete {
                    return Err(RuleError::IncompleteEvidence);
                }
                result.gaps.insert(ReconciliationGap::FirstParentCoverage);
                preceding = &change.accepted;
                continue;
            }
            let old = absent(paths, &change.first_parent);
            let new = absent(paths, &change.accepted);
            let newly_absent: BTreeSet<_> = new.difference(&old).cloned().collect();
            let restored: BTreeSet<_> = old.difference(&new).cloned().collect();
            let count = newly_absent.len() as i128 - restored.len() as i128;
            let association =
                associate(&interval.repository, &change.accepted.commit, &change.prs)?;
            if !matches!(association, PrAssociation::Unique(_)) {
                result.gaps.insert(ReconciliationGap::PrAssociation);
            }
            total_contribution = total_contribution
                .checked_add(count)
                .ok_or(RuleError::ArithmeticOverflow)?;
            result.contributions.push(MergeContribution {
                repository: interval.repository.clone(),
                commit: change.accepted.commit.clone(),
                first_parent: change.first_parent.commit.clone(),
                newly_absent,
                restored,
                paths: count,
                fraction: Fraction::new(count, population.basis.denominator)?,
                association,
            });
            preceding = &change.accepted;
        }
        if preceding != &interval.after.tree {
            return Err(RuleError::IntegrityFailure);
        }
        if interval.coverage != Coverage::Complete {
            result.gaps.insert(ReconciliationGap::FirstParentCoverage);
        }
    }
    if repositories.iter().ne(population.repositories.keys()) {
        result.gaps.insert(ReconciliationGap::FirstParentCoverage);
        result.gaps.insert(ReconciliationGap::EndpointBinding);
    }
    if !result.gaps.contains(&ReconciliationGap::EndpointBinding)
        && !result.gaps.contains(&ReconciliationGap::DefinitionMismatch)
    {
        let residual = total_delta
            .checked_sub(total_contribution)
            .ok_or(RuleError::ArithmeticOverflow)?;
        if !result
            .gaps
            .contains(&ReconciliationGap::FirstParentCoverage)
            && residual != 0
        {
            return Err(RuleError::IntegrityFailure);
        }
        result.delta_bound = Some(total_delta);
        result.residual = Some(residual);
    }
    // Stable structured repository order independent of input interval ordering.
    // Each repository's changes retain their proven first-parent order.
    result
        .contributions
        .sort_by(|a, b| a.repository.cmp(&b.repository));
    Ok(result)
}
fn record_tree<'a>(
    pins: &mut BTreeMap<&'a str, &'a Tree>,
    tree: &'a Tree,
) -> Result<(), RuleError> {
    tree.validate_pins()?;
    if let Some(previous) = pins.insert(&tree.commit, tree)
        && previous != tree
    {
        return Err(RuleError::IntegrityFailure);
    }
    // The same immutable tree object cannot describe different path sets.
    if pins.values().any(|p| {
        p.tree == tree.tree
            && p.paths != tree.paths
            && p.coverage == Coverage::Complete
            && tree.coverage == Coverage::Complete
    }) {
        return Err(RuleError::IntegrityFailure);
    }
    Ok(())
}
