//! Actions: list, validate and apply. Apply is the one write path for
//! Ontology-owned types: grant check, parameter typing, expected-head
//! check, criteria, observations and the `action_log` row in ONE
//! IMMEDIATE transaction, then the `edits.jsonl` append after commit.
use crate::principal::Principal;
use crate::refusal::{open_store, st, store_err};
use crate::{Answer, Context, Failure, Refusal, internal, sha256_hex};
use ont_core::schema::{
    ActionType, Condition, Criterion, Owner, PropertyType, ScalarType, Schema, SchemaSet,
};
use ont_core::{Change, Coverage, Id, Knowledge, Lineage, Observation, Scope};
use ont_store::Append;
use ont_store::schema_mode::{ActionLogEntry, SchemaHead};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

pub const FRESH_UNTIL_MAX: u64 = 253402300799;

#[doc(hidden)]
pub static FAIL_AFTER_COMMIT: AtomicBool = AtomicBool::new(false);
thread_local! {
    #[doc(hidden)]
    pub static FAIL_AFTER_COMMIT_LOCAL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
#[doc(hidden)]
pub fn __set_fail_after_commit(v: bool) {
    FAIL_AFTER_COMMIT_LOCAL.with(|c| c.set(v));
}
fn take_fail_after_commit() -> bool {
    FAIL_AFTER_COMMIT_LOCAL.with(|c| c.replace(false))
        || FAIL_AFTER_COMMIT.swap(false, Ordering::SeqCst)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListActionsRequest {
    #[serde(default)]
    pub op: Option<String>,
    #[serde(default)]
    pub domain: Option<String>,
}
impl ListActionsRequest {
    pub fn parse(bytes: &[u8]) -> Result<Self, Refusal> {
        let req: Self = crate::cli::parse_request(bytes)?;
        if req.op.as_deref().is_some_and(|op| op != "list_actions") {
            return Err(Refusal::new("invalid_request"));
        }
        Ok(req)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedHead {
    pub id: Id,
    pub revision: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidateActionRequest {
    #[serde(default)]
    pub op: Option<String>,
    pub action_type: String,
    #[serde(default)]
    pub parameters: Value,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub expected: Option<Vec<ExpectedHead>>,
}
impl ValidateActionRequest {
    pub fn parse(bytes: &[u8]) -> Result<Self, Refusal> {
        let req: Self = crate::cli::parse_request(bytes)?;
        if req.op.as_deref().is_some_and(|op| op != "validate_action") {
            return Err(Refusal::new("invalid_request"));
        }
        Ok(req)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyActionRequest {
    #[serde(default)]
    pub op: Option<String>,
    pub action_type: String,
    #[serde(default)]
    pub parameters: Value,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub expected: Option<Vec<ExpectedHead>>,
    #[serde(default)]
    pub request_id: Option<String>,
}
impl ApplyActionRequest {
    pub fn parse(bytes: &[u8]) -> Result<Self, Refusal> {
        let req: Self = crate::cli::parse_request(bytes)?;
        if req.op.as_deref().is_some_and(|op| op != "apply_action") {
            return Err(Refusal::new("invalid_request"));
        }
        Ok(req)
    }
}

pub fn canonical(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut out = Map::new();
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            for k in keys {
                out.insert(k.clone(), canonical(&m[k]));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(canonical).collect()),
        other => other.clone(),
    }
}
fn canonical_string(v: &Value) -> String {
    serde_json::to_string(&canonical(v)).unwrap_or_default()
}
fn invalid_parameter(name: &str) -> Failure {
    Refusal::new("invalid_parameter")
        .with("parameter", json!(name))
        .into()
}
fn find_action_type<'a>(set: &'a SchemaSet, name: &str) -> Option<(&'a Schema, &'a ActionType)> {
    for domain in set.pins().keys() {
        if let Some(schema) = set.schema(domain)
            && let Some(at) = schema.action_types.get(name)
        {
            return Some((schema, at));
        }
    }
    None
}
fn actor_json(ctx: &Context, agent: &Option<String>, session: &Option<String>) -> Value {
    let agent = agent.clone().or_else(|| ctx.agent.clone());
    let session = session.clone().or_else(|| ctx.session.clone());
    let client = if ctx.client.is_empty() {
        "cli".to_string()
    } else {
        ctx.client.clone()
    };
    let mut m = Map::new();
    m.insert("principal".into(), json!("agent"));
    m.insert("client".into(), json!(client));
    if let Some(a) = agent {
        m.insert("agent".into(), json!(a));
    }
    if let Some(s) = session {
        m.insert("session".into(), json!(s));
    }
    Value::Object(m)
}
fn valid_request_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}
fn derive_request_action_id(request_id: &str) -> String {
    let h = sha256_hex(format!("request_id:{request_id}").as_bytes());
    format!("act-rq-{}", &h[..32])
}
fn allocate_action_id(
    now: u64,
    action_type: &str,
    params: &Value,
    basis: &Value,
    log_len: usize,
    request_id: &Option<String>,
) -> String {
    if let Some(r) = request_id {
        return derive_request_action_id(r);
    }
    let seed = canonical_string(&json!({
        "now":now,"action_type":action_type,"parameters":params,"basis":basis,"log_len":log_len
    }));
    let h = sha256_hex(seed.as_bytes());
    format!("act-{now}-{}", &h[..16])
}

fn parse_decimal_ok(s: &str) -> bool {
    let rest = s.strip_prefix('-').unwrap_or(s);
    if rest.is_empty() || rest.starts_with('+') {
        return false;
    }
    let (int, frac) = match rest.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (rest, None),
    };
    !int.is_empty()
        && int.bytes().all(|b| b.is_ascii_digit())
        && frac.is_none_or(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()))
}
fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}
fn cmp_values(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            if let (Some(xi), Some(yi)) = (x.as_i64(), y.as_i64()) {
                Some(xi.cmp(&yi))
            } else if let (Some(xu), Some(yu)) = (x.as_u64(), y.as_u64()) {
                Some(xu.cmp(&yu))
            } else {
                x.as_f64()?.partial_cmp(&y.as_f64()?)
            }
        }
        (Value::String(x), Value::String(y)) => Some(x.cmp(y)),
        _ => None,
    }
}
fn value_matches(schema: &Schema, ty: &PropertyType, v: &Value) -> bool {
    match ty {
        PropertyType::Scalar(s) => match s {
            ScalarType::String => v.is_string(),
            ScalarType::Integer | ScalarType::Timestamp => {
                v.as_i64().is_some() || v.as_u64().is_some()
            }
            ScalarType::Boolean => v.is_boolean(),
            ScalarType::Decimal => v.as_str().is_some_and(parse_decimal_ok),
            ScalarType::Date => v.as_str().is_some_and(valid_date),
        },
        PropertyType::Ref(r) => {
            let Ok(id) = serde_json::from_value::<Id>(v.clone()) else {
                return false;
            };
            id.validate().is_ok() && id.object_type == r.target
        }
        PropertyType::Array(a) => v
            .as_array()
            .is_some_and(|items| items.iter().all(|i| value_matches(schema, &a.array, i))),
        PropertyType::Struct(st) => {
            let Some(map) = v.as_object() else {
                return false;
            };
            map.len() == st.struct_.len()
                && st
                    .struct_
                    .iter()
                    .all(|(k, t)| map.get(k).is_some_and(|fv| value_matches(schema, t, fv)))
        }
        PropertyType::ValueType(vt) => {
            let Some(def) = schema.value_types.get(&vt.value_type) else {
                return false;
            };
            let base_ty = PropertyType::Scalar(def.base);
            if !value_matches(schema, &base_ty, v) {
                return false;
            }
            if let Some(vals) = &def.enum_values
                && !vals.contains(v)
            {
                return false;
            }
            if let Some(min) = &def.min
                && cmp_values(v, min) == Some(std::cmp::Ordering::Less)
            {
                return false;
            }
            if let Some(max) = &def.max
                && cmp_values(v, max) == Some(std::cmp::Ordering::Greater)
            {
                return false;
            }
            true
        }
    }
}
fn validate_parameters(
    schema: &Schema,
    at: &ActionType,
    params: &Value,
) -> Result<BTreeMap<String, Value>, Failure> {
    let Some(obj) = params.as_object() else {
        return Err(Refusal::new("invalid_request").into());
    };
    for key in obj.keys() {
        if !at.parameters.contains_key(key) {
            return Err(invalid_parameter(key));
        }
    }
    let mut out = BTreeMap::new();
    for (name, def) in &at.parameters {
        match obj.get(name) {
            None => {
                if def.required {
                    return Err(invalid_parameter(name));
                }
            }
            Some(v) => {
                if !value_matches(schema, &def.ty, v) {
                    return Err(invalid_parameter(name));
                }
                if let (Some(limit), Value::String(s)) = (def.max_bytes, v)
                    && s.len() as u64 > limit
                {
                    return Err(invalid_parameter(name));
                }
                out.insert(name.clone(), v.clone());
            }
        }
    }
    Ok(out)
}

