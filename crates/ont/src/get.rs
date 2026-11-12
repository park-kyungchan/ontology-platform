//! `get`: the newest 100 history rows in append order, after admission. An
//! object the principal may not read and an object that does not exist give
//! the same answer (`unavailable`).
use crate::principal::Principal;
use crate::refusal::{open_store, st};
use crate::{Answer, Context, Failure, Refusal, internal, pin_json};
use ont_core::schema::{SchemaPin, SchemaSet};
use ont_core::{Id, Object, Query, Response, Snapshot};
use ont_store::schema_mode::{HeadAccess, SchemaHistory, SchemaRead, StoreRefusal};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetRequest {
    #[serde(default)]
    pub op: Option<String>,
    pub id: Id,
}
impl GetRequest {
    /// Parse the strict request (at most `MAX_REQUEST` bytes; an `op`, when
    /// present, must be `get`).
    pub fn parse(bytes: &[u8]) -> Result<Self, Refusal> {
        let req: Self = crate::cli::parse_request(bytes)?;
        if req.op.as_deref().is_some_and(|op| op != "get") {
            return Err(Refusal::input(
                "invalid_request",
                "/op",
                "must be get when non-null",
            ));
        }
        Ok(req)
    }
}

pub(crate) struct OneObject(pub(crate) Option<Object>);
impl Snapshot for OneObject {
    fn get(&self, id: &Id) -> Result<Option<Object>, ont_core::Error> {
        Ok(self.0.clone().filter(|o| &o.id == id))
    }
    fn targets(&self, _root: &Id, _relation: &str) -> Result<Vec<Id>, ont_core::Error> {
        Ok(Vec::new())
    }
}

fn not_found() -> Failure {
    Refusal::new(ont_core::Error::Unavailable.code()).into()
}

pub fn get(ctx: &Context, req: &GetRequest) -> Result<Answer, Failure> {
    let mut store = open_store(&ctx.store, false)?;
    st!(store.read_schema(|tx| read_get(ctx, req, tx)))
}
// Keep admission and the metadata count/load boundary observable in tests.
trait GetSource {
    fn schema_set(&self) -> &SchemaSet;
    fn head_access(&self, id: &Id) -> Result<Option<HeadAccess>, StoreRefusal>;
    fn history(&self, id: &Id) -> Result<Option<SchemaHistory>, StoreRefusal>;
}
impl GetSource for SchemaRead<'_> {
    fn schema_set(&self) -> &SchemaSet {
        self.schema_set()
    }
    fn head_access(&self, id: &Id) -> Result<Option<HeadAccess>, StoreRefusal> {
        self.head_access(id)
    }
    fn history(&self, id: &Id) -> Result<Option<SchemaHistory>, StoreRefusal> {
        self.history(id)
    }
}
fn read_get(ctx: &Context, req: &GetRequest, tx: &dyn GetSource) -> Result<Answer, Failure> {
    let set = tx.schema_set();
    let principal = Principal::bind(set)?;
    let admission = principal
        .authority()
        .admit(principal.session())
        .map_err(|_| Failure::Internal(internal!("authority_failure")))?;
    let access = st!(tx.head_access(&req.id)).ok_or_else(not_found)?;
    let metadata = Object {
        id: access.id.clone(),
        scope: access.scope,
        head: String::new(),
        observations: vec![],
    };
    match admission.query(
        &OneObject(Some(metadata)),
        Query::Get { id: req.id.clone() },
        ctx.now,
    ) {
        Ok(Response::Ok { .. }) => {}
        Err(ont_core::Error::InvalidRequest) => return Err(invalid_id(req)),
        Ok(Response::Refused { .. }) | Err(ont_core::Error::Unavailable) => return Err(not_found()),
        _ => return Err(Failure::Internal(internal!("authority_failure"))),
    }
    let history = match st!(tx.history(&req.id)) {
        Some(h) => h,
        None => return Err(not_found()),
    };
    let object = Object {
        id: history.id.clone(),
        scope: history.scope.clone(),
        head: history.head.clone(),
        observations: history
            .observations
            .iter()
            .map(|o| o.observation.clone())
            .collect(),
    };
    let response = admission.query(
        &OneObject(Some(object)),
        Query::Get { id: req.id.clone() },
        ctx.now,
    );
    let (revision, mut result) = match response {
        Ok(Response::Ok {
            authorization_revision,
            result,
        }) => (
            authorization_revision,
            serde_json::to_value(&result).unwrap_or(Value::Null),
        ),
        Ok(Response::Refused { .. }) | Err(ont_core::Error::Unavailable) => {
            return Err(not_found());
        }
        Err(ont_core::Error::InvalidRequest) => return Err(invalid_id(req)),
        Err(_) => return Err(Failure::Internal(internal!("authority_failure"))),
    };
    let policy_revision = principal.revision()?;
    if revision != policy_revision {
        return Err(Failure::Internal(internal!("authority_generation_changed")));
    }
    result["basis"] = serde_json::json!({
        "older_omitted": history.older_omitted,
        "oversized_excluded": history.oversized_excluded,
    });
    // Attach each observation's stored pin.
    let pin_by_revision: BTreeMap<&str, &SchemaPin> = history
        .observations
        .iter()
        .map(|o| (o.observation.revision.as_str(), &o.schema_pin))
        .collect();
    if let Some(objects) = result.get_mut("objects").and_then(|o| o.as_array_mut()) {
        for view in objects.iter_mut() {
            let Some(observations) = view.get_mut("observations").and_then(|o| o.as_array_mut())
            else {
                continue;
            };
            for obs in observations.iter_mut() {
                let revision = obs
                    .get("revision")
                    .and_then(|r| r.as_str())
                    .unwrap_or("")
                    .to_string();
                if let Some(pin) = pin_by_revision.get(revision.as_str())
                    && let Value::Object(map) = obs
                {
                    map.insert("schema".into(), pin_json(pin));
                }
            }
        }
    }
    let pins: BTreeMap<_, _> = set
        .schema_of(&req.id.object_type)
        .map(|s| s.domain.clone())
        .iter()
        .filter_map(|d| set.pin(d).map(|p| (d.clone(), p.clone())))
        .collect();
    Ok(Answer::Enveloped {
        pins,
        revision: policy_revision,
        result,
    })
}

fn invalid_id(req: &GetRequest) -> Failure {
    let (field, _) = [
        ("/id/object_type", &req.id.object_type),
        ("/id/namespace/instance", &req.id.namespace.instance),
        ("/id/namespace/account", &req.id.namespace.account),
        ("/id/native_key", &req.id.native_key),
    ]
    .into_iter()
    .find(|(_, v)| v.is_empty() || v.len() > 512)
    .unwrap_or(("/id", &req.id.native_key));
    Refusal::input(
        "invalid_request",
        field,
        "each id component is a nonempty string of at most 512 UTF-8 bytes",
    )
    .into()
}

#[cfg(test)]
#[path = "get_tests.rs"]
mod tests;
