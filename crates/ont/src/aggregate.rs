//! Bounded count/group reduction in one schema snapshot and native admission.
use crate::around::AroundSource;
use crate::get::OneObject;
use crate::principal::Principal;
use crate::refusal::{open_store, st};
use crate::search::{self, Filter, SCAN_LIMIT, Tri};
use crate::{Answer, Context, Failure, Refusal, internal};
use ont_core::schema::{Owner, PropertyType};
use ont_core::{Knowledge, Object, Query};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

pub const MAX_GROUP_KEYS: usize = 2;
pub const MAX_BUCKETS: usize = 1_000;
pub const MAX_METRICS: usize = 1;
/// Compact JSON bytes of one group value, checked before validation/copying.
pub const MAX_GROUP_VALUE_BYTES: usize = 1_024;
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AggregateRequest {
    #[serde(default)]
    pub op: Option<String>,
    pub object_type: String,
    #[serde(default, rename = "where")]
    pub filter: Option<Filter>,
    pub group_by: Vec<String>,
    pub metrics: Vec<Value>,
}
impl AggregateRequest {
    pub fn parse(bytes: &[u8]) -> Result<Self, Refusal> {
        let req: Self = crate::cli::parse_request(bytes)?;
        req.check()?;
        Ok(req)
    }
    fn check(&self) -> Result<(), Refusal> {
        if self.op.as_deref().is_some_and(|op| op != "aggregate") {
            return Err(Refusal::input(
                "invalid_request",
                "/op",
                "use aggregate for aggregate, search for search, around for search_around",
            ));
        }
        if self.group_by.len() > MAX_GROUP_KEYS {
            return Err(Refusal::input(
                "invalid_group_by",
                "/group_by",
                "0 to 2 distinct declared properties",
            ));
        }
        if self.metrics.len() != MAX_METRICS || self.metrics[0] != "count" {
            return Err(Refusal::input(
                "unsupported_metric",
                "/metrics",
                "only metrics:[\"count\"] is supported; exactly 1 metric",
            ));
        }
        Ok(())
    }
}
// A bounded writer avoids allocating an unbounded encoded group key. JSON
// serialization stops on the first write that would exceed the byte budget.
struct Budget(usize);
impl Write for Budget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.0 {
            return Err(std::io::ErrorKind::FileTooLarge.into());
        }
        self.0 -= bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn bounded(value: &Value) -> Result<(), Failure> {
    serde_json::to_writer(Budget(MAX_GROUP_VALUE_BYTES), value).map_err(|_| {
        Refusal::input(
            "group_value_too_large",
            "/group_by",
            "one group value must use at most 1024 compact JSON bytes",
        )
        .into()
    })
}
fn bucket(groups: &mut BTreeMap<String, Value>, key: Value, count: u64) -> Result<(), Failure> {
    let encoded = key.to_string();
    if let Some(group) = groups.get_mut(&encoded) {
        group["count"] = json!(group["count"].as_u64().unwrap_or(0) + count);
    } else {
        if groups.len() == MAX_BUCKETS {
            return Err(Refusal::input(
                "result_too_large",
                "/group_by",
                "at most 1000 buckets, including enum zero buckets",
            )
            .into());
        }
        groups.insert(encoded, json!({"key":key,"count":count}));
    }
    Ok(())
}

