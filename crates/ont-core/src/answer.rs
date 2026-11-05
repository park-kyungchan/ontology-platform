//! Closed deterministic publication of native results. No query/request input,
//! authorization, source acquisition, or free-text inference. V2 adds exact arithmetic.
use crate::{Error, Response, action::ActionOutput};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

mod v2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Version {
    V1,
    V2,
}
impl Version {
    pub fn schema(self) -> &'static str {
        match self {
            Self::V1 => SCHEMA,
            Self::V2 => "ontology-answer/v2",
        }
    }
}

pub const SCHEMA: &str = "ontology-answer/v1";

/// Constructible only from typed native results or enumerated native outcomes.
/// Prose may only restate fields in this record.
#[derive(Debug, Clone, Serialize)]
pub struct AnswerRecord {
    schema: &'static str,
    outcome: Outcome,
    authorization_revision: Cell,
    result: Cell,
}
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Outcome {
    Ok,
    Refused { code: &'static str },
}
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
enum Cell {
    Unavailable {
        status: &'static str,
        reason: &'static str,
        native: Value,
    },
    Object(BTreeMap<String, Cell>),
    Array(Vec<Cell>),
    Native(Value),
}
impl Cell {
    fn unavailable(reason: &'static str, native: Value) -> Self {
        Self::Unavailable {
            status: "not_available",
            reason,
            native,
        }
    }
    // Rebuild an enriched publication tree without reordering existing explicit
    // unavailable cells. Knowledge payloads remain opaque at the same boundary.
    fn publication(value: Value) -> Self {
        match value {
            Value::Object(fields)
                if fields.get("kind").and_then(Value::as_str) == Some("value") =>
            {
                Self::Native(Value::Object(fields))
            }
            Value::Object(mut fields) => {
                if fields.len() == 3
                    && fields.get("status").and_then(Value::as_str) == Some("not_available")
                {
                    let reason = match fields.get("reason").and_then(Value::as_str) {
                        Some("native_null") => Some("native_null"),
                        Some("missing") => Some("missing"),
                        Some("null") => Some("null"),
                        Some("blank") => Some("blank"),
                        Some("unknown") => Some("unknown"),
                        Some("not_performed") => Some("not_performed"),
                        Some("missing_endpoints") => Some("missing_endpoints"),
                        Some("missing_endpoint_values") => Some("missing_endpoint_values"),
                        Some("empty_denominator") => Some("empty_denominator"),
                        Some("incomparable_components") => Some("incomparable_components"),
                        Some("invalid_rational") => Some("invalid_rational"),
                        Some("arithmetic_overflow") => Some("arithmetic_overflow"),
                        Some("unbound_endpoints") => Some("unbound_endpoints"),
                        Some("unbound_aggregate_change") => Some("unbound_aggregate_change"),
                        _ => None,
                    };
                    if let Some(reason) = reason
                        && let Some(native) = fields.remove("native")
                    {
                        return Self::unavailable(reason, native);
                    }
                }
                Self::Object(
                    fields
                        .into_iter()
                        .map(|(k, v)| (k, Self::publication(v)))
                        .collect(),
                )
            }
            Value::Array(values) => {
                Self::Array(values.into_iter().map(Self::publication).collect())
            }
            value => Self::Native(value),
        }
    }
    fn native(value: Value) -> Self {
        match value {
            Value::Null => Self::unavailable("native_null", Value::Null),
            Value::Array(values) => Self::Array(values.into_iter().map(Self::native).collect()),
            Value::Object(fields) => {
                // Only typed native structures reach this function. Arbitrary
                // JSON occurs exclusively behind Knowledge::Value, which is an
                // opaque leaf: never reinterpret user data as native Knowledge.
                match fields.get("kind").and_then(Value::as_str) {
                    Some("value") => return Self::Native(Value::Object(fields)),
                    Some(kind) if fields.len() == 1 => {
                        let reason = match kind {
                            "missing" => Some("missing"),
                            "null" => Some("null"),
                            "blank" => Some("blank"),
                            "unknown" => Some("unknown"),
                            "not_performed" => Some("not_performed"),
                            _ => None,
                        };
                        if let Some(reason) = reason {
                            return Self::unavailable(reason, Value::Object(fields));
                        }
                    }
                    _ => {}
                }
                Self::Object(
                    fields
                        .into_iter()
                        .map(|(key, value)| (key, Self::native(value)))
                        .collect(),
                )
            }
            value => Self::Native(value),
        }
    }
}
impl AnswerRecord {
    pub fn from_response(response: &Response) -> Result<Self, Error> {
        match response {
            Response::Ok {
                authorization_revision,
                result,
            } => Ok(Self {
                schema: SCHEMA,
                outcome: Outcome::Ok,
                authorization_revision: Cell::Native((*authorization_revision).into()),
                result: Cell::native(
                    serde_json::to_value(result).map_err(|_| Error::StoreFailure)?,
                ),
            }),
            Response::Refused { code } => Ok(Self::from_outcome(code)),
        }
    }
    /// Additive selector; the historical constructor remains byte-identical v1.
    pub fn from_response_version(response: &Response, version: Version) -> Result<Self, Error> {
        let mut record = Self::from_response(response)?;
        record.schema = version.schema();
        if version == Version::V2
            && let Response::Ok { result, .. } = response
        {
            let mut cells =
                serde_json::to_value(&record.result).map_err(|_| Error::StoreFailure)?;
            v2::enrich(result, &mut cells);
            record.result = Cell::publication(cells);
        }
        Ok(record)
    }
    pub fn from_outcome_version(code: &str, version: Version) -> Self {
        let mut record = Self::from_outcome(code);
        record.schema = version.schema();
        record
    }
    pub fn from_error_version(error: Error, version: Version) -> Self {
        Self::from_outcome_version(error.code(), version)
    }
    pub fn from_action_version(action: &ActionOutput, version: Version) -> Result<Self, Error> {
        let mut record = Self::from_action(action)?;
        record.schema = version.schema();
        Ok(record)
    }
    /// Preserve native action state and post-condition readback. This does not
    /// execute an action or authorize an arbitrary ActionOutput for publication.
    pub fn from_action(action: &ActionOutput) -> Result<Self, Error> {
        if action.status != "ok" {
            return Ok(Self::from_outcome(&action.status));
        }
        #[derive(Serialize)]
        struct ActionResult<'a> {
            kind: &'static str,
            action: &'a ActionOutput,
        }
        Ok(Self {
            schema: SCHEMA,
            outcome: Outcome::Ok,
            authorization_revision: Cell::Native(action.authorization_revision.into()),
            result: Cell::native(
                serde_json::to_value(ActionResult {
                    kind: "action",
                    action,
                })
                .map_err(|_| Error::StoreFailure)?,
            ),
        })
    }
    pub fn from_error(error: Error) -> Self {
        Self::from_outcome(error.code())
    }
    /// A refusal code can be a String in the native Response. Never echo an
    /// unknown string: the versioned publication vocabulary is closed.
    pub fn from_outcome(code: &str) -> Self {
        let code = native_code(code).unwrap_or("refused");
        Self {
            schema: SCHEMA,
            outcome: Outcome::Refused { code },
            authorization_revision: Cell::unavailable(code, Value::Null),
            result: Cell::unavailable(code, Value::Null),
        }
    }
    /// Native refusal classification, independent of serialization success.
    pub fn is_refused(&self) -> bool {
        matches!(self.outcome, Outcome::Refused { .. })
    }
    pub fn render(&self) -> Result<String, Error> {
        serde_json::to_string(self).map_err(|_| Error::StoreFailure)
    }
}

