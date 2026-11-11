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
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

pub const FRESH_UNTIL_MAX: u64 = 253402300799;

thread_local! {
    #[doc(hidden)]
    pub static FAIL_AFTER_COMMIT_LOCAL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
/// Test seam: simulate a crash between the commit and the edits-log
/// append. This is the one documented test seam for that step.
#[doc(hidden)]
pub fn __set_fail_after_commit(v: bool) {
    FAIL_AFTER_COMMIT_LOCAL.with(|c| c.set(v));
}
fn take_fail_after_commit() -> bool {
    FAIL_AFTER_COMMIT_LOCAL.with(|c| c.replace(false))
}

thread_local! {
    #[doc(hidden)]
    pub static PAUSE_AFTER_COMMIT_LOCAL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
#[doc(hidden)]
pub static PAUSE_AFTER_COMMIT_ENTERED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
#[doc(hidden)]
pub static PAUSE_AFTER_COMMIT_RELEASE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
/// Test seam: pause between the commit and the edits-log sync of an
/// apply on this thread, until [`__release_pause_after_commit`] is called.
#[doc(hidden)]
pub fn __set_pause_after_commit(v: bool) {
    PAUSE_AFTER_COMMIT_LOCAL.with(|c| c.set(v));
}
#[doc(hidden)]
pub fn __pause_after_commit_entered() -> bool {
    PAUSE_AFTER_COMMIT_ENTERED.load(std::sync::atomic::Ordering::SeqCst)
}
#[doc(hidden)]
pub fn __release_pause_after_commit() {
    PAUSE_AFTER_COMMIT_RELEASE.store(true, std::sync::atomic::Ordering::SeqCst);
}
#[doc(hidden)]
pub fn __reset_pause_after_commit() {
    PAUSE_AFTER_COMMIT_ENTERED.store(false, std::sync::atomic::Ordering::SeqCst);
    PAUSE_AFTER_COMMIT_RELEASE.store(false, std::sync::atomic::Ordering::SeqCst);
}
fn maybe_pause_after_commit() {
    let armed = PAUSE_AFTER_COMMIT_LOCAL.with(|c| c.replace(false));
    if !armed {
        return;
    }
    PAUSE_AFTER_COMMIT_ENTERED.store(true, std::sync::atomic::Ordering::SeqCst);
    for _ in 0..10_000 {
        if PAUSE_AFTER_COMMIT_RELEASE.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
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
    #[serde(default)]
    pub request_id: Option<String>,
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
fn valid_actor_field(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
}
fn effective_client(ctx: &Context) -> String {
    if ctx.client.is_empty() {
        "cli".to_string()
    } else {
        ctx.client.clone()
    }
}
fn check_actor_bounds(
    ctx: &Context,
    agent: &Option<String>,
    session: &Option<String>,
) -> Result<(), Failure> {
    let client = effective_client(ctx);
    let agent = agent.clone().or_else(|| ctx.agent.clone());
    let session = session.clone().or_else(|| ctx.session.clone());
    if !valid_actor_field(&client)
        || agent.as_deref().is_some_and(|a| !valid_actor_field(a))
        || session.as_deref().is_some_and(|s| !valid_actor_field(s))
    {
        return Err(Refusal::new("invalid_request").into());
    }
    Ok(())
}
fn check_parameters_size(params: &Value) -> Result<(), Failure> {
    if serde_json::to_string(&canonical(params)).is_ok_and(|s| s.len() > crate::MAX_REQUEST) {
        return Err(Refusal::new("invalid_request").into());
    }
    Ok(())
}
fn actor_json(ctx: &Context, agent: &Option<String>, session: &Option<String>) -> Value {
    let agent = agent.clone().or_else(|| ctx.agent.clone());
    let session = session.clone().or_else(|| ctx.session.clone());
    let client = effective_client(ctx);
    let mut m = Map::new();
    m.insert("principal".into(), json!("agent"));
    m.insert("client".into(), json!(client));
    m.insert("actor_trust".into(), json!("self_reported"));
    if let Some(a) = agent {
        m.insert("agent".into(), json!(a));
    }
    if let Some(s) = session {
        m.insert("session".into(), json!(s));
    }
    Value::Object(m)
}
fn effective_agent(ctx: &Context, agent: &Option<String>) -> Option<String> {
    agent.clone().or_else(|| ctx.agent.clone())
}
fn valid_request_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}
fn derive_request_action_id(client: &str, agent: Option<&str>, request_id: &str) -> String {
    let agent = agent.unwrap_or("");
    let mut buf = Vec::new();
    for field in [client, agent, request_id] {
        buf.extend_from_slice(format!("{}:", field.len()).as_bytes());
        buf.extend_from_slice(field.as_bytes());
    }
    let h = sha256_hex(&buf);
    format!("act-rq-{}", &h[..32])
}
fn allocate_action_id(
    now: u64,
    action_type: &str,
    params: &Value,
    basis: &Value,
    log_len: usize,
    request_action_id: &Option<String>,
) -> String {
    if let Some(r) = request_action_id {
        return r.clone();
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
            // The core check includes the value type's pattern.
            schema
                .validate_value(&PropertyType::ValueType(vt.clone()), v)
                .is_ok()
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
    at: &'a ActionType,
}
impl Eval<'_> {
    fn note(&mut self, head: &SchemaHead) {
        self.basis.insert(
            Snapshot::key(&head.id),
            json!({"id":head.id,"revision":head.head.observation.revision}),
        );
    }
    fn not_evaluable() -> Failure {
        Refusal::new("criterion_not_evaluable").into()
    }
    fn resolve_param_ref(&mut self, p: &str) -> Option<Value> {
        let (name, prop) = p.split_once('.')?;
        let v = self.params.get(name)?;
        let id = id_from_value(v)?;
        let head = self.snap.live(&id)?.clone();
        self.note(&head);
        field_value(&head, prop).cloned().filter(|v| !v.is_null())
    }
    fn resolve_property(&mut self, name: &str, ctx: Option<&SchemaHead>) -> Option<Value> {
        if let Some(head) = ctx {
            let head = head.clone();
            self.note(&head);
            return field_value(&head, name).cloned().filter(|v| !v.is_null());
        }
        // Bare name: only with exactly one reference parameter.
        let mut refs = self
            .at
            .parameters
            .iter()
            .filter(|(_, d)| matches!(d.ty, PropertyType::Ref(_)));
        let (pname, _) = refs.next()?;
        if refs.next().is_some() {
            return None;
        }
        let v = self.params.get(pname)?;
        let id = id_from_value(v)?;
        let head = self.snap.live(&id)?.clone();
        self.note(&head);
        field_value(&head, name).cloned().filter(|v| !v.is_null())
    }
    fn resolve(
        &mut self,
        v: &Value,
        ctx: Option<&SchemaHead>,
        property_position: bool,
    ) -> Option<Value> {
        if let Value::String(s) = v {
            if let Some(p) = s.strip_prefix('$') {
                if p.contains('.') {
                    return self.resolve_param_ref(p);
                }
                return self.params.get(p).cloned();
            }
            if property_position {
                return self.resolve_property(s, ctx);
            }
        }
        Some(v.clone())
    }
    fn values_equal(a: &Value, b: &Value) -> Result<bool, Failure> {
        if a == b {
            return Ok(true);
        }
        if let Some(ord) = cmp_values(a, b) {
            return Ok(ord == std::cmp::Ordering::Equal);
        }
        // Same JSON kind: equality is decidable; different kinds are not.
        let same_kind = matches!(
            (a, b),
            (Value::String(_), Value::String(_))
                | (Value::Bool(_), Value::Bool(_))
                | (Value::Array(_), Value::Array(_))
                | (Value::Object(_), Value::Object(_))
                | (Value::Null, Value::Null)
        );
        if same_kind {
            Ok(false)
        } else {
            Err(Self::not_evaluable())
        }
    }
    fn eval_group(
        &mut self,
        all: &Option<Vec<Condition>>,
        any: &Option<Vec<Condition>>,
        none: &Option<Vec<Condition>>,
        ctx: Option<&SchemaHead>,
    ) -> Result<bool, Failure> {
        if let Some(cs) = all {
            let mut out = true;
            for c in cs {
                out &= self.eval(c, ctx)?;
            }
            return Ok(out);
        }
        if let Some(cs) = any {
            let mut out = false;
            for c in cs {
                out |= self.eval(c, ctx)?;
            }
            return Ok(out);
        }
        if let Some(cs) = none {
            let mut out = true;
            for c in cs {
                out &= !self.eval(c, ctx)?;
            }
            return Ok(out);
        }
        Ok(true)
    }
    fn eval(&mut self, c: &Condition, ctx: Option<&SchemaHead>) -> Result<bool, Failure> {
        match c {
            Condition::Exists(s) => Ok(self.exists_fresh(s, false)),
            Condition::Fresh(s) => Ok(self.exists_fresh(s, true)),
            Condition::Eq(p) => self.compare_eq(p, ctx),
            Condition::Ne(p) => Ok(!self.compare_eq(p, ctx)?),
            Condition::In(p) => {
                let a = self
                    .resolve(&p[0], ctx, true)
                    .ok_or_else(Self::not_evaluable)?;
                let b = self
                    .resolve(&p[1], ctx, false)
                    .ok_or_else(Self::not_evaluable)?;
                Ok(match b {
                    Value::Array(items) => items
                        .iter()
                        .any(|i| a == *i || cmp_values(&a, i) == Some(std::cmp::Ordering::Equal)),
                    _ => false,
                })
            }
            Condition::Lt(p) => self.compare_order(p, ctx, |o| o == std::cmp::Ordering::Less),
            Condition::Le(p) => self.compare_order(p, ctx, |o| o != std::cmp::Ordering::Greater),
            Condition::Gt(p) => self.compare_order(p, ctx, |o| o == std::cmp::Ordering::Greater),
            Condition::Ge(p) => self.compare_order(p, ctx, |o| o != std::cmp::Ordering::Less),
            Condition::Actor(v) => {
                let want = self.resolve(v, ctx, false);
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
                let mut out = true;
                for x in cs {
                    out &= self.eval(x, ctx)?;
                }
                Ok(out)
            }
            Condition::Any(cs) => {
                let mut out = false;
                for x in cs {
                    out |= self.eval(x, ctx)?;
                }
                Ok(out)
            }
            Condition::NoneOf(cs) => {
                let mut out = true;
                for x in cs {
                    out &= !self.eval(x, ctx)?;
                }
                Ok(out)
            }
        }
    }
    fn compare_eq(&mut self, p: &[Value; 2], ctx: Option<&SchemaHead>) -> Result<bool, Failure> {
        let a = self
            .resolve(&p[0], ctx, true)
            .ok_or_else(Self::not_evaluable)?;
        let b = self
            .resolve(&p[1], ctx, false)
            .ok_or_else(Self::not_evaluable)?;
        Self::values_equal(&a, &b)
    }
    fn compare_order(
        &mut self,
        p: &[Value; 2],
        ctx: Option<&SchemaHead>,
        ok: impl Fn(std::cmp::Ordering) -> bool,
    ) -> Result<bool, Failure> {
        let a = self
            .resolve(&p[0], ctx, true)
            .ok_or_else(Self::not_evaluable)?;
        let b = self
            .resolve(&p[1], ctx, false)
            .ok_or_else(Self::not_evaluable)?;
        cmp_values(&a, &b).map(&ok).ok_or_else(Self::not_evaluable)
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
        at,
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
        let _ = (at, set_map);
        if ty.primary_key.from.as_deref() == Some("$action_id") {
            return Ok(self.action_id.clone());
        }
        if let Some(from) = &ty.primary_key.from
            && from.starts_with('/')
        {
            // The one pointer form `schema check` accepts for an
            // Ontology-owned key: a pointer into a parameter value.
            if let Some(v) = self.params.values().find_map(|pv| {
                pv.pointer(from)
                    .and_then(|x| x.as_str())
                    .map(str::to_string)
            }) {
                return Ok(v);
            }
            return Err(invalid_parameter("key"));
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
        {
            let _ = (fields, head, scope);
            if or_modify {
                // The key (from a parameter or a pointer) names an
                // existing object: plan a modification, as in validate.
                return self.do_modify(t, set_map, Some(id));
            }
            return Err(revision_conflict(&id));
        }
        let _ = at;
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
/// The one field check shared by validate and apply: every planned
/// object (except a retraction, whose fields are empty by definition)
/// must satisfy the store's field validation before anything is written.
fn check_planned(set: &SchemaSet, planner: &Planner<'_>) -> Result<(), Failure> {
    for p in &planner.planned {
        if p.change == Change::Retraction {
            continue;
        }
        if let Err(v) = set.validate_fields(&p.id.object_type, &p.fields) {
            let prop = v
                .pointer
                .strip_prefix("/fields/")
                .unwrap_or(&v.pointer)
                .replace("~1", "/")
                .replace("~0", "~");
            return Err(Refusal::new("invalid_parameter")
                .with("parameter", json!(prop.clone()))
                .with("property", json!(prop))
                .into());
        }
    }
    Ok(())
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
        "submitted_at":e.submitted_at,"parameters":e.parameters,"basis":e.basis,"edits":e.edits,"outcome":e.outcome,"actor_trust":"self_reported"
    }))
}
/// Whether the edits log is in step with the action log table after a
/// write: `written`, `pending` (the committed action stands; the next
/// write repairs the file), or `rebuilt` (a diverged file was kept aside
/// and rewritten from the table, which always wins).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditsLogStatus {
    Written,
    Pending,
    Rebuilt,
}
impl EditsLogStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Written => "written",
            Self::Pending => "pending",
            Self::Rebuilt => "rebuilt",
        }
    }
}
fn utc_stamp_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let tod = secs % 86_400;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    format!(
        "{year:04}{m:02}{d:02}T{:02}{:02}{:02}Z",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    )
}
fn fsync_dir(dir: &std::path::Path) {
    if let Ok(f) = std::fs::File::open(dir) {
        let _ = f.sync_all();
    }
}
fn rename_aside(path: &std::path::Path, dir: &std::path::Path) -> bool {
    let stamp = utc_stamp_now();
    let Ok(dir_file) = std::fs::File::open(dir) else {
        return false;
    };
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "edits.jsonl".to_string());
    for n in 0.. {
        let aside = format!("edits.jsonl.diverged-{stamp}-{n}");
        match rustix::fs::renameat_with(
            &dir_file,
            &file_name,
            &dir_file,
            &aside,
            rustix::fs::RenameFlags::NOREPLACE,
        ) {
            Ok(()) => return true,
            Err(e) if e == rustix::io::Errno::EXIST => continue,
            Err(_) => return false,
        }
    }
    false
}
pub fn sync_edits_file(store_path: &std::path::Path) -> EditsLogStatus {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    let path = edits_path(store_path);
    let dir = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let lock_path = dir.join("edits.jsonl.lock");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .mode(0o600)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC).bits() as i32)
        .open(&lock_path);
    let Ok(lock) = lock else {
        return EditsLogStatus::Pending;
    };
    if rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockExclusive).is_err() {
        return EditsLogStatus::Pending;
    }
    // Read the committed table only after the lock is held, so the
    // comparison never uses a snapshot taken before another writer's
    // commit and sync.
    let log = match ont_store::Store::open_schema_existing(store_path) {
        Ok(mut s) => match s.action_log() {
            Ok(l) => l,
            Err(_) => return EditsLogStatus::Pending,
        },
        Err(_) => return EditsLogStatus::Pending,
    };
    let expected: Vec<String> = log.iter().map(|(seq, e)| log_line(*seq, e)).collect();
    let pending = EditsLogStatus::Pending;
    if path.symlink_metadata().is_err() {
        let created = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .custom_flags(
                (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC).bits() as i32,
            )
            .open(&path);
        let Ok(mut f) = created else { return pending };
        for line in &expected {
            if f.write_all(format!("{line}\n").as_bytes()).is_err() {
                return pending;
            }
        }
        if f.sync_all().is_err() {
            return pending;
        }
        fsync_dir(&dir);
        return EditsLogStatus::Written;
    }
    let opened = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC).bits() as i32)
        .open(&path);
    let Ok(mut file) = opened else { return pending };
    let Ok(meta) = file.metadata() else {
        return pending;
    };
    let caller_uid = std::fs::metadata("/proc/self")
        .map(|m| m.uid())
        .unwrap_or(u32::MAX);
    if meta.permissions().mode() & 0o777 != 0o600 || meta.uid() != caller_uid {
        eprintln!(
            "warning: edits_log_pending@crates/ont/src/action.rs:{} edits.jsonl is not 0600 owned by the caller",
            line!()
        );
        return pending;
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return pending;
    }
    let complete_len = if bytes.ends_with(b"\n") || bytes.is_empty() {
        bytes.len()
    } else {
        bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1)
    };
    let make_fresh = |expected: &Vec<String>| -> EditsLogStatus {
        let created = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .custom_flags(
                (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC).bits() as i32,
            )
            .open(&path);
        let Ok(mut f) = created else {
            return EditsLogStatus::Pending;
        };
        for line in expected {
            if f.write_all(format!("{line}\n").as_bytes()).is_err() {
                return EditsLogStatus::Pending;
            }
        }
        if f.sync_all().is_err() {
            return EditsLogStatus::Pending;
        }
        fsync_dir(&dir);
        EditsLogStatus::Rebuilt
    };
    let Ok(complete_text) = std::str::from_utf8(&bytes[..complete_len]) else {
        if !rename_aside(&path, &dir) {
            return pending;
        }
        return make_fresh(&expected);
    };
    let lines: Vec<&str> = if complete_text.is_empty() {
        Vec::new()
    } else {
        complete_text[..complete_text.len() - 1]
            .split('\n')
            .collect()
    };
    if lines.len() > expected.len() || lines.iter().enumerate().any(|(i, l)| *l != expected[i]) {
        if !rename_aside(&path, &dir) {
            return pending;
        }
        return make_fresh(&expected);
    }
    if complete_len < bytes.len() && file.set_len(complete_len as u64).is_err() {
        return pending;
    }
    if file.seek(SeekFrom::End(0)).is_err() {
        return pending;
    }
    for line in &expected[lines.len()..] {
        if file.write_all(format!("{line}\n").as_bytes()).is_err() {
            return pending;
        }
    }
    if file.sync_all().is_err() {
        return pending;
    }
    EditsLogStatus::Written
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
fn insert_snap(snap: &mut Snapshot, h: SchemaHead, max: usize) -> Result<(), Failure> {
    let is_new = snap.head(&h.id).is_none();
    snap.insert(h);
    if is_new && snap.heads.len() > max {
        return Err(Refusal::new("snapshot_too_large").into());
    }
    Ok(())
}
fn collect_linked_scans(set: &SchemaSet, c: &Condition, out: &mut BTreeSet<String>) {
    match c {
        Condition::Linked(link) => {
            for schema in set.pins().keys().filter_map(|d| set.schema(d)) {
                for lt in schema.link_types.values() {
                    if lt.names.reverse == link.link {
                        out.insert(lt.from.clone());
                    }
                }
            }
            if let Some(g) = &link.condition {
                for cs in [&g.all, &g.any, &g.none].into_iter().flatten() {
                    for x in cs {
                        collect_linked_scans(set, x, out);
                    }
                }
            }
        }
        Condition::All(cs) | Condition::Any(cs) | Condition::NoneOf(cs) => {
            for x in cs {
                collect_linked_scans(set, x, out);
            }
        }
        _ => {}
    }
}
trait SnapshotSource {
    fn fetch_head(&mut self, id: &Id) -> Result<Option<SchemaHead>, Failure>;
    fn fetch_scan(
        &mut self,
        ty: &str,
        limit: usize,
        after: Option<&Id>,
    ) -> Result<Vec<SchemaHead>, Failure>;
}
impl SnapshotSource for ont_store::Store {
    fn fetch_head(&mut self, id: &Id) -> Result<Option<SchemaHead>, Failure> {
        self.head(id).map_err(store_err)
    }
    fn fetch_scan(
        &mut self,
        ty: &str,
        limit: usize,
        after: Option<&Id>,
    ) -> Result<Vec<SchemaHead>, Failure> {
        self.scan_heads(ty, limit, after).map_err(store_err)
    }
}
impl SnapshotSource for ont_store::schema_mode::SchemaWrite<'_> {
    fn fetch_head(&mut self, id: &Id) -> Result<Option<SchemaHead>, Failure> {
        self.head(id).map_err(store_err)
    }
    fn fetch_scan(
        &mut self,
        ty: &str,
        limit: usize,
        after: Option<&Id>,
    ) -> Result<Vec<SchemaHead>, Failure> {
        self.scan_heads_after(ty, limit, after).map_err(store_err)
    }
}
fn load_snapshot(
    ctx: &Context,
    set: &SchemaSet,
    at: &ActionType,
    params: &BTreeMap<String, Value>,
    expected: &Option<Vec<ExpectedHead>>,
    source: &mut dyn SnapshotSource,
) -> Result<Snapshot, Failure> {
    let max = ctx.snapshot_max_heads;
    let page = ctx.snapshot_page_size.max(1);
    let mut snap = Snapshot::default();
    let mut ids: Vec<Id> = Vec::new();
    for v in params.values() {
        ids.extend(ids_from_value(v));
    }
    if let Some(list) = expected {
        for e in list {
            ids.push(e.id.clone());
        }
    }
    for id in &ids {
        if let Some(h) = source.fetch_head(id)? {
            insert_snap(&mut snap, h, max)?;
        }
    }
    // One hop: objects referenced by the heads just loaded (linked
    // forward targets and reference fields).
    let mut hop: Vec<Id> = Vec::new();
    for h in snap.heads.values() {
        for v in h.head.observation.fields.values() {
            if let Knowledge::Value(v) = v {
                hop.extend(ids_from_value(v));
            }
        }
    }
    for id in &hop {
        if snap.head(id).is_none()
            && let Some(h) = source.fetch_head(id)?
        {
            insert_snap(&mut snap, h, max)?;
        }
    }
    // Reverse `linked` conditions scan by type, paged until exhausted.
    let mut scan_types = BTreeSet::new();
    for crit in &at.submission_criteria {
        for cs in [&crit.all, &crit.any, &crit.none].into_iter().flatten() {
            for c in cs {
                collect_linked_scans(set, c, &mut scan_types);
            }
        }
    }
    for ty in &scan_types {
        let mut after: Option<Id> = None;
        loop {
            let heads = source.fetch_scan(ty, page, after.as_ref())?;
            let full = heads.len() == page;
            after = heads.last().map(|h| h.id.clone());
            for h in heads {
                insert_snap(&mut snap, h, max)?;
            }
            if !full {
                break;
            }
        }
    }
    Ok(snap)
}
fn augment_planner(
    planner: &mut Planner<'_>,
    at: &ActionType,
    max: usize,
    source: &mut dyn SnapshotSource,
) -> Result<(), Failure> {
    let mut targets: Vec<(String, String)> = Vec::new();
    for rule in &at.rules {
        for t in [
            rule.create_object.clone(),
            rule.create_or_modify_object.clone(),
        ]
        .into_iter()
        .flatten()
        {
            if let Ok(key) = planner.create_key(at, &t, &BTreeMap::new()) {
                targets.push((t, key));
            }
        }
    }
    for (t, key) in targets {
        let id = Id {
            object_type: t,
            namespace: planner.schema.namespace.clone(),
            native_key: key,
        };
        if planner.snap.head(&id).is_none()
            && let Some(h) = source.fetch_head(&id)?
        {
            insert_snap(&mut planner.snap, h, max)?;
        }
    }
    Ok(())
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
    check_actor_bounds(ctx, &req.agent, &req.session)?;
    check_parameters_size(&req.parameters)?;
    if let Some(r) = &req.request_id
        && !valid_request_id(r)
    {
        return Err(Refusal::new("invalid_request").into());
    }
    let params = validate_parameters(&schema, &at, &req.parameters)?;
    let params_canon = canonical(&Value::Object(
        params.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
    ));
    let actor = actor_json(ctx, &req.agent, &req.session);
    let snap = load_snapshot(ctx, &set, &at, &params, &req.expected, &mut store)?;
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
        let request_action_id = req.request_id.as_ref().map(|r| {
            derive_request_action_id(
                &effective_client(ctx),
                effective_agent(ctx, &req.agent).as_deref(),
                r,
            )
        });
        let action_id = allocate_action_id(
            ctx.now,
            &req.action_type,
            &params_canon,
            &basis_json,
            log_len,
            &request_action_id,
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
        augment_planner(&mut planner, &at, ctx.snapshot_max_heads, &mut store)?;
        edits = planner.plan(&at)?;
        check_planned(&set, &planner)?;
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
    check_actor_bounds(ctx, &req.agent, &req.session)?;
    check_parameters_size(&req.parameters)?;
    let mut store = open_store(&ctx.store, true)?;
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
                let derived = derive_request_action_id(
                    &effective_client(ctx),
                    effective_agent(ctx, &req.agent).as_deref(),
                    rid,
                );
                if let Some((_, entry)) = tx.find_action(&derived).map_err(store_err)? {
                    if entry.action_type == req.action_type && canonical(&entry.parameters) == params_canon {
                        return Ok(ApplyOutcome::Already { action_id: derived, edits: entry.edits.clone() });
                    }
                    return Err(Refusal::new("request_id_reused").into());
                }
            }
            let snap = load_snapshot(ctx, &tx_set, &tat, &tparams, &req.expected, tx)?;
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
            let request_action_id = req.request_id.as_ref().map(|r| {
                derive_request_action_id(
                    &effective_client(ctx),
                    effective_agent(ctx, &req.agent).as_deref(),
                    r,
                )
            });
            let action_id = allocate_action_id(ctx.now, &req.action_type, &params_canon, &basis_json, log_len, &request_action_id);
            let mut planner = Planner { schema: &tschema, set: &tx_set, snap: snap.clone(), params: &tparams, action_id: action_id.clone(), edit_index: 0, planned: Vec::new(), overlay: BTreeMap::new() };
            augment_planner(&mut planner, &tat, ctx.snapshot_max_heads, tx)?;
            let edits = planner.plan(&tat)?;
            check_planned(&tx_set, &planner)?;
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
        Some(ApplyOutcome::Already { action_id, edits }) => {
            let edits_log = sync_edits_file(&ctx.store);
            Ok(Answer::Enveloped {
                pins: pins_for(&set, &schema.domain),
                revision,
                result: json!({"outcome":"already_applied","action_id":action_id,"edits":edits,"action_type":req.action_type,"edits_log":edits_log.as_str(),"actor_trust":"self_reported"}),
            })
        }
        Some(ApplyOutcome::Applied { action_id, edits }) => {
            if take_fail_after_commit() {
                return Err(Failure::Internal(internal!("crash_after_commit")));
            }
            maybe_pause_after_commit();
            // After the commit the action stands whatever happens to
            // the file: a failed append is reported, never an error.
            let edits_log = sync_edits_file(&ctx.store);
            Ok(Answer::Enveloped {
                pins: pins_for(&set, &schema.domain),
                revision,
                result: json!({"outcome":"applied","action_id":action_id,"edits":edits,"action_type":req.action_type,"edits_log":edits_log.as_str(),"actor_trust":"self_reported"}),
            })
        }
        None => Err(Failure::Internal(internal!("authority_failure"))),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use ont_core::schema::{ActionRule, Schema};

    #[test]
    fn planner_refuses_source_owned() {
        let fixture: &[u8] = include_bytes!("../tests/fixtures/action-work.json");
        let checked = Schema::check_bytes(fixture, &BTreeSet::new()).unwrap();
        let schema = checked.schema().clone();
        let set = SchemaSet::new(vec![checked]).unwrap();
        let at = ActionType {
            description: None,
            parameters: BTreeMap::new(),
            submission_criteria: Vec::new(),
            rules: vec![ActionRule {
                create_object: Some("task".to_string()),
                modify_object: None,
                create_or_modify_object: None,
                delete_object: None,
                set_reference: None,
                clear_reference: None,
                set: BTreeMap::new(),
            }],
        };
        let params = BTreeMap::new();
        let mut planner = Planner {
            schema: &schema,
            set: &set,
            snap: Snapshot::default(),
            params: &params,
            action_id: "act-test".to_string(),
            edit_index: 0,
            planned: Vec::new(),
            overlay: BTreeMap::new(),
        };
        let err = planner.plan(&at).unwrap_err();
        match err {
            Failure::Refused(r) => assert_eq!(r.code, "action_edits_source_owned"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn retry_id_length_prefixed() {
        assert_ne!(
            derive_request_action_id("a:b", None, "rid-1"),
            derive_request_action_id("a", Some("b:"), "rid-1")
        );
        assert!(derive_request_action_id("a:b", None, "rid-1").starts_with("act-rq-"));
    }
}
