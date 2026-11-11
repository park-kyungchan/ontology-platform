//! `search`: the current heads of one object type that match a filter.
//!
//! Request: `{"op":"search","object_type":T,"where":F,"limit":N,
//! "continuation":C}` with `F` one of `{"eq":[p,v]}`, `ne`, `lt`, `le`, `gt`,
//! `ge`, `{"in":[p,[v,..]]}`, `{"prefix":[p,"text"]}`, `{"unresolved":p}`,
//! or `{"all":[F,..]}`, `{"any":[F,..]}`, `{"none":[F,..]}`.
//!
//! Admission is the one of `get` (the `Authority` admits each object); a
//! type the principal may not read answers exactly as a type that does not
//! exist (`unavailable`), and hidden objects are not counted. A comparison
//! on a property that is not in `value` state is unknown (three-valued
//! logic): such an object is excluded and counted in `unresolved_excluded`.
//! At most 10,000 heads are scanned per request, else `result_too_large`;
//! retracted heads are never scanned.
use crate::get::OneObject;
use crate::principal::Principal;
use crate::refusal::{open_store, st};
use crate::{Answer, Context, Failure, Refusal, internal};
use ont_core::schema::{PropertyType, ScalarType, Schema};
use ont_core::{Id, Knowledge, Object, Output, Query, Response};
use ont_store::schema_mode::SchemaHead;
use serde::Deserialize;
use serde_json::{Value, json};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

/// Heads scanned per request.
pub const SCAN_LIMIT: usize = 10_000;
pub const DEFAULT_LIMIT: u64 = 100;
pub const MAX_LIMIT: u64 = 1_000;
/// Most leaf conditions in one filter, and the deepest nesting.
const MAX_LEAVES: usize = 64;
const MAX_DEPTH: usize = 8;
const MAX_IN_VALUES: usize = 100;
const CONTINUATION_PREFIX: &str = "c1:";