// Fixed ontology-answer/v1 publication vocabulary. It covers core, native
// Program rules, and the pilot's acquisition/output boundaries. No prefix or
// caller-defined value is admitted. A new native code needs an explicit entry.
fn native_code(code: &str) -> Option<&'static str> {
    const CODES: &[&str] = &[
        "invalid_request",
        "invalid_evidence",
        "unavailable",
        "principal_unknown",
        "stale_authorization",
        "derive_forbidden",
        "revision_conflict",
        "scope_conflict",
        "store_failure",
        "authority_failure",
        "action_forbidden",
        "review_required",
        "review_stale",
        "target_conflict",
        "target_identity",
        "field_fenced",
        "retry_forbidden",
        "program_source_unavailable",
        "program_capture_bounds",
        "invalid_program_capture",
        "invalid_program_intake_contract",
        "program_projection_failure",
        "invalid_guardrail_contract",
        "invalid_guardrail_object",
        "invalid_guardrail_evidence",
        "invalid_git_configuration",
        "git_source_unavailable",
        "git_source_rate_limited",
        "git_source_timeout",
        "git_capture_limit",
        "invalid_git_response",
        "git_first_parent_gap",
        "git_projection_store_failure",
        "invalid_program_definition",
        "invalid_program_window",
        "incompatible_program_unit",
        "invalid_program_evidence",
        "incomplete_program_evidence",
        "unsupported_path_algorithm",
        "attribution_integrity_failure",
        "program_arithmetic_overflow",
        "pilot_ambiguous_snapshot_input",
        "pilot_clock_failure",
        "pilot_composition_collision",
        "pilot_config_custody",
        "pilot_config_not_ready",
        "pilot_evidence_failure",
        "pilot_incompatible_definition",
        "pilot_incomplete_population",
        "pilot_invalid_artifact",
        "pilot_invalid_config",
        "pilot_manifest_mismatch",
        "pilot_missing_endpoint",
        "pilot_platform_unavailable",
        "pilot_serialization_failure",
        "pilot_uid_refused",
        "pilot_input_failure",
        "pilot_output_failure",
    ];
    CODES.iter().copied().find(|known| *known == code)
}