pub fn aggregate(ctx: &Context, req: &AggregateRequest) -> Result<Answer, Failure> {
    req.check()?;
    let mut store = open_store(&ctx.store, false)?;
    st!(store.read_schema(|tx| read_aggregate(ctx, req, tx)))
}
fn read_aggregate(
    ctx: &Context,
    req: &AggregateRequest,
    tx: &dyn AroundSource,
) -> Result<Answer, Failure> {
    req.check()?;
    let set = tx.schema_set();
    let principal = Principal::bind(set)?;
    let schema = set
        .schema_of(&req.object_type)
        .ok_or_else(|| Refusal::new("unavailable"))?;
    if !principal
        .view()
        .read_types(&schema.domain)
        .contains(&req.object_type)
    {
        return Err(Refusal::new("unavailable").into());
    }
    // Require a grant with both flags, just as native derive admission does.
    // This preflight has the same result on empty and populated stores.
    if !principal.view().entries.iter().any(|e| {
        e.domain == schema.domain
            && e.read.contains(&req.object_type)
            && e.derive.contains(&req.object_type)
    }) {
        return Err(Refusal::new(ont_core::Error::DeriveForbidden.code())
            .with(
                "rule",
                json!("aggregate requires a read and derive grant on the requested type"),
            )
            .into());
    }
    let properties = set
        .properties(&req.object_type)
        .ok_or_else(|| Refusal::new("unavailable"))?;
    let mut used = search::check_filter(set, &req.object_type, req.filter.as_ref())?;
    if let Some(f) = &req.filter {
        crate::around::unresolved_properties(f, &mut used);
    }
    let mut enums = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut combinations = 1usize;
    for (i, p) in req.group_by.iter().enumerate() {
        let def = properties
            .get(p)
            .filter(|_| seen.insert(p))
            .ok_or_else(|| {
                Refusal::input(
                    "invalid_group_by",
                    &format!("/group_by/{i}"),
                    "choose distinct properties of this readable type",
                )
                .with("available", json!(properties.keys().collect::<Vec<_>>()))
            })?;
        used.insert(p.clone());
        if let PropertyType::ValueType(r) = &def.ty
            && let Some(values) = schema
                .value_types
                .get(&r.value_type)
                .and_then(|v| v.enum_values.as_ref())
        {
            if values.len() > MAX_BUCKETS / combinations {
                return Err(Refusal::input(
                    "result_too_large",
                    "/group_by",
                    "at most 1000 buckets, including enum zero buckets",
                )
                .into());
            }
            combinations *= values.len();
            for v in values {
                bounded(v)?;
            }
            enums.insert(p, values);
        }
    }
    let admission = principal
        .authority()
        .admit(principal.session())
        .map_err(|_| internal!("authority_failure"))?;
    let mut groups = BTreeMap::new();
    // Complete enum products exist even on an empty store. Mixed enum keys
    // expand for each observed combination of non-enum keys.
    let expand = |key: &Value, groups: &mut BTreeMap<String, Value>| -> Result<(), Failure> {
        let mut keys = vec![key.clone()];
        for (p, values) in &enums {
            let mut next = Vec::new();
            for key in keys {
                for v in *values {
                    let mut k = key.clone();
                    k[*p] = v.clone();
                    next.push(k);
                }
            }
            keys = next;
        }
        for key in keys {
            bucket(groups, key, 0)?;
        }
        Ok(())
    };
    if enums.len() == req.group_by.len() {
        expand(&json!({}), &mut groups)?;
    }
    let (mut scanned, mut excluded, mut stale, mut partial) = (0, 0, 0, 0);
    let mut oldest: Option<u64> = None;
    let mut stale_sources = BTreeSet::new();
    let mut after = None;
    loop {
        let page = st!(tx.scan_head_access(&req.object_type, 256, after.as_ref()));
        if page.is_empty() {
            break;
        }
        after = page.last().map(|a| a.id.clone());
        for access in page {
            if access.retracted {
                continue;
            }
            let object = Object {
                id: access.id.clone(),
                scope: access.scope.clone(),
                head: String::new(),
                observations: vec![],
            };
            let snapshot = OneObject(Some(object));
            match admission.query(
                &snapshot,
                Query::Get {
                    id: access.id.clone(),
                },
                ctx.now,
            ) {
                Ok(_) => {}
                Err(ont_core::Error::Unavailable) => continue,
                _ => return Err(internal!("authority_failure").into()),
            }
            // A readable object outside the derive scope refuses before loading.
            if let Err(e) = admission.query(
                &snapshot,
                Query::Decide {
                    root: access.id.clone(),
                    relation: "aggregate".into(),
                },
                ctx.now,
            ) {
                return Err(Refusal::new(e.code()).into());
            }
            if scanned == SCAN_LIMIT {
                return Err(Refusal::new("result_too_large").into());
            }
            let Some(head) = st!(tx.head_after_access(&access)) else {
                continue;
            };
            scanned += 1;
            let obs = &head.head.observation;
            oldest = Some(oldest.map_or(obs.observed_at, |v| v.min(obs.observed_at)));
            if ctx.now > obs.fresh_until {
                stale += 1;
                if let Owner::Source(s) = &schema.object_types[&req.object_type].owner {
                    stale_sources.insert(format!("{s}: ont index --source {s}"));
                }
            }
            if obs.coverage == ont_core::Coverage::Partial {
                partial += 1;
            }
            let mut valid = true;
            for p in &used {
                if let Some(Knowledge::Value(v)) = obs.fields.get(p) {
                    // Bound group values before validation (which may walk arrays).
                    if seen.contains(p) {
                        bounded(v)?;
                    }
                    valid &= schema.validate_value(&properties[p].ty, v).is_ok();
                } else {
                    valid = false;
                }
            }
            if !valid {
                excluded += 1;
                continue;
            }
            match req
                .filter
                .as_ref()
                .map_or(Tri::True, |f| search::eval(f, &obs.fields))
            {
                Tri::False => continue,
                Tri::Unknown => {
                    excluded += 1;
                    continue;
                }
                Tri::True => {}
            }
            let mut key = json!({});
            for p in &req.group_by {
                if let Some(Knowledge::Value(v)) = obs.fields.get(p) {
                    key[p] = v.clone();
                }
            }
            // Existing zero buckets mark combinations already expanded.
            if !groups.contains_key(&key.to_string()) {
                let mut base = key.clone();
                for p in enums.keys() {
                    base.as_object_mut().map(|m| m.remove(*p));
                }
                expand(&base, &mut groups)?;
            }
            bucket(&mut groups, key, 1)?;
        }
    }
    Ok(Answer::Enveloped {
        pins: set
            .pin(&schema.domain)
            .map(|p| (schema.domain.clone(), p.clone()))
            .into_iter()
            .collect(),
        revision: principal.revision()?,
        result: json!({"kind":"aggregation","groups":groups.into_values().collect::<Vec<_>>(),"basis":{
            "scanned":scanned,"unresolved_excluded":excluded,"stale":stale,"partial":partial,"oldest_observed_at":oldest,
            "qualified":excluded == 0 && stale == 0 && partial == 0,"stale_sources":stale_sources}}),
    })
}

#[cfg(test)]
#[path = "aggregate_tests.rs"]
mod tests;
