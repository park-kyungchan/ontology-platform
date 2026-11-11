//! Closed PR5-compatible wire contracts and recordings, agent-platform 6e252fd.
use super::model::GIB;
use super::{IntakeError, Result, hex, text};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
const WEEK: u64 = 604800;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Month {
    pub year: u16,
    pub month: u8,
}
impl Month {
    pub fn bounds(&self) -> Result<(u64, u64)> {
        if !(1970..=9999).contains(&self.year) || !(1..=12).contains(&self.month) {
            return Err(IntakeError::InvalidContract);
        }
        fn leap(y: u64) -> bool {
            y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400))
        }
        let mut days = 0u64;
        for y in 1970..u64::from(self.year) {
            days += if leap(y) { 366 } else { 365 };
        }
        let lengths = [
            31,
            if leap(self.year.into()) { 29 } else { 28 },
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
        for n in lengths.iter().take(usize::from(self.month - 1)) {
            days += n;
        }
        Ok((
            days * 86400,
            (days + lengths[usize::from(self.month - 1)]) * 86400,
        ))
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskUnit {
    Bytes,
    Gb,
    Gib,
}
impl DiskUnit {
    fn multiplier(&self) -> u64 {
        match self {
            Self::Bytes => 1,
            Self::Gb => 1_000_000_000,
            Self::Gib => GIB,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DiskMinimum {
    Exact { quantity: u64, unit: DiskUnit },
    UnresolvedGbOrGib { quantity: u64 },
}
impl DiskMinimum {
    pub fn bounds(&self) -> Result<(u64, u64)> {
        let mul = |a: u64, b: u64| a.checked_mul(b).ok_or(IntakeError::InvalidContract);
        match self {
            Self::Exact { quantity, unit } => {
                let n = mul(*quantity, unit.multiplier())?;
                Ok((n, n))
            }
            Self::UnresolvedGbOrGib { quantity } => {
                Ok((mul(*quantity, 1_000_000_000)?, mul(*quantity, GIB)?))
            }
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Baseline {
    pub supported: u64,
    pub total: u64,
    pub evaluation_set_sha256: String,
    pub result_revision: String,
    pub artifact_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Contract {
    Billing {
        account: String,
        month: Month,
        linux_skus: BTreeSet<String>,
        limit: u64,
    },
    Disk {
        minimum: DiskMinimum,
    },
    Recall {
        baseline: Baseline,
    },
    Defects {
        week_start: u64,
        limit: u64,
    },
}
impl Contract {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Billing { .. } => "billing",
            Self::Disk { .. } => "disk",
            Self::Recall { .. } => "recall",
            Self::Defects { .. } => "defects",
        }
    }
    pub fn unit(&self) -> &'static str {
        match self {
            Self::Billing { .. } => "linux_minutes",
            Self::Disk { .. } => "bytes",
            Self::Recall { .. } => "fraction",
            Self::Defects { .. } => "count",
        }
    }
    pub fn budget(&self) -> u64 {
        match self {
            Self::Billing { .. } => 86400,
            Self::Disk { .. } => 900,
            _ => 3600,
        }
    }
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Billing {
                account,
                month,
                linux_skus,
                ..
            } => {
                month.bounds()?;
                if !text(account)
                    || linux_skus.is_empty()
                    || linux_skus.len() > 64
                    || linux_skus.iter().any(|s| !text(s))
                {
                    return Err(IntakeError::InvalidContract);
                }
            }
            Self::Disk { minimum } => {
                minimum.bounds()?;
            }
            Self::Recall { baseline: b } => {
                if b.total == 0
                    || b.supported > b.total
                    || !hex(&b.evaluation_set_sha256, 64)
                    || !hex(&b.artifact_sha256, 64)
                    || !text(&b.result_revision)
                {
                    return Err(IntakeError::InvalidContract);
                }
            }
            Self::Defects { week_start, .. } => {
                if *week_start % 86400 != 0
                    || (*week_start / 86400 + 3) % 7 != 0
                    || week_start.checked_add(WEEK).is_none()
                {
                    return Err(IntakeError::InvalidContract);
                }
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recording {
    pub source_revision: String,
    pub capture_start: u64,
    pub capture_end: u64,
    pub outcome: Outcome,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Outcome {
    NotPerformed {},
    Failed {},
    Baseline {
        observed_at: u64,
        value: u64,
        total: Option<u64>,
    },
    Billing {
        account: String,
        month: Month,
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
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub number: u32,
    pub has_more: bool,
    pub items: Vec<Usage>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Usage {
    pub id: String,
    pub sku: String,
    pub unit: String,
    pub quantity: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Incident {
    pub id: String,
    pub escaped_at: u64,
    pub fixed_at: Option<u64>,
}

impl Contract {
    pub(super) fn adopted(&self) -> crate::program_guardrails::Contract {
        use crate::program_guardrails as g;
        match self {
            Self::Billing {
                account,
                month,
                linux_skus,
                limit,
            } => g::Contract::Billing {
                account: account.clone(),
                month: g::UtcMonth {
                    year: month.year,
                    month: month.month,
                },
                linux_skus: linux_skus.clone(),
                limit: *limit,
            },
            Self::Disk { minimum } => g::Contract::Disk {
                minimum: match minimum {
                    DiskMinimum::Exact { quantity, unit } => g::DiskMinimum::Exact {
                        quantity: *quantity,
                        unit: match unit {
                            DiskUnit::Bytes => g::DiskUnit::Bytes,
                            DiskUnit::Gb => g::DiskUnit::Gb,
                            DiskUnit::Gib => g::DiskUnit::Gib,
                        },
                    },
                    DiskMinimum::UnresolvedGbOrGib { quantity } => {
                        g::DiskMinimum::UnresolvedGbOrGib {
                            quantity: *quantity,
                        }
                    }
                },
            },
            Self::Recall { baseline: b } => g::Contract::Recall {
                baseline: g::RecallBaseline {
                    supported: b.supported,
                    total: b.total,
                    evaluation_set_sha256: b.evaluation_set_sha256.clone(),
                    result_revision: b.result_revision.clone(),
                    artifact_sha256: b.artifact_sha256.clone(),
                },
            },
            Self::Defects { week_start, limit } => g::Contract::Defects {
                week_start: *week_start,
                limit: *limit,
            },
        }
    }
}
