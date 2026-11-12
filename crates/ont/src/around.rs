//! Object-set reads over declared reference links, in one snapshot/admission.
use crate::get::OneObject;
use crate::principal::Principal;
use crate::refusal::{open_store, st};
use crate::search::{self, DEFAULT_LIMIT, Filter, MAX_LIMIT, SCAN_LIMIT, Tri};
use crate::{Answer, Context, Failure, Refusal, internal};
use ont_core::schema::{LinkType, Owner, SchemaSet};
use ont_core::{Admission, Id, Knowledge, Object, Query, Response, Scope};
use ont_store::schema_mode::{
    CandidateCondition, HeadAccess, SchemaHead, SchemaRead, StoreRefusal,
};
use serde::Deserialize;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub const MAX_HOPS: usize = 3;
/// Reference entries charged across all hops, including repeated/missing IDs.
pub const MAX_REFERENCE_ENTRIES: usize = 10_000;
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FromSet {
    pub object_type: String,
    #[serde(default, rename = "where")]
    pub filter: Option<Filter>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AroundRequest {
    #[serde(default)]
    pub op: Option<String>,
    pub from: FromSet,
    pub path: Vec<String>,
    #[serde(default)]
    pub limit: Option<u64>,
}
impl AroundRequest {
    pub fn parse(bytes: &[u8]) -> Result<Self, Refusal> {
        let req: Self = crate::cli::parse_request(bytes)?;
        if req.op.as_deref().is_some_and(|op| op != "search_around") {
            return Err(Refusal::input(
                "invalid_request",
                "/op",
                "around accepts search_around; use search for search, aggregate for aggregate",
            ));
        }
        Ok(req)
    }
}
struct Hop {
    link: LinkType,
    reverse: bool,
}
fn plan(
    set: &SchemaSet,
    readable: &BTreeSet<String>,
    ty: &str,
    path: &[String],
) -> Result<Vec<Hop>, Failure> {
    if path.is_empty() || path.len() > MAX_HOPS {
        return Err(Refusal::input(
            "invalid_path",
            "/path",
            "1 to 3 forward or reverse link names",
        )
        .into());
    }
    let mut reached = ty.to_string();
    let mut hops = Vec::new();
    for (i, name) in path.iter().enumerate() {
        let mut available = BTreeMap::new();
        for domain in set.pins().keys() {
            if let Some(schema) = set.schema(domain) {
                for link in schema.link_types.values() {
                    // Match describe: a usable link has two readable endpoints.
                    if !readable.contains(&link.from) || !readable.contains(&link.to) {
                        continue;
                    }
                    if link.from == reached {
                        available.insert(
                            link.names.forward.clone(),
                            Hop {
                                link: link.clone(),
                                reverse: false,
                            },
                        );
                    }
                    if link.to == reached {
                        available.insert(
                            link.names.reverse.clone(),
                            Hop {
                                link: link.clone(),
                                reverse: true,
                            },
                        );
                    }
                }
            }
        }
        let Some(hop) = available.remove(name) else {
            return Err(Refusal::input(
                "invalid_link",
                &format!("/path/{i}"),
                "choose a forward or reverse link name available from the reached type",
            )
            .with("available", json!(available.keys().collect::<Vec<_>>()))
            .into());
        };
        reached = if hop.reverse {
            &hop.link.from
        } else {
            &hop.link.to
        }
        .clone();
        hops.push(hop);
    }
    Ok(hops)
}

#[derive(Default)]
struct Basis {
    scanned: usize,
    oversized_excluded: usize,
    dangling: usize,
    stale: usize,
    partial: usize,
    unresolved_excluded: usize,
    used_unresolved: bool,
    stale_sources: BTreeSet<String>,
}
pub(crate) trait AroundSource {
    fn schema_set(&self) -> &SchemaSet;
    fn head_access(&self, id: &Id) -> Result<Option<HeadAccess>, StoreRefusal>;
    fn candidate_access(
        &self,
        ty: &str,
        condition: Option<&CandidateCondition>,
        now: u64,
        limit: usize,
        after: Option<&Id>,
        scopes: &[Scope],
    ) -> Result<Vec<HeadAccess>, StoreRefusal>;
    fn head_after_access(&self, access: &HeadAccess) -> Result<Option<SchemaHead>, StoreRefusal>;
}
impl AroundSource for SchemaRead<'_> {
    fn schema_set(&self) -> &SchemaSet {
        self.schema_set()
    }
    fn head_access(&self, id: &Id) -> Result<Option<HeadAccess>, StoreRefusal> {
        self.head_access(id)
    }
    fn candidate_access(
        &self,
        ty: &str,
        condition: Option<&CandidateCondition>,
        now: u64,
        limit: usize,
        after: Option<&Id>,
        scopes: &[Scope],
    ) -> Result<Vec<HeadAccess>, StoreRefusal> {
        self.candidate_access(ty, condition, now, limit, after, scopes)
    }
    fn head_after_access(&self, access: &HeadAccess) -> Result<Option<SchemaHead>, StoreRefusal> {
        self.head_after_access(access)
    }
}
struct Reader<'a, 'b> {
    tx: &'a dyn AroundSource,
    admission: &'a Admission<'b>,
    now: u64,
    loaded: BTreeMap<Id, Arc<SchemaHead>>,
    counts: BTreeMap<String, usize>,
    oversized: BTreeSet<Id>,
    basis: Basis,
}
impl Reader<'_, '_> {
    fn allowed(&self, id: &Id, scope: &Scope) -> Result<bool, Failure> {
        let object = Object {
            id: id.clone(),
            scope: scope.clone(),
            head: String::new(),
            observations: vec![],
        };
        match self.admission.query(
            &OneObject(Some(object)),
            Query::Get { id: id.clone() },
            self.now,
        ) {
            Ok(Response::Ok { .. }) => Ok(true),
            Ok(Response::Refused { .. }) | Err(ont_core::Error::Unavailable) => Ok(false),
            _ => Err(internal!("authority_failure").into()),
        }
    }
    fn load(&mut self, access: HeadAccess) -> Result<Option<Arc<SchemaHead>>, Failure> {
        if access.retracted || !self.allowed(&access.id, &access.scope)? {
            return Ok(None);
        }
        if self.oversized.contains(&access.id) {
            return Ok(None);
        }
        if let Some(head) = self.loaded.get(&access.id) {
            return Ok(Some(head.clone()));
        }
        let count = self
            .counts
            .entry(access.id.object_type.clone())
            .or_default();
        if *count == SCAN_LIMIT {
            return Err(Refusal::new("result_too_large").into());
        }
        let head = match self.tx.head_after_access(&access) {
            Ok(Some(head)) => head,
            Ok(None) => return Ok(None),
            Err(e) if e.code == ont_core::Error::ObservationTooLarge.code() => {
                *count += 1;
                self.basis.scanned += 1;
                self.basis.oversized_excluded += 1;
                self.oversized.insert(access.id);
                return Ok(None);
            }
            Err(e) => return Err(crate::refusal::store_err(e)),
        };
        *count += 1;
        self.basis.scanned += 1;
        let obs = &head.head.observation;
        if self.now > obs.fresh_until {
            self.basis.stale += 1;
            if let Some(schema) = self.tx.schema_set().schema_of(&head.id.object_type)
                && let Owner::Source(source) = &schema.object_types[&head.id.object_type].owner
            {
                self.basis
                    .stale_sources
                    .insert(format!("{source}: ont index --source {source}"));
            }
        }
        if obs.coverage == ont_core::Coverage::Partial {
            self.basis.partial += 1;
        }
        // Share loaded heads: copying a head here would clone its reference
        // arrays before the traversal has checked their entry budget.
        let head = Arc::new(head);
        self.loaded.insert(head.id.clone(), head.clone());
        Ok(Some(head))
    }
    fn named(&mut self, id: &Id) -> Result<Option<Arc<SchemaHead>>, Failure> {
        if let Some(head) = self.loaded.get(id) {
            return Ok(Some(head.clone()));
        }
        match st!(self.tx.head_access(id)) {
            Some(access) => self.load(access),
            None => Ok(None),
        }
    }
    fn scan(
        &mut self,
        ty: &str,
        condition: Option<&CandidateCondition>,
        scopes: &[Scope],
    ) -> Result<Vec<Arc<SchemaHead>>, Failure> {
        let mut after = None;
        let mut heads = Vec::new();
        loop {
            let page =
                st!(self
                    .tx
                    .candidate_access(ty, condition, self.now, 256, after.as_ref(), scopes));
            if page.is_empty() {
                break;
            }
            after = page.last().map(|a| a.id.clone());
            for access in page {
                if let Some(head) = self.load(access)? {
                    heads.push(head);
                }
            }
        }
        Ok(heads)
    }
}
fn refs<'a>(
    head: &'a SchemaHead,
    via: &str,
    examined: &mut usize,
) -> Result<Option<impl Iterator<Item = Id> + 'a>, Failure> {
    let Some(Knowledge::Value(value)) = head.head.observation.fields.get(via) else {
        return Ok(None);
    };
    let values = value
        .as_array()
        .map_or(std::slice::from_ref(value), Vec::as_slice);
    // Reserve the complete array before iteration/deserialization or lookups,
    // including reverse scans that may find a match before its final entry.
    if values.len() > MAX_REFERENCE_ENTRIES - *examined {
        return Err(Refusal::input(
            "result_too_large",
            "/path",
            "at most 10000 reference entries across all hops",
        )
        .with("max_reference_entries", json!(MAX_REFERENCE_ENTRIES))
        .into());
    }
    *examined += values.len();
    Ok(Some(
        values
            .iter()
            .filter_map(|value| Id::deserialize(value).ok()),
    ))
}

