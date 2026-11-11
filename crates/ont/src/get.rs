//! `get`: the full history of one object, admitted for the principal. An
//! object the principal may not read and an object that does not exist give
//! the same answer (`unavailable`).
use crate::principal::Principal;
use crate::refusal::st;
use crate::{Answer, Context, Failure, Refusal, internal, pin_json};
use ont_core::schema::SchemaPin;
use ont_core::{Id, Object, Query, Response, Snapshot};
use ont_store::Store;
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
            return Err(Refusal::new("invalid_request"));
        }
        Ok(req)
    }
}

struct OneObject(Option<Object>);
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
    let mut store = st!(Store::open_schema_existing(&ctx.store));
    let history = match st!(store.history(&req.id)) {
        Some(h) => h,
        None => return Err(not_found()),
    };
    let set = st!(store.schema_set()).clone();
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
    let principal = Principal::bind(&set)?;
    let admission = principal
        .authority()
        .admit(principal.session())
        .map_err(|_| Failure::Internal(internal!("authority_failure")))?;
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
        Err(ont_core::Error::InvalidRequest) => return Err(Refusal::new("invalid_request").into()),
        Err(_) => return Err(Failure::Internal(internal!("authority_failure"))),
    };
    if revision != principal.revision() {
        return Err(Failure::Internal(internal!("authority_generation_changed")));
    }
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
        revision: principal.revision(),
        result,
    })
}