/// One filter node.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Filter {
    Eq(String, Value),
    Ne(String, Value),
    In(String, Vec<Value>),
    Lt(String, Value),
    Le(String, Value),
    Gt(String, Value),
    Ge(String, Value),
    Prefix(String, Value),
    Unresolved(String),
    All(Vec<Filter>),
    Any(Vec<Filter>),
    #[serde(rename = "none")]
    NoneOf(Vec<Filter>),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchRequest {
    #[serde(default)]
    pub op: Option<String>,
    pub object_type: String,
    #[serde(rename = "where", default)]
    pub filter: Option<Filter>,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub continuation: Option<String>,
}
impl SearchRequest {
    /// Parse the strict request (at most `MAX_REQUEST` bytes; an `op`, when
    /// present, must be `search`).
    pub fn parse(bytes: &[u8]) -> Result<Self, Refusal> {
        let req: Self = crate::cli::parse_request(bytes)?;
        if req.op.as_deref().is_some_and(|op| op != "search") {
            return Err(Refusal::input(
                "invalid_request",
                "/op",
                "must be search when non-null",
            ));
        }
        Ok(req)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tri {
    True,
    False,
    Unknown,
}
impl Tri {
    fn from(b: bool) -> Self {
        if b { Self::True } else { Self::False }
    }
    fn not(self) -> Self {
        match self {
            Self::True => Self::False,
            Self::False => Self::True,
            Self::Unknown => Self::Unknown,
        }
    }
}

fn refuse_property(code: &'static str, _property: &str) -> Failure {
    Refusal::input(
        code,
        "/where",
        match code {
            "undeclared_property" => {
                "filter property must be declared on the requested object type"
            }
            "invalid_filter_operator" => {
                "ordering requires string, integer, timestamp or date; prefix requires string"
            }
            "wrong_value_type" => {
                "filter value must satisfy the declared property type; prefix requires a string"
            }
            _ => "a valid declared property filter",
        },
    )
    .into()
}

/// The scalar a property type reduces to (through value types), if any.
fn base_of(schema: &Schema, ty: &PropertyType) -> Option<ScalarType> {
    match ty {
        PropertyType::Scalar(s) => Some(*s),
        PropertyType::ValueType(r) => schema.value_types.get(&r.value_type).map(|d| d.base),
        _ => None,
    }
}

/// Check every leaf against the declared properties of the type; collect the
/// properties that comparisons use.
struct Checker<'a> {
    set: &'a ont_core::schema::SchemaSet,
    schema: &'a Schema,
    object_type: &'a str,
    leaves: usize,
    used: BTreeSet<String>,
}
impl Checker<'_> {
    fn walk(&mut self, f: &Filter, depth: usize) -> Result<(), Failure> {
        if depth > MAX_DEPTH {
            return Err(Refusal::input(
                "filter_too_large",
                "/where",
                "filter depth at most 8 (root depth 0)",
            )
            .into());
        }
        match f {
            Filter::All(v) | Filter::Any(v) | Filter::NoneOf(v) => {
                for child in v {
                    self.walk(child, depth + 1)?;
                }
                Ok(())
            }
            Filter::Unresolved(p) => {
                self.leaf()?;
                self.declared(p).map(|_| ())
            }
            Filter::Eq(p, v) | Filter::Ne(p, v) => {
                self.leaf()?;
                self.comparison(p, std::slice::from_ref(v), Op::Equality)
            }
            Filter::In(p, vs) => {
                self.leaf()?;
                if vs.len() > MAX_IN_VALUES {
                    return Err(Refusal::input(
                        "filter_too_large",
                        "/where",
                        "at most 100 values in an in filter",
                    )
                    .into());
                }
                self.comparison(p, vs, Op::Equality)
            }
            Filter::Lt(p, v) | Filter::Le(p, v) | Filter::Gt(p, v) | Filter::Ge(p, v) => {
                self.leaf()?;
                self.comparison(p, std::slice::from_ref(v), Op::Ordering)
            }
            Filter::Prefix(p, v) => {
                self.leaf()?;
                self.comparison(p, std::slice::from_ref(v), Op::Prefix)
            }
        }
    }
    fn leaf(&mut self) -> Result<(), Failure> {
        self.leaves += 1;
        if self.leaves > MAX_LEAVES {
            return Err(
                Refusal::input("filter_too_large", "/where", "at most 64 leaf conditions").into(),
            );
        }
        Ok(())
    }
    fn declared(&self, property: &str) -> Result<&ont_core::schema::PropertyDef, Failure> {
        self.set
            .properties(self.object_type)
            .and_then(|props| props.get(property))
            .ok_or_else(|| refuse_property("undeclared_property", property))
    }
    fn comparison(&mut self, property: &str, values: &[Value], op: Op) -> Result<(), Failure> {
        let def = self.declared(property)?;
        let base = base_of(self.schema, &def.ty);
        let allowed = match op {
            Op::Equality => true,
            Op::Ordering => matches!(
                base,
                Some(
                    ScalarType::String
                        | ScalarType::Integer
                        | ScalarType::Timestamp
                        | ScalarType::Date
                )
            ),
            Op::Prefix => base == Some(ScalarType::String),
        };
        if !allowed {
            return Err(refuse_property("invalid_filter_operator", property));
        }
        for value in values {
            if op == Op::Prefix && !value.is_string() {
                return Err(refuse_property("wrong_value_type", property));
            }
            // The ingest rule decides the type: every other property is
            // `missing`, so only this value is checked.
            let mut fields: BTreeMap<String, Knowledge> = self
                .set
                .properties(self.object_type)
                .map(|p| p.keys().map(|k| (k.clone(), Knowledge::Missing)).collect())
                .unwrap_or_default();
            fields.insert(property.to_string(), Knowledge::Value(value.clone()));
            if let Err(v) = self.set.validate_fields(self.object_type, &fields)
                && v.code == ont_core::schema::codes::WRONG_VALUE_TYPE
            {
                if matches!(def.ty, PropertyType::Ref(_)) && value.is_string() {
                    return Err(Refusal::input("wrong_value_type", "/where",
                        "filter value must satisfy the declared property type; prefix requires a string")
                        .with("hint", json!("A reference filter takes the full Id object with object_type, namespace and native_key; search on the referenced type returns it."))
                        .into());
                }
                return Err(refuse_property("wrong_value_type", property));
            }
        }
        self.used.insert(property.to_string());
        Ok(())
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Equality,
    Ordering,
    Prefix,
}

fn compare(a: &Value, b: &Value) -> Option<Ordering> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let wide = |n: &serde_json::Number| {
                n.as_i64()
                    .map(i128::from)
                    .or_else(|| n.as_u64().map(i128::from))
            };
            Some(wide(x)?.cmp(&wide(y)?))
        }
        (Value::String(x), Value::String(y)) => Some(x.cmp(y)),
        _ => None,
    }
}