#[derive(Default, Clone)]
struct Snapshot {
    heads: BTreeMap<String, SchemaHead>,
}
impl Snapshot {
    fn key(id: &Id) -> String {
        canonical_string(&serde_json::to_value(id).unwrap_or(Value::Null))
    }
    fn insert(&mut self, h: SchemaHead) {
        self.heads.insert(Self::key(&h.id), h);
    }
    fn head(&self, id: &Id) -> Option<&SchemaHead> {
        self.heads.get(&Self::key(id))
    }
    fn live(&self, id: &Id) -> Option<&SchemaHead> {
        self.head(id).filter(|h| !h.retracted)
    }
    fn scan_type(&self, object_type: &str) -> Vec<&SchemaHead> {
        self.heads
            .values()
            .filter(|h| h.id.object_type == object_type && !h.retracted)
            .collect()
    }
}
fn field_value<'a>(head: &'a SchemaHead, prop: &str) -> Option<&'a Value> {
    match head.head.observation.fields.get(prop) {
        Some(Knowledge::Value(v)) => Some(v),
        _ => None,
    }
}
fn id_from_value(v: &Value) -> Option<Id> {
    serde_json::from_value(v.clone()).ok()
}
fn ids_from_value(v: &Value) -> Vec<Id> {
    if let Some(id) = id_from_value(v) {
        return vec![id];
    }
    v.as_array()
        .map(|a| a.iter().filter_map(id_from_value).collect())
        .unwrap_or_default()
}
struct Eval<'a> {
    set: &'a SchemaSet,
    snap: &'a Snapshot,
    params: &'a BTreeMap<String, Value>,
    actor: &'a Value,
    now: u64,
    basis: BTreeMap<String, Value>,
}
impl Eval<'_> {
    fn note(&mut self, head: &SchemaHead) {
        self.basis.insert(
            Snapshot::key(&head.id),
            json!({"id":head.id,"revision":head.head.observation.revision}),
        );
    }
    fn resolve(&self, v: &Value, ctx: Option<&SchemaHead>) -> Option<Value> {
        if let Value::String(s) = v {
            if let Some(p) = s.strip_prefix('$') {
                return self.params.get(p).cloned();
            }
            if let Some(head) = ctx
                && head.head.observation.fields.contains_key(s)
            {
                return field_value(head, s).cloned();
            }
            // A bare property name at the top level names a property of
            // an object a reference parameter points to.
            for pv in self.params.values() {
                if let Some(id) = id_from_value(pv)
                    && let Some(h) = self.snap.live(&id)
                    && h.head.observation.fields.contains_key(s)
                {
                    return field_value(h, s).cloned();
                }
            }
        }
        Some(v.clone())
    }
    fn eval_group(
        &mut self,
        all: &Option<Vec<Condition>>,
        any: &Option<Vec<Condition>>,
        none: &Option<Vec<Condition>>,
        ctx: Option<&SchemaHead>,
    ) -> Result<bool, Failure> {
        if let Some(cs) = all {
            for c in cs {
                if !self.eval(c, ctx)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        if let Some(cs) = any {
            for c in cs {
                if self.eval(c, ctx)? {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
        if let Some(cs) = none {
            for c in cs {
                if self.eval(c, ctx)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        Ok(true)
    }
    fn eval(&mut self, c: &Condition, ctx: Option<&SchemaHead>) -> Result<bool, Failure> {
        match c {
            Condition::Exists(s) => Ok(self.exists_fresh(s, false)),
            Condition::Fresh(s) => Ok(self.exists_fresh(s, true)),
            Condition::Eq(p) => Ok(self.compare(p, ctx, |o| o == std::cmp::Ordering::Equal, true)),
            Condition::Ne(p) => Ok(!self.compare(p, ctx, |o| o == std::cmp::Ordering::Equal, true)),
            Condition::In(p) => {
                let a = self.resolve(&p[0], ctx);
                let b = self.resolve(&p[1], ctx);
                Ok(match (a, b) {
                    (Some(a), Some(Value::Array(items))) => items.contains(&a),
                    _ => false,
                })
            }
            Condition::Lt(p) => Ok(self.compare(p, ctx, |o| o == std::cmp::Ordering::Less, false)),
            Condition::Le(p) => {
                Ok(self.compare(p, ctx, |o| o != std::cmp::Ordering::Greater, false))
            }
            Condition::Gt(p) => {
                Ok(self.compare(p, ctx, |o| o == std::cmp::Ordering::Greater, false))
            }
            Condition::Ge(p) => Ok(self.compare(p, ctx, |o| o != std::cmp::Ordering::Less, false)),
            Condition::Actor(v) => {
                let want = self.resolve(v, ctx);
                Ok(match want {
                    Some(Value::String(s)) => {
                        self.actor.get("agent").and_then(|x| x.as_str()) == Some(s.as_str())
                            || self.actor.get("principal").and_then(|x| x.as_str())
                                == Some(s.as_str())
                    }
                    Some(Value::Object(m)) => {
                        m.iter().all(|(k, val)| self.actor.get(k) == Some(val))
                    }
                    Some(other) => self.actor.get("agent") == Some(&other),
                    None => false,
                })
            }
            Condition::Linked(link) => self.linked(link),
            Condition::All(cs) => {
                for x in cs {
                    if !self.eval(x, ctx)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Condition::Any(cs) => {
                for x in cs {
                    if self.eval(x, ctx)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Condition::NoneOf(cs) => {
                for x in cs {
                    if self.eval(x, ctx)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
        }
    }
    fn compare(
        &self,
        p: &[Value; 2],
        ctx: Option<&SchemaHead>,
        ok: impl Fn(std::cmp::Ordering) -> bool,
        equality: bool,
    ) -> bool {
        let a = self.resolve(&p[0], ctx);
        let b = self.resolve(&p[1], ctx);
        match (a, b) {
            (Some(a), Some(b)) => {
                if equality && ctx.is_none() {
                    return a == b;
                }
                if a == b && equality {
                    return true;
                }
                cmp_values(&a, &b).is_some_and(ok)
            }
            _ => false,
        }
    }
    fn exists_fresh(&mut self, s: &str, fresh: bool) -> bool {
        let Some(p) = s.strip_prefix('$') else {
            return false;
        };
        let Some(v) = self.params.get(p).cloned() else {
            return false;
        };
        if let Some(id) = id_from_value(&v) {
            match self.snap.live(&id) {
                Some(h) => {
                    let is_fresh = self.now <= h.head.observation.fresh_until;
                    let rev = h.head.observation.revision.clone();
                    let head_clone = SchemaHead {
                        id: h.id.clone(),
                        scope: h.scope.clone(),
                        head: h.head.clone(),
                        retracted: h.retracted,
                        schema_pin: h.schema_pin.clone(),
                    };
                    let _ = rev;
                    self.note(&head_clone);
                    !fresh || is_fresh
                }
                None => false,
            }
        } else {
            !v.is_null()
        }
    }
    fn linked(&mut self, link: &ont_core::schema::LinkedConditionWire) -> Result<bool, Failure> {
        let Some(pname) = link.from.strip_prefix('$') else {
            return Err(Refusal::new("criterion_not_evaluable")
                .with("link", json!(link.link))
                .into());
        };
        let Some(from_val) = self.params.get(pname).cloned() else {
            return Ok(false);
        };
        let Some(from_id) = id_from_value(&from_val) else {
            return Ok(false);
        };
        let mut found = None;
        for schema in self.set.pins().keys().filter_map(|d| self.set.schema(d)) {
            for lt in schema.link_types.values() {
                if lt.names.forward == link.link || lt.names.reverse == link.link {
                    found = Some(lt.clone());
                }
            }
        }
        let Some(lt) = found else {
            return Err(Refusal::new("criterion_not_evaluable")
                .with("link", json!(link.link))
                .into());
        };
        let mut candidates: Vec<SchemaHead> = Vec::new();
        if lt.names.forward == link.link {
            if let Some(src) = self.snap.live(&from_id).cloned() {
                self.note(&src);
                if let Some(v) = field_value(&src, &lt.via).cloned() {
                    for tid in ids_from_value(&v) {
                        if let Some(t) = self.snap.live(&tid).cloned() {
                            candidates.push(t);
                        }
                    }
                }
            }
        } else if let Some(src) = self.snap.live(&from_id).cloned() {
            self.note(&src);
            for h in self.snap.scan_type(&lt.from) {
                if let Some(v) = field_value(h, &lt.via)
                    && ids_from_value(v).contains(&from_id)
                {
                    candidates.push(h.clone());
                }
            }
        }
        for cand in candidates {
            self.note(&cand);
            match &link.condition {
                None => return Ok(true),
                Some(g) => {
                    if self.eval_group(&g.all, &g.any, &g.none, Some(&cand))? {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }
}
type CriteriaOutcome = (Vec<Value>, BTreeMap<String, Value>, Vec<Criterion>);
type ObjectState = (BTreeMap<String, Knowledge>, Option<String>, bool, Scope);
fn eval_criteria(
    set: &SchemaSet,
    snap: &Snapshot,
    at: &ActionType,
    params: &BTreeMap<String, Value>,
    actor: &Value,
    now: u64,
) -> Result<CriteriaOutcome, Failure> {
    let mut ev = Eval {
        set,
        snap,
        params,
        actor,
        now,
        basis: BTreeMap::new(),
    };
    let mut results = Vec::new();
    let mut failed = Vec::new();
    for crit in &at.submission_criteria {
        let passed = ev.eval_group(&crit.all, &crit.any, &crit.none, None)?;
        results.push(json!({"code":crit.code,"message":crit.message,"passed":passed}));
        if !passed {
            failed.push(crit.clone());
        }
    }
    Ok((results, ev.basis, failed))
}

fn substitute(v: &Value, params: &BTreeMap<String, Value>) -> Value {
    match v {
        Value::String(s) => match s.strip_prefix('$') {
            Some(p) => params.get(p).cloned().unwrap_or(Value::Null),
            None => v.clone(),
        },
        Value::Array(a) => Value::Array(a.iter().map(|x| substitute(x, params)).collect()),
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, x)| (k.clone(), substitute(x, params)))
                .collect(),
        ),
        other => other.clone(),
    }
}
#[derive(Clone)]
struct Planned {
    id: Id,
    scope: Scope,
    change: Change,
    supersedes: Option<String>,
    fields: BTreeMap<String, Knowledge>,
    revision: String,
}
struct Planner<'a> {
    schema: &'a Schema,
    set: &'a SchemaSet,
    snap: Snapshot,
    params: &'a BTreeMap<String, Value>,
    action_id: String,
    edit_index: usize,
    planned: Vec<Planned>,
    overlay: BTreeMap<String, ObjectState>,
}
impl<'a> Planner<'a> {
    fn state(&self, id: &Id) -> Option<ObjectState> {
        if let Some(s) = self.overlay.get(&Snapshot::key(id)) {
            return Some(s.clone());
        }
        self.snap.head(id).map(|h| {
            (
                h.head.observation.fields.clone(),
                Some(h.head.observation.revision.clone()),
                h.retracted,
                h.scope.clone(),
            )
        })
    }
    fn ref_target(&self, object_type: &str) -> Option<Id> {
        self.params
            .values()
            .find_map(|v| id_from_value(v).filter(|id| id.object_type == object_type))
    }
    fn ref_target_typed(&self, at: &ActionType, object_type: &str) -> Option<Id> {
        for (name, def) in &at.parameters {
            if let PropertyType::Ref(r) = &def.ty
                && r.target == object_type
                && let Some(v) = self.params.get(name)
                && let Some(id) = id_from_value(v)
            {
                return Some(id);
            }
        }
        self.ref_target(object_type)
    }
    fn create_key(
        &self,
        at: &ActionType,
        object_type: &str,
        set_map: &BTreeMap<String, Value>,
    ) -> Result<String, Failure> {
        let ty = self
            .schema
            .object_types
            .get(object_type)
            .ok_or_else(|| Failure::from(Refusal::new("unknown_type")))?;
        if ty.primary_key.from.as_deref() == Some("$action_id") {
            return Ok(self.action_id.clone());
        }
        if let Some(from) = &ty.primary_key.from {
            if let Some(p) = from.strip_prefix('$')
                && let Some(v) = self.params.get(p)
                && let Some(s) = v.as_str()
            {
                return Ok(s.to_string());
            }
            if from.starts_with('/')
                && let Some(v) = self.params.values().find_map(|pv| {
                    pv.pointer(from)
                        .and_then(|x| x.as_str())
                        .map(str::to_string)
                })
            {
                return Ok(v);
            }
        }
        if let Some(id) = self.ref_target_typed(at, object_type) {
            return Ok(id.native_key);
        }
        if let Some(k) = set_map
            .get("native_key")
            .or_else(|| set_map.get("key"))
            .and_then(|v| v.as_str())
        {
            return Ok(k.to_string());
        }
        Ok(self.action_id.clone())
    }
    fn scope_for_create(&self, at: &ActionType, object_type: &str) -> Scope {
        let mut keys = BTreeMap::new();
        if !self.schema.scope_keys.is_empty() {
            for name in self.params.keys() {
                let _ = at;
                if let Some(v) = self.params.get(name)
                    && let Some(id) = id_from_value(v)
                    && let Some(h) = self.snap.live(&id)
                    && let Scope::Domain { keys: ks, .. } = &h.scope
                {
                    keys = ks.clone();
                    break;
                }
            }
        }
        let _ = object_type;
        Scope::Domain {
            namespace: self.schema.namespace.clone(),
            domain: self.schema.domain.clone(),
            keys,
        }
    }
    fn push(
        &mut self,
        id: Id,
        scope: Scope,
        change: Change,
        supersedes: Option<String>,
        fields: BTreeMap<String, Knowledge>,
    ) -> Value {
        let revision = format!("{}#{}", self.action_id, self.edit_index);
        self.edit_index += 1;
        let retracted = change == Change::Retraction;
        self.overlay.insert(
            Snapshot::key(&id),
            (
                fields.clone(),
                Some(revision.clone()),
                retracted,
                scope.clone(),
            ),
        );
        self.planned.push(Planned {
            id: id.clone(),
            scope,
            change: change.clone(),
            supersedes,
            fields,
            revision: revision.clone(),
        });
        json!({"id":id,"revision":revision,"change":serde_json::to_value(change).unwrap_or(Value::Null)})
    }
    fn plan(&mut self, at: &ActionType) -> Result<Vec<Value>, Failure> {
        let mut edits = Vec::new();
        for rule in &at.rules {
            // Defensive: only Ontology-owned types.
            let targets: Vec<String> = [
                rule.create_object.clone(),
                rule.modify_object.clone(),
                rule.create_or_modify_object.clone(),
                rule.delete_object.clone(),
                rule.set_reference.as_ref().map(|r| r.object_type.clone()),
                rule.clear_reference.as_ref().map(|r| r.object_type.clone()),
            ]
            .into_iter()
            .flatten()
            .collect();
            for t in &targets {
                match self.set.owner(t) {
                    Some(Owner::Ontology) => {}
                    Some(Owner::Source(_)) => {
                        return Err(Refusal::new("action_edits_source_owned")
                            .with("object_type", json!(t))
                            .into());
                    }
                    None => return Err(Refusal::new("unknown_type").into()),
                }
            }
            let set_map: BTreeMap<String, Value> = rule
                .set
                .iter()
                .map(|(k, v)| (k.clone(), substitute(v, self.params)))
                .collect();
            if let Some(t) = &rule.create_object {
                edits.push(self.do_create(at, t, &set_map, false)?);
            } else if let Some(t) = &rule.create_or_modify_object {
                // If a live target exists, modify; else create.
                if let Some(id) = self.ref_target_typed(at, t)
                    && self.state(&id).is_some_and(|(_, _, r, _)| !r)
                {
                    edits.push(self.do_modify(t, &set_map, Some(id))?);
                } else {
                    edits.push(self.do_create(at, t, &set_map, true)?);
                }
            } else if let Some(t) = &rule.modify_object {
                edits.push(self.do_modify(t, &set_map, None)?);
            } else if let Some(t) = &rule.delete_object {
                let id = self
                    .ref_target_typed(at, t)
                    .ok_or_else(|| invalid_parameter(t))?;
                let (fields, head, retracted, scope) =
                    self.state(&id).ok_or_else(|| revision_conflict(&id))?;
                let _ = fields;
                if retracted {
                    return Err(revision_conflict(&id));
                }
                edits.push(self.push(id, scope, Change::Retraction, head, BTreeMap::new()));
            } else if let Some(r) = &rule.set_reference {
                edits.push(self.do_set_ref(at, r, false)?);
            } else if let Some(r) = &rule.clear_reference {
                edits.push(self.do_set_ref(at, r, true)?);
            }
        }
        Ok(edits)
    }
    fn do_create(
        &mut self,
        at: &ActionType,
        t: &str,
        set_map: &BTreeMap<String, Value>,
        or_modify: bool,
    ) -> Result<Value, Failure> {
        let key = self.create_key(at, t, set_map)?;
        let id = Id {
            object_type: t.to_string(),
            namespace: self.schema.namespace.clone(),
            native_key: key,
        };
        if let Some((fields, head, retracted, scope)) = self.state(&id)
            && !retracted
            && !or_modify
        {
            let _ = (fields, head, scope);
            return Err(revision_conflict(&id));
        }
        if let Some((cur, head, retracted, scope)) = self.state(&id)
            && !retracted
            && or_modify
            && self.ref_target_typed(at, t).is_some()
        {
            let _ = (cur, head.clone(), scope.clone());
            return self.do_modify(t, set_map, Some(id));
        }
        let scope = self.scope_for_create(at, t);
        let fields: BTreeMap<String, Knowledge> = set_map
            .iter()
            .map(|(k, v)| (k.clone(), Knowledge::Value(v.clone())))
            .collect();
        Ok(self.push(id, scope, Change::Initial, None, fields))
    }
    fn do_modify(
        &mut self,
        t: &str,
        set_map: &BTreeMap<String, Value>,
        target: Option<Id>,
    ) -> Result<Value, Failure> {
        let id = target
            .or_else(|| {
                self.params
                    .values()
                    .find_map(|v| id_from_value(v).filter(|i| i.object_type == t))
            })
            .ok_or_else(|| invalid_parameter(t))?;
        let (cur, head, retracted, scope) =
            self.state(&id).ok_or_else(|| revision_conflict(&id))?;
        if retracted {
            return Err(revision_conflict(&id));
        }
        let mut fields = cur;
        for (k, v) in set_map {
            fields.insert(k.clone(), Knowledge::Value(v.clone()));
        }
        Ok(self.push(id, scope, Change::Update, head, fields))
    }
    fn do_set_ref(
        &mut self,
        at: &ActionType,
        r: &ont_core::schema::ReferenceRule,
        clear: bool,
    ) -> Result<Value, Failure> {
        let _ = at;
        let id = self
            .ref_target_typed_from_schema(&r.object_type)
            .ok_or_else(|| invalid_parameter(&r.object_type))?;
        let (cur, head, retracted, scope) =
            self.state(&id).ok_or_else(|| revision_conflict(&id))?;
        if retracted {
            return Err(revision_conflict(&id));
        }
        let mut fields = cur;
        if clear {
            fields.insert(r.property.clone(), Knowledge::Missing);
        } else {
            let raw = r
                .value
                .clone()
                .ok_or_else(|| invalid_parameter(&r.property))?;
            let v = substitute(&raw, self.params);
            // Array ref: add unique.
            let is_array = self
                .schema
                .object_types
                .get(&r.object_type)
                .and_then(|ty| ty.properties.get(&r.property))
                .is_some_and(|p| matches!(p.ty, PropertyType::Array(_)));
            if is_array {
                let mut arr: Vec<Value> = match fields.get(&r.property) {
                    Some(Knowledge::Value(Value::Array(a))) => a.clone(),
                    _ => Vec::new(),
                };
                if !arr.contains(&v) {
                    arr.push(v);
                }
                fields.insert(r.property.clone(), Knowledge::Value(Value::Array(arr)));
            } else {
                fields.insert(r.property.clone(), Knowledge::Value(v));
            }
        }
        Ok(self.push(id, scope, Change::Update, head, fields))
    }
    fn ref_target_typed_from_schema(&self, object_type: &str) -> Option<Id> {
        self.params
            .values()
            .find_map(|v| id_from_value(v).filter(|i| i.object_type == object_type))
    }
}
fn revision_conflict(id: &Id) -> Failure {
    Refusal::new("revision_conflict")
        .with("id", serde_json::to_value(id).unwrap_or(Value::Null))
        .into()
}

pub fn edits_path(store: &std::path::Path) -> PathBuf {
    match store.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.join("edits.jsonl"),
        _ => PathBuf::from("edits.jsonl"),
    }
}
pub fn log_line(seq: i64, e: &ActionLogEntry) -> String {
    canonical_string(&json!({
        "seq":seq,"action_id":e.action_id,"action_type":e.action_type,"domain":e.domain,
        "schema_domain":e.schema_domain,"schema_version":e.schema_version,"actor":e.actor,
        "submitted_at":e.submitted_at,"parameters":e.parameters,"basis":e.basis,"edits":e.edits,"outcome":e.outcome
    }))
}
pub fn sync_edits_file(
    store_path: &std::path::Path,
    log: &[(i64, ActionLogEntry)],
) -> Result<(), Failure> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = edits_path(store_path);
    let mut present: BTreeSet<i64> = BTreeSet::new();
    if let Ok(text) = std::fs::read_to_string(&path) {
        for line in text.lines() {
            if let Ok(v) = serde_json::from_str::<Value>(line)
                && let Some(s) = v.get("seq").and_then(|x| x.as_i64())
            {
                present.insert(s);
            }
        }
    }
    let missing: Vec<String> = log
        .iter()
        .filter(|(seq, _)| !present.contains(seq))
        .map(|(seq, e)| log_line(*seq, e))
        .collect();
    if missing.is_empty() && path.exists() {
        return Ok(());
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC).bits() as i32)
        .open(&path)
        .map_err(|_| Failure::Internal(internal!("edits_log_unavailable")))?;
    for line in &missing {
        writeln!(file, "{line}")
            .map_err(|_| Failure::Internal(internal!("edits_log_unavailable")))?;
    }
    file.sync_all()
        .map_err(|_| Failure::Internal(internal!("edits_log_unavailable")))?;
    Ok(())
}
fn pins_for(set: &SchemaSet, domain: &str) -> BTreeMap<String, ont_core::schema::SchemaPin> {
    set.pin(domain)
        .map(|p| (domain.to_string(), p.clone()))
        .into_iter()
        .collect()
}
fn check_expected(
    snap: &Snapshot,
    expected: &Option<Vec<ExpectedHead>>,
) -> Result<Vec<Value>, Failure> {
    let mut basis = Vec::new();
    if let Some(list) = expected {
        for e in list {
            match snap.head(&e.id) {
                Some(h) if h.head.observation.revision == e.revision && !h.retracted => {
                    basis.push(json!({"id":e.id,"revision":e.revision}));
                }
                _ => return Err(revision_conflict(&e.id)),
            }
        }
    }
    Ok(basis)
}
fn load_snapshot_read(store: &mut ont_store::Store, set: &SchemaSet) -> Result<Snapshot, Failure> {
    let mut snap = Snapshot::default();
    let types: BTreeSet<String> = set
        .pins()
        .keys()
        .filter_map(|d| set.schema(d))
        .flat_map(|s| s.object_types.keys().cloned())
        .collect();
    for t in types {
        // Include retracted check via head per scanned? scan hides retracted;
        // retracted objects are loaded on demand by callers through head.
        for h in st!(store.scan_heads(&t, 10_000, None)) {
            snap.insert(h);
        }
    }
    Ok(snap)
}

pub fn list_actions(ctx: &Context, req: &ListActionsRequest) -> Result<Answer, Failure> {
    let mut store = open_store(&ctx.store, false)?;
    let set = st!(store.schema_set()).clone();
    let principal = Principal::bind(&set)?;
    let view = principal.view().clone();
    if let Some(d) = &req.domain
        && set.schema(d).is_none()
    {
        return Err(Refusal::new("unknown_domain")
            .with("domain", json!(d))
            .into());
    }
    let domains: Vec<String> = match &req.domain {
        Some(d) => vec![d.clone()],
        None => set.pins().keys().cloned().collect(),
    };
    let mut actions = Vec::new();
    for domain in &domains {
        let Some(schema) = set.schema(domain) else {
            continue;
        };
        let granted = view.action_types(domain);
        for (name, at) in &schema.action_types {
            if !granted.contains(name) {
                continue;
            }
            let mut edit_types: Vec<String> = Vec::new();
            for r in &at.rules {
                for t in [
                    r.create_object.clone(),
                    r.modify_object.clone(),
                    r.create_or_modify_object.clone(),
                    r.delete_object.clone(),
                    r.set_reference.as_ref().map(|x| x.object_type.clone()),
                    r.clear_reference.as_ref().map(|x| x.object_type.clone()),
                ]
                .into_iter()
                .flatten()
                {
                    if !edit_types.contains(&t) {
                        edit_types.push(t);
                    }
                }
            }
            actions.push(json!({
                "domain":domain,"name":name,"description":at.description,
                "parameters":at.parameters.iter().map(|(pname,p)| {
                    let allowed = match &p.ty {
                        PropertyType::ValueType(vt) => schema.value_types.get(&vt.value_type)
                            .and_then(|d| d.enum_values.clone()).map(Value::Array).unwrap_or(Value::Null),
                        _ => Value::Null,
                    };
                    json!({"name":pname,"type":serde_json::to_value(&p.ty).unwrap_or(Value::Null),"required":p.required,"allowed_values":allowed,"max_bytes":p.max_bytes})
                }).collect::<Vec<_>>(),
                "criteria":at.submission_criteria.iter().map(|c| json!({"code":c.code,"message":c.message})).collect::<Vec<_>>(),
                "criterion_codes":at.submission_criteria.iter().map(|c| c.code.clone()).collect::<Vec<_>>(),
                "edits_object_types":edit_types
            }));
        }
    }
    let pins: BTreeMap<_, _> = domains
        .iter()
        .filter_map(|d| set.pin(d).map(|p| (d.clone(), p.clone())))
        .collect();
    Ok(Answer::Enveloped {
        pins,
        revision: principal.revision()?,
        result: json!({"actions":actions}),
    })
}

pub fn validate_action(ctx: &Context, req: &ValidateActionRequest) -> Result<Answer, Failure> {
    let mut store = open_store(&ctx.store, false)?;
    let set = st!(store.schema_set()).clone();
    let principal = Principal::bind(&set)?;
    let (schema, at) = find_action_type(&set, &req.action_type)
        .ok_or_else(|| Failure::from(Refusal::new("unknown_action")))?;
    let (schema, at) = (schema.clone(), at.clone());
    if !principal
        .view()
        .action_types(&schema.domain)
        .contains(&req.action_type)
    {
        return Err(Refusal::new("action_forbidden").into());
    }
    let params = validate_parameters(&schema, &at, &req.parameters)?;
    let params_canon = canonical(&Value::Object(
        params.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
    ));
    let actor = actor_json(ctx, &req.agent, &req.session);
    let mut snap = load_snapshot_read(&mut store, &set)?;
    // Retracted / expected heads on demand.
    if let Some(list) = &req.expected {
        for e in list {
            if let Some(h) = st!(store.head(&e.id)) {
                snap.insert(h);
            }
        }
    }
    for v in params.values() {
        if let Some(id) = id_from_value(v)
            && let Some(h) = st!(store.head(&id))
        {
            snap.insert(h);
        }
    }
    let expected_basis = check_expected(&snap, &req.expected)?;
    let (criteria, basis_map, failed) = eval_criteria(&set, &snap, &at, &params, &actor, ctx.now)?;
    let mut basis: Vec<Value> = basis_map.values().cloned().collect();
    for b in expected_basis {
        if !basis.contains(&b) {
            basis.push(b);
        }
    }
    let valid = failed.is_empty();
    let mut edits = Vec::new();
    if valid {
        let log_len = st!(store.action_log()).len();
        let basis_json = Value::Array(basis.clone());
        let action_id = allocate_action_id(
            ctx.now,
            &req.action_type,
            &params_canon,
            &basis_json,
            log_len,
            &None,
        );
        let mut planner = Planner {
            schema: &schema,
            set: &set,
            snap: snap.clone(),
            params: &params,
            action_id,
            edit_index: 0,
            planned: Vec::new(),
            overlay: BTreeMap::new(),
        };
        edits = planner.plan(&at)?;
    }
    Ok(Answer::Enveloped {
        pins: pins_for(&set, &schema.domain),
        revision: principal.revision()?,
        result: json!({"valid":valid,"criteria":criteria,"basis":basis,"edits":edits,"action_type":req.action_type}),
    })
}

enum ApplyOutcome {
    Applied { action_id: String, edits: Value },
    Already { action_id: String, edits: Value },
}

pub fn apply_action(ctx: &Context, req: &ApplyActionRequest) -> Result<Answer, Failure> {
    if let Some(r) = &req.request_id
        && !valid_request_id(r)
    {
        return Err(Refusal::new("invalid_request").into());
    }
    let mut store = open_store(&ctx.store, true)?;
    // Repair first: the database wins over edits.jsonl.
    let pre_log = st!(store.action_log());
    sync_edits_file(&ctx.store, &pre_log)?;
    let set = st!(store.schema_set()).clone();
    let principal = Principal::bind(&set)?;
    let revision = principal.revision()?;
    let (schema, at) = find_action_type(&set, &req.action_type)
        .ok_or_else(|| Failure::from(Refusal::new("unknown_action")))?;
    let (schema, at) = (schema.clone(), at.clone());
    if !principal
        .view()
        .action_types(&schema.domain)
        .contains(&req.action_type)
    {
        return Err(Refusal::new("action_forbidden").into());
    }
    let params = validate_parameters(&schema, &at, &req.parameters)?;
    let params_canon = canonical(&Value::Object(
        params.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
    ));
    let actor = actor_json(ctx, &req.agent, &req.session);
    let mut captured: Option<Failure> = None;
    let mut outcome: Option<ApplyOutcome> = None;
    let write_result = store.write(ctx.now, |tx| {
        let tx_set = tx.schema_set().clone();
        let run = |tx: &mut ont_store::schema_mode::SchemaWrite<'_>| -> Result<ApplyOutcome, Failure> {
            let (tschema, tat) = find_action_type(&tx_set, &req.action_type)
                .ok_or_else(|| Failure::from(Refusal::new("unknown_action")))?;
            let (tschema, tat) = (tschema.clone(), tat.clone());
            let tprincipal = Principal::bind(&tx_set)?;
            if !tprincipal.view().action_types(&tschema.domain).contains(&req.action_type) {
                return Err(Refusal::new("action_forbidden").into());
            }
            let tparams = validate_parameters(&tschema, &tat, &req.parameters)?;
            // Idempotent retry inside the transaction.
            if let Some(rid) = &req.request_id {
                let derived = derive_request_action_id(rid);
                if let Some((_, entry)) = tx.find_action(&derived).map_err(store_err)? {
                    if entry.action_type == req.action_type && canonical(&entry.parameters) == params_canon {
                        return Ok(ApplyOutcome::Already { action_id: derived, edits: entry.edits.clone() });
                    }
                    return Err(Refusal::new("request_id_reused").into());
                }
            }
            let mut snap = Snapshot::default();
            let types: BTreeSet<String> = tx_set.pins().keys().filter_map(|d| tx_set.schema(d))
                .flat_map(|s| s.object_types.keys().cloned()).collect();
            for t in types {
                for h in tx.scan_heads(&t, 10_000).map_err(store_err)? { snap.insert(h); }
            }
            if let Some(list) = &req.expected {
                for e in list {
                    if let Some(h) = tx.head(&e.id).map_err(store_err)? { snap.insert(h); }
                }
            }
            for v in tparams.values() {
                if let Some(id) = id_from_value(v)
                    && let Some(h) = tx.head(&id).map_err(store_err)? { snap.insert(h); }
            }
            let expected_basis = check_expected(&snap, &req.expected)?;
            let (criteria, basis_map, failed) = eval_criteria(&tx_set, &snap, &tat, &tparams, &actor, ctx.now)?;
            let _ = criteria;
            if !failed.is_empty() {
                let codes: Vec<String> = failed.iter().map(|c| c.code.clone()).collect();
                let failures: Vec<Value> = failed.iter().map(|c| json!({"code":c.code,"message":c.message})).collect();
                let message = failed.first().map(|c| c.message.clone()).unwrap_or_default();
                return Err(Refusal::new("submission_criteria")
                    .with("failed", json!(codes)).with("failures", json!(failures)).with("message", json!(message)).into());
            }
            let mut basis: Vec<Value> = basis_map.values().cloned().collect();
            for b in expected_basis { if !basis.contains(&b) { basis.push(b); } }
            let basis_json = Value::Array(basis.clone());
            let log_len = tx.action_log().map_err(store_err)?.len();
            let action_id = allocate_action_id(ctx.now, &req.action_type, &params_canon, &basis_json, log_len, &req.request_id);
            let mut planner = Planner { schema: &tschema, set: &tx_set, snap: snap.clone(), params: &tparams, action_id: action_id.clone(), edit_index: 0, planned: Vec::new(), overlay: BTreeMap::new() };
            let edits = planner.plan(&tat)?;
            let edits_json = Value::Array(edits.clone());
            let pin_version = tx_set.pin(&tschema.domain).map(|p| p.version).unwrap_or(0);
            let record = json!({"action_id":action_id,"action_type":req.action_type,"domain":tschema.domain,"schema_domain":tschema.domain,"schema_version":pin_version,"actor":actor,"submitted_at":ctx.now,"parameters":params_canon,"basis":basis_json,"edits":edits_json,"outcome":"applied"});
            let artifact = sha256_hex(canonical_string(&record).as_bytes());
            let planned = planner.planned.clone();
            for (i, p) in planned.iter().enumerate() {
                let obs = Observation { revision: p.revision.clone(), source_revision: action_id.clone(), observed_at: ctx.now, fresh_until: FRESH_UNTIL_MAX, change: p.change.clone(), supersedes: p.supersedes.clone(), coverage: Coverage::Complete, fields: p.fields.clone(), lineage: Lineage { artifact_sha256: artifact.clone(), pointer: format!("/edits/{i}") } };
                tx.append(&Append { id: p.id.clone(), scope: p.scope.clone(), observation: obs }).map_err(store_err)?;
            }
            let entry = ActionLogEntry { action_id: action_id.clone(), action_type: req.action_type.clone(), domain: tschema.domain.clone(), schema_domain: tschema.domain.clone(), schema_version: pin_version, actor: actor.clone(), submitted_at: ctx.now, parameters: params_canon.clone(), basis: basis_json, edits: edits_json, outcome: "applied".to_string() };
            tx.log_action(&entry).map_err(store_err)?;
            Ok(ApplyOutcome::Applied { action_id, edits: entry.edits.clone() })
        };
        match run(tx) {
            Ok(o) => { outcome = Some(o); Ok(()) }
            Err(f) => { captured = Some(f); Err(ont_store::schema_mode::StoreRefusal::new("action_refused")) }
        }
    });
    if let Some(f) = captured {
        return Err(f);
    }
    if let Err(e) = write_result {
        return Err(store_err(e));
    }
    match outcome {
        Some(ApplyOutcome::Already { action_id, edits }) => Ok(Answer::Enveloped {
            pins: pins_for(&set, &schema.domain),
            revision,
            result: json!({"outcome":"already_applied","action_id":action_id,"edits":edits,"action_type":req.action_type}),
        }),
        Some(ApplyOutcome::Applied { action_id, edits }) => {
            if take_fail_after_commit() {
                return Err(Failure::Internal(internal!("crash_after_commit")));
            }
            let log = st!(store.action_log());
            sync_edits_file(&ctx.store, &log)?;
            Ok(Answer::Enveloped {
                pins: pins_for(&set, &schema.domain),
                revision,
                result: json!({"outcome":"applied","action_id":action_id,"edits":edits,"action_type":req.action_type}),
            })
        }
        None => Err(Failure::Internal(internal!("authority_failure"))),
    }
}
