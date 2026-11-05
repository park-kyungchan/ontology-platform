//! Wire schema mirrored from agent-platform 6e252fd, model.rs. No collector dependency.
use super::{IntakeError, Result, digest, hex, text};
use serde::{Deserialize, Serialize};

pub const SCHEMA: &str = "ws-replan-kpi-snapshot/v2";
pub const DEFINITION: &str = "ws-replan-kpi-definition/v2";
pub const ALGORITHM: &str = "exact-utf8-path-membership/v2";
pub const PRODUCER: &str = "producer-v2-exact-paths/1";
pub const GIB: u64 = 1 << 30;
pub const LINE_LIMIT: usize = 64 * 1024;
pub const JOURNAL_LIMIT: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum K<T> {
    Value(T),
    Unknown,
    NotPerformed,
}
impl<T> K<T> {
    pub fn get(&self) -> Option<&T> {
        if let Self::Value(v) = self {
            Some(v)
        } else {
            None
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Interval {
    pub start: u64,
    pub end: u64,
}
impl Interval {
    pub fn valid(&self) -> bool {
        self.start <= self.end
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRef {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}
impl ArtifactRef {
    pub fn new(path: &str, bytes: &[u8]) -> Self {
        Self {
            path: path.into(),
            sha256: digest(bytes),
            bytes: bytes.len() as u64,
        }
    }
    pub fn valid(&self) -> bool {
        !self.path.is_empty()
            && self.path.len() <= 256
            && self
                .path
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            && self.path != "."
            && self.path != ".."
            && hex(&self.sha256, 64)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Captured,
    Failed,
    NotPerformed,
    Missing,
    Invalid,
    Incomplete,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Qualification {
    Qualified,
    MissingSource,
    NotPerformed,
    FailedSource,
    InvalidSource,
    IncompleteSource,
    SettlementLag,
    UnverifiedBaseline,
    FutureSource,
    StaleSource,
    UnresolvedDiskUnit,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub status: Status,
    pub reason: String,
    pub capture: Interval,
    pub observed_at: K<u64>,
    pub artifact: K<ArtifactRef>,
    pub exit_code: K<u32>,
    pub signal: K<u32>,
}
impl Source {
    pub fn new(status: Status, reason: &str, start: u64, end: u64) -> Self {
        let unattempted = status == Status::NotPerformed;
        Self {
            status,
            reason: reason.into(),
            capture: Interval { start, end },
            observed_at: K::Unknown,
            artifact: K::Unknown,
            exit_code: if unattempted {
                K::NotPerformed
            } else {
                K::Unknown
            },
            signal: if unattempted {
                K::NotPerformed
            } else {
                K::Unknown
            },
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sample<T> {
    pub value: K<T>,
    pub source: Source,
    pub source_qualified: bool,
    pub qualification: Qualification,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub github_id: u64,
    pub full_name: String,
}
impl Identity {
    pub fn valid(&self) -> bool {
        self.github_id > 0
            && self.full_name.split('/').count() == 2
            && self.full_name.split('/').all(|s| {
                !s.is_empty()
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Population {
    pub total: u64,
    pub path_set_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Repository {
    pub manifest_key: String,
    pub identity: Identity,
    pub ref_name: String,
    pub ref_read: Source,
    pub commit_sha: K<String>,
    pub tree_sha: K<String>,
    pub tree_read: Source,
    pub backend: String,
    pub population: Population,
    pub removed: K<u64>,
    pub leaf_path_set_sha256: K<String>,
    pub absent_path_set_sha256: K<String>,
    pub membership: K<ArtifactRef>,
    pub source_qualified: bool,
    pub qualification: Qualification,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyValue {
    pub removed: u64,
    pub total: u64,
    pub unit: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParityValue {
    pub qualified: u64,
    pub total: u64,
    pub bounded_implemented: K<u64>,
    pub unit: String,
    pub tool_set_sha256: String,
    pub result_revision: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ratio {
    pub numerator: u64,
    pub denominator: u64,
    pub unit: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuardrailSample {
    pub kind: String,
    pub value: K<u64>,
    pub unit: String,
    pub total: K<u64>,
    pub free_gib: K<Ratio>,
    pub source: Source,
    pub source_qualified: bool,
    pub qualification: Qualification,
    pub contract: ArtifactRef,
    pub recording: K<ArtifactRef>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildEvidence {
    pub source_commit: String,
    pub source_tree: String,
    pub binary_sha256: String,
    pub clean: bool,
}
impl BuildEvidence {
    pub fn validate(&self) -> Result<()> {
        if self.clean
            && hex(&self.source_commit, 40)
            && hex(&self.source_tree, 40)
            && hex(&self.binary_sha256, 64)
        {
            Ok(())
        } else {
            Err(IntakeError::InvalidData)
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Producer {
    pub revision: String,
    pub source_commit: String,
    pub source_tree: String,
    pub binary_sha256: String,
    pub build_evidence: ArtifactRef,
    pub git_version: String,
    pub gh_version: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestPin {
    pub source_id: String,
    pub sha256: String,
    pub bytes: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefinitionPin {
    pub revision: String,
    pub algorithm_revision: String,
    pub contract: ArtifactRef,
    pub manifest: ManifestPin,
    pub request: ArtifactRef,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub schema_version: String,
    pub measurement_id: String,
    pub capture_status: String,
    pub capture: Interval,
    pub producer: Producer,
    pub definition: DefinitionPin,
    pub kpi_legacy_removal: Sample<LegacyValue>,
    pub kpi_mcp_parity: Sample<ParityValue>,
    pub guard_ci_minutes_month: GuardrailSample,
    pub guard_disk_free_gib: GuardrailSample,
    pub guard_escaped_defects_week: GuardrailSample,
    pub guard_recall_regression: GuardrailSample,
    pub repositories: Vec<Repository>,
    pub artifacts: Vec<ArtifactRef>,
}
pub fn measurement_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 96
        && s.as_bytes()[0].is_ascii_alphanumeric()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
pub fn qualified_status(flags: &[(bool, Qualification)]) -> String {
    if flags
        .iter()
        .all(|(q, r)| *q && *r == Qualification::Qualified)
    {
        "complete"
    } else if flags.iter().any(|(q, _)| *q) {
        "partial"
    } else {
        "failed"
    }
    .into()
}
impl Snapshot {
    pub fn status(&self) -> String {
        qualified_status(&[
            (
                self.kpi_legacy_removal.source_qualified,
                self.kpi_legacy_removal.qualification.clone(),
            ),
            (
                self.kpi_mcp_parity.source_qualified,
                self.kpi_mcp_parity.qualification.clone(),
            ),
            (
                self.guard_ci_minutes_month.source_qualified,
                self.guard_ci_minutes_month.qualification.clone(),
            ),
            (
                self.guard_disk_free_gib.source_qualified,
                self.guard_disk_free_gib.qualification.clone(),
            ),
            (
                self.guard_escaped_defects_week.source_qualified,
                self.guard_escaped_defects_week.qualification.clone(),
            ),
            (
                self.guard_recall_regression.source_qualified,
                self.guard_recall_regression.qualification.clone(),
            ),
        ])
    }
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != SCHEMA
            || !measurement_id(&self.measurement_id)
            || !self.capture.valid()
            || self.capture_status != self.status()
            || self.definition.revision != DEFINITION
            || self.definition.algorithm_revision != ALGORITHM
            || self.producer.revision != PRODUCER
            || !hex(&self.producer.source_commit, 40)
            || !hex(&self.producer.source_tree, 40)
            || !hex(&self.producer.binary_sha256, 64)
            || !text(&self.producer.git_version)
            || !text(&self.producer.gh_version)
            || self.repositories.is_empty()
        {
            return Err(IntakeError::InvalidData);
        }
        if self.artifacts.windows(2).any(|w| w[0].path >= w[1].path)
            || self.artifacts.iter().any(|a| !a.valid())
        {
            return Err(IntakeError::InvalidData);
        }
        let artifact = |a: &ArtifactRef| -> Result<()> {
            if !a.valid() || !self.artifacts.contains(a) {
                return Err(IntakeError::InvalidData);
            }
            Ok(())
        };
        artifact(&self.producer.build_evidence)?;
        artifact(&self.definition.contract)?;
        artifact(&self.definition.request)?;
        if self.definition.manifest.source_id != "ws-replan-plan-a-v1-manifest"
            || !hex(&self.definition.manifest.sha256, 64)
            || self.definition.manifest.bytes == 0
            || self.definition.manifest.bytes > 8 * 1024 * 1024
        {
            return Err(IntakeError::InvalidData);
        }
        let source = |s: &Source| -> Result<()> {
            if !s.capture.valid()
                || s.capture.start < self.capture.start
                || s.capture.end > self.capture.end
                || s.reason.is_empty()
                || !s
                    .reason
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            {
                return Err(IntakeError::InvalidData);
            }
            if let K::Value(a) = &s.artifact {
                artifact(a)?;
            }
            Ok(())
        };
        source(&self.kpi_legacy_removal.source)?;
        source(&self.kpi_mcp_parity.source)?;
        if matches!(self.kpi_legacy_removal.value, K::Value(_))
            != self.kpi_legacy_removal.source_qualified
            || matches!(self.kpi_mcp_parity.value, K::Value(_))
                != self.kpi_mcp_parity.source_qualified
        {
            return Err(IntakeError::InvalidData);
        }
        for (q, reason, s) in [
            (
                self.kpi_legacy_removal.source_qualified,
                &self.kpi_legacy_removal.qualification,
                &self.kpi_legacy_removal.source,
            ),
            (
                self.kpi_mcp_parity.source_qualified,
                &self.kpi_mcp_parity.qualification,
                &self.kpi_mcp_parity.source,
            ),
        ] {
            if q && (*reason != Qualification::Qualified
                || s.status != Status::Captured
                || s.observed_at.get().is_none_or(|t| *t > self.capture.end))
            {
                return Err(IntakeError::InvalidData);
            }
        }
        let mut previous = "";
        let mut total = 0u64;
        let mut removed = 0u64;
        let mut all = true;
        for r in &self.repositories {
            source(&r.ref_read)?;
            source(&r.tree_read)?;
            if !matches!(
                r.backend.as_str(),
                "local_git" | "github_api" | "not_performed"
            ) || r.commit_sha.get().is_some_and(|v| !hex(v, 40))
                || r.tree_sha.get().is_some_and(|v| !hex(v, 40))
            {
                return Err(IntakeError::InvalidData);
            }
            if let K::Value(a) = &r.membership {
                artifact(a)?;
            }
            if r.manifest_key.as_str() <= previous
                || !r.identity.valid()
                || r.ref_name != "refs/heads/main"
                || !hex(&r.population.path_set_sha256, 64)
            {
                return Err(IntakeError::InvalidData);
            }
            previous = &r.manifest_key;
            total = total
                .checked_add(r.population.total)
                .ok_or(IntakeError::InvalidData)?;
            if r.source_qualified {
                let n = *r.removed.get().ok_or(IntakeError::InvalidData)?;
                if n > r.population.total
                    || !r.commit_sha.get().is_some_and(|s| hex(s, 40))
                    || !r.tree_sha.get().is_some_and(|s| hex(s, 40))
                    || r.qualification != Qualification::Qualified
                    || r.ref_read.status != Status::Captured
                    || r.tree_read.status != Status::Captured
                    || r.ref_read.capture.end > r.tree_read.capture.end
                    || self.capture.end - r.ref_read.capture.end > 900
                    || !r.leaf_path_set_sha256.get().is_some_and(|s| hex(s, 64))
                    || !r.absent_path_set_sha256.get().is_some_and(|s| hex(s, 64))
                    || r.membership.get().is_none()
                {
                    return Err(IntakeError::InvalidData);
                }
                removed = removed.checked_add(n).ok_or(IntakeError::InvalidData)?;
            } else {
                all = false;
                if matches!(r.removed, K::Value(_)) {
                    return Err(IntakeError::InvalidData);
                }
            }
        }
        if let K::Value(v) = &self.kpi_legacy_removal.value {
            if !all
                || !self.kpi_legacy_removal.source_qualified
                || v.removed != removed
                || v.total != total
                || v.total == 0
                || v.unit != "fraction"
            {
                return Err(IntakeError::InvalidData);
            }
        } else if self.kpi_legacy_removal.source_qualified {
            return Err(IntakeError::InvalidData);
        }
        if let K::Value(v) = &self.kpi_mcp_parity.value
            && (!self.kpi_mcp_parity.source_qualified
                || v.total == 0
                || v.qualified > v.total
                || v.bounded_implemented.get().is_some_and(|n| *n > v.total)
                || v.unit != "fraction"
                || !hex(&v.tool_set_sha256, 64)
                || !text(&v.result_revision))
        {
            return Err(IntakeError::InvalidData);
        }
        for (g, kind, unit) in [
            (&self.guard_ci_minutes_month, "billing", "linux_minutes"),
            (&self.guard_disk_free_gib, "disk", "bytes"),
            (&self.guard_escaped_defects_week, "defects", "count"),
            (&self.guard_recall_regression, "recall", "fraction"),
        ] {
            source(&g.source)?;
            artifact(&g.contract)?;
            if let K::Value(a) = &g.recording {
                artifact(a)?;
            }
            if g.kind != kind
                || g.unit != unit
                || matches!(g.value, K::Value(_)) != g.source_qualified
            {
                return Err(IntakeError::InvalidData);
            }
            if g.source_qualified
                && (g.source.status != Status::Captured
                    || g.source
                        .observed_at
                        .get()
                        .is_none_or(|t| *t > self.capture.end)
                    || !matches!(
                        g.qualification,
                        Qualification::Qualified | Qualification::UnresolvedDiskUnit
                    )
                    || (g.qualification == Qualification::UnresolvedDiskUnit && kind != "disk"))
            {
                return Err(IntakeError::InvalidData);
            }
            if kind == "recall"
                && g.source_qualified
                && (g.total.get().is_none_or(|n| *n == 0) || g.value.get() > g.total.get())
            {
                return Err(IntakeError::InvalidData);
            }
            if kind != "recall" && !matches!(g.total, K::Unknown) {
                return Err(IntakeError::InvalidData);
            }
            if kind == "disk" {
                if matches!(g.free_gib, K::Value(_)) != g.source_qualified {
                    return Err(IntakeError::InvalidData);
                }
                if let K::Value(r) = &g.free_gib
                    && (r.unit != "GiB"
                        || r.denominator != GIB
                        || Some(&r.numerator) != g.value.get())
                {
                    return Err(IntakeError::InvalidData);
                }
            } else if !matches!(g.free_gib, K::Unknown) {
                return Err(IntakeError::InvalidData);
            }
        }
        Ok(())
    }
}