// Qualification requires value states even for an explicit unresolved predicate.
pub(crate) fn unresolved_properties(filter: &Filter, used: &mut BTreeSet<String>) {
    match filter {
        Filter::Unresolved(p) => {
            used.insert(p.clone());
        }
        Filter::All(fs) | Filter::Any(fs) | Filter::NoneOf(fs) => {
            for f in fs {
                unresolved_properties(f, used);
            }
        }
        _ => {}
    }
}

pub fn around(ctx: &Context, req: &AroundRequest) -> Result<Answer, Failure> {
    let limit = req.limit.unwrap_or(DEFAULT_LIMIT);
    if limit == 0 || limit > MAX_LIMIT {
        return Err(Refusal::input(
            if limit == 0 {
                "invalid_request"
            } else {
                "limit_too_large"
            },
            "/limit",
            "integer from 1 to 1000",
        )
        .into());
    }
    // Structural refusals also precede opening the store.
    if req.path.is_empty() || req.path.len() > MAX_HOPS {
        return Err(Refusal::input(
            "invalid_path",
            "/path",
            "1 to 3 forward or reverse link names",
        )
        .into());
    }
    let mut store = open_store(&ctx.store, false)?;
    st!(store.read_schema(|tx| read_around(ctx, req, tx)))
}

