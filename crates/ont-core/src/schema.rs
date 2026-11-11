//! `ontology-schema/v1` model, semantic checks, pin and value validation.
//! Pure data: no source is read and no store is touched here.
use crate::{Error, Id, Knowledge, Namespace};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub const FORMAT: &str = "ontology-schema/v1";
const SOURCE_KINDS: &[&str] = &[
    "markdown_fields",
    "markdown_table",
    "file_digest",
    "git_local",
    "github_api",
    "json_records",
    "command",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub code: String,
    pub pointer: String,
}
impl Violation {
    fn new(code: &str, pointer: String) -> Self {
        Self {
            code: code.into(),
            pointer,
        }
    }
}
impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.code, self.pointer)
    }
}
fn esc(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}
pub fn valid_name(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScalarType {
    String,
    Integer,
    Boolean,
    Decimal,
    Timestamp,
    Date,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PropertyType {
    Scalar(ScalarType),
    Ref(RefType),
    Array(ArrayType),
    Struct(StructType),
    ValueType(ValueTypeRef),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefType {
    #[serde(rename = "ref")]
    pub target: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArrayType {
    pub array: Box<PropertyType>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructType {
    #[serde(rename = "struct")]
    pub struct_: BTreeMap<String, PropertyType>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueTypeRef {
    pub value_type: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropertyDef {
    #[serde(rename = "type")]
    pub ty: PropertyType,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueTypeDef {
    pub base: ScalarType,
    #[serde(rename = "enum", default, skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceDef {
    pub kind: String,
    #[serde(flatten)]
    pub options: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Owner {
    Ontology,
    Source(String),
}
impl Serialize for Owner {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Ontology => s.serialize_str("ontology"),
            Self::Source(name) => {
                #[derive(Serialize)]
                struct W<'a> {
                    source: &'a str,
                }
                W { source: name }.serialize(s)
            }
        }
    }
}
impl<'de> Deserialize<'de> for Owner {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum W {
            Text(String),
            Map { source: String },
        }
        match W::deserialize(d)? {
            W::Text(t) if t == "ontology" => Ok(Self::Ontology),
            W::Text(t) => Err(serde::de::Error::custom(format!(
                "invalid owner {t:?}, expected \"ontology\""
            ))),
            W::Map { source } => Ok(Self::Source(source)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrimaryKey {
    pub from: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectType {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub owner: Owner,
    pub primary_key: PrimaryKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub properties: BTreeMap<String, PropertyDef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cardinality {
    ManyToOne,
    ManyToMany,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkNames {
    pub forward: String,
    pub reverse: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkType {
    pub from: String,
    pub via: String,
    pub to: String,
    pub cardinality: Cardinality,
    pub names: LinkNames,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConditionGroup {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all: Option<Vec<Condition>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub any: Option<Vec<Condition>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub none: Option<Vec<Condition>>,
}
impl ConditionGroup {
    fn arms(&self) -> Vec<&Vec<Condition>> {
        [&self.all, &self.any, &self.none]
            .into_iter()
            .flatten()
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Condition {
    Exists(String),
    Fresh(String),
    Eq([Value; 2]),
    Ne([Value; 2]),
    In([Value; 2]),
    Lt([Value; 2]),
    Le([Value; 2]),
    Gt([Value; 2]),
    Ge([Value; 2]),
    Linked(LinkedConditionWire),
    Actor(Value),
    All(Vec<Condition>),
    Any(Vec<Condition>),
    #[serde(rename = "none")]
    NoneOf(Vec<Condition>),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkedConditionWire {
    pub from: String,
    pub link: String,
    #[serde(rename = "where", default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<ConditionGroup>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Criterion {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all: Option<Vec<Condition>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub any: Option<Vec<Condition>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub none: Option<Vec<Condition>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceRule {
    pub object_type: String,
    pub property: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_object: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modify_object: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_or_modify_object: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete_object: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_reference: Option<ReferenceRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clear_reference: Option<ReferenceRule>,
    #[serde(default)]
    pub set: BTreeMap<String, Value>,
}
impl ActionRule {
    fn object_target(&self) -> Option<&str> {
        self.create_object
            .as_deref()
            .or(self.modify_object.as_deref())
            .or(self.create_or_modify_object.as_deref())
            .or(self.delete_object.as_deref())
    }
    fn kind_count(&self) -> usize {
        [
            self.create_object.is_some(),
            self.modify_object.is_some(),
            self.create_or_modify_object.is_some(),
            self.delete_object.is_some(),
            self.set_reference.is_some(),
            self.clear_reference.is_some(),
        ]
        .into_iter()
        .filter(|b| *b)
        .count()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionType {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub parameters: BTreeMap<String, PropertyDef>,
    #[serde(default)]
    pub submission_criteria: Vec<Criterion>,
    #[serde(default)]
    pub rules: Vec<ActionRule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionDef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaGrant {
    pub principal: String,
    #[serde(default)]
    pub scope: BTreeMap<String, Value>,
    #[serde(default)]
    pub read: Vec<String>,
    #[serde(default)]
    pub derive: Vec<String>,
    #[serde(default)]
    pub actions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Schema {
    pub format: String,
    pub domain: String,
    pub version: u64,
    pub namespace: Namespace,
    #[serde(default)]
    pub scope_keys: Vec<String>,
    #[serde(default)]
    pub value_types: BTreeMap<String, ValueTypeDef>,
    #[serde(default)]
    pub sources: BTreeMap<String, SourceDef>,
    #[serde(default)]
    pub object_types: BTreeMap<String, ObjectType>,
    #[serde(default)]
    pub link_types: BTreeMap<String, LinkType>,
    #[serde(default)]
    pub action_types: BTreeMap<String, ActionType>,
    #[serde(default)]
    pub functions: BTreeMap<String, FunctionDef>,
    #[serde(default)]
    pub grants: Vec<SchemaGrant>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaPin {
    pub domain: String,
    pub version: u64,
    pub sha256: String,
}
impl SchemaPin {
    pub fn for_bytes(domain: &str, version: u64, bytes: &[u8]) -> Self {
        let mut h = Sha256::new();
        h.update(bytes);
        Self {
            domain: domain.into(),
            version,
            sha256: format!("{:x}", h.finalize()),
        }
    }
}

impl Schema {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        serde_json::from_slice(bytes).map_err(|_| Error::SchemaViolation)
    }
    pub fn pin(&self, bytes: &[u8]) -> SchemaPin {
        SchemaPin::for_bytes(&self.domain, self.version, bytes)
    }
    pub fn check(&self, other_domains_types: &BTreeSet<String>) -> Vec<Violation> {
        check(self, other_domains_types)
    }
    /// Parse, check, and pin over the exact bytes.
    pub fn check_bytes(
        bytes: &[u8],
        other_domains_types: &BTreeSet<String>,
    ) -> Result<SchemaPin, Vec<Violation>> {
        let schema: Self = serde_json::from_slice(bytes).map_err(|e| {
            let msg = e.to_string();
            let code = if msg.contains("unknown field") {
                "unknown_field"
            } else if msg.contains("duplicate field") {
                "duplicate_field"
            } else if msg.contains("missing field") {
                "missing_field"
            } else {
                "invalid_schema"
            };
            vec![Violation::new(code, String::new())]
        })?;
        let violations = check(&schema, other_domains_types);
        if violations.is_empty() {
            Ok(schema.pin(bytes))
        } else {
            Err(violations)
        }
    }

    pub fn validate_fields(
        &self,
        object_type: &str,
        fields: &BTreeMap<String, Knowledge>,
    ) -> Result<(), Violation> {
        let Some(ty) = self.object_types.get(object_type) else {
            return Err(Violation::new("unknown_type", String::new()));
        };
        for name in fields.keys() {
            if !ty.properties.contains_key(name) {
                return Err(Violation::new(
                    "undeclared_property",
                    format!("/fields/{}", esc(name)),
                ));
            }
        }
        for (name, prop) in &ty.properties {
            match fields.get(name) {
                None if prop.required => {
                    return Err(Violation::new(
                        "missing_required_property",
                        format!("/fields/{}", esc(name)),
                    ));
                }
                Some(Knowledge::Value(v)) => {
                    validate_value(self, &prop.ty, v)
                        .map_err(|code| Violation::new(code, format!("/fields/{}", esc(name))))?;
                    if let (Some(limit), Value::String(s)) = (prop.max_bytes, v)
                        && s.len() as u64 > limit
                    {
                        return Err(Violation::new(
                            "value_constraint_violation",
                            format!("/fields/{}", esc(name)),
                        ));
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// Semantic checks of DESIGN section 3 "Format and checks".
pub fn check(schema: &Schema, other_domains_types: &BTreeSet<String>) -> Vec<Violation> {
    let mut out: Vec<Violation> = Vec::new();
    let v = |code: &str, pointer: String, out: &mut Vec<Violation>| {
        out.push(Violation::new(code, pointer));
    };
    if schema.format != FORMAT {
        v("invalid_format", "/format".into(), &mut out);
    }
    if !valid_name(&schema.domain) {
        v("invalid_name", "/domain".into(), &mut out);
    }
    if schema.version == 0 {
        v("invalid_version", "/version".into(), &mut out);
    }
    if schema.namespace.instance.is_empty() || schema.namespace.account.is_empty() {
        v("invalid_namespace", "/namespace".into(), &mut out);
    }
    let mut seen_keys = BTreeSet::new();
    for (i, k) in schema.scope_keys.iter().enumerate() {
        if !valid_name(k) {
            v("invalid_name", format!("/scope_keys/{i}"), &mut out);
        }
        if !seen_keys.insert(k.clone()) {
            v("duplicate_scope_key", format!("/scope_keys/{i}"), &mut out);
        }
    }
    for (name, def) in &schema.value_types {
        let p = format!("/value_types/{}", esc(name));
        if !valid_name(name) {
            v("invalid_name", p.clone(), &mut out);
        }
        if let Some(vals) = &def.enum_values
            && (vals.is_empty() || vals.iter().any(|x| !scalar_matches(def.base, x)))
        {
            v("invalid_value_type_definition", p.clone(), &mut out);
        }
        if let Some(pat) = &def.pattern
            && Regex::compile(pat).is_none()
        {
            v("invalid_value_type_definition", p.clone(), &mut out);
        }
        if let (Some(a), Some(b)) = (&def.min, &def.max)
            && compare_values(a, b) == Some(std::cmp::Ordering::Greater)
        {
            v("invalid_value_type_definition", p, &mut out);
        }
    }
    for (name, src) in &schema.sources {
        if !valid_name(name) {
            v("invalid_name", format!("/sources/{}", esc(name)), &mut out);
        }
        if !SOURCE_KINDS.contains(&src.kind.as_str()) {
            v(
                "unknown_source_kind",
                format!("/sources/{}/kind", esc(name)),
                &mut out,
            );
        }
    }
    for (name, ty) in &schema.object_types {
        let base = format!("/object_types/{}", esc(name));
        if !valid_name(name) {
            v("invalid_name", base.clone(), &mut out);
        }
        if other_domains_types.contains(name) {
            v("duplicate_type_name", base.clone(), &mut out);
        }
        if let Owner::Source(s) = &ty.owner
            && !schema.sources.contains_key(s)
        {
            v("unknown_source", format!("{base}/owner"), &mut out);
        }
        let pk = &ty.primary_key.from;
        if pk != "$action_id" && !(pk.starts_with('/') && pk.len() > 1) {
            v(
                "invalid_primary_key",
                format!("{base}/primary_key/from"),
                &mut out,
            );
        }
        if let Some(title) = &ty.title
            && !ty.properties.contains_key(title)
        {
            v("unknown_property", format!("{base}/title"), &mut out);
        }
        for (pname, prop) in &ty.properties {
            let pp = format!("{base}/properties/{}", esc(pname));
            if !valid_name(pname) {
                v("invalid_name", pp.clone(), &mut out);
            }
            check_type(schema, &prop.ty, &pp, false, &mut out);
            match (&ty.owner, &prop.from) {
                (Owner::Ontology, Some(_)) => {
                    v("ontology_property_from", format!("{pp}/from"), &mut out)
                }
                (Owner::Source(_), None) if prop.required => {
                    v("required_property_without_from", pp.clone(), &mut out)
                }
                (_, Some(f)) if !f.starts_with('/') => {
                    v("invalid_from_pointer", format!("{pp}/from"), &mut out)
                }
                _ => {}
            }
        }
    }
    for name in schema.functions.keys() {
        if !valid_name(name) {
            v(
                "invalid_name",
                format!("/functions/{}", esc(name)),
                &mut out,
            );
        }
    }
    // Link types.
    let mut link_names: BTreeMap<String, &LinkType> = BTreeMap::new();
    for (name, lt) in &schema.link_types {
        let base = format!("/link_types/{}", esc(name));
        if !valid_name(name) {
            v("invalid_name", base.clone(), &mut out);
        }
        for (field, target) in [("from", &lt.from), ("to", &lt.to)] {
            if !schema.object_types.contains_key(target) {
                v("unknown_type", format!("{base}/{field}"), &mut out);
            }
        }
        for lname in [&lt.names.forward, &lt.names.reverse] {
            if !valid_name(lname) {
                v("invalid_name", format!("{base}/names"), &mut out);
            }
            if link_names.insert(lname.clone(), lt).is_some() {
                v("duplicate_link_name", format!("{base}/names"), &mut out);
            }
        }
        if let Some(from_ty) = schema.object_types.get(&lt.from) {
            match from_ty.properties.get(&lt.via) {
                None => v("unknown_property", format!("{base}/via"), &mut out),
                Some(prop) => {
                    let ok = match (&prop.ty, lt.cardinality) {
                        (PropertyType::Ref(r), Cardinality::ManyToOne) => r.target == lt.to,
                        (PropertyType::Array(a), Cardinality::ManyToMany) => {
                            matches!(&*a.array, PropertyType::Ref(r) if r.target == lt.to)
                        }
                        _ => false,
                    };
                    if !ok {
                        v("invalid_link_backing", format!("{base}/via"), &mut out);
                    }
                }
            }
        }
    }
    // Action types.
    for (name, at) in &schema.action_types {
        let base = format!("/action_types/{}", esc(name));
        if !valid_name(name) {
            v("invalid_name", base.clone(), &mut out);
        }
        for (pname, param) in &at.parameters {
            let pp = format!("{base}/parameters/{}", esc(pname));
            if !valid_name(pname) {
                v("invalid_name", pp.clone(), &mut out);
            }
            check_type(schema, &param.ty, &pp, false, &mut out);
            if param.from.is_some() {
                v("invalid_from_pointer", format!("{pp}/from"), &mut out);
            }
        }
        let mut codes = BTreeSet::new();
        for (ci, crit) in at.submission_criteria.iter().enumerate() {
            let cp = format!("{base}/submission_criteria/{ci}");
            if !valid_name(&crit.code) {
                v("invalid_name", format!("{cp}/code"), &mut out);
            }
            if !codes.insert(crit.code.clone()) {
                v("duplicate_criterion_code", format!("{cp}/code"), &mut out);
            }
            let arms: Vec<&Vec<Condition>> = [&crit.all, &crit.any, &crit.none]
                .into_iter()
                .flatten()
                .collect();
            if arms.len() != 1 {
                v("invalid_criterion", cp.clone(), &mut out);
                continue;
            }
            for cond in arms[0] {
                check_condition(schema, at, cond, None, &cp, &mut out);
            }
        }
        for (ri, rule) in at.rules.iter().enumerate() {
            let rp = format!("{base}/rules/{ri}");
            if rule.kind_count() != 1 {
                v("invalid_rule", rp.clone(), &mut out);
                continue;
            }
            if let Some(target) = rule.object_target() {
                match schema.object_types.get(target) {
                    None => v("unknown_type", rp.clone(), &mut out),
                    Some(ty) => {
                        if matches!(ty.owner, Owner::Source(_)) {
                            v("action_edits_source_owned", rp.clone(), &mut out);
                        }
                        for key in rule.set.keys() {
                            if !ty.properties.contains_key(key) {
                                v(
                                    "unknown_property",
                                    format!("{rp}/set/{}", esc(key)),
                                    &mut out,
                                );
                            }
                        }
                    }
                }
            }
            for val in rule.set.values() {
                scan_params(schema, at, val, &rp, &mut out);
            }
            for refr in [&rule.set_reference, &rule.clear_reference]
                .into_iter()
                .flatten()
            {
                let tp = format!("{rp}/reference");
                match schema.object_types.get(&refr.object_type) {
                    None => v("unknown_type", tp.clone(), &mut out),
                    Some(ty) => match ty.properties.get(&refr.property) {
                        None => v("unknown_property", tp.clone(), &mut out),
                        Some(p)
                            if !matches!(p.ty, PropertyType::Ref(_) | PropertyType::Array(_)) =>
                        {
                            v("invalid_reference_property", tp.clone(), &mut out)
                        }
                        _ => {}
                    },
                }
                if let Some(val) = &refr.value {
                    scan_params(schema, at, val, &tp, &mut out);
                }
            }
            if rule
                .clear_reference
                .as_ref()
                .is_some_and(|r| r.value.is_some())
            {
                v("invalid_rule", rp, &mut out);
            }
        }
    }
    for (gi, g) in schema.grants.iter().enumerate() {
        let gp = format!("/grants/{gi}");
        if g.principal.is_empty() {
            v("invalid_principal", format!("{gp}/principal"), &mut out);
        }
        for t in g.read.iter().chain(&g.derive) {
            if !schema.object_types.contains_key(t) {
                v("unknown_type", gp.clone(), &mut out);
            }
        }
        for a in &g.actions {
            if !schema.action_types.contains_key(a) {
                v("unknown_action", gp.clone(), &mut out);
            }
        }
    }
    out
}

fn check_type(
    schema: &Schema,
    ty: &PropertyType,
    pointer: &str,
    in_struct: bool,
    out: &mut Vec<Violation>,
) {
    match ty {
        PropertyType::Scalar(_) => {}
        PropertyType::Ref(r) => {
            if !schema.object_types.contains_key(&r.target) {
                out.push(Violation::new("unknown_type", pointer.into()));
            }
        }
        PropertyType::ValueType(vt) => {
            if !schema.value_types.contains_key(&vt.value_type) {
                out.push(Violation::new("unknown_value_type", pointer.into()));
            }
        }
        PropertyType::Array(a) => check_type(schema, &a.array, pointer, in_struct, out),
        PropertyType::Struct(s) => {
            if in_struct {
                out.push(Violation::new("struct_in_struct", pointer.into()));
            }
            for (fname, fty) in &s.struct_ {
                if !valid_name(fname) {
                    out.push(Violation::new("invalid_name", pointer.into()));
                }
                check_type(schema, fty, pointer, true, out);
            }
        }
    }
}

fn param_name(s: &str) -> Option<&str> {
    s.strip_prefix('$')
}
fn scan_params(
    schema: &Schema,
    at: &ActionType,
    val: &Value,
    pointer: &str,
    out: &mut Vec<Violation>,
) {
    let _ = schema;
    match val {
        Value::String(s) => {
            if let Some(p) = param_name(s)
                && !at.parameters.contains_key(p)
            {
                out.push(Violation::new("unknown_parameter", pointer.into()));
            }
        }
        Value::Array(items) => {
            for i in items {
                scan_params(schema, at, i, pointer, out);
            }
        }
        Value::Object(map) => {
            for i in map.values() {
                scan_params(schema, at, i, pointer, out);
            }
        }
        _ => {}
    }
}

fn check_condition(
    schema: &Schema,
    at: &ActionType,
    cond: &Condition,
    context: Option<&str>,
    pointer: &str,
    out: &mut Vec<Violation>,
) {
    match cond {
        Condition::Exists(s) | Condition::Fresh(s) => {
            if let Some(p) = param_name(s)
                && !at.parameters.contains_key(p)
            {
                out.push(Violation::new("unknown_parameter", pointer.into()));
            }
        }
        Condition::Eq(pair)
        | Condition::Ne(pair)
        | Condition::In(pair)
        | Condition::Lt(pair)
        | Condition::Le(pair)
        | Condition::Gt(pair)
        | Condition::Ge(pair) => {
            if let (Some(ctx), Value::String(first)) = (context, &pair[0])
                && !first.starts_with('$')
            {
                let known = schema
                    .object_types
                    .get(ctx)
                    .is_some_and(|t| t.properties.contains_key(first));
                if !known {
                    out.push(Violation::new("unknown_property", pointer.into()));
                }
            }
            for operand in pair {
                scan_params(schema, at, operand, pointer, out);
            }
        }
        Condition::Actor(val) => scan_params(schema, at, val, pointer, out),
        Condition::Linked(link) => {
            if let Some(p) = param_name(&link.from)
                && !at.parameters.contains_key(p)
            {
                out.push(Violation::new("unknown_parameter", pointer.into()));
            }
            let lt = schema
                .link_types
                .values()
                .find(|l| l.names.forward == link.link || l.names.reverse == link.link);
            let Some(lt) = lt else {
                out.push(Violation::new("unknown_link", pointer.into()));
                return;
            };
            // Resolve the type reached by following the named link.
            let from_ty = param_name(&link.from)
                .and_then(|p| at.parameters.get(p))
                .and_then(|param| match &param.ty {
                    PropertyType::Ref(r) => Some(r.target.as_str()),
                    _ => None,
                });
            let target = match from_ty {
                Some(ft) if lt.names.forward == link.link && lt.from == ft => Some(lt.to.as_str()),
                Some(ft) if lt.names.reverse == link.link && lt.to == ft => Some(lt.from.as_str()),
                _ => None,
            };
            if let Some(group) = &link.condition {
                let arms = group.arms();
                if arms.len() != 1 {
                    out.push(Violation::new("invalid_criterion", pointer.into()));
                    return;
                }
                for c in arms[0] {
                    check_condition(schema, at, c, target, pointer, out);
                }
            }
        }
        Condition::All(cs) | Condition::Any(cs) | Condition::NoneOf(cs) => {
            for c in cs {
                check_condition(schema, at, c, context, pointer, out);
            }
        }
    }
}

fn scalar_matches(base: ScalarType, v: &Value) -> bool {
    match base {
        ScalarType::String => v.is_string(),
        ScalarType::Integer | ScalarType::Timestamp => v.as_i64().is_some() || v.as_u64().is_some(),
        ScalarType::Boolean => v.is_boolean(),
        ScalarType::Decimal => v.as_str().is_some_and(|s| parse_decimal(s).is_some()),
        ScalarType::Date => v.as_str().is_some_and(valid_date),
    }
}

fn validate_value(schema: &Schema, ty: &PropertyType, v: &Value) -> Result<(), &'static str> {
    match ty {
        PropertyType::Scalar(s) => {
            if scalar_matches(*s, v) {
                Ok(())
            } else {
                Err("wrong_value_type")
            }
        }
        PropertyType::Ref(r) => {
            let id: Id = serde_json::from_value(v.clone()).map_err(|_| "wrong_value_type")?;
            if id.validate().is_err() || id.object_type != r.target {
                return Err("wrong_value_type");
            }
            Ok(())
        }
        PropertyType::Array(a) => {
            let Value::Array(items) = v else {
                return Err("wrong_value_type");
            };
            for item in items {
                validate_value(schema, &a.array, item)?;
            }
            Ok(())
        }
        PropertyType::Struct(s) => {
            let Value::Object(map) = v else {
                return Err("wrong_value_type");
            };
            for (fname, fty) in &s.struct_ {
                match map.get(fname) {
                    Some(fv) => validate_value(schema, fty, fv)?,
                    None => return Err("wrong_value_type"),
                }
            }
            if map.keys().any(|k| !s.struct_.contains_key(k)) {
                return Err("wrong_value_type");
            }
            Ok(())
        }
        PropertyType::ValueType(vt) => {
            let Some(def) = schema.value_types.get(&vt.value_type) else {
                return Err("wrong_value_type");
            };
            if !scalar_matches(def.base, v) {
                return Err("wrong_value_type");
            }
            if let Some(vals) = &def.enum_values
                && !vals.contains(v)
            {
                return Err("value_constraint_violation");
            }
            if let (ScalarType::String, Some(pat), Value::String(s)) = (def.base, &def.pattern, v) {
                match Regex::compile(pat) {
                    Some(re) if re.is_match(s) => {}
                    _ => return Err("value_constraint_violation"),
                }
            }
            if let Some(min) = &def.min
                && compare_values(v, min) == Some(std::cmp::Ordering::Less)
            {
                return Err("value_constraint_violation");
            }
            if let Some(max) = &def.max
                && compare_values(v, max) == Some(std::cmp::Ordering::Greater)
            {
                return Err("value_constraint_violation");
            }
            Ok(())
        }
    }
}

fn compare_values(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            if let (Some(xi), Some(yi)) = (x.as_i64(), y.as_i64()) {
                Some(xi.cmp(&yi))
            } else {
                x.as_f64()?.partial_cmp(&y.as_f64()?)
            }
        }
        (Value::String(x), Value::String(y)) => match (parse_decimal(x), parse_decimal(y)) {
            (Some(dx), Some(dy)) => Some(cmp_decimal(&dx, &dy)),
            _ => Some(x.cmp(y)),
        },
        _ => None,
    }
}

type DecimalParts = (bool, String, String);
fn parse_decimal(s: &str) -> Option<DecimalParts> {
    let (neg, rest) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let (int, frac) = match rest.split_once('.') {
        Some((i, f)) => (i, f),
        None => (rest, ""),
    };
    if int.is_empty() && frac.is_empty() {
        return None;
    }
    if !int.bytes().all(|b| b.is_ascii_digit()) || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((
        neg,
        int.trim_start_matches('0').to_string(),
        frac.trim_end_matches('0').to_string(),
    ))
}
fn cmp_decimal(a: &DecimalParts, b: &DecimalParts) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let zero = |d: &DecimalParts| d.1.is_empty() && d.2.is_empty();
    if zero(a) && zero(b) {
        return Ordering::Equal;
    }
    match (a.0, b.0) {
        (true, false) => return Ordering::Less,
        (false, true) => return Ordering::Greater,
        _ => {}
    }
    let mag =
        a.1.len()
            .cmp(&b.1.len())
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| {
                let n = a.2.len().max(b.2.len());
                let fa = format!("{:0<n$}", a.2);
                let fb = format!("{:0<n$}", b.2);
                fa.cmp(&fb)
            });
    if a.0 { mag.reverse() } else { mag }
}

fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let (Ok(y), Ok(m), Ok(d)) = (
        s[0..4].parse::<i64>(),
        s[5..7].parse::<u32>(),
        s[8..10].parse::<u32>(),
    ) else {
        return false;
    };
    if !(1..=12).contains(&m) || d == 0 {
        return false;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    d <= days[(m - 1) as usize]
}

/// Small backtracking regex (search semantics): literals, `.`, classes,
/// escapes `\d\w\s` (+negations), groups, alternation, `* + ? {n,m}`, `^ $`.
#[derive(Debug, Clone)]
enum Node {
    Char(char),
    Any,
    Class {
        neg: bool,
        ranges: Vec<(char, char)>,
    },
    Start,
    End,
    Seq(Vec<Node>),
    Alt(Vec<Node>),
    Repeat {
        node: Box<Node>,
        min: u32,
        max: Option<u32>,
    },
}
#[derive(Debug)]
struct Regex {
    root: Node,
}
impl Regex {
    fn compile(pattern: &str) -> Option<Self> {
        let chars: Vec<char> = pattern.chars().collect();
        let mut p = Parser {
            chars: &chars,
            pos: 0,
        };
        let root = p.alt()?;
        if p.pos != chars.len() {
            return None;
        }
        Some(Self { root })
    }
    fn is_match(&self, text: &str) -> bool {
        let chars: Vec<char> = text.chars().collect();
        (0..=chars.len()).any(|start| !match_node(&self.root, &chars, start).is_empty())
    }
}
struct Parser<'a> {
    chars: &'a [char],
    pos: usize,
}
impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }
    fn alt(&mut self) -> Option<Node> {
        let mut arms = vec![self.seq()?];
        while self.peek() == Some('|') {
            self.pos += 1;
            arms.push(self.seq()?);
        }
        if arms.len() == 1 {
            arms.pop()
        } else {
            Some(Node::Alt(arms))
        }
    }
    fn seq(&mut self) -> Option<Node> {
        let mut nodes = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let atom = self.atom()?;
            nodes.push(self.repeat(atom)?);
        }
        Some(Node::Seq(nodes))
    }
    fn repeat(&mut self, atom: Node) -> Option<Node> {
        let (min, max) = match self.peek() {
            Some('*') => {
                self.pos += 1;
                (0, None)
            }
            Some('+') => {
                self.pos += 1;
                (1, None)
            }
            Some('?') => {
                self.pos += 1;
                (0, Some(1))
            }
            Some('{') => {
                let save = self.pos;
                self.pos += 1;
                let mut min_s = String::new();
                while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    min_s.push(self.peek()?);
                    self.pos += 1;
                }
                if min_s.is_empty() {
                    self.pos = save;
                    return Some(atom);
                }
                let min: u32 = min_s.parse().ok()?;
                let max = if self.peek() == Some(',') {
                    self.pos += 1;
                    let mut max_s = String::new();
                    while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                        max_s.push(self.peek()?);
                        self.pos += 1;
                    }
                    if max_s.is_empty() {
                        None
                    } else {
                        Some(max_s.parse().ok()?)
                    }
                } else {
                    Some(min)
                };
                if self.peek() != Some('}') {
                    return None;
                }
                self.pos += 1;
                (min, max)
            }
            _ => return Some(atom),
        };
        Some(Node::Repeat {
            node: Box::new(atom),
            min,
            max,
        })
    }
    fn atom(&mut self) -> Option<Node> {
        match self.peek()? {
            '(' => {
                self.pos += 1;
                if self.chars[self.pos..].starts_with(&['?', ':']) {
                    self.pos += 2;
                }
                let inner = self.alt()?;
                if self.peek() != Some(')') {
                    return None;
                }
                self.pos += 1;
                Some(inner)
            }
            '[' => {
                self.pos += 1;
                let neg = self.peek() == Some('^');
                if neg {
                    self.pos += 1;
                }
                let mut ranges = Vec::new();
                let mut first = true;
                loop {
                    let c = self.peek()?;
                    if c == ']' && !first {
                        self.pos += 1;
                        break;
                    }
                    first = false;
                    self.pos += 1;
                    let lo = if c == '\\' {
                        let e = self.peek()?;
                        self.pos += 1;
                        match e {
                            'd' => {
                                ranges.push(('0', '9'));
                                continue;
                            }
                            'w' => {
                                ranges.extend([('a', 'z'), ('A', 'Z'), ('0', '9'), ('_', '_')]);
                                continue;
                            }
                            's' => {
                                ranges.extend([(' ', ' '), ('\t', '\t'), ('\n', '\n')]);
                                continue;
                            }
                            other => other,
                        }
                    } else {
                        c
                    };
                    if self.peek() == Some('-')
                        && self.chars.get(self.pos + 1).is_some_and(|n| *n != ']')
                    {
                        self.pos += 1;
                        let hi = self.peek()?;
                        self.pos += 1;
                        ranges.push((lo, hi));
                    } else {
                        ranges.push((lo, lo));
                    }
                }
                Some(Node::Class { neg, ranges })
            }
            '.' => {
                self.pos += 1;
                Some(Node::Any)
            }
            '^' => {
                self.pos += 1;
                Some(Node::Start)
            }
            '$' => {
                self.pos += 1;
                Some(Node::End)
            }
            '\\' => {
                self.pos += 1;
                let e = self.peek()?;
                self.pos += 1;
                Some(match e {
                    'd' => Node::Class {
                        neg: false,
                        ranges: vec![('0', '9')],
                    },
                    'D' => Node::Class {
                        neg: true,
                        ranges: vec![('0', '9')],
                    },
                    'w' => Node::Class {
                        neg: false,
                        ranges: vec![('a', 'z'), ('A', 'Z'), ('0', '9'), ('_', '_')],
                    },
                    'W' => Node::Class {
                        neg: true,
                        ranges: vec![('a', 'z'), ('A', 'Z'), ('0', '9'), ('_', '_')],
                    },
                    's' => Node::Class {
                        neg: false,
                        ranges: vec![(' ', ' '), ('\t', '\t'), ('\n', '\n'), ('\r', '\r')],
                    },
                    'S' => Node::Class {
                        neg: true,
                        ranges: vec![(' ', ' '), ('\t', '\t'), ('\n', '\n'), ('\r', '\r')],
                    },
                    other => Node::Char(other),
                })
            }
            other => {
                self.pos += 1;
                Some(Node::Char(other))
            }
        }
    }
}
fn match_node(node: &Node, text: &[char], pos: usize) -> BTreeSet<usize> {
    match node {
        Node::Char(c) => {
            if text.get(pos) == Some(c) {
                BTreeSet::from([pos + 1])
            } else {
                BTreeSet::new()
            }
        }
        Node::Any => {
            if pos < text.len() {
                BTreeSet::from([pos + 1])
            } else {
                BTreeSet::new()
            }
        }
        Node::Class { neg, ranges } => match text.get(pos) {
            Some(c) if ranges.iter().any(|(lo, hi)| lo <= c && c <= hi) != *neg => {
                BTreeSet::from([pos + 1])
            }
            _ => BTreeSet::new(),
        },
        Node::Start => {
            if pos == 0 {
                BTreeSet::from([pos])
            } else {
                BTreeSet::new()
            }
        }
        Node::End => {
            if pos == text.len() {
                BTreeSet::from([pos])
            } else {
                BTreeSet::new()
            }
        }
        Node::Seq(nodes) => {
            let mut cur = BTreeSet::from([pos]);
            for n in nodes {
                let mut next = BTreeSet::new();
                for p in cur {
                    next.extend(match_node(n, text, p));
                }
                cur = next;
                if cur.is_empty() {
                    break;
                }
            }
            cur
        }
        Node::Alt(arms) => arms.iter().flat_map(|a| match_node(a, text, pos)).collect(),
        Node::Repeat { node, min, max } => {
            let mut result = BTreeSet::new();
            let mut cur = BTreeSet::from([pos]);
            if *min == 0 {
                result.insert(pos);
            }
            let limit = max.unwrap_or(text.len() as u32 + 1);
            let mut count = 0u32;
            while count < limit {
                let mut next = BTreeSet::new();
                for p in &cur {
                    next.extend(match_node(node, text, *p));
                }
                // Zero-width repeats cannot progress; stop to avoid a loop.
                next.retain(|p| *p != pos || count >= *min);
                if next.is_empty() {
                    break;
                }
                count += 1;
                if count >= *min {
                    result.extend(&next);
                }
                cur = next;
            }
            result
        }
    }
}

/// Several checked domains held together; type names are unique across it.
#[derive(Debug, Clone, Default)]
pub struct SchemaSet {
    domains: BTreeMap<String, Schema>,
    pins: BTreeMap<String, SchemaPin>,
}
impl SchemaSet {
    pub fn new(schemas: Vec<Schema>) -> Result<Self, Vec<Violation>> {
        Self::build(schemas.into_iter().map(|s| (s, None)).collect())
    }
    pub fn with_pins(entries: Vec<(Schema, SchemaPin)>) -> Result<Self, Vec<Violation>> {
        Self::build(entries.into_iter().map(|(s, p)| (s, Some(p))).collect())
    }
    fn build(entries: Vec<(Schema, Option<SchemaPin>)>) -> Result<Self, Vec<Violation>> {
        let types_by_domain: Vec<(String, BTreeSet<String>)> = entries
            .iter()
            .map(|(s, _)| (s.domain.clone(), s.object_types.keys().cloned().collect()))
            .collect();
        let mut violations = Vec::new();
        let mut domains: BTreeMap<String, Schema> = BTreeMap::new();
        let mut pins = BTreeMap::new();
        for (schema, pin) in entries {
            if domains.contains_key(&schema.domain) {
                violations.push(Violation::new("duplicate_domain", "/domain".into()));
                continue;
            }
            let others: BTreeSet<String> = types_by_domain
                .iter()
                .filter(|(d, _)| d != &schema.domain)
                .flat_map(|(_, ts)| ts.iter().cloned())
                .collect();
            violations.extend(check(&schema, &others));
            if let Some(p) = pin {
                pins.insert(schema.domain.clone(), p);
            }
            domains.insert(schema.domain.clone(), schema);
        }
        if violations.is_empty() {
            Ok(Self { domains, pins })
        } else {
            Err(violations)
        }
    }
    pub fn schema(&self, domain: &str) -> Option<&Schema> {
        self.domains.get(domain)
    }
    pub fn pin(&self, domain: &str) -> Option<&SchemaPin> {
        self.pins.get(domain)
    }
    pub fn object_type(&self, name: &str) -> Option<&ObjectType> {
        self.domains.values().find_map(|s| s.object_types.get(name))
    }
    pub fn schema_of(&self, object_type: &str) -> Option<&Schema> {
        self.domains
            .values()
            .find(|s| s.object_types.contains_key(object_type))
    }
    pub fn properties(&self, object_type: &str) -> Option<&BTreeMap<String, PropertyDef>> {
        self.object_type(object_type).map(|t| &t.properties)
    }
    pub fn owner(&self, object_type: &str) -> Option<Owner> {
        self.object_type(object_type).map(|t| t.owner.clone())
    }
    pub fn link_types_from<'a>(&'a self, object_type: &str) -> Vec<(&'a String, &'a LinkType)> {
        self.domains
            .values()
            .flat_map(|s| &s.link_types)
            .filter(|(_, l)| l.from == object_type)
            .collect()
    }
    pub fn link_types_to<'a>(&'a self, object_type: &str) -> Vec<(&'a String, &'a LinkType)> {
        self.domains
            .values()
            .flat_map(|s| &s.link_types)
            .filter(|(_, l)| l.to == object_type)
            .collect()
    }
    pub fn validate_fields(
        &self,
        object_type: &str,
        fields: &BTreeMap<String, Knowledge>,
    ) -> Result<(), Violation> {
        match self.schema_of(object_type) {
            Some(s) => s.validate_fields(object_type, fields),
            None => Err(Violation::new("unknown_type", String::new())),
        }
    }
}