fn eval(f: &Filter, fields: &BTreeMap<String, Knowledge>) -> Tri {
    let value_of = |p: &str| match fields.get(p) {
        Some(Knowledge::Value(v)) => Some(v),
        _ => None,
    };
    let order = |p: &String, v: &Value, ok: fn(Ordering) -> bool| match value_of(p) {
        None => Tri::Unknown,
        Some(have) => Tri::from(compare(have, v).is_some_and(ok)),
    };
    match f {
        Filter::Eq(p, v) => value_of(p).map_or(Tri::Unknown, |have| Tri::from(have == v)),
        Filter::Ne(p, v) => value_of(p).map_or(Tri::Unknown, |have| Tri::from(have != v)),
        Filter::In(p, vs) => value_of(p).map_or(Tri::Unknown, |have| Tri::from(vs.contains(have))),
        Filter::Lt(p, v) => order(p, v, |o| o == Ordering::Less),
        Filter::Le(p, v) => order(p, v, |o| o != Ordering::Greater),
        Filter::Gt(p, v) => order(p, v, |o| o == Ordering::Greater),
        Filter::Ge(p, v) => order(p, v, |o| o != Ordering::Less),
        Filter::Prefix(p, v) => match (value_of(p), v.as_str()) {
            (None, _) => Tri::Unknown,
            (Some(have), Some(prefix)) => {
                Tri::from(have.as_str().is_some_and(|s| s.starts_with(prefix)))
            }
            (Some(_), None) => Tri::False,
        },
        Filter::Unresolved(p) => Tri::from(value_of(p).is_none()),
        Filter::All(v) => {
            let mut out = Tri::True;
            for child in v {
                match eval(child, fields) {
                    Tri::False => return Tri::False,
                    Tri::Unknown => out = Tri::Unknown,
                    Tri::True => {}
                }
            }
            out
        }
        Filter::Any(v) => {
            let mut out = Tri::False;
            for child in v {
                match eval(child, fields) {
                    Tri::True => return Tri::True,
                    Tri::Unknown => out = Tri::Unknown,
                    Tri::False => {}
                }
            }
            out
        }
        Filter::NoneOf(v) => eval(&Filter::Any(v.clone()), fields).not(),
    }
}