fn read_around(
    ctx: &Context,
    req: &AroundRequest,
    tx: &dyn AroundSource,
) -> Result<Answer, Failure> {
    let set = tx.schema_set();
    let principal = Principal::bind(set)?;
    let schema = set
        .schema_of(&req.from.object_type)
        .ok_or_else(|| Refusal::new("unavailable"))?;
    if !principal
        .view()
        .read_types(&schema.domain)
        .contains(&req.from.object_type)
    {
        return Err(Refusal::new("unavailable").into());
    }
    let hops = plan(
        set,
        &principal.view().all_read_types(),
        &req.from.object_type,
        &req.path,
    )?;
    let mut used = search::check_filter(set, &req.from.object_type, req.from.filter.as_ref())
        .map_err(|mut e| {
            if let Failure::Refused(r) = &mut e
                && let Some(field) = r.extra.get("field").and_then(serde_json::Value::as_str)
            {
                r.extra
                    .insert("field".into(), json!(format!("/from{field}")));
            }
            e
        })?;
    if let Some(f) = &req.from.filter {
        unresolved_properties(f, &mut used);
    }
    let revision = principal.revision()?;
    let admission = principal
        .authority()
        .admit(principal.session())
        .map_err(|_| internal!("authority_failure"))?;
    let mut reader = Reader {
        tx,
        admission: &admission,
        now: ctx.now,
        loaded: BTreeMap::new(),
        counts: BTreeMap::new(),
        oversized: BTreeSet::new(),
        basis: Basis::default(),
    };
    let mut current = BTreeMap::new();
    let condition = search::candidate_condition(req.from.filter.as_ref(), &used);
    let scopes = search::read_scopes(&principal, &req.from.object_type);
    for head in reader.scan(&req.from.object_type, condition.as_ref(), &scopes)? {
        let fields = &head.head.observation.fields;
        if used
            .iter()
            .any(|p| !matches!(fields.get(p), Some(Knowledge::Value(_))))
        {
            reader.basis.used_unresolved = true;
        }
        match req
            .from
            .filter
            .as_ref()
            .map_or(Tri::True, |f| search::eval(f, fields))
        {
            Tri::True => {
                current.insert(head.id.clone(), head);
            }
            Tri::Unknown => reader.basis.unresolved_excluded += 1,
            Tri::False => {}
        }
    }
    let mut domains = BTreeSet::from([schema.domain.clone()]);
    let mut examined = 0;
    for hop in &hops {
        let target_type = if hop.reverse {
            &hop.link.from
        } else {
            &hop.link.to
        };
        if let Some(s) = set.schema_of(target_type) {
            domains.insert(s.domain.clone());
        }
        let mut next = BTreeMap::new();
        if hop.reverse {
            if !current.is_empty() {
                for head in reader.scan(
                    target_type,
                    None,
                    &search::read_scopes(&principal, target_type),
                )? {
                    match refs(&head, &hop.link.via, &mut examined)? {
                        Some(ids) => {
                            let mut linked = false;
                            for id in ids {
                                if id.object_type != hop.link.to {
                                    reader.basis.unresolved_excluded += 1;
                                } else {
                                    linked |= current.contains_key(&id);
                                }
                            }
                            if linked {
                                next.insert(head.id.clone(), head.clone());
                            }
                        }
                        None => reader.basis.used_unresolved = true,
                    }
                }
            }
        } else {
            for head in current.values() {
                match refs(head, &hop.link.via, &mut examined)? {
                    None => reader.basis.used_unresolved = true,
                    Some(ids) => {
                        for id in ids {
                            // Old schema pins may have allowed a different target.
                            if id.object_type != hop.link.to {
                                reader.basis.unresolved_excluded += 1;
                                continue;
                            }
                            match reader.named(&id)? {
                                Some(target) => {
                                    next.insert(id, target);
                                }
                                None => reader.basis.dangling += 1,
                            }
                        }
                    }
                }
            }
        }
        current = next;
    }
    let matched = current.len();
    let objects: Vec<_> = current
        .values()
        .take(req.limit.unwrap_or(DEFAULT_LIMIT) as usize)
        .map(|head| {
            let ty = set
                .schema_of(&head.id.object_type)
                .and_then(|s| s.object_types.get(&head.id.object_type));
            let observed = ont_core::Observed {
                observation: head.head.observation.clone(),
                stale: ctx.now > head.head.observation.fresh_until,
            };
            search::object_json(head, ty, &observed)
        })
        .collect();
    let b = reader.basis;
    let qualified = b.stale == 0
        && b.partial == 0
        && b.unresolved_excluded == 0
        && !b.used_unresolved
        && b.dangling == 0
        && b.oversized_excluded == 0;
    let pins = domains
        .into_iter()
        .filter_map(|d| set.pin(&d).map(|p| (d, p.clone())))
        .collect();
    Ok(Answer::Enveloped {
        pins,
        revision,
        result: json!({"kind":"object_set","objects":objects,"continuation":null,
            "basis":{"hops":hops.len(),"scanned":b.scanned,"matched":matched,"dangling":b.dangling,"stale":b.stale,"partial":b.partial,
                "unresolved_excluded":b.unresolved_excluded,"oversized_excluded":b.oversized_excluded,"qualified":qualified,"stale_sources":b.stale_sources}}),
    })
}

#[cfg(test)]
#[path = "around_tests.rs"]
mod tests;
