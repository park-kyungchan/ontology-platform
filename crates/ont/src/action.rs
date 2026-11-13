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
use ont_core::{
    Change, Coverage, Id, Knowledge, Lineage, Object, Observation, Query, Response, Scope,
};
use ont_store::Append;
use ont_store::schema_mode::{ActionLogEntry, HeadAccess, SchemaHead};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

pub use ont_store::NEVER_STALE as FRESH_UNTIL_MAX;

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
            return Err(Refusal::input(
                "invalid_request",
                "/op",
                "must be list_actions when non-null",
            ));
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
            return Err(Refusal::input(
                "invalid_request",
                "/op",
                "must be validate_action when non-null",
            ));
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
            return Err(Refusal::input(
                "invalid_request",
                "/op",
                "must be apply_action when non-null",
            ));
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
fn parameter_field(name: &str) -> String {
    crate::input::pointer("/parameters", name)
}
fn parameter_refusal(field: &str, rule: &str) -> Refusal {
    let mut refusal = Refusal::input("invalid_parameter", field, rule);
    if let Some(name) = field
        .strip_prefix("/parameters/")
        .and_then(|s| s.split('/').next())
    {
        refusal = refusal.with(
            "parameter",
            json!(name.replace("~1", "/").replace("~0", "~")),
        );
    }
    refusal
}
fn invalid_parameter(field: &str, rule: &str) -> Failure {
    parameter_refusal(field, rule).into()
}
/// A rule expression identifies an input only when exactly one supplied
/// parameter feeds it. A referenced object's property is not in the request:
/// its origin is the supplied reference id, not an invented child field.
fn rule_input(params: &BTreeMap<String, Value>, expression: &Value) -> Option<String> {
    fn collect(params: &BTreeMap<String, Value>, value: &Value, names: &mut BTreeSet<String>) {
        match value {
            Value::String(s) => {
                if let Some(name) = s.strip_prefix('$').and_then(|p| p.split('.').next())
                    && params.contains_key(name)
                {
                    names.insert(name.to_string());
                }
            }
            Value::Array(values) => {
                for value in values {
                    collect(params, value, names);
                }
            }
            Value::Object(values) => {
                for value in values.values() {
                    collect(params, value, names);
                }
            }
            _ => {}
        }
    }
    let mut names = BTreeSet::new();
    collect(params, expression, &mut names);
    (names.len() == 1)
        .then(|| parameter_field(names.iter().next().map(String::as_str).unwrap_or("")))
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
    for (field, value) in [
        ("/client", Some(client.as_str())),
        ("/agent", agent.as_deref()),
        ("/session", session.as_deref()),
    ] {
        if value.is_some_and(|v| !valid_actor_field(v)) {
            return Err(Refusal::input(
                "invalid_request",
                field,
                "1 to 64 ASCII characters: letters, digits, '.', '_', ':', '-'",
            )
            .into());
        }
    }
    Ok(())
}
fn check_parameters_size(params: &Value) -> Result<(), Failure> {
    if serde_json::to_string(&canonical(params)).is_ok_and(|s| s.len() > crate::MAX_REQUEST) {
        return Err(Refusal::input(
            "invalid_request",
            "/parameters",
            "canonical JSON at most 65536 bytes",
        )
        .into());
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
        return Err(Refusal::input("invalid_request", "/parameters", "a JSON object").into());
    };
    for key in obj.keys() {
        if !at.parameters.contains_key(key) {
            return Err(invalid_parameter(
                &parameter_field(key),
                "parameter must be declared by action list",
            ));
        }
    }
    let mut out = BTreeMap::new();
    for (name, def) in &at.parameters {
        match obj.get(name) {
            None => {
                if def.required {
                    return Err(invalid_parameter(
                        &parameter_field(name),
                        "required parameter",
                    ));
                }
            }
            Some(Value::Null) if !def.required => {}
            Some(v) => {
                if !value_matches(schema, &def.ty, v) {
                    return Err(invalid_parameter(
                        &parameter_field(name),
                        &format!(
                            "must satisfy declared type {} and its value-type constraints",
                            serde_json::to_value(&def.ty).unwrap_or(Value::Null)
                        ),
                    ));
                }
                if let (Some(limit), Value::String(s)) = (def.max_bytes, v)
                    && s.len() as u64 > limit
                {
                    return Err(invalid_parameter(
                        &parameter_field(name),
                        &format!("string at most {limit} UTF-8 bytes"),
                    ));
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
    // Only the documented creation-occupancy exception may inspect this set.
    unreadable_occupied: BTreeSet<String>,
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
// Resolve only after the native read policy admits the original object. Both
// rule and criterion expressions use this boundary; no read refusal adds data.
fn reference_head<'a>(
    snap: &'a Snapshot,
    id: &Id,
    principal: &Principal,
    now: u64,
) -> Result<&'a SchemaHead, Failure> {
    let head = snap.head(id);
    if let Some(h) = head
        && !admit_scope(principal, &h.id, &h.scope, now)?
    {
        return Err(Refusal::new("rule_reference_unavailable").into());
    }
    head.filter(|h| !h.retracted)
        .ok_or_else(|| Refusal::new("rule_reference_unavailable").into())
}
// Native Get admission needs only identity and scope. The empty observation
// list prevents fields/revisions from being read merely to authorize a load.
fn admit_scope(principal: &Principal, id: &Id, scope: &Scope, now: u64) -> Result<bool, Failure> {
    let object = Object {
        id: id.clone(),
        scope: scope.clone(),
        head: String::new(),
        observations: vec![],
    };
    let admission = principal
        .authority()
        .admit(principal.session())
        .map_err(|_| internal!("authority_failure"))?;
    match admission.query(
        &crate::get::OneObject(Some(object)),
        Query::Get { id: id.clone() },
        now,
    ) {
        Ok(Response::Ok { .. }) => Ok(true),
        Ok(Response::Refused { .. }) | Err(ont_core::Error::Unavailable) => Ok(false),
        Err(_) => Err(internal!("authority_failure").into()),
    }
}
// Check rule reference preconditions before expected revisions, so an unreadable,
// missing source is never misreported as an ordinary revision race. Availability
// of every rule reference is checked before staleness. Criteria resolve their
// own references in evaluation order, so skipped operands are never prechecked.
#[derive(Default)]
struct ReferenceInputs {
    basis: BTreeMap<String, Value>,
    stale: bool,
}
fn check_reference_inputs(
    principal: &Principal,
    snap: &Snapshot,
    at: &ActionType,
    params: &BTreeMap<String, Value>,
    now: u64,
) -> Result<ReferenceInputs, Failure> {
    fn walk(
        principal: &Principal,
        snap: &Snapshot,
        params: &BTreeMap<String, Value>,
        now: u64,
        value: &Value,
        inputs: &mut ReferenceInputs,
    ) -> Result<(), Failure> {
        match value {
            Value::String(s) => {
                if let Some((name, _)) = s.strip_prefix('$').and_then(|p| p.split_once('.'))
                    && let Some(value) = params.get(name)
                {
                    let id = id_from_value(value).ok_or_else(|| {
                        invalid_parameter(
                            &parameter_field(name),
                            "an object id of the parameter target type",
                        )
                    })?;
                    let head = reference_head(snap, &id, principal, now)?;
                    inputs.basis.insert(
                        Snapshot::key(&id),
                        json!({"id":id,"revision":head.head.observation.revision}),
                    );
                    inputs.stale |= now > head.head.observation.fresh_until;
                }
            }
            Value::Array(a) => {
                for value in a {
                    walk(principal, snap, params, now, value, inputs)?;
                }
            }
            Value::Object(m) => {
                for value in m.values() {
                    walk(principal, snap, params, now, value, inputs)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    let mut inputs = ReferenceInputs::default();
    for rule in &at.rules {
        for value in rule.set.values() {
            walk(principal, snap, params, now, value, &mut inputs)?;
        }
        if let Some(value) = rule.set_reference.as_ref().and_then(|r| r.value.as_ref()) {
            walk(principal, snap, params, now, value, &mut inputs)?;
        }
    }
    Ok(inputs)
}
struct Eval<'a> {
    principal: &'a Principal,
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
    fn resolve_param_ref(&mut self, p: &str) -> Result<Option<Value>, Failure> {
        let Some((name, prop)) = p.split_once('.') else {
            return Ok(None);
        };
        let Some(value) = self.params.get(name) else {
            return Ok(None);
        };
        let id = id_from_value(value).ok_or_else(|| {
            invalid_parameter(
                &parameter_field(name),
                "an object id of the parameter target type",
            )
        })?;
        let head = reference_head(self.snap, &id, self.principal, self.now)?.clone();
        self.note(&head);
        if self.now > head.head.observation.fresh_until {
            return Err(Refusal::new("rule_value_stale").into());
        }
        Ok(field_value(&head, prop).cloned().filter(|v| !v.is_null()))
    }
    fn resolve_property(
        &mut self,
        name: &str,
        ctx: Option<&SchemaHead>,
    ) -> Result<Option<Value>, Failure> {
        if let Some(head) = ctx {
            // linked() admits every candidate before passing it as context.
            let head = head.clone();
            self.note(&head);
            return Ok(field_value(&head, name).cloned().filter(|v| !v.is_null()));
        }
        // Bare name: only with exactly one reference parameter.
        let mut refs = self
            .at
            .parameters
            .iter()
            .filter(|(_, d)| matches!(d.ty, PropertyType::Ref(_)));
        let Some((pname, _)) = refs.next() else {
            return Ok(None);
        };
        if refs.next().is_some() {
            return Ok(None);
        }
        let Some(id) = self.params.get(pname).and_then(id_from_value) else {
            return Ok(None);
        };
        let Some(head) = self.visible_head(&id)? else {
            return Ok(None);
        };
        self.note(&head);
        Ok(field_value(&head, name).cloned().filter(|v| !v.is_null()))
    }
    fn resolve(
        &mut self,
        v: &Value,
        ctx: Option<&SchemaHead>,
        property_position: bool,
    ) -> Result<Option<Value>, Failure> {
        if let Value::String(s) = v {
            if let Some(p) = s.strip_prefix('$') {
                if p == "now" {
                    return Ok(Some(json!(application_time(self.now)?)));
                }
                if p.contains('.') {
                    return self.resolve_param_ref(p);
                }
                return Ok(self.params.get(p).cloned());
            }
            if property_position {
                return self.resolve_property(s, ctx);
            }
        }
        Ok(Some(v.clone()))
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
            return self.eval_all(cs, ctx);
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
    fn eval_all(&mut self, cs: &[Condition], ctx: Option<&SchemaHead>) -> Result<bool, Failure> {
        for c in cs {
            if !self.eval(c, ctx)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
    fn eval(&mut self, c: &Condition, ctx: Option<&SchemaHead>) -> Result<bool, Failure> {
        match c {
            Condition::Present(s) => {
                let Some(p) = s.strip_prefix('$') else {
                    return Ok(false);
                };
                if let Some((name, prop)) = p.split_once('.') {
                    let Some(value) = self.params.get(name) else {
                        return Ok(false);
                    };
                    let id = id_from_value(value).ok_or_else(|| {
                        invalid_parameter(
                            &parameter_field(name),
                            "an object id of the parameter target type",
                        )
                    })?;
                    let head = reference_head(self.snap, &id, self.principal, self.now)?.clone();
                    self.note(&head);
                    return Ok(field_value(&head, prop).is_some());
                }
                Ok(self.params.contains_key(p))
            }
            Condition::Exists(s) => self.exists_fresh(s, false),
            Condition::Fresh(s) => self.exists_fresh(s, true),
            Condition::Eq(p) => self.compare_eq(p, ctx),
            Condition::Ne(p) => Ok(!self.compare_eq(p, ctx)?),
            Condition::In(p) => {
                let a = self
                    .resolve(&p[0], ctx, true)?
                    .ok_or_else(Self::not_evaluable)?;
                let b = self
                    .resolve(&p[1], ctx, false)?
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
                let want = self.resolve(v, ctx, false)?;
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
            Condition::All(cs) => self.eval_all(cs, ctx),
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
            .resolve(&p[0], ctx, !p.iter().any(|v| v.as_str() == Some("$now")))?
            .ok_or_else(Self::not_evaluable)?;
        let b = self
            .resolve(&p[1], ctx, false)?
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
            .resolve(&p[0], ctx, !p.iter().any(|v| v.as_str() == Some("$now")))?
            .ok_or_else(Self::not_evaluable)?;
        let b = self
            .resolve(&p[1], ctx, false)?
            .ok_or_else(Self::not_evaluable)?;
        cmp_values(&a, &b).map(&ok).ok_or_else(Self::not_evaluable)
    }
    // Presence-style reads hide inadmissible heads exactly as missing heads.
    // All admission decisions still go through the shared reference boundary.
    fn visible_head(&self, id: &Id) -> Result<Option<SchemaHead>, Failure> {
        match reference_head(self.snap, id, self.principal, self.now) {
            Ok(head) => Ok(Some(head.clone())),
            Err(Failure::Refused(r)) if r.code == "rule_reference_unavailable" => Ok(None),
            Err(failure) => Err(failure),
        }
    }
    fn exists_fresh(&mut self, s: &str, fresh: bool) -> Result<bool, Failure> {
        let Some(p) = s.strip_prefix('$') else {
            return Ok(false);
        };
        let Some(v) = self.params.get(p).cloned() else {
            return Ok(false);
        };
        if let Some(id) = id_from_value(&v) {
            match self.visible_head(&id)? {
                Some(head) => {
                    self.note(&head);
                    Ok(!fresh || self.now <= head.head.observation.fresh_until)
                }
                None => Ok(false),
            }
        } else {
            Ok(!v.is_null())
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
            if let Some(src) = self.visible_head(&from_id)? {
                self.note(&src);
                if let Some(v) = field_value(&src, &lt.via).cloned() {
                    for tid in ids_from_value(&v) {
                        if let Some(t) = self.visible_head(&tid)? {
                            candidates.push(t);
                        }
                    }
                }
            }
        } else if let Some(src) = self.visible_head(&from_id)? {
            self.note(&src);
            for h in self.snap.scan_type(&lt.from) {
                let Some(head) = self.visible_head(&h.id)? else {
                    continue;
                };
                if let Some(v) = field_value(&head, &lt.via)
                    && ids_from_value(v).contains(&from_id)
                {
                    candidates.push(head);
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
    principal: &Principal,
    set: &SchemaSet,
    snap: &Snapshot,
    at: &ActionType,
    params: &BTreeMap<String, Value>,
    actor: &Value,
    now: u64,
) -> Result<CriteriaOutcome, Failure> {
    let mut ev = Eval {
        principal,
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
        let passed = match ev.eval_group(&crit.all, &crit.any, &crit.none, None) {
            Ok(passed) => passed,
            Err(Failure::Refused(r)) if r.code == "rule_value_stale" => {
                results.push(json!({"code":crit.code,"message":crit.message,"passed":false,"precondition":"rule_value_stale"}));
                failed.push(crit.clone());
                continue;
            }
            Err(failure) => return Err(failure),
        };
        results.push(json!({"code":crit.code,"message":crit.message,"passed":passed}));
        if !passed {
            failed.push(crit.clone());
        }
    }
    Ok((results, ev.basis, failed))
}

#[derive(Clone)]
struct Planned {
    id: Id,
    scope: Scope,
    change: Change,
    supersedes: Option<String>,
    fields: BTreeMap<String, Knowledge>,
    revision: String,
    inputs: BTreeMap<String, String>,
}
struct Planner<'a> {
    principal: &'a Principal,
    schema: &'a Schema,
    set: &'a SchemaSet,
    snap: Snapshot,
    params: &'a BTreeMap<String, Value>,
    action_id: String,
    now: u64,
    basis: BTreeMap<String, Value>,
    edit_index: usize,
    planned: Vec<Planned>,
    overlay: BTreeMap<String, ObjectState>,
}
impl<'a> Planner<'a> {
    fn substitute(&mut self, v: &Value) -> Result<Option<Value>, Failure> {
        match v {
            Value::String(s) => {
                let Some(p) = s.strip_prefix('$') else {
                    return Ok(Some(v.clone()));
                };
                if p == "now" {
                    return Ok(Some(json!(application_time(self.now)?)));
                }
                if let Some((name, prop)) = p.split_once('.') {
                    let Some(value) = self.params.get(name) else {
                        return Ok(None);
                    };
                    let id = id_from_value(value).ok_or_else(|| {
                        invalid_parameter(
                            &parameter_field(name),
                            "an object id of the parameter target type",
                        )
                    })?;
                    let head = reference_head(&self.snap, &id, self.principal, self.now)?;
                    self.basis.insert(
                        Snapshot::key(&id),
                        json!({"id":id,"revision":head.head.observation.revision}),
                    );
                    if self.now > head.head.observation.fresh_until {
                        return Err(Refusal::new("rule_value_stale").into());
                    }
                    return field_value(head, prop)
                        .cloned()
                        .filter(|v| !v.is_null())
                        .map(Some)
                        .ok_or_else(|| {
                            Refusal::new("rule_value_unavailable")
                                .with("parameter", json!(name))
                                .with("property", json!(prop))
                                .into()
                        });
                }
                Ok(self.params.get(p).cloned())
            }
            Value::Array(values) => {
                let mut out = Vec::new();
                for value in values {
                    let Some(value) = self.substitute(value)? else {
                        return Ok(None);
                    };
                    out.push(value);
                }
                Ok(Some(Value::Array(out)))
            }
            Value::Object(values) => {
                let mut out = Map::new();
                for (key, value) in values {
                    let Some(value) = self.substitute(value)? else {
                        return Ok(None);
                    };
                    out.insert(key.clone(), value);
                }
                Ok(Some(Value::Object(out)))
            }
            _ => Ok(Some(v.clone())),
        }
    }

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
    fn target_field(&self, at: &ActionType, object_type: &str) -> String {
        let names: Vec<_> = at
            .parameters
            .iter()
            .filter(|(name, def)| {
                self.params.contains_key(*name)
                    && matches!(&def.ty, PropertyType::Ref(r) if r.target == object_type)
            })
            .map(|(name, _)| name)
            .collect();
        if names.len() == 1 {
            parameter_field(names[0])
        } else {
            "/parameters".to_string()
        }
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
        let _ = set_map;
        if ty.primary_key.from.as_deref() == Some("$action_id") {
            return Ok(self.action_id.clone());
        }
        if let Some(from) = &ty.primary_key.from
            && from.starts_with('/')
        {
            let name = at.create_key_parameter(object_type, from).ok_or_else(|| {
                invalid_parameter(
                    "/parameters",
                    "schema does not determine the primary-key parameter",
                )
            })?;
            let value = self.params.get(name);
            let pointed = value.and_then(|v| v.pointer(from));
            if let Some(key) = pointed.and_then(Value::as_str) {
                return Ok(key.to_string());
            }
            let parent = parameter_field(name);
            let field = if pointed.is_some() {
                format!("{parent}{from}")
            } else {
                parent
            };
            return Err(invalid_parameter(
                &field,
                "schema rule requires a string at the primary-key pointer in its designated parameter",
            ));
        }
        Ok(self.action_id.clone())
    }
    // Snapshot contains only admitted live heads; hidden parameters equal missing.
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
        let inputs = if change == Change::Update {
            self.planned
                .iter()
                .rev()
                .find(|p| p.id == id)
                .map(|p| p.inputs.clone())
                .unwrap_or_default()
        } else {
            BTreeMap::new()
        };
        self.planned.push(Planned {
            id: id.clone(),
            scope,
            change: change.clone(),
            supersedes,
            fields,
            revision: revision.clone(),
            inputs,
        });
        json!({"id":id,"revision":revision,"change":serde_json::to_value(change).unwrap_or(Value::Null)})
    }
    fn plan(&mut self, at: &ActionType) -> Result<Vec<Value>, Failure> {
        let mut edits = Vec::new();
        for rule in &at.rules {
            let first_edit = self.planned.len();
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
            let mut set_map = BTreeMap::new();
            for (key, value) in &rule.set {
                if let Some(value) = self.substitute(value)? {
                    set_map.insert(key.clone(), value);
                }
            }
            if !rule.set.is_empty()
                && set_map.is_empty()
                && (rule.create_object.is_some()
                    || rule.modify_object.is_some()
                    || rule.create_or_modify_object.is_some())
            {
                if let Some(t) = &rule.modify_object {
                    self.modify_target(at, t, None)?;
                }
                continue;
            }
            if let Some(t) = &rule.create_object {
                edits.push(self.do_create(at, t, &set_map, false)?);
            } else if let Some(t) = &rule.create_or_modify_object {
                // Pointer keys designate the upsert target as well as new keys.
                if self
                    .schema
                    .object_types
                    .get(t)
                    .and_then(|ty| ty.primary_key.from.as_deref())
                    .is_some_and(|p| p.starts_with('/'))
                {
                    edits.push(self.do_create(at, t, &set_map, true)?);
                } else {
                    // Action-id keyed types retain their reference-target behavior.
                    if let Some(id) = self.ref_target_typed(at, t)
                        && self.snap.unreadable_occupied.contains(&Snapshot::key(&id))
                    {
                        return Err(Refusal::new("rule_reference_unavailable").into());
                    }
                    if let Some(id) = self.ref_target_typed(at, t)
                        && self.state(&id).is_some_and(|(_, _, r, _)| !r)
                    {
                        edits.push(self.do_modify(at, t, &set_map, Some(id))?);
                    } else {
                        edits.push(self.do_create(at, t, &set_map, true)?);
                    }
                }
            } else if let Some(t) = &rule.modify_object {
                edits.push(self.do_modify(at, t, &set_map, None)?);
            } else if let Some(t) = &rule.delete_object {
                let id = self.ref_target_typed(at, t).ok_or_else(|| {
                    invalid_parameter(
                        &self.target_field(at, t),
                        "an object id parameter whose object_type matches the rule target",
                    )
                })?;
                let (_, head, _, scope) = self.target_state(&id)?;
                edits.push(self.push(id, scope, Change::Retraction, head, BTreeMap::new()));
            } else if let Some(r) = &rule.set_reference {
                edits.extend(self.do_set_ref(at, r, false)?);
            } else if let Some(r) = &rule.clear_reference {
                edits.extend(self.do_set_ref(at, r, true)?);
            }
            let mut assignments: BTreeMap<String, Option<String>> = rule
                .set
                .iter()
                .filter(|(property, _)| set_map.contains_key(*property))
                .map(|(property, value)| (property.clone(), rule_input(self.params, value)))
                .collect();
            if let Some(r) = &rule.set_reference {
                assignments.insert(
                    r.property.clone(),
                    r.value.as_ref().and_then(|v| rule_input(self.params, v)),
                );
            }
            if let Some(r) = &rule.clear_reference {
                assignments.insert(r.property.clone(), None);
            }
            for planned in &mut self.planned[first_edit..] {
                for (property, input) in &assignments {
                    if let Some(input) = input {
                        planned.inputs.insert(property.clone(), input.clone());
                    } else {
                        planned.inputs.remove(property);
                    }
                }
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
        if self.snap.unreadable_occupied.contains(&Snapshot::key(&id)) {
            return Err(if or_modify {
                Refusal::new("rule_reference_unavailable").into()
            } else {
                revision_conflict(&id)
            });
        }
        if let Some((fields, head, retracted, scope)) = self.state(&id)
            && !retracted
        {
            let _ = (fields, head, scope);
            if or_modify {
                // The key (from a parameter or a pointer) names an
                // existing object: plan a modification, as in validate.
                return self.do_modify(at, t, set_map, Some(id));
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
    fn modify_target(
        &self,
        at: &ActionType,
        t: &str,
        target: Option<Id>,
    ) -> Result<(Id, ObjectState), Failure> {
        let id = target
            .or_else(|| {
                self.params
                    .values()
                    .find_map(|v| id_from_value(v).filter(|i| i.object_type == t))
            })
            .ok_or_else(|| {
                invalid_parameter(
                    &self.target_field(at, t),
                    "an object id parameter whose object_type matches the rule target",
                )
            })?;
        Ok((id.clone(), self.target_state(&id)?))
    }
    // Admit original live heads even after an earlier rule overlaid them. Only
    // newly created/recreated objects have no original live head to admit.
    fn target_state(&self, id: &Id) -> Result<ObjectState, Failure> {
        if self.snap.live(id).is_some() || !self.overlay.contains_key(&Snapshot::key(id)) {
            reference_head(&self.snap, id, self.principal, self.now)?;
        }
        self.state(id)
            .filter(|(_, _, retracted, _)| !retracted)
            .ok_or_else(|| Refusal::new("rule_reference_unavailable").into())
    }
    fn do_modify(
        &mut self,
        at: &ActionType,
        t: &str,
        set_map: &BTreeMap<String, Value>,
        target: Option<Id>,
    ) -> Result<Value, Failure> {
        let (id, (cur, head, _, scope)) = self.modify_target(at, t, target)?;
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
    ) -> Result<Option<Value>, Failure> {
        let _ = at;
        let id = self
            .ref_target_typed_from_schema(&r.object_type)
            .ok_or_else(|| {
                invalid_parameter(
                    &self.target_field(at, &r.object_type),
                    "an object id parameter whose object_type matches the reference rule target",
                )
            })?;
        let (cur, head, _, scope) = self.target_state(&id)?;
        let mut fields = cur;
        if clear {
            fields.insert(r.property.clone(), Knowledge::Missing);
        } else {
            let raw = r.value.as_ref().ok_or_else(|| {
                invalid_parameter("/parameters", "schema rule requires a reference value")
            })?;
            let Some(v) = self.substitute(raw)? else {
                return Ok(None);
            };
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
        Ok(Some(self.push(id, scope, Change::Update, head, fields)))
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
            let field = p
                .inputs
                .get(&prop)
                .map(String::as_str)
                .unwrap_or("/parameters");
            return Err(parameter_refusal(field, "schema rule produces a property that must satisfy its declared type, required flag and value-type constraints")
                .with("property", json!(prop)).into());
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
fn application_time(secs: u64) -> Result<String, Failure> {
    if secs > FRESH_UNTIL_MAX {
        return Err(Refusal::new("application_time_out_of_range").into());
    }
    let stamp = utc_stamp(secs);
    Ok(format!(
        "{}-{}-{}T{}:{}:{}Z",
        &stamp[..4],
        &stamp[4..6],
        &stamp[6..8],
        &stamp[9..11],
        &stamp[11..13],
        &stamp[13..15]
    ))
}
fn uses_now(at: &ActionType) -> bool {
    fn contains(v: &Value) -> bool {
        match v {
            Value::String(s) => s == "$now",
            Value::Array(a) => a.iter().any(contains),
            Value::Object(m) => m.values().any(contains),
            _ => false,
        }
    }
    at.rules.iter().any(|r| {
        r.set.values().any(contains)
            || r.set_reference
                .as_ref()
                .and_then(|r| r.value.as_ref())
                .is_some_and(contains)
    }) || at.submission_criteria.iter().any(|c| {
        [&c.all, &c.any, &c.none]
            .into_iter()
            .flatten()
            .any(|cs| serde_json::to_value(cs).as_ref().is_ok_and(contains))
    })
}
fn utc_stamp_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    utc_stamp(secs)
}
fn utc_stamp(secs: u64) -> String {
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
    sync_edits_file_with(store_path, false)
}
/// Compare every exported line against the authoritative table, repairing divergence.
pub fn verify_edits_file(store_path: &std::path::Path) -> EditsLogStatus {
    sync_edits_file_with(store_path, true)
}
fn sync_edits_file_with(store_path: &std::path::Path, verify: bool) -> EditsLogStatus {
    use std::os::unix::fs::OpenOptionsExt;
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
    if verify {
        full_sync_locked(store_path, &path, &dir)
    } else {
        tail_sync_locked(store_path, &path, &dir)
    }
}
fn full_sync_locked(
    store_path: &std::path::Path,
    path: &std::path::Path,
    dir: &std::path::Path,
) -> EditsLogStatus {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
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
            .open(path);
        let Ok(mut f) = created else { return pending };
        for line in &expected {
            if f.write_all(format!("{line}\n").as_bytes()).is_err() {
                return pending;
            }
        }
        if f.sync_all().is_err() {
            return pending;
        }
        fsync_dir(dir);
        return EditsLogStatus::Written;
    }
    let opened = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC).bits() as i32)
        .open(path);
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
            .open(path);
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
        fsync_dir(dir);
        EditsLogStatus::Rebuilt
    };
    let Ok(complete_text) = std::str::from_utf8(&bytes[..complete_len]) else {
        if !rename_aside(path, dir) {
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
        if !rename_aside(path, dir) {
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
/// Return the start and bytes (without newline) of the last complete line.
fn last_complete_line(
    file: &mut std::fs::File,
    len: u64,
) -> std::io::Result<Option<(u64, Vec<u8>)>> {
    let mut window = 65_536u64;
    loop {
        let start = len.saturating_sub(window);
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        file.take(len - start).read_to_end(&mut bytes)?;
        if let Some(end) = bytes.iter().rposition(|b| *b == b'\n') {
            if let Some(previous) = bytes[..end].iter().rposition(|b| *b == b'\n') {
                return Ok(Some((
                    start + previous as u64 + 1,
                    bytes[previous + 1..end].to_vec(),
                )));
            }
            if start == 0 {
                return Ok(Some((0, bytes[..end].to_vec())));
            }
        } else if start == 0 {
            return Ok(None);
        }
        window = window.saturating_mul(4);
    }
}
fn tail_sync_locked(
    store_path: &std::path::Path,
    path: &std::path::Path,
    dir: &std::path::Path,
) -> EditsLogStatus {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    if path.symlink_metadata().is_err() {
        return full_sync_locked(store_path, path, dir);
    }
    let Ok(mut file) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC).bits() as i32)
        .open(path)
    else {
        return EditsLogStatus::Pending;
    };
    let Ok(meta) = file.metadata() else {
        return EditsLogStatus::Pending;
    };
    let caller_uid = std::fs::metadata("/proc/self")
        .map(|m| m.uid())
        .unwrap_or(u32::MAX);
    if meta.permissions().mode() & 0o777 != 0o600 || meta.uid() != caller_uid {
        eprintln!(
            "warning: edits_log_pending@crates/ont/src/action.rs:{} edits.jsonl is not 0600 owned by the caller",
            line!()
        );
        return EditsLogStatus::Pending;
    }
    if meta.len() == 0 {
        return full_sync_locked(store_path, path, dir);
    }
    let tail = match last_complete_line(&mut file, meta.len()) {
        Ok(Some(tail)) => tail,
        Ok(None) => return full_sync_locked(store_path, path, dir),
        Err(_) => return EditsLogStatus::Pending,
    };
    let seq = std::str::from_utf8(&tail.1)
        .ok()
        .and_then(|line| serde_json::from_str::<Value>(line).ok())
        .and_then(|value| value.as_object()?.get("seq")?.as_i64());
    let Some(seq) = seq else {
        return full_sync_locked(store_path, path, dir);
    };
    // The caller holds the flock before opening the table, including on retries.
    let log = match ont_store::Store::open_schema_existing(store_path) {
        Ok(mut store) => match store.action_log_from(seq) {
            Ok(log) => log,
            Err(_) => return EditsLogStatus::Pending,
        },
        Err(_) => return EditsLogStatus::Pending,
    };
    if log.first().is_none_or(|(row_seq, entry)| {
        *row_seq != seq || log_line(*row_seq, entry).as_bytes() != tail.1
    }) {
        return full_sync_locked(store_path, path, dir);
    }
    let complete_len = tail.0 + tail.1.len() as u64 + 1;
    if complete_len < meta.len() && file.set_len(complete_len).is_err() {
        return EditsLogStatus::Pending;
    }
    if file.seek(SeekFrom::End(0)).is_err() {
        return EditsLogStatus::Pending;
    }
    for (seq, entry) in &log[1..] {
        if file
            .write_all(format!("{}\n", log_line(*seq, entry)).as_bytes())
            .is_err()
        {
            return EditsLogStatus::Pending;
        }
    }
    if file.sync_all().is_err() {
        return EditsLogStatus::Pending;
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
    principal: &Principal,
    now: u64,
) -> Result<Vec<Value>, Failure> {
    let mut basis = Vec::new();
    if let Some(list) = expected {
        for e in list {
            let head = reference_head(snap, &e.id, principal, now)?;
            if head.head.observation.revision != e.revision {
                return Err(revision_conflict(&e.id));
            }
            basis.push(json!({"id":e.id,"revision":e.revision}));
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
fn collect_linked_scans(
    set: &SchemaSet,
    snap: &Snapshot,
    params: &BTreeMap<String, Value>,
    c: &Condition,
    out: &mut BTreeSet<(String, String, Id)>,
) {
    match c {
        Condition::Linked(link) => {
            for schema in set.pins().keys().filter_map(|d| set.schema(d)) {
                for lt in schema.link_types.values() {
                    if lt.names.reverse == link.link
                        && let Some(id) = link
                            .from
                            .strip_prefix('$')
                            .and_then(|p| params.get(p))
                            .and_then(id_from_value)
                        && snap.live(&id).is_some()
                    {
                        out.insert((lt.from.clone(), lt.via.clone(), id));
                    }
                }
            }
            if let Some(g) = &link.condition {
                for cs in [&g.all, &g.any, &g.none].into_iter().flatten() {
                    for x in cs {
                        collect_linked_scans(set, snap, params, x, out);
                    }
                }
            }
        }
        Condition::All(cs) | Condition::Any(cs) | Condition::NoneOf(cs) => {
            for x in cs {
                collect_linked_scans(set, snap, params, x, out);
            }
        }
        _ => {}
    }
}
trait SnapshotSource {
    fn fetch_access(&mut self, id: &Id) -> Result<Option<HeadAccess>, Failure>;
    fn fetch_head(&mut self, access: &HeadAccess) -> Result<Option<SchemaHead>, Failure>;
    #[allow(clippy::too_many_arguments)]
    fn fetch_linked(
        &mut self,
        ty: &str,
        via: &str,
        target: &Id,
        scopes: &[Scope],
        now: u64,
        limit: usize,
        after: Option<&Id>,
    ) -> Result<Vec<HeadAccess>, Failure>;
    fn fetch_scan(
        &mut self,
        ty: &str,
        limit: usize,
        after: Option<&Id>,
    ) -> Result<Vec<HeadAccess>, Failure>;
}
impl SnapshotSource for ont_store::Store {
    fn fetch_access(&mut self, id: &Id) -> Result<Option<HeadAccess>, Failure> {
        self.head_access(id).map_err(store_err)
    }
    fn fetch_head(&mut self, access: &HeadAccess) -> Result<Option<SchemaHead>, Failure> {
        self.head_after_access(access).map_err(store_err)
    }
    fn fetch_linked(
        &mut self,
        ty: &str,
        via: &str,
        target: &Id,
        scopes: &[Scope],
        now: u64,
        limit: usize,
        after: Option<&Id>,
    ) -> Result<Vec<HeadAccess>, Failure> {
        let condition = ont_store::schema_mode::CandidateCondition {
            property: via.into(),
            values: vec![
                serde_json::to_value(target).map_err(|_| Refusal::new("invalid_request"))?,
            ],
            used: BTreeSet::new(),
            // linked already answers not linked when the backing field has no value.
            absent: false,
        };
        self.candidate_access(ty, Some(&condition), now, limit, after, scopes)
            .map_err(store_err)
    }
    fn fetch_scan(
        &mut self,
        ty: &str,
        limit: usize,
        after: Option<&Id>,
    ) -> Result<Vec<HeadAccess>, Failure> {
        self.scan_head_access(ty, limit, after).map_err(store_err)
    }
}
impl SnapshotSource for ont_store::schema_mode::SchemaWrite<'_> {
    fn fetch_access(&mut self, id: &Id) -> Result<Option<HeadAccess>, Failure> {
        self.head_access(id).map_err(store_err)
    }
    fn fetch_head(&mut self, access: &HeadAccess) -> Result<Option<SchemaHead>, Failure> {
        self.head_after_access(access).map_err(store_err)
    }
    fn fetch_linked(
        &mut self,
        ty: &str,
        via: &str,
        target: &Id,
        scopes: &[Scope],
        now: u64,
        limit: usize,
        after: Option<&Id>,
    ) -> Result<Vec<HeadAccess>, Failure> {
        let condition = ont_store::schema_mode::CandidateCondition {
            property: via.into(),
            values: vec![
                serde_json::to_value(target).map_err(|_| Refusal::new("invalid_request"))?,
            ],
            used: BTreeSet::new(),
            // linked already answers not linked when the backing field has no value.
            absent: false,
        };
        self.candidate_access(ty, Some(&condition), now, limit, after, scopes)
            .map_err(store_err)
    }
    fn fetch_scan(
        &mut self,
        ty: &str,
        limit: usize,
        after: Option<&Id>,
    ) -> Result<Vec<HeadAccess>, Failure> {
        self.scan_head_access(ty, limit, after).map_err(store_err)
    }
}
// All payload fetches pass here after metadata-only native admission. Filtered
// heads never enter the snapshot or its bound. Occupancy retains no payload.
fn load_access(
    snap: &mut Snapshot,
    access: HeadAccess,
    principal: &Principal,
    now: u64,
    max: usize,
    source: &mut dyn SnapshotSource,
) -> Result<(), Failure> {
    let key = Snapshot::key(&access.id);
    if access.retracted {
        return Ok(());
    }
    if !admit_scope(principal, &access.id, &access.scope, now)? {
        return Ok(());
    }
    if !snap.heads.contains_key(&key)
        && let Some(h) = source.fetch_head(&access)?
    {
        // Store rechecks admitted metadata before fetching this payload.
        if !h.retracted && admit_scope(principal, &h.id, &h.scope, now)? {
            insert_snap(snap, h, max)?;
        }
    }
    Ok(())
}
fn load_named(
    snap: &mut Snapshot,
    id: &Id,
    principal: &Principal,
    now: u64,
    max: usize,
    source: &mut dyn SnapshotSource,
) -> Result<(), Failure> {
    if snap.head(id).is_none()
        && let Some(access) = source.fetch_access(id)?
    {
        load_access(snap, access, principal, now, max, source)?;
    }
    Ok(())
}
// Only creation targets retain an unreadable occupancy bit. Ordinary named
// reads and scans discard rejected metadata, so hidden scans retain no IDs.
fn load_creation_target(
    snap: &mut Snapshot,
    id: &Id,
    principal: &Principal,
    now: u64,
    max: usize,
    source: &mut dyn SnapshotSource,
) -> Result<(), Failure> {
    if snap.head(id).is_none()
        && let Some(access) = source.fetch_access(id)?
    {
        if !access.retracted && !admit_scope(principal, &access.id, &access.scope, now)? {
            snap.unreadable_occupied.insert(Snapshot::key(id));
        } else {
            load_access(snap, access, principal, now, max, source)?;
        }
    }
    Ok(())
}
// Preserve existing eager loads except for parameters used only by bare
// present. Rule planning also reads parameter heads implicitly (targets and
// inherited scope), so those dependencies are retained too.
fn presence_only_parameters(
    at: &ActionType,
    set: &SchemaSet,
    params: &BTreeMap<String, Value>,
) -> BTreeSet<String> {
    fn references(value: &Value, reads: &mut BTreeSet<String>) {
        match value {
            Value::String(s) => {
                if let Some(p) = s.strip_prefix('$') {
                    reads.insert(p.split_once('.').map_or(p, |(name, _)| name).into());
                }
            }
            Value::Array(values) => {
                for value in values {
                    references(value, reads);
                }
            }
            Value::Object(values) => {
                for value in values.values() {
                    references(value, reads);
                }
            }
            _ => {}
        }
    }
    fn condition(
        c: &Condition,
        at: &ActionType,
        linked_context: bool,
        only: &mut BTreeSet<String>,
        reads: &mut BTreeSet<String>,
    ) {
        match c {
            Condition::Present(s) if !s.contains('.') => {
                if let Some(p) = s.strip_prefix('$') {
                    only.insert(p.into());
                }
            }
            Condition::All(cs) | Condition::Any(cs) | Condition::NoneOf(cs) => {
                for c in cs {
                    condition(c, at, linked_context, only, reads);
                }
            }
            Condition::Linked(link) => {
                references(&json!(link.from), reads);
                if let Some(g) = &link.condition {
                    for cs in [&g.all, &g.any, &g.none].into_iter().flatten() {
                        for c in cs {
                            condition(c, at, true, only, reads);
                        }
                    }
                }
            }
            _ => {
                references(&serde_json::to_value(c).unwrap_or(Value::Null), reads);
                // Legacy bare-property comparisons implicitly read the action's
                // reference parameter. Retain that dependency as well.
                if let Condition::Eq(p)
                | Condition::Ne(p)
                | Condition::In(p)
                | Condition::Lt(p)
                | Condition::Le(p)
                | Condition::Gt(p)
                | Condition::Ge(p) = c
                    && !linked_context
                    && p[0].as_str().is_some_and(|s| !s.starts_with('$'))
                {
                    reads.extend(
                        at.parameters
                            .iter()
                            .filter(|(_, p)| matches!(p.ty, PropertyType::Ref(_)))
                            .map(|(name, _)| name.clone()),
                    );
                }
            }
        }
    }
    let mut only = BTreeSet::new();
    let mut reads = BTreeSet::new();
    for c in &at.submission_criteria {
        for cs in [&c.all, &c.any, &c.none].into_iter().flatten() {
            for c in cs {
                condition(c, at, false, &mut only, &mut reads);
            }
        }
    }
    for rule in &at.rules {
        references(
            &serde_json::to_value(rule).unwrap_or(Value::Null),
            &mut reads,
        );
        // Target operations select a supplied reference by its declared type.
        let targets = [
            rule.modify_object.as_deref(),
            rule.delete_object.as_deref(),
            rule.create_or_modify_object.as_deref(),
            rule.set_reference.as_ref().map(|r| r.object_type.as_str()),
            rule.clear_reference
                .as_ref()
                .map(|r| r.object_type.as_str()),
        ];
        for (name, p) in &at.parameters {
            if matches!(&p.ty, PropertyType::Ref(r) if targets.contains(&Some(r.target.as_str())))
                || params
                    .get(name)
                    .and_then(id_from_value)
                    .is_some_and(|id| targets.contains(&Some(id.object_type.as_str())))
            {
                reads.insert(name.clone());
            }
        }
        if rule
            .create_object
            .as_deref()
            .or(rule.create_or_modify_object.as_deref())
            .and_then(|t| set.schema_of(t))
            .is_some_and(|s| !s.scope_keys.is_empty())
        {
            // Creation can inherit scope from any supplied reference parameter.
            reads.extend(params.keys().cloned());
        }
    }
    only.retain(|name| !reads.contains(name));
    only
}
fn load_snapshot(
    ctx: &Context,
    set: &SchemaSet,
    principal: &Principal,
    at: &ActionType,
    params: &BTreeMap<String, Value>,
    expected: &Option<Vec<ExpectedHead>>,
    source: &mut dyn SnapshotSource,
) -> Result<Snapshot, Failure> {
    let max = ctx.snapshot_max_heads;
    let page = ctx.snapshot_page_size.max(1);
    let mut snap = Snapshot::default();
    let mut ids: Vec<Id> = Vec::new();
    let presence_only = presence_only_parameters(at, set, params);
    for (name, v) in params {
        if !presence_only.contains(name) {
            ids.extend(ids_from_value(v));
        }
    }
    if let Some(list) = expected {
        for e in list {
            ids.push(e.id.clone());
        }
    }
    for id in &ids {
        load_named(&mut snap, id, principal, ctx.now, max, source)?;
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
        load_named(&mut snap, id, principal, ctx.now, max, source)?;
    }
    // Reverse `linked` conditions page candidates; list references retain type scans.
    let mut scan_types = BTreeSet::new();
    for crit in &at.submission_criteria {
        for cs in [&crit.all, &crit.any, &crit.none].into_iter().flatten() {
            for c in cs {
                collect_linked_scans(set, &snap, params, c, &mut scan_types);
            }
        }
    }
    for (ty, via, target) in &scan_types {
        let list_reference = set
            .properties(ty)
            .and_then(|props| props.get(via))
            .is_some_and(|prop| matches!(prop.ty, ont_core::schema::PropertyType::Array(_)));
        let scopes = crate::search::read_scopes(principal, ty);
        let mut after: Option<Id> = None;
        loop {
            let heads = if list_reference {
                source.fetch_scan(ty, page, after.as_ref())?
            } else {
                source.fetch_linked(ty, via, target, &scopes, ctx.now, page, after.as_ref())?
            };
            let full = heads.len() == page;
            after = heads.last().map(|h| h.id.clone());
            for h in heads {
                load_access(&mut snap, h, principal, ctx.now, max, source)?;
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
        if let Some(t) = &rule.create_or_modify_object
            && let Some(id) = planner.ref_target_typed(at, t)
        {
            load_creation_target(
                &mut planner.snap,
                &id,
                planner.principal,
                planner.now,
                max,
                source,
            )?;
        }
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
        load_creation_target(
            &mut planner.snap,
            &id,
            planner.principal,
            planner.now,
            max,
            source,
        )?;
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
        return Err(Refusal::input(
            "unknown_domain",
            "/domain",
            "a domain installed in this store",
        )
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
                    json!({"name":pname,"type":serde_json::to_value(&p.ty).unwrap_or(Value::Null),"required":p.required,"description":p.description,"allowed_values":allowed,"max_bytes":p.max_bytes})
                }).collect::<Vec<_>>(),
                "criteria":at.submission_criteria,
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
    let (schema, at) = find_action_type(&set, &req.action_type).ok_or_else(|| {
        Failure::from(Refusal::input(
            "unknown_action",
            "/action_type",
            "an installed action type; use action list",
        ))
    })?;
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
        return Err(Refusal::input(
            "invalid_request",
            "/request_id",
            "1 to 64 ASCII characters: letters, digits, '.', '_', '-'",
        )
        .into());
    }
    let params = validate_parameters(&schema, &at, &req.parameters)?;
    let params_canon = canonical(&Value::Object(
        params.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
    ));
    let actor = actor_json(ctx, &req.agent, &req.session);
    let snap = load_snapshot(
        ctx,
        &set,
        &principal,
        &at,
        &params,
        &req.expected,
        &mut store,
    )?;
    let references = check_reference_inputs(&principal, &snap, &at, &params, ctx.now)?;
    let (criteria, basis_map, failed) =
        eval_criteria(&principal, &set, &snap, &at, &params, &actor, ctx.now)?;
    let stale = references.stale
        || criteria
            .iter()
            .any(|c| c["precondition"] == "rule_value_stale");
    let expected_basis = match check_expected(&snap, &req.expected, &principal, ctx.now) {
        // Apply gives stale values precedence over expected-revision refusals.
        // Retain the usual valid:false preview when expected itself passes.
        Err(Failure::Refused(_)) if stale => return Err(Refusal::new("rule_value_stale").into()),
        result => result?,
    };
    let mut basis: Vec<Value> = references.basis.values().cloned().collect();
    for b in basis_map.values() {
        if !basis.contains(b) {
            basis.push(b.clone());
        }
    }
    for b in expected_basis {
        if !basis.contains(&b) {
            basis.push(b);
        }
    }
    let valid = failed.is_empty() && !stale;
    let mut edits = Vec::new();
    if valid {
        let log_len = st!(store.action_count());
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
            principal: &principal,
            schema: &schema,
            set: &set,
            snap: snap.clone(),
            params: &params,
            action_id,
            now: ctx.now,
            basis: BTreeMap::new(),
            edit_index: 0,
            planned: Vec::new(),
            overlay: BTreeMap::new(),
        };
        augment_planner(&mut planner, &at, ctx.snapshot_max_heads, &mut store)?;
        edits = planner.plan(&at)?;
        check_planned(&set, &planner)?;
        for b in planner.basis.values() {
            if !basis.contains(b) {
                basis.push(b.clone());
            }
        }
    }
    let mut result = json!({"valid":valid,"criteria":criteria,"basis":basis,"edits":edits,"action_type":req.action_type});
    if uses_now(&at) {
        result["application_time"] = json!("set_at_application");
    }
    if stale {
        result["preconditions"] = json!([{"code":"rule_value_stale","passed":false}]);
    }
    Ok(Answer::Enveloped {
        pins: pins_for(&set, &schema.domain),
        revision: principal.revision()?,
        result,
    })
}

enum ApplyOutcome {
    Applied { action_id: String, edits: Value },
    Already { action_id: String, edits: Value },
}

/// Fixed recovery text keyed only by criterion codes, never request values or
/// interpolated criterion messages. Unknown domain criteria use a generic repair.
fn submission_hint(codes: &[String]) -> String {
    let mut hints = Vec::new();
    for code in codes {
        let hint = match code.as_str() {
            "decision_already_current" => {
                "a current decision already owns the topic and supersede_decision takes its Id"
            }
            "duplicate_active_hold" => {
                "an active hold already covers the effect and lift_hold takes its Id once its condition is satisfied"
            }
            "older_measurement" => {
                "a later observation already exists and record_measurement needs the latest observation time, with equal-time correction allowed"
            }
            "task_not_current" => "the task must be current; run ont index --source task_states",
            "hold_not_active" => "lift_hold requires an active hold",
            _ => "satisfy the failed criteria listed in failures before retrying",
        };
        if !hints.contains(&hint) {
            hints.push(hint);
        }
    }
    format!("Resolve the submission criteria: {}.", hints.join("; "))
}

pub fn apply_action(ctx: &Context, req: &ApplyActionRequest) -> Result<Answer, Failure> {
    if let Some(r) = &req.request_id
        && !valid_request_id(r)
    {
        return Err(Refusal::input(
            "invalid_request",
            "/request_id",
            "1 to 64 ASCII characters: letters, digits, '.', '_', '-'",
        )
        .into());
    }
    check_actor_bounds(ctx, &req.agent, &req.session)?;
    check_parameters_size(&req.parameters)?;
    let mut store = open_store(&ctx.store, true)?;
    let set = st!(store.schema_set()).clone();
    let principal = Principal::bind(&set)?;
    let revision = principal.revision()?;
    let (schema, at) = find_action_type(&set, &req.action_type).ok_or_else(|| {
        Failure::from(Refusal::input(
            "unknown_action",
            "/action_type",
            "an installed action type; use action list",
        ))
    })?;
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
                .ok_or_else(|| Failure::from(Refusal::input("unknown_action", "/action_type", "an installed action type; use action list")))?;
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
            let snap = load_snapshot(ctx, &tx_set, &tprincipal, &tat, &tparams, &req.expected, tx)?;
            let references = check_reference_inputs(&tprincipal, &snap, &tat, &tparams, ctx.now)?;
            let (criteria, basis_map, failed) = eval_criteria(&tprincipal, &tx_set, &snap, &tat, &tparams, &actor, ctx.now)?;
            if references.stale || criteria.iter().any(|c| c["precondition"] == "rule_value_stale") {
                return Err(Refusal::new("rule_value_stale").into());
            }
            let expected_basis = check_expected(&snap, &req.expected, &tprincipal, ctx.now)?;
            if !failed.is_empty() {
                let codes: Vec<String> = failed.iter().map(|c| c.code.clone()).collect();
                let failures: Vec<Value> = failed.iter().map(|c| json!({"code":c.code,"message":c.message})).collect();
                let message = failed.first().map(|c| c.message.clone()).unwrap_or_default();
                return Err(Refusal::new("submission_criteria")
                    .with("hint", json!(submission_hint(&codes)))
                    .with("failed", json!(codes)).with("failures", json!(failures)).with("message", json!(message)).into());
            }
            let mut basis: Vec<Value> = references.basis.values().cloned().collect();
            for b in basis_map.values() { if !basis.contains(b) { basis.push(b.clone()); } }
            for b in expected_basis { if !basis.contains(&b) { basis.push(b); } }
            let basis_json = Value::Array(basis.clone());
            let log_len = tx.action_count().map_err(store_err)?;
            let request_action_id = req.request_id.as_ref().map(|r| {
                derive_request_action_id(
                    &effective_client(ctx),
                    effective_agent(ctx, &req.agent).as_deref(),
                    r,
                )
            });
            let action_id = allocate_action_id(ctx.now, &req.action_type, &params_canon, &basis_json, log_len, &request_action_id);
            let mut planner = Planner { principal: &tprincipal, schema: &tschema, set: &tx_set, snap: snap.clone(), params: &tparams, action_id: action_id.clone(), now: ctx.now, basis: BTreeMap::new(), edit_index: 0, planned: Vec::new(), overlay: BTreeMap::new() };
            augment_planner(&mut planner, &tat, ctx.snapshot_max_heads, tx)?;
            let edits = planner.plan(&tat)?;
            check_planned(&tx_set, &planner)?;
            for b in planner.basis.values() { if !basis.contains(b) { basis.push(b.clone()); } }
            let basis_json = Value::Array(basis);
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

    fn check_tail(bytes: &[u8], expected: Option<(u64, Vec<u8>)>) {
        let path = std::env::temp_dir().join(format!(
            "ont-tail-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        std::fs::remove_file(path).unwrap();
        file.write_all(bytes).unwrap();
        let actual = last_complete_line(&mut file, bytes.len() as u64).unwrap();
        assert_eq!(
            actual.as_ref().map(|(offset, line)| (*offset, line.len())),
            expected
                .as_ref()
                .map(|(offset, line)| (*offset, line.len()))
        );
        assert_eq!(actual, expected);
    }
    #[test]
    fn last_complete_line_long_line() {
        let mut bytes = b"first\n".to_vec();
        bytes.extend(vec![b'x'; 200_000]);
        bytes.push(b'\n');
        check_tail(&bytes, Some((6, vec![b'x'; 200_000])));
    }
    #[test]
    fn last_complete_line_no_newline() {
        check_tail(&vec![b'x'; 200_000], None);
    }
    #[test]
    fn last_complete_line_incomplete_fragment() {
        check_tail(b"first\nlast\nfragment", Some((6, b"last".to_vec())));
        let mut bytes = b"first\nlast\n".to_vec();
        bytes.extend(vec![b'x'; 200_000]);
        check_tail(&bytes, Some((6, b"last".to_vec())));
    }
    #[test]
    fn last_complete_line_empty() {
        check_tail(b"", None);
    }

    struct CountingSource {
        access: Option<HeadAccess>,
        payload_reads: usize,
        access_reads: usize,
        scans: usize,
    }
    impl SnapshotSource for CountingSource {
        fn fetch_access(&mut self, _: &Id) -> Result<Option<HeadAccess>, Failure> {
            self.access_reads += 1;
            Ok(self.access.clone())
        }
        fn fetch_head(&mut self, _: &HeadAccess) -> Result<Option<SchemaHead>, Failure> {
            self.payload_reads += 1;
            panic!("unadmitted payload read")
        }
        fn fetch_linked(
            &mut self,
            _: &str,
            _: &str,
            _: &Id,
            _: &[Scope],
            _: u64,
            _: usize,
            _: Option<&Id>,
        ) -> Result<Vec<HeadAccess>, Failure> {
            self.scans += 1;
            panic!("scan from unadmitted source")
        }
        fn fetch_scan(
            &mut self,
            _: &str,
            _: usize,
            _: Option<&Id>,
        ) -> Result<Vec<HeadAccess>, Failure> {
            self.scans += 1;
            panic!("scan from unadmitted source")
        }
    }

    #[test]
    fn unreadable_named_heads_fetch_no_payload_and_start_no_scan() {
        let mut wire: Value =
            serde_json::from_slice(include_bytes!("../tests/fixtures/action-work.json")).unwrap();
        wire["scope_keys"] = json!(["tenant"]);
        wire["grants"][0]["scope"] = json!({"tenant":"allowed"});
        wire["object_types"]["task"]["owner"] = json!("ontology");
        wire["object_types"]["task"]["primary_key"] = json!({"from":"$action_id"});
        for prop in wire["object_types"]["task"]["properties"]
            .as_object_mut()
            .unwrap()
            .values_mut()
        {
            prop.as_object_mut().unwrap().remove("from");
        }
        let checked =
            Schema::check_bytes(&serde_json::to_vec(&wire).unwrap(), &BTreeSet::new()).unwrap();
        let at = checked.schema().action_types["set_hold"].clone();
        let set = SchemaSet::new(vec![checked]).unwrap();
        let principal = Principal::bind(&set).unwrap();
        let id = Id {
            object_type: "task".into(),
            namespace: set.schema("work").unwrap().namespace.clone(),
            native_key: "unavailable".into(),
        };
        let params = BTreeMap::from([("task".into(), serde_json::to_value(&id).unwrap())]);
        let ctx = Context::with_snapshot_bounds(PathBuf::new(), 1000, 1, 1);
        for hidden in [false, true] {
            let mut source = CountingSource {
                access: hidden.then(|| HeadAccess {
                    id: id.clone(),
                    scope: Scope::domain(
                        id.namespace.clone(),
                        "work",
                        BTreeMap::from([("tenant".into(), "blocked".into())]),
                    )
                    .unwrap(),
                    retracted: false,
                }),
                payload_reads: 0,
                access_reads: 0,
                scans: 0,
            };
            let snap =
                load_snapshot(&ctx, &set, &principal, &at, &params, &None, &mut source).unwrap();
            assert!(snap.heads.is_empty());
            assert!(snap.unreadable_occupied.is_empty());
            assert_eq!(source.payload_reads, 0);
            assert_eq!(source.scans, 0);
        }
    }

    #[test]
    fn present_parameter_fetches_no_metadata_payload_or_scans() {
        let mut wire: Value =
            serde_json::from_slice(include_bytes!("../tests/fixtures/action-work.json")).unwrap();
        wire["scope_keys"] = json!(["tenant"]);
        wire["grants"][0]["scope"] = json!({"tenant":"allowed"});
        wire["object_types"]["task"]["owner"] = json!("ontology");
        wire["object_types"]["task"]["primary_key"] = json!({"from":"$action_id"});
        for prop in wire["object_types"]["task"]["properties"]
            .as_object_mut()
            .unwrap()
            .values_mut()
        {
            prop.as_object_mut().unwrap().remove("from");
        }
        wire["action_types"]["probe_presence"] = json!({
            "parameters":{"task":{"type":{"ref":"task"}}},
            "submission_criteria":[{"code":"present","message":"Supply task.","all":[{"present":"$task"}]}]
        });
        wire["grants"][0]["actions"]
            .as_array_mut()
            .unwrap()
            .push(json!("probe_presence"));
        let checked =
            Schema::check_bytes(&serde_json::to_vec(&wire).unwrap(), &BTreeSet::new()).unwrap();
        let at = checked.schema().action_types["probe_presence"].clone();
        let set = SchemaSet::new(vec![checked]).unwrap();
        let principal = Principal::bind(&set).unwrap();
        let id = Id {
            object_type: "task".into(),
            namespace: set.schema("work").unwrap().namespace.clone(),
            native_key: "target".into(),
        };
        let ctx = Context::with_snapshot_bounds(PathBuf::new(), 1000, 1, 1);
        // A live readable target must also avoid the payload fetch that would
        // panic in this source. A dangling target therefore behaves identically.
        for state in ["omitted", "live", "missing", "retracted", "unreadable"] {
            let params = if state == "omitted" {
                BTreeMap::new()
            } else {
                BTreeMap::from([("task".into(), json!(id))])
            };
            let mut source = CountingSource {
                access: (!matches!(state, "omitted" | "missing")).then(|| HeadAccess {
                    id: id.clone(),
                    scope: Scope::domain(
                        id.namespace.clone(),
                        "work",
                        BTreeMap::from([(
                            "tenant".into(),
                            if state == "unreadable" {
                                "blocked"
                            } else {
                                "allowed"
                            }
                            .into(),
                        )]),
                    )
                    .unwrap(),
                    retracted: state == "retracted",
                }),
                payload_reads: 0,
                scans: 0,
                access_reads: 0,
            };
            let snap =
                load_snapshot(&ctx, &set, &principal, &at, &params, &None, &mut source).unwrap();
            let (results, basis, failed) =
                eval_criteria(&principal, &set, &snap, &at, &params, &json!({}), 1000).unwrap();
            assert_eq!(results[0]["passed"], state != "omitted", "{state}");
            assert_eq!(failed.is_empty(), state != "omitted");
            assert!(basis.is_empty());
            assert!(snap.heads.is_empty());
            assert_eq!(
                (source.access_reads, source.payload_reads, source.scans),
                (0, 0, 0),
                "{state}"
            );
            if state == "omitted" {
                let mut property_at = at.clone();
                property_at.submission_criteria[0].all =
                    Some(vec![Condition::Present("$task.directory".into())]);
                let snap = load_snapshot(
                    &ctx,
                    &set,
                    &principal,
                    &property_at,
                    &params,
                    &None,
                    &mut source,
                )
                .unwrap();
                let (results, basis, _) = eval_criteria(
                    &principal,
                    &set,
                    &snap,
                    &property_at,
                    &params,
                    &json!({}),
                    1000,
                )
                .unwrap();
                assert_eq!(results[0]["passed"], false);
                assert!(basis.is_empty());
                assert_eq!(
                    (source.access_reads, source.payload_reads, source.scans),
                    (0, 0, 0)
                );
            }
        }
    }

    #[test]
    fn application_time_utc_boundaries() {
        assert_eq!(application_time(0).unwrap(), "1970-01-01T00:00:00Z");
        assert_eq!(
            application_time(951_782_400).unwrap(),
            "2000-02-29T00:00:00Z"
        );
        assert_eq!(
            application_time(FRESH_UNTIL_MAX).unwrap(),
            "9999-12-31T23:59:59Z"
        );
        assert!(application_time(FRESH_UNTIL_MAX + 1).is_err());
    }

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
        let principal = Principal::bind(&set).unwrap();
        let mut planner = Planner {
            principal: &principal,
            schema: &schema,
            set: &set,
            snap: Snapshot::default(),
            params: &params,
            action_id: "act-test".to_string(),
            now: 1000,
            basis: BTreeMap::new(),
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
    // Private planner guards are exercised directly where typed request/schema
    // admission would otherwise reject the malformed input first.
    fn with_feedback_planner(params: BTreeMap<String, Value>, run: impl FnOnce(&mut Planner<'_>)) {
        let checked = Schema::check_bytes(
            include_bytes!("../tests/fixtures/render-board.json"),
            &BTreeSet::new(),
        )
        .unwrap();
        let schema = checked.schema().clone();
        let set = SchemaSet::new(vec![checked]).unwrap();
        let principal = Principal::bind(&set).unwrap();
        let mut planner = Planner {
            principal: &principal,
            schema: &schema,
            set: &set,
            snap: Snapshot::default(),
            params: &params,
            action_id: "act-feedback".into(),
            now: 1000,
            basis: BTreeMap::new(),
            edit_index: 0,
            planned: Vec::new(),
            overlay: BTreeMap::new(),
        };
        run(&mut planner);
    }
    fn feedback_action(rule: Value) -> ActionType {
        serde_json::from_value(json!({"parameters":{"subject":{"type":{"ref":"item"},"required":false},"payload":{"type":"string","required":false}},"rules":[rule]})).unwrap()
    }
    fn assert_parameter_field(err: Failure, field: &str) -> Refusal {
        let Failure::Refused(r) = err else {
            panic!("{err:?}");
        };
        assert_eq!(r.code, "invalid_parameter");
        assert_eq!(r.extra["field"], field);
        assert!(r.extra["rule"].is_string());
        r
    }
    #[test]
    fn primary_key_feedback_names_nested_spec_key() {
        with_feedback_planner(
            BTreeMap::from([("spec".into(), json!({"key":42}))]),
            |planner| {
                let mut schema = planner.schema.clone();
                schema
                    .object_types
                    .get_mut("item")
                    .unwrap()
                    .primary_key
                    .from = Some("/key".into());
                let key_planner = Planner {
                    schema: &schema,
                    ..Planner {
                        principal: planner.principal,
                        schema: planner.schema,
                        set: planner.set,
                        snap: planner.snap.clone(),
                        params: planner.params,
                        action_id: planner.action_id.clone(),
                        now: planner.now,
                        basis: BTreeMap::new(),
                        edit_index: 0,
                        planned: Vec::new(),
                        overlay: BTreeMap::new(),
                    }
                };
                let err = key_planner
                    .create_key(
                        &serde_json::from_value(json!({"parameters":{"spec":{"type":{"struct":{"key":"string"}},"required":true}},"rules":[{"create_object":"item"}]})).unwrap(),
                        "item",
                        &BTreeMap::new(),
                    )
                    .unwrap_err();
                assert_parameter_field(err, "/parameters/spec/key");
            },
        );
    }
    #[test]
    fn delete_target_feedback_uses_subject_not_item() {
        with_feedback_planner(
            BTreeMap::from([("subject".into(), json!("not-an-id"))]),
            |planner| {
                assert_parameter_field(
                    planner
                        .plan(&feedback_action(json!({"delete_object":"item"})))
                        .unwrap_err(),
                    "/parameters/subject",
                );
            },
        );
    }
    #[test]
    fn modify_target_feedback_uses_subject_not_item() {
        with_feedback_planner(
            BTreeMap::from([("subject".into(), json!("not-an-id"))]),
            |planner| {
                assert_parameter_field(
                    planner
                        .plan(&feedback_action(
                            json!({"modify_object":"item","set":{"title":"literal"}}),
                        ))
                        .unwrap_err(),
                    "/parameters/subject",
                );
            },
        );
    }
    #[test]
    fn reference_target_feedback_uses_subject_not_type_or_property() {
        with_feedback_planner(
            BTreeMap::from([("subject".into(), json!("not-an-id"))]),
            |planner| {
                assert_parameter_field(planner.plan(&feedback_action(json!({"set_reference":{"object_type":"item","property":"ref","value":"$subject"}}))).unwrap_err(), "/parameters/subject");
            },
        );
    }
    #[test]
    fn missing_reference_rule_value_feedback_points_at_parameters() {
        let id = Id {
            object_type: "item".into(),
            namespace: ont_core::Namespace {
                instance: "test".into(),
                account: "root".into(),
            },
            native_key: "target".into(),
        };
        with_feedback_planner(BTreeMap::from([("subject".into(), json!(id))]), |planner| {
            planner.overlay.insert(
                Snapshot::key(&id),
                (
                    BTreeMap::new(),
                    None,
                    false,
                    Scope::Domain {
                        namespace: id.namespace.clone(),
                        domain: "board".into(),
                        keys: BTreeMap::new(),
                    },
                ),
            );
            let r = assert_parameter_field(
                planner
                    .plan(&feedback_action(
                        json!({"set_reference":{"object_type":"item","property":"ref"}}),
                    ))
                    .unwrap_err(),
                "/parameters",
            );
            assert!(r.extra["rule"].as_str().unwrap().contains("schema rule"));
        });
    }
    #[test]
    fn planned_property_feedback_names_payload_not_title() {
        with_feedback_planner(BTreeMap::from([("payload".into(), json!(7))]), |planner| {
            planner
                .plan(&feedback_action(
                    json!({"create_object":"item","set":{"title":"$payload"}}),
                ))
                .unwrap();
            let r = assert_parameter_field(
                check_planned(planner.set, planner).unwrap_err(),
                "/parameters/payload",
            );
            assert!(r.extra["rule"].as_str().unwrap().contains("schema rule"));
        });
    }

    #[test]
    fn unresolved_optional_reference_value_skips_and_clear_needs_no_value() {
        let id = Id {
            object_type: "item".into(),
            namespace: ont_core::Namespace {
                instance: "test".into(),
                account: "root".into(),
            },
            native_key: "target".into(),
        };
        with_feedback_planner(BTreeMap::from([("subject".into(), json!(id))]), |planner| {
            planner.overlay.insert(
                Snapshot::key(&id),
                (
                    BTreeMap::new(),
                    None,
                    false,
                    Scope::Domain {
                        namespace: id.namespace.clone(),
                        domain: "board".into(),
                        keys: BTreeMap::new(),
                    },
                ),
            );
            assert!(planner.plan(&feedback_action(json!({"set_reference":{"object_type":"item","property":"ref","value":"$payload"}}))).unwrap().is_empty());
            assert!(planner.planned.is_empty());
            assert_eq!(
                planner
                    .plan(&feedback_action(
                        json!({"clear_reference":{"object_type":"item","property":"ref"}})
                    ))
                    .unwrap()
                    .len(),
                1
            );
            assert_eq!(planner.planned[0].fields["ref"], Knowledge::Missing);
        });
    }
    #[test]
    fn planned_literal_failure_has_no_invented_parameter() {
        with_feedback_planner(
            BTreeMap::from([("payload".into(), json!("unrelated"))]),
            |planner| {
                planner
                    .plan(&feedback_action(
                        json!({"create_object":"item","set":{"title":7}}),
                    ))
                    .unwrap();
                let r = assert_parameter_field(
                    check_planned(planner.set, planner).unwrap_err(),
                    "/parameters",
                );
                assert!(r.extra["rule"].as_str().unwrap().contains("schema rule"));
                assert!(!r.extra.contains_key("parameter"));
            },
        );
    }
    #[test]
    fn primary_key_feedback_uses_present_parent_or_aggregate_for_ambiguity() {
        for (params, expected) in [
            (
                BTreeMap::from([("spec".into(), json!({}))]),
                "/parameters/spec",
            ),
            (
                BTreeMap::from([
                    ("spec".into(), json!({"key":1})),
                    ("other".into(), json!({"key":2})),
                ]),
                "/parameters",
            ),
        ] {
            with_feedback_planner(params, |planner| {
                let mut schema = planner.schema.clone();
                schema
                    .object_types
                    .get_mut("item")
                    .unwrap()
                    .primary_key
                    .from = Some("/key".into());
                let key_planner = Planner {
                    schema: &schema,
                    principal: planner.principal,
                    set: planner.set,
                    snap: planner.snap.clone(),
                    params: planner.params,
                    action_id: planner.action_id.clone(),
                    now: planner.now,
                    basis: BTreeMap::new(),
                    edit_index: 0,
                    planned: Vec::new(),
                    overlay: BTreeMap::new(),
                };
                let err = key_planner
                    .create_key(
                        &serde_json::from_value(json!({"parameters": if expected == "/parameters" { json!({"spec":{"type":{"struct":{"key":"string"}},"required":true},"other":{"type":{"struct":{"key":"string"}},"required":true}}) } else { json!({"spec":{"type":{"struct":{"key":"string"}},"required":true}}) },"rules":[{"create_object":"item"}]})).unwrap(),
                        "item",
                        &BTreeMap::new(),
                    )
                    .unwrap_err();
                let r = assert_parameter_field(err, expected);
                let request = json!({"parameters": planner.params});
                assert!(
                    request
                        .pointer(r.extra["field"].as_str().unwrap())
                        .is_some()
                );
            });
        }
    }
}