fn encode_continuation(key: &str) -> String {
    let hex: String = key.bytes().map(|b| format!("{b:02x}")).collect();
    format!("{CONTINUATION_PREFIX}{hex}")
}
fn decode_continuation(token: &str) -> Option<String> {
    let hex = token.strip_prefix(CONTINUATION_PREFIX)?;
    if hex.is_empty() || hex.len() % 2 != 0 || !hex.is_ascii() {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect();
    String::from_utf8(bytes?).ok()
}

fn unavailable() -> Failure {
    Refusal::new(ont_core::Error::Unavailable.code()).into()
}

pub fn search(ctx: &Context, req: &SearchRequest) -> Result<Answer, Failure> {
    let limit = match req.limit {
        None => DEFAULT_LIMIT,
        Some(0) => {
            return Err(
                Refusal::input("invalid_request", "/limit", "integer from 1 to 1000").into(),
            );
        }
        Some(n) if n > MAX_LIMIT => {
            return Err(
                Refusal::input("limit_too_large", "/limit", "integer from 1 to 1000").into(),
            );
        }
        Some(n) => n,
    } as usize;
    let mut store = open_store(&ctx.store, false)?;
    let set = st!(store.schema_set()).clone();
    let principal = Principal::bind(&set)?;
    let revision = principal.revision()?;
    // A type that does not exist and one the principal may not read look the
    // same, before anything else about the request is judged.
    let Some(schema) = set.schema_of(&req.object_type) else {
        return Err(unavailable());
    };
    if !principal
        .view()
        .read_types(&schema.domain)
        .contains(&req.object_type)
    {
        return Err(unavailable());
    }
    let mut checker = Checker {
        set: &set,
        schema,
        object_type: &req.object_type,
        leaves: 0,
        used: BTreeSet::new(),
    };
    if let Some(f) = &req.filter {
        checker.walk(f, 0)?;
    }
    let used = checker.used;
    let after = match &req.continuation {
        None => None,
        Some(token) => {
            let key = decode_continuation(token)
                .ok_or_else(|| Failure::from(Refusal::input("invalid_continuation", "/continuation", "c1: followed by nonempty ASCII pairs of hexadecimal digits (a pair may start with '+'), decoding to UTF-8; use the previous result continuation")))?;
            Some(Id {
                object_type: req.object_type.clone(),
                namespace: schema.namespace.clone(),
                native_key: key,
            })
        }
    };
    let heads = st!(store.scan_heads(&req.object_type, SCAN_LIMIT, after.as_ref()));
    let admission = principal
        .authority()
        .admit(principal.session())
        .map_err(|_| Failure::Internal(internal!("authority_failure")))?;
    let ty = schema.object_types.get(&req.object_type);
    let source = ty.and_then(|t| match &t.owner {
        ont_core::schema::Owner::Source(s) => Some(s.clone()),
        ont_core::schema::Owner::Ontology => None,
    });
    let mut basis = Basis::default();
    let mut objects: Vec<Value> = Vec::new();
    let mut last_matched: Option<usize> = None;
    for (index, head) in heads.iter().enumerate() {
        let obs = &head.head.observation;
        let object = Object {
            id: head.id.clone(),
            scope: head.scope.clone(),
            head: obs.revision.clone(),
            observations: vec![obs.clone()],
        };
        let response = admission.query(
            &OneObject(Some(object)),
            Query::Get {
                id: head.id.clone(),
            },
            ctx.now,
        );
        let view = match response {
            Ok(Response::Ok {
                authorization_revision,
                result: Output::Objects { mut objects },
            }) if objects.len() == 1 => {
                // The same check as `get`: the answer's generation is the
                // one the principal was bound under.
                if authorization_revision != revision {
                    return Err(Failure::Internal(internal!("authority_generation_changed")));
                }
                objects.remove(0)
            }
            Ok(Response::Refused { .. }) | Err(ont_core::Error::Unavailable) => continue,
            _ => return Err(Failure::Internal(internal!("authority_failure"))),
        };
        let Some(observed) = view.observations.into_iter().next() else {
            return Err(Failure::Internal(internal!("authority_failure")));
        };
        basis.scanned += 1;
        let stale = observed.stale;
        let partial = obs.coverage == ont_core::Coverage::Partial;
        if stale {
            basis.stale += 1;
            if let Some(s) = &source {
                basis
                    .stale_sources
                    .insert(format!("{s}: ont index --source {s}"));
            }
        }
        if partial {
            basis.partial += 1;
        }
        let used_resolved = used
            .iter()
            .all(|p| matches!(obs.fields.get(p), Some(Knowledge::Value(_))));
        if !used_resolved {
            basis.used_unresolved = true;
        }
        let verdict = match &req.filter {
            None => Tri::True,
            Some(f) => eval(f, &obs.fields),
        };
        match verdict {
            Tri::False => {}
            Tri::Unknown => basis.unresolved_excluded += 1,
            Tri::True => {
                basis.matched += 1;
                objects.push(object_json(head, ty, &observed));
                last_matched = Some(index);
                if objects.len() == limit {
                    break;
                }
            }
        }
    }
    // More heads after the last one looked at?
    let scanned_all = heads.len() < SCAN_LIMIT;
    let continuation = if objects.len() == limit {
        let at = last_matched.unwrap_or(0);
        let more = at + 1 < heads.len() || !scanned_all;
        more.then(|| encode_continuation(&heads[at].id.native_key))
    } else {
        if !scanned_all {
            let id = &heads[heads.len() - 1].id;
            if !st!(store.scan_heads(&req.object_type, 1, Some(id))).is_empty() {
                return Err(Refusal::new("result_too_large").into());
            }
        }
        None
    };
    let qualified = basis.stale == 0
        && basis.partial == 0
        && basis.unresolved_excluded == 0
        && !basis.used_unresolved;
    let pins: BTreeMap<_, _> = set
        .pin(&schema.domain)
        .map(|p| (schema.domain.clone(), p.clone()))
        .into_iter()
        .collect();
    Ok(Answer::Enveloped {
        pins,
        revision,
        result: json!({
            "kind":"object_set",
            "objects":objects,
            "basis":{
                "scanned":basis.scanned,
                "matched":basis.matched,
                "unresolved_excluded":basis.unresolved_excluded,
                "stale":basis.stale,
                "partial":basis.partial,
                "qualified":qualified,
                "stale_sources":basis.stale_sources.into_iter().collect::<Vec<_>>()
            },
            "continuation":continuation
        }),
    })
}

#[derive(Default)]
struct Basis {
    scanned: u64,
    matched: u64,
    unresolved_excluded: u64,
    stale: u64,
    partial: u64,
    used_unresolved: bool,
    stale_sources: BTreeSet<String>,
}

fn object_json(
    head: &SchemaHead,
    ty: Option<&ont_core::schema::ObjectType>,
    observed: &ont_core::Observed,
) -> Value {
    let obs = &head.head.observation;
    let title = ty
        .and_then(|t| t.title.as_ref())
        .and_then(|p| obs.fields.get(p))
        .and_then(|k| match k {
            Knowledge::Value(Value::String(s)) => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_else(|| head.id.native_key.clone());
    json!({
        "id": head.id,
        "title": title,
        "head": observed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuation_round_trips_and_refuses_garbage() {
        let token = encode_continuation("task-é/1");
        assert_eq!(decode_continuation(&token).as_deref(), Some("task-é/1"));
        for bad in ["", "c1:", "c1:zz", "c1:abc", "x1:6161", "c1:ff"] {
            assert_eq!(decode_continuation(bad), None, "{bad}");
        }
    }

    #[test]
    fn three_valued_combinators() {
        let mut fields = BTreeMap::new();
        fields.insert("a".to_string(), Knowledge::Value(json!(5)));
        fields.insert("b".to_string(), Knowledge::Missing);
        let eq_a = Filter::Eq("a".into(), json!(5));
        let eq_b = Filter::Eq("b".into(), json!(1));
        assert!(eval(&eq_a, &fields) == Tri::True);
        assert!(eval(&eq_b, &fields) == Tri::Unknown);
        assert!(eval(&Filter::All(vec![eq_a.clone(), eq_b.clone()]), &fields) == Tri::Unknown);
        assert!(eval(&Filter::Any(vec![eq_a.clone(), eq_b.clone()]), &fields) == Tri::True);
        assert!(eval(&Filter::NoneOf(vec![eq_b.clone()]), &fields) == Tri::Unknown);
        assert!(eval(&Filter::NoneOf(vec![eq_a]), &fields) == Tri::False);
        assert!(eval(&Filter::Unresolved("b".into()), &fields) == Tri::True);
        assert!(eval(&Filter::Unresolved("a".into()), &fields) == Tri::False);
    }
}
