//! `describe`: the installed domains, types, links, actions, functions and
//! sources that the principal's grants admit.
//!
//! Rules: a domain is listed only when the grant view admits at least one
//! of its object types (`read`) or action types (`actions`); an object type
//! when `read` admits it, with its declared properties (a `ref` property
//! keeps its declared target type name, which may name a type that is not
//! admitted: that is a stated limit of the claim); a link type only when
//! both of its ends are admitted; an action type when `actions` admits it; a
//! source only when an admitted object type names it as owner; the
//! functions of a listed domain (no function grant exists yet).
use crate::principal::Principal;
use crate::refusal::{open_store, st};
use crate::{Answer, Context, Failure, Refusal};
use ont_core::schema::Owner;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DescribeRequest {
    #[serde(default)]
    pub op: Option<String>,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub object_type: Option<String>,
}
impl DescribeRequest {
    /// Parse the strict request (at most `MAX_REQUEST` bytes; an `op`, when
    /// present, must be `describe`).
    pub fn parse(bytes: &[u8]) -> Result<Self, Refusal> {
        let req: Self = crate::cli::parse_request(bytes)?;
        if req.op.as_deref().is_some_and(|op| op != "describe") {
            return Err(Refusal::new("invalid_request"));
        }
        Ok(req)
    }
}

pub fn describe(ctx: &Context, req: &DescribeRequest) -> Result<Answer, Failure> {
    let mut store = open_store(&ctx.store, false)?;
    let provenance = st!(store.schema_provenance());
    let set = st!(store.schema_set()).clone();
    let principal = Principal::bind(&set)?;
    let view = principal.view();
    let listed = view.domains();
    if let Some(domain) = &req.domain
        && !listed.contains(domain)
    {
        // A domain that is not installed and one that is not admitted look
        // the same.
        return Err(Refusal::new("unknown_domain")
            .with("domain", json!(domain))
            .into());
    }
    let domains: Vec<String> = match &req.domain {
        Some(d) => vec![d.clone()],
        None => listed.iter().cloned().collect(),
    };
    let all_read = view.all_read_types();
    let mut object_types = Vec::new();
    let mut link_types = Vec::new();
    let mut action_types = Vec::new();
    let mut functions = Vec::new();
    let mut sources = Vec::new();
    for domain in &domains {
        let Some(schema) = set.schema(domain) else {
            continue;
        };
        let readable = view.read_types(domain);
        let actions = view.action_types(domain);
        let mut owning_sources: BTreeSet<&String> = BTreeSet::new();
        for (name, ty) in &schema.object_types {
            if !readable.contains(name) {
                continue;
            }
            if let Owner::Source(s) = &ty.owner {
                owning_sources.insert(s);
            }
            if req.object_type.as_ref().is_some_and(|w| w != name) {
                continue;
            }
            object_types.push(json!({
                "domain":domain,
                "name":name,
                "description":ty.description,
                "title":ty.title,
                "primary_key":serde_json::to_value(&ty.primary_key).unwrap_or(Value::Null),
                "owner":serde_json::to_value(&ty.owner).unwrap_or(Value::Null),
                "properties":ty.properties.iter().map(|(pname,p)| {
                    json!({
                        "name":pname,
                        "description":p.description,
                        "type":serde_json::to_value(&p.ty).unwrap_or(Value::Null),
                        "required":p.required,
                        "from":p.from,
                        "template":p.template,
                        "transform":p.transform,
                        "max_bytes":p.max_bytes
                    })
                }).collect::<Vec<_>>()
            }));
        }
        for (name, lt) in &schema.link_types {
            if !all_read.contains(&lt.from) || !all_read.contains(&lt.to) {
                continue;
            }
            if req
                .object_type
                .as_ref()
                .is_some_and(|w| w != &lt.from && w != &lt.to)
            {
                continue;
            }
            link_types.push(json!({
                "domain":domain,
                "name":name,
                "from":lt.from,
                "via":lt.via,
                "to":lt.to,
                "cardinality":serde_json::to_value(lt.cardinality).unwrap_or(Value::Null),
                "names":{"forward":lt.names.forward,"reverse":lt.names.reverse}
            }));
        }
        for (name, at) in &schema.action_types {
            if !actions.contains(name) {
                continue;
            }
            action_types.push(json!({
                "domain":domain,
                "name":name,
                "description":at.description,
                "parameters":at.parameters.iter().map(|(pname,p)| {
                    json!({
                        "name":pname,
                        "description":p.description,
                        "type":serde_json::to_value(&p.ty).unwrap_or(Value::Null),
                        "required":p.required,
                        "max_bytes":p.max_bytes
                    })
                }).collect::<Vec<_>>(),
                "criterion_codes":at.submission_criteria.iter().map(|c| c.code.clone()).collect::<Vec<_>>()
            }));
        }
        for name in schema.functions.keys() {
            functions.push(json!({"domain":domain,"name":name}));
        }
        for (name, src) in &schema.sources {
            if owning_sources.contains(name) {
                sources.push(json!({
                    "domain":domain,
                    "name":name,
                    "kind":src.kind,
                    "options":src.options,
                    "exclude_rule":src.options.get("exclude").map(|_| {
                        "an exclude pattern that matches a directory excludes everything under it"
                    })
                }));
            }
        }
    }
    let pins: BTreeMap<_, _> = domains
        .iter()
        .filter_map(|d| set.pin(d).map(|p| (d.clone(), p.clone())))
        .collect();
    Ok(Answer::Enveloped {
        revision: principal.revision()?,
        result: json!({
            "domains":domains.iter().map(|d| {
                let p = set.pin(d);
                let mut domain = json!({"domain":d,"version":p.map(|p| p.version),"sha256":p.map(|p| p.sha256.clone())});
                if let Some(prov) = provenance.get(d) {
                    if let Some(repository) = &prov.repository {
                        domain["repository"] = json!(repository);
                    }
                    if let Some(commit) = &prov.commit {
                        domain["commit"] = json!(commit);
                    }
                    if let Some(path) = &prov.path {
                        domain["path"] = json!(path);
                    }
                }
                domain
            }).collect::<Vec<_>>(),
            "object_types":object_types,
            "link_types":link_types,
            "action_types":action_types,
            "functions":functions,
            "sources":sources
        }),
        pins,
    })
}
