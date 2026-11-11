//! O17-R3 emitter-derived (code, pointer-class) diagnostic table.
//! Emitter lines refer to ont-core/src/schema.rs at parent 064e822.
//! Names and indices below are example bindings; recursive types/expressions
//! reuse their property's/parameter's/rule's class. Structural parser errors
//! are generic JSON-field classes, sampled at root, property and parameter.
//! Runtime value/field emitters are included separately from declarations.
#![allow(clippy::unwrap_used)]
use super::schema_input_refusal;
use ont_core::Knowledge;
use ont_core::schema::{Schema, SchemaSet};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn base(profile: &str) -> Value {
    let mut schema = json!({"format":"ontology-schema/v1","domain":"demo","version":1,"namespace":{"instance":"test","account":"root"},"sources":{"input":{"kind":"json_records"}},"object_types":{"item":{"owner":"ontology","primary_key":{"from":"$action_id"},"properties":{"label":{"type":"string"},"flag":{"type":"boolean"},"ref":{"type":{"ref":"item"}}}}}});
    if matches!(profile, "source" | "file_source" | "digest_source") {
        schema["object_types"]["item"]["owner"] = json!({"source":"input"});
        schema["object_types"]["item"]["primary_key"]["from"] = json!("/key");
        schema["object_types"]["item"]["properties"]["label"]["from"] = json!("/label");
    }
    if profile == "file_source" {
        schema["sources"]["input"] =
            json!({"kind":"markdown_fields","glob":"/tmp/*.md","fresh_for_s":1});
        schema["object_types"]["item"]["primary_key"]["from"] = json!("/path");
        schema["object_types"]["item"]["properties"]["label"]["from"] = json!("/stem");
    }
    if profile == "digest_source" {
        schema["sources"]["input"] =
            json!({"kind":"file_digest","paths":["/tmp/*.md"],"fresh_for_s":1});
        schema["object_types"]["item"]["primary_key"]["from"] = json!("/path");
        schema["object_types"]["item"]["properties"]["label"]["from"] = json!("/path");
    }
    if matches!(profile, "action" | "action_link") {
        schema["action_types"] = json!({"change":{"parameters":{"payload":{"type":"string"},"subject":{"type":{"ref":"item"}}},"rules":[]}});
    }
    if matches!(profile, "link" | "action_link") {
        schema["link_types"] = json!({"edge":{"from":"item","to":"item","via":"ref","cardinality":"many_to_one","names":{"forward":"next","reverse":"previous"}}});
    }
    if profile == "view" {
        schema["views"] = json!({"table":{"object_type":"item","order":[{"property":"label","direction":"asc"}],"columns":[{"header":"Label","property":"label"}]}});
    }
    schema
}
fn put(root: &mut Value, pointer: &str, value: Value) {
    let parts: Vec<_> = pointer
        .trim_start_matches('/')
        .split('/')
        .map(|s| s.replace("~1", "/").replace("~0", "~"))
        .collect();
    let mut at = root;
    for part in &parts[..parts.len() - 1] {
        if at.get(part).is_none() {
            at[part] = json!({});
        }
        at = at.get_mut(part).unwrap();
    }
    at[parts.last().unwrap()] = value;
}
fn diagnostic_case(
    code: &str,
    pointer: &str,
    profile: &str,
    patch: Value,
    expected: &str,
    forbidden: &[&str],
) {
    let mut schema = base(profile);
    assert!(
        Schema::check_bytes(schema.to_string().as_bytes(), &BTreeSet::new()).is_ok(),
        "baseline {profile}: {schema}"
    );
    for (path, value) in patch.as_object().unwrap() {
        if !path.starts_with('@') {
            put(&mut schema, path, value.clone());
        }
    }
    if let Some(paths) = patch["@remove"].as_array() {
        for path in paths {
            let (parent, key) = path.as_str().unwrap().rsplit_once('/').unwrap();
            if let Some(Value::Object(map)) = schema.pointer_mut(parent) {
                map.remove(key);
            }
        }
    }
    let mut bytes = schema.to_string().into_bytes();
    let others: BTreeSet<String> = patch["@others"]
        .as_array()
        .map(|a| a.iter().map(|s| s.as_str().unwrap().to_string()).collect())
        .unwrap_or_default();
    let violations = match patch["@mode"].as_str() {
        Some("duplicate") => {
            let path = patch["@duplicate"].as_str().unwrap();
            let key = path.rsplit('/').next().unwrap();
            let needle = format!("\"{key}\":{}", schema.pointer(path).unwrap());
            let text = String::from_utf8(bytes).unwrap();
            assert!(text.contains(&needle));
            bytes = text
                .replacen(&needle, &format!("{needle},{needle}"), 1)
                .into_bytes();
            Schema::check_bytes(&bytes, &others).unwrap_err()
        }
        Some("duplicate_view") => {
            let first = Schema::check_bytes(&bytes, &others).unwrap();
            let mut second = schema.clone();
            second["domain"] = json!("other");
            let object = second["object_types"]
                .as_object_mut()
                .unwrap()
                .remove("item")
                .unwrap();
            second["object_types"]["other"] = object;
            second["object_types"]["other"]["properties"]["ref"]["type"] = json!({"ref":"other"});
            second["views"]["table"]["object_type"] = json!("other");
            let second = Schema::check_bytes(second.to_string().as_bytes(), &others).unwrap();
            SchemaSet::new(vec![first, second]).unwrap_err()
        }
        Some(mode @ ("unknown_type" | "value" | "fields")) => {
            let checked = Schema::check_bytes(&bytes, &others).unwrap();
            let ty = checked.schema().object_types["item"].properties["label"]
                .ty
                .clone();
            let set = SchemaSet::new(vec![checked]).unwrap();
            let v = match mode {
                "unknown_type" => set
                    .validate_fields("missing", &BTreeMap::new())
                    .unwrap_err(),
                "value" => set
                    .validate_value("item", &ty, &patch["@value"])
                    .unwrap_err(),
                _ => set
                    .validate_fields(
                        "item",
                        &BTreeMap::from([(
                            "label".into(),
                            Knowledge::Value(patch["@value"].clone()),
                        )]),
                    )
                    .unwrap_err(),
            };
            vec![v]
        }
        _ => Schema::check_bytes(&bytes, &others).unwrap_err(),
    };
    let diagnostics: Vec<_> = violations
        .iter()
        .filter(|v| v.code == code)
        .map(|v| schema_input_refusal(&bytes, std::slice::from_ref(v)))
        .collect();
    let r = diagnostics
        .iter()
        .find(|r| r.extra["field"] == pointer)
        .unwrap_or_else(|| {
            panic!(
                "expected {code}@{pointer}; violations={violations:?}; diagnostics={diagnostics:?}"
            )
        });
    assert_eq!(r.code, "schema_violation");
    assert_eq!(r.extra["rule"], expected, "{code}@{pointer}");
    for word in forbidden {
        assert!(
            !r.extra["rule"].as_str().unwrap().contains(word),
            "{code}@{pointer}: {r:?}"
        );
    }
}
// One generated Rust test for every row. The row itself is the reviewable
// table: test name, code, class pointer, core emitter lines, text changed,
// baseline profile, minimal edits, exact true rule and forbidden assertions.
macro_rules! cases {
    ($( $name:ident => ($code:literal,$pointer:literal,$emitters:literal,$changed:literal,$profile:literal,$patch:expr,$rule:literal,$forbidden:expr); )*) => {
        $(#[test] fn $name(){diagnostic_case($code,$pointer,$profile,$patch,$rule,$forbidden);})*
    }
}
cases! {
    invalid_name_domain => ("invalid_name","/domain","1565",false,"plain",json!({"/domain": "Bad"}),"1 to 64 ASCII characters: lowercase letter first; then lowercase letters, digits or underscore",&["reserved"]);
    invalid_name_scope_key => ("invalid_name","/scope_keys/0","1588",false,"plain",json!({"/scope_keys": ["Bad"]}),"1 to 64 ASCII characters: lowercase letter first; then lowercase letters, digits or underscore",&["reserved"]);
    invalid_name_value_type => ("invalid_name","/value_types/Bad","1601",false,"plain",json!({"/value_types": {"Bad": {"base": "string"}}}),"1 to 64 ASCII characters: lowercase letter first; then lowercase letters, digits or underscore",&["reserved"]);
    invalid_name_source => ("invalid_name","/sources/Bad","1645",false,"plain",json!({"/sources": {"Bad": {"kind": "json_records"}}}),"1 to 64 ASCII characters: lowercase letter first; then lowercase letters, digits or underscore",&["reserved"]);
    invalid_name_object_type => ("invalid_name","/object_types/Bad","1668",false,"plain",json!({"/object_types/Bad": {"owner": "ontology", "primary_key": {"from": "$action_id"}}}),"1 to 64 ASCII characters: lowercase letter first; then lowercase letters, digits or underscore",&["reserved"]);
    invalid_name_property => ("invalid_name","/object_types/item/properties/Bad","1792",false,"plain",json!({"/object_types/item/properties/Bad": {"type": "string"}}),"1 to 64 ASCII characters: lowercase letter first; then lowercase letters, digits or underscore",&["reserved"]);
    invalid_name_function => ("invalid_name","/functions/Bad","1875",false,"plain",json!({"/functions": {"Bad": {}}}),"1 to 64 ASCII characters: lowercase letter first; then lowercase letters, digits or underscore",&["reserved"]);
    invalid_name_link_type => ("invalid_name","/link_types/Bad","1886",false,"plain",json!({"/link_types/Bad": {"from": "item", "to": "item", "via": "ref", "cardinality": "many_to_one", "names": {"forward": "next", "reverse": "previous"}}}),"1 to 64 ASCII characters: lowercase letter first; then lowercase letters, digits or underscore",&["reserved"]);
    invalid_name_link_forward => ("invalid_name","/link_types/edge/names/forward","1899",false,"link",json!({"/link_types/edge/names/forward": "Bad"}),"1 to 64 ASCII characters: lowercase letter first; then lowercase letters, digits or underscore",&["reserved"]);
    invalid_name_link_reverse => ("invalid_name","/link_types/edge/names/reverse","1899",false,"link",json!({"/link_types/edge/names/reverse": "Bad"}),"1 to 64 ASCII characters: lowercase letter first; then lowercase letters, digits or underscore",&["reserved"]);
    invalid_name_action_type => ("invalid_name","/action_types/Bad","1943",false,"plain",json!({"/action_types/Bad": {"rules": []}}),"1 to 64 ASCII characters: lowercase letter first; then lowercase letters, digits or underscore",&["reserved"]);
    invalid_name_criterion_code => ("invalid_name","/action_types/change/submission_criteria/0/code","1971",false,"action",json!({"/action_types/change/submission_criteria": [{"code": "Bad", "message": "test", "all": []}]}),"1 to 64 ASCII characters: lowercase letter first; then lowercase letters, digits or underscore",&["reserved"]);
    invalid_name_property_struct_field => ("invalid_name","/object_types/item/properties/label/type/struct/Bad","2170",false,"plain",json!({"/object_types/item/properties/label/type": {"struct": {"Bad": "string"}}}),"1 to 64 ASCII characters: lowercase letter first; then lowercase letters, digits or underscore",&["reserved"]);
    invalid_name_parameter_struct_field => ("invalid_name","/action_types/change/parameters/payload/type/struct/Bad","2170",false,"action",json!({"/action_types/change/parameters/payload/type": {"struct": {"Bad": "string"}}}),"1 to 64 ASCII characters: lowercase letter first; then lowercase letters, digits or underscore",&["reserved"]);
    invalid_name_view => ("invalid_name","/views/Bad","1146",false,"plain",json!({"/views/Bad": {"object_type": "item", "order": [{"property": "label", "direction": "asc"}], "columns": [{"header": "Label", "property": "label"}]}}),"1 to 64 ASCII characters: lowercase letter first; then lowercase letters, digits or underscore",&["reserved"]);
    invalid_name_parameter => ("invalid_name","/action_types/change/parameters/now","1948",false,"action",json!({"/action_types/change/parameters/now": {"type": "string"}}),"1 to 64 ASCII characters: lowercase letter first; then lowercase letters, digits or underscore; now is reserved for action parameters",&[]);
    invalid_namespace_instance => ("invalid_namespace","/namespace/instance","1579",true,"plain",json!({"/namespace/instance": ""}),"namespace component must be a nonempty string of at most 512 UTF-8 bytes",&[]);
    invalid_namespace_account => ("invalid_namespace","/namespace/account","1579",true,"plain",json!({"/namespace/account": ""}),"namespace component must be a nonempty string of at most 512 UTF-8 bytes",&[]);
    duplicate_type_object => ("duplicate_type_name","/object_types/item","1671",false,"plain",json!({"@others": ["item"]}),"name must be unique across installed object types and views",&[]);
    duplicate_type_view => ("duplicate_type_name","/views/table","2909",false,"view",json!({"@mode": "duplicate_view"}),"name must be unique across installed object types and views",&[]);
    duplicate_link_forward => ("duplicate_link_name","/link_types/second/names/forward","1906",true,"link",json!({"/link_types/second": {"from": "item", "to": "item", "via": "ref", "cardinality": "many_to_one", "names": {"forward": "next", "reverse": "other"}}}),"forward and reverse link names must be unique across this schema",&[]);
    duplicate_link_reverse => ("duplicate_link_name","/link_types/edge/names/reverse","1906",true,"link",json!({"/link_types/edge/names/reverse": "next"}),"forward and reverse link names must be unique across this schema",&[]);
    invalid_value_type_enum => ("invalid_value_type_definition","/value_types/word/enum","1607",true,"plain",json!({"/value_types": {"word": {"base": "string", "enum": []}}}),"enum must be nonempty and every value must match the declared scalar base",&[]);
    invalid_value_type_pattern => ("invalid_value_type_definition","/value_types/word/pattern","1614",true,"plain",json!({"/value_types": {"word": {"base": "string", "pattern": "["}}}),"pattern requires string base and a compilable whole-value regular expression",&[]);
    invalid_value_type_min => ("invalid_value_type_definition","/value_types/word/min","1624",true,"plain",json!({"/value_types": {"word": {"base": "string", "min": false}}}),"bound must match the non-boolean scalar base (decimal/date use their valid string forms)",&[]);
    invalid_value_type_max => ("invalid_value_type_definition","/value_types/word/max","1624,1636",true,"plain",json!({"/value_types": {"word": {"base": "string", "max": false}}}),"bound must match the non-boolean scalar base; when min and max are valid, max must be at least min",&[]);
    unknown_type_property_direct => ("unknown_type","/object_types/item/properties/label/type","2148",false,"plain",json!({"/object_types/item/properties/label/type": {"ref": "missing"}}),"object type name must be declared in this schema",&[]);
    unknown_type_property_array => ("unknown_type","/object_types/item/properties/label/type/array","2148",false,"plain",json!({"/object_types/item/properties/label/type": {"array": {"ref": "missing"}}}),"object type name must be declared in this schema",&[]);
    unknown_type_property_struct => ("unknown_type","/object_types/item/properties/label/type/struct/part","2148",false,"plain",json!({"/object_types/item/properties/label/type": {"struct": {"part": {"ref": "missing"}}}}),"object type name must be declared in this schema",&[]);
    unknown_value_type_property_direct => ("unknown_value_type","/object_types/item/properties/label/type","2153",false,"plain",json!({"/object_types/item/properties/label/type": {"value_type": "missing"}}),"value type name must be declared in schema value_types",&[]);
    unknown_value_type_property_array => ("unknown_value_type","/object_types/item/properties/label/type/array","2153",false,"plain",json!({"/object_types/item/properties/label/type": {"array": {"value_type": "missing"}}}),"value type name must be declared in schema value_types",&[]);
    unknown_value_type_property_struct => ("unknown_value_type","/object_types/item/properties/label/type/struct/part","2153",false,"plain",json!({"/object_types/item/properties/label/type": {"struct": {"part": {"value_type": "missing"}}}}),"value type name must be declared in schema value_types",&[]);
    struct_in_struct_property => ("struct_in_struct","/object_types/item/properties/label/type/struct/part","2165",true,"plain",json!({"/object_types/item/properties/label/type": {"struct": {"part": {"struct": {"leaf": "string"}}}}}),"a struct cannot contain another struct, including through an array",&[]);
    unknown_type_parameter_direct => ("unknown_type","/action_types/change/parameters/payload/type","2148",false,"action",json!({"/action_types/change/parameters/payload/type": {"ref": "missing"}}),"object type name must be declared in this schema",&[]);
    unknown_type_parameter_array => ("unknown_type","/action_types/change/parameters/payload/type/array","2148",false,"action",json!({"/action_types/change/parameters/payload/type": {"array": {"ref": "missing"}}}),"object type name must be declared in this schema",&[]);
    unknown_type_parameter_struct => ("unknown_type","/action_types/change/parameters/payload/type/struct/part","2148",false,"action",json!({"/action_types/change/parameters/payload/type": {"struct": {"part": {"ref": "missing"}}}}),"object type name must be declared in this schema",&[]);
    unknown_value_type_parameter_direct => ("unknown_value_type","/action_types/change/parameters/payload/type","2153",false,"action",json!({"/action_types/change/parameters/payload/type": {"value_type": "missing"}}),"value type name must be declared in schema value_types",&[]);
    unknown_value_type_parameter_array => ("unknown_value_type","/action_types/change/parameters/payload/type/array","2153",false,"action",json!({"/action_types/change/parameters/payload/type": {"array": {"value_type": "missing"}}}),"value type name must be declared in schema value_types",&[]);
    unknown_value_type_parameter_struct => ("unknown_value_type","/action_types/change/parameters/payload/type/struct/part","2153",false,"action",json!({"/action_types/change/parameters/payload/type": {"struct": {"part": {"value_type": "missing"}}}}),"value type name must be declared in schema value_types",&[]);
    struct_in_struct_parameter => ("struct_in_struct","/action_types/change/parameters/payload/type/struct/part","2165",true,"action",json!({"/action_types/change/parameters/payload/type": {"struct": {"part": {"struct": {"leaf": "string"}}}}}),"a struct cannot contain another struct, including through an array",&[]);
    unknown_type_view => ("unknown_type","/views/table/object_type","1162",false,"view",json!({"/views/table/object_type": "missing"}),"object type name must be declared in this schema",&[]);
    unknown_type_link_from => ("unknown_type","/link_types/edge/from","1890",false,"link",json!({"/link_types/edge/from": "missing"}),"object type name must be declared in this schema",&[]);
    unknown_type_link_to => ("unknown_type","/link_types/edge/to","1890",false,"link",json!({"/link_types/edge/to": "missing"}),"object type name must be declared in this schema",&[]);
    unknown_type_create_object => ("unknown_type","/action_types/change/rules/0/create_object","2003",false,"action",json!({"/action_types/change/rules": [{"create_object": "missing"}]}),"object type name must be declared in this schema",&[]);
    unknown_type_modify_object => ("unknown_type","/action_types/change/rules/0/modify_object","2003",false,"action",json!({"/action_types/change/rules": [{"modify_object": "missing"}]}),"object type name must be declared in this schema",&[]);
    unknown_type_create_or_modify_object => ("unknown_type","/action_types/change/rules/0/create_or_modify_object","2003",false,"action",json!({"/action_types/change/rules": [{"create_or_modify_object": "missing"}]}),"object type name must be declared in this schema",&[]);
    unknown_type_delete_object => ("unknown_type","/action_types/change/rules/0/delete_object","2003",false,"action",json!({"/action_types/change/rules": [{"delete_object": "missing"}]}),"object type name must be declared in this schema",&[]);
    unknown_type_set_reference => ("unknown_type","/action_types/change/rules/0/set_reference/object_type","2046",false,"action",json!({"/action_types/change/rules": [{"set_reference": {"object_type": "missing", "property": "ref"}}]}),"object type name must be declared in this schema",&[]);
    unknown_type_clear_reference => ("unknown_type","/action_types/change/rules/0/clear_reference/object_type","2046",false,"action",json!({"/action_types/change/rules": [{"clear_reference": {"object_type": "missing", "property": "ref"}}]}),"object type name must be declared in this schema",&[]);
    unknown_type_grant_read => ("unknown_type","/grants/0/read/0","2124",false,"plain",json!({"/grants": [{"principal": "agent", "read": ["missing"]}]}),"object type name must be declared in this schema",&[]);
    unknown_type_grant_derive => ("unknown_type","/grants/0/derive/0","2124",false,"plain",json!({"/grants": [{"principal": "agent", "derive": ["missing"]}]}),"object type name must be declared in this schema",&[]);
    unknown_type_runtime => ("unknown_type","","1316,2994,3006",false,"plain",json!({"@mode": "unknown_type"}),"object type name must be declared in this schema",&[]);
    unknown_property_view_column => ("unknown_property","/views/table/columns","1170",false,"view",json!({"/views/table/columns": [{"header": "Missing", "property": "missing"}]}),"property must be declared on the referenced object type; view columns and order also allow $key",&[]);
    unknown_property_view_order => ("unknown_property","/views/table/order","1175",false,"view",json!({"/views/table/order": [{"property": "missing", "direction": "asc"}]}),"property must be declared on the referenced object type; view columns and order also allow $key",&[]);
    unknown_property_title => ("unknown_property","/object_types/item/title","1787",false,"plain",json!({"/object_types/item/title": "missing"}),"property must be declared on the referenced object type; view columns and order also allow $key",&[]);
    unknown_property_link_via => ("unknown_property","/link_types/edge/via","1914",false,"link",json!({"/link_types/edge/via": "missing"}),"property must be declared on the referenced object type; view columns and order also allow $key",&[]);
    unknown_property_rule_set => ("unknown_property","/action_types/change/rules/0/set/missing","2011",false,"action",json!({"/action_types/change/rules": [{"create_object": "item", "set": {"missing": "literal"}}]}),"property must be declared on the referenced object type; view columns and order also allow $key",&[]);
    unknown_property_set_reference => ("unknown_property","/action_types/change/rules/0/set_reference/property","2056",false,"action",json!({"/action_types/change/rules": [{"set_reference": {"object_type": "item", "property": "missing"}}]}),"property must be declared on the referenced object type; view columns and order also allow $key",&[]);
    unknown_property_clear_reference => ("unknown_property","/action_types/change/rules/0/clear_reference/property","2056",false,"action",json!({"/action_types/change/rules": [{"clear_reference": {"object_type": "item", "property": "missing"}}]}),"property must be declared on the referenced object type; view columns and order also allow $key",&[]);
    unknown_property_rule_expression => ("unknown_property","/action_types/change/rules/0/set/label","2431",false,"action",json!({"/action_types/change/rules": [{"create_object": "item", "set": {"label": "$subject.missing"}}]}),"property must be declared on the referenced object type; view columns and order also allow $key",&[]);
    unknown_property_reference_expression => ("unknown_property","/action_types/change/rules/0/set_reference/value","2431",false,"action",json!({"/action_types/change/rules": [{"set_reference": {"object_type": "item", "property": "ref", "value": "$subject.missing"}}]}),"property must be declared on the referenced object type; view columns and order also allow $key",&[]);
    unknown_property_criterion_explicit => ("unknown_property","/action_types/change/submission_criteria/0/all/0/eq/0","2431",false,"action",json!({"/action_types/change/submission_criteria": [{"code": "test", "message": "test", "all": [{"eq": ["$subject.missing", "literal"]}]}]}),"property must be declared on the referenced object type; view columns and order also allow $key",&[]);
    unknown_property_criterion_bare => ("unknown_property","/action_types/change/submission_criteria/0/all/0/eq/0","2550",false,"action",json!({"/action_types/change/submission_criteria": [{"code": "test", "message": "test", "all": [{"eq": ["missing", "literal"]}]}]}),"property must be declared on the referenced object type; view columns and order also allow $key",&[]);
    unknown_property_criterion_linked => ("unknown_property","/action_types/change/submission_criteria/0/all/0/linked/where/all/0/eq/0","2521",false,"action_link",json!({"/action_types/change/submission_criteria": [{"code": "test", "message": "test", "all": [{"linked": {"from": "$subject", "link": "next", "where": {"all": [{"eq": ["missing", "literal"]}]}}}]}]}),"property must be declared on the referenced object type; view columns and order also allow $key",&[]);
    invalid_from_property => ("invalid_from_pointer","/object_types/item/properties/label/from","1866",true,"source",json!({"/object_types/item/properties/label/from": "bad"}),"property from must start with /",&[]);
    invalid_from_parameter => ("invalid_from_pointer","/action_types/change/parameters/payload/from","1953",true,"action",json!({"/action_types/change/parameters/payload/from": "bad"}),"from is forbidden on action parameters",&[]);
    invalid_template_primary_key => ("invalid_template","/object_types/item/primary_key/template","1735,1765,1530",true,"source",json!({"/object_types/item/primary_key/template": "{"}),"primary-key template requires source ownership and no from; at most 256 UTF-8 bytes and 8 placeholders; placeholders contain nonempty ASCII letters, digits or underscore with balanced braces",&[]);
    invalid_template_property => ("invalid_template","/object_types/item/properties/label/template","1812,1860,1530",true,"source",json!({"/object_types/item/properties/label/template": "{"}),"property template requires source ownership and no from; at most 256 UTF-8 bytes and 8 placeholders; placeholders contain nonempty ASCII letters, digits or underscore with balanced braces",&[]);
    invalid_template_parameter => ("invalid_template","/action_types/change/parameters/payload/template","1956",true,"action",json!({"/action_types/change/parameters/payload/template": "{"}),"template is forbidden on action parameters",&[]);
    invalid_transform_primary_key => ("invalid_transform","/object_types/item/primary_key/transform","1779",true,"source",json!({"/object_types/item/primary_key/transform": "parse_integer"}),"primary-key transform must be a supported string-producing form: first_word, {segment_after:string} or {replace_prefix:[from,to]}; parse_integer is forbidden. Requires source ownership, from or template, and from must not be $action_id; segment_after at most 1024 UTF-8 bytes starting and ending with /; replace_prefix nonempty from, both strings at most 1024 UTF-8 bytes",&["integer base","source-owned property"]);
    invalid_transform_parameter => ("invalid_transform","/action_types/change/parameters/payload/transform","1960",true,"action",json!({"/action_types/change/parameters/payload/transform": "first_word"}),"transform is forbidden on action parameters",&["source-owned property","integer base"]);
    invalid_transform_property => ("invalid_transform","/object_types/item/properties/label/transform","1847",false,"source",json!({"/object_types/item/properties/label/transform": "unlisted"}),"transform must be first_word, parse_integer, {segment_after:string} or {replace_prefix:[from,to]}; segment_after: at most 1024 UTF-8 bytes, starting and ending with '/'; replace_prefix: nonempty from, each string at most 1024 UTF-8 bytes. Requires a source-owned property with from or template; parse_integer requires integer base, other transforms require string base or ref",&[]);
    unknown_source_field_primary_key => ("unknown_source_field","/object_types/item/primary_key/from","1715,1725",false,"file_source",json!({"/object_types/item/primary_key/from": "/missing"}),"field must be produced by the declared source kind",&[]);
    unknown_source_field_property => ("unknown_source_field","/object_types/item/properties/label/from","1715,1725",false,"file_source",json!({"/object_types/item/properties/label/from": "/missing"}),"field must be produced by the declared source kind",&[]);
    unknown_template_field_primary_key => ("unknown_template_field","/object_types/item/primary_key/template","1540",false,"file_source",json!({"/object_types/item/primary_key/template": "{missing}", "@remove": ["/object_types/item/primary_key/from"]}),"field must be produced by the declared source kind",&[]);
    unknown_template_field_property => ("unknown_template_field","/object_types/item/properties/label/template","1540",false,"file_source",json!({"/object_types/item/properties/label/template": "{missing}", "@remove": ["/object_types/item/properties/label/from"]}),"field must be produced by the declared source kind",&[]);
    template_field_not_string_primary_key => ("template_field_not_string","/object_types/item/primary_key/template","1545",false,"file_source",json!({"/object_types/item/primary_key/template": "{size}", "@remove": ["/object_types/item/primary_key/from"]}),"template placeholder must name a string field produced by the declared source kind",&[]);
    template_field_not_string_property => ("template_field_not_string","/object_types/item/properties/label/template","1545",false,"file_source",json!({"/object_types/item/properties/label/template": "{size}", "@remove": ["/object_types/item/properties/label/from"]}),"template placeholder must name a string field produced by the declared source kind",&[]);
    reference_array_from => ("reference_array_from_source_unsupported","/object_types/item/properties/refs/from","1802",true,"source",json!({"/object_types/item/properties/refs": {"type": {"array": {"ref": "item"}}, "from": "/ref"}}),"source-owned arrays of references cannot use from or template",&[]);
    reference_array_template => ("reference_array_from_source_unsupported","/object_types/item/properties/refs/template","1802",true,"source",json!({"/object_types/item/properties/refs": {"type": {"array": {"ref": "item"}}, "template": "{path}"}}),"source-owned arrays of references cannot use from or template",&[]);
    invalid_link_via => ("invalid_link_backing","/link_types/edge/via","1927",true,"link",json!({"/link_types/edge/via": "label"}),"link backing must be a ref or array of refs whose target equals link to",&[]);
    invalid_link_cardinality => ("invalid_link_backing","/link_types/edge/cardinality","1930",true,"link",json!({"/link_types/edge/cardinality": "many_to_many"}),"ref backing requires many_to_one; array-of-ref backing requires many_to_many",&[]);
    invalid_schema_view_args => ("invalid_schema","/views/table/args","1150",true,"view",json!({"/views/table/args": ["Bad"]}),"view args must be distinct names of 1 to 64 ASCII characters, lowercase letter first then lowercase letters, digits or underscore",&[]);
    invalid_schema_view_limit => ("invalid_schema","/views/table/limit","1153",true,"view",json!({"/views/table/limit": 0}),"view limit must be an integer from 1 to 1000",&[]);
    invalid_schema_view_order => ("invalid_schema","/views/table/order","1156",true,"view",json!({"/views/table/order": []}),"view order must contain at least one entry",&[]);
    invalid_schema_view_columns => ("invalid_schema","/views/table/columns","1159,1167",true,"view",json!({"/views/table/columns": []}),"view columns must be nonempty and each header must exclude |, CR and LF",&[]);
    invalid_schema_view_filter => ("invalid_schema","/views/table/filter","1183",true,"view",json!({"/views/table/filter": {"bad": []}}),"view filter must use declared properties/args and supported operators/types, depth at most 8 (root 0), at most 64 leaves and at most 100 in values",&[]);
    invalid_schema_description_property => ("invalid_schema","/object_types/item/properties/label/description","2199",true,"plain",json!({"/object_types/item/properties/label/description": "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"}),"description must be at most 300 UTF-8 bytes",&[]);
    invalid_schema_description_parameter => ("invalid_schema","/action_types/change/parameters/payload/description","2199",true,"action",json!({"/action_types/change/parameters/payload/description": "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"}),"description must be at most 300 UTF-8 bytes",&[]);
    invalid_schema_serialized_field => ("invalid_schema","/version","1289",false,"plain",json!({"/version": false}),"expected u64",&[]);
    invalid_criterion_group => ("invalid_criterion","/action_types/change/submission_criteria/0","1986",true,"action",json!({"/action_types/change/submission_criteria": [{"code": "test", "message": "test"}]}),"criterion group must contain exactly one of all, any or none",&[]);
    invalid_criterion_linked_group => ("invalid_criterion","/action_types/change/submission_criteria/0/all/0/linked/where","2630",true,"action_link",json!({"/action_types/change/submission_criteria": [{"code": "test", "message": "test", "all": [{"linked": {"from": "$subject", "link": "next", "where": {}}}]}]}),"criterion group must contain exactly one of all, any or none",&[]);
    invalid_criterion_clock_operand => ("invalid_criterion","/action_types/change/submission_criteria/0/all/0/eq/1","2495,2507",true,"action",json!({"/action_types/change/submission_criteria": [{"code": "test", "message": "test", "all": [{"eq": ["$now", 1]}]}]}),"non-in comparison using builtin $now requires string-typed operands and $-prefixed declared parameter references; a bare property comparison without a linked context requires exactly one reference parameter",&[]);
    invalid_criterion_bare_operand => ("invalid_criterion","/action_types/change/submission_criteria/0/all/0/eq/0","2540",true,"action",json!({"@remove": ["/action_types/change/parameters/subject"], "/action_types/change/submission_criteria": [{"code": "test", "message": "test", "all": [{"eq": ["label", "literal"]}]}]}),"non-in comparison using builtin $now requires string-typed operands and $-prefixed declared parameter references; a bare property comparison without a linked context requires exactly one reference parameter",&[]);
    invalid_rule_kind => ("invalid_rule","/action_types/change/rules/0","1997",true,"action",json!({"/action_types/change/rules": [{}]}),"rule must select exactly one of create_object, modify_object, create_or_modify_object, delete_object, set_reference or clear_reference",&[]);
    invalid_rule_clear_value => ("invalid_rule","/action_types/change/rules/0/clear_reference/value","2088",true,"action",json!({"/action_types/change/rules": [{"clear_reference": {"object_type": "item", "property": "ref", "value": "literal"}}]}),"clear_reference must not supply value",&[]);
    invalid_rule_set_expression => ("invalid_rule","/action_types/change/rules/0/set/flag","2367,2376",true,"action",json!({"/action_types/change/rules": [{"create_object": "item", "set": {"flag": "$now"}}]}),"rule value using $now requires string base and an unrestricted or canonical UTC pattern; a referenced parameter property must have a compatible declared type/base; required properties on create/create_or_modify must not depend on optional parameters",&[]);
    invalid_rule_reference_expression => ("invalid_rule","/action_types/change/rules/0/set_reference/value","2367,2376",true,"action",json!({"/action_types/change/rules": [{"set_reference": {"object_type": "item", "property": "ref", "value": "$now"}}]}),"rule value using $now requires string base and an unrestricted or canonical UTC pattern; a referenced parameter property must have a compatible declared type/base; required properties on create/create_or_modify must not depend on optional parameters",&[]);
    invalid_rule_required_input => ("invalid_rule","/action_types/change/rules/0/set/label","2254",true,"action",json!({"/object_types/item/properties/label/required": true, "/action_types/change/rules": [{"create_object": "item", "set": {"label": "$payload"}}]}),"rule value using $now requires string base and an unrestricted or canonical UTC pattern; a referenced parameter property must have a compatible declared type/base; required properties on create/create_or_modify must not depend on optional parameters",&[]);
    action_edits_create_object => ("action_edits_source_owned","/action_types/change/rules/0/create_object","2006",true,"action",json!({"/object_types/item/owner": {"source": "input"}, "/object_types/item/primary_key/from": "/key", "/action_types/change/rules": [{"create_object": "item"}]}),"action rules may edit only ontology-owned object types",&[]);
    action_edits_modify_object => ("action_edits_source_owned","/action_types/change/rules/0/modify_object","2006",true,"action",json!({"/object_types/item/owner": {"source": "input"}, "/object_types/item/primary_key/from": "/key", "/action_types/change/rules": [{"modify_object": "item"}]}),"action rules may edit only ontology-owned object types",&[]);
    action_edits_create_or_modify_object => ("action_edits_source_owned","/action_types/change/rules/0/create_or_modify_object","2006",true,"action",json!({"/object_types/item/owner": {"source": "input"}, "/object_types/item/primary_key/from": "/key", "/action_types/change/rules": [{"create_or_modify_object": "item"}]}),"action rules may edit only ontology-owned object types",&[]);
    action_edits_delete_object => ("action_edits_source_owned","/action_types/change/rules/0/delete_object","2006",true,"action",json!({"/object_types/item/owner": {"source": "input"}, "/object_types/item/primary_key/from": "/key", "/action_types/change/rules": [{"delete_object": "item"}]}),"action rules may edit only ontology-owned object types",&[]);
    action_edits_set_reference => ("action_edits_source_owned","/action_types/change/rules/0/set_reference/object_type","2050",true,"action",json!({"/object_types/item/owner": {"source": "input"}, "/object_types/item/primary_key/from": "/key", "/action_types/change/rules": [{"set_reference": {"object_type": "item", "property": "ref"}}]}),"action rules may edit only ontology-owned object types",&[]);
    action_edits_clear_reference => ("action_edits_source_owned","/action_types/change/rules/0/clear_reference/object_type","2050",true,"action",json!({"/object_types/item/owner": {"source": "input"}, "/object_types/item/primary_key/from": "/key", "/action_types/change/rules": [{"clear_reference": {"object_type": "item", "property": "ref"}}]}),"action rules may edit only ontology-owned object types",&[]);
    unknown_parameter_rule_set => ("unknown_parameter","/action_types/change/rules/0/set/label","2427,2579,2592",false,"action",json!({"/action_types/change/rules": [{"create_object": "item", "set": {"label": "$missing"}}]}),"parameter reference must name a declared action parameter",&[]);
    unknown_parameter_reference_value => ("unknown_parameter","/action_types/change/rules/0/set_reference/value","2427,2579,2592",false,"action",json!({"/action_types/change/rules": [{"set_reference": {"object_type": "item", "property": "ref", "value": "$missing"}}]}),"parameter reference must name a declared action parameter",&[]);
    unknown_parameter_criterion => ("unknown_parameter","/action_types/change/submission_criteria/0/all/0/eq/0","2427,2579,2592",false,"action",json!({"/action_types/change/submission_criteria": [{"code": "test", "message": "test", "all": [{"eq": ["$missing", "literal"]}]}]}),"parameter reference must name a declared action parameter",&[]);
    reference_read_forbidden_rule_set => ("reference_read_forbidden","/action_types/change/rules/0/set/label","2298",true,"action",json!({"/action_types/change/rules": [{"create_object": "item", "set": {"label": "$subject.label"}}], "/grants": [{"principal": "agent", "actions": ["change"]}]}),"every grant allowing this action must also read the target type of a dereferenced parameter",&[]);
    reference_read_forbidden_reference_value => ("reference_read_forbidden","/action_types/change/rules/0/set_reference/value","2298",true,"action",json!({"/action_types/change/rules": [{"set_reference": {"object_type": "item", "property": "ref", "value": "$subject.label"}}], "/grants": [{"principal": "agent", "actions": ["change"]}]}),"every grant allowing this action must also read the target type of a dereferenced parameter",&[]);
    reference_read_forbidden_criterion => ("reference_read_forbidden","/action_types/change/submission_criteria/0/all/0/eq/0","2298",true,"action",json!({"/action_types/change/submission_criteria": [{"code": "test", "message": "test", "all": [{"eq": ["$subject.label", "literal"]}]}], "/grants": [{"principal": "agent", "actions": ["change"]}]}),"every grant allowing this action must also read the target type of a dereferenced parameter",&[]);
    unknown_parameter_exists => ("unknown_parameter","/action_types/change/submission_criteria/0/all/0/exists","2579",false,"action",json!({"/action_types/change/submission_criteria": [{"code": "test", "message": "test", "all": [{"exists": "$missing"}]}]}),"parameter reference must name a declared action parameter",&[]);
    unknown_parameter_fresh => ("unknown_parameter","/action_types/change/submission_criteria/0/all/0/fresh","2579",false,"action",json!({"/action_types/change/submission_criteria": [{"code": "test", "message": "test", "all": [{"fresh": "$missing"}]}]}),"parameter reference must name a declared action parameter",&[]);
    unknown_parameter_actor => ("unknown_parameter","/action_types/change/submission_criteria/0/all/0/actor","2427",false,"action",json!({"/action_types/change/submission_criteria": [{"code": "test", "message": "test", "all": [{"actor": "$missing"}]}]}),"parameter reference must name a declared action parameter",&[]);
    unknown_parameter_linked => ("unknown_parameter","/action_types/change/submission_criteria/0/all/0/linked/from","2592",false,"action_link",json!({"/action_types/change/submission_criteria": [{"code": "test", "message": "test", "all": [{"linked": {"from": "$missing", "link": "next"}}]}]}),"parameter reference must name a declared action parameter",&[]);
    invalid_source_option_glob => ("invalid_source_option","/sources/input/glob","723,729,737,741",false,"file_source",json!({"/sources/input/glob": []}),"markdown_fields allows only glob, exclude, head_lines, complete_listing, fresh_for_s; file_digest allows only paths, exclude, complete_listing, fresh_for_s. glob: required string or 1 to 16 strings; paths: required 1 to 256 strings; exclude: optional 0 to 256 strings; head_lines: optional integer 1 to 10000; complete_listing: optional boolean; fresh_for_s: required integer 1 to 315360000",&[]);
    invalid_source_option_paths => ("invalid_source_option","/sources/input/paths","701,704,710,746",false,"digest_source",json!({"/sources/input/paths": []}),"markdown_fields allows only glob, exclude, head_lines, complete_listing, fresh_for_s; file_digest allows only paths, exclude, complete_listing, fresh_for_s. glob: required string or 1 to 16 strings; paths: required 1 to 256 strings; exclude: optional 0 to 256 strings; head_lines: optional integer 1 to 10000; complete_listing: optional boolean; fresh_for_s: required integer 1 to 315360000",&[]);
    invalid_source_option_exclude => ("invalid_source_option","/sources/input/exclude","701,704,710",false,"file_source",json!({"/sources/input/exclude": false}),"markdown_fields allows only glob, exclude, head_lines, complete_listing, fresh_for_s; file_digest allows only paths, exclude, complete_listing, fresh_for_s. glob: required string or 1 to 16 strings; paths: required 1 to 256 strings; exclude: optional 0 to 256 strings; head_lines: optional integer 1 to 10000; complete_listing: optional boolean; fresh_for_s: required integer 1 to 315360000",&[]);
    invalid_source_option_head_lines => ("invalid_source_option","/sources/input/head_lines","752",false,"file_source",json!({"/sources/input/head_lines": 0}),"markdown_fields allows only glob, exclude, head_lines, complete_listing, fresh_for_s; file_digest allows only paths, exclude, complete_listing, fresh_for_s. glob: required string or 1 to 16 strings; paths: required 1 to 256 strings; exclude: optional 0 to 256 strings; head_lines: optional integer 1 to 10000; complete_listing: optional boolean; fresh_for_s: required integer 1 to 315360000",&[]);
    invalid_source_option_complete_listing => ("invalid_source_option","/sources/input/complete_listing","758",false,"file_source",json!({"/sources/input/complete_listing": 1}),"markdown_fields allows only glob, exclude, head_lines, complete_listing, fresh_for_s; file_digest allows only paths, exclude, complete_listing, fresh_for_s. glob: required string or 1 to 16 strings; paths: required 1 to 256 strings; exclude: optional 0 to 256 strings; head_lines: optional integer 1 to 10000; complete_listing: optional boolean; fresh_for_s: required integer 1 to 315360000",&[]);
    invalid_source_option_fresh_for_s => ("invalid_source_option","/sources/input/fresh_for_s","762",false,"file_source",json!({"/sources/input/fresh_for_s": 0}),"markdown_fields allows only glob, exclude, head_lines, complete_listing, fresh_for_s; file_digest allows only paths, exclude, complete_listing, fresh_for_s. glob: required string or 1 to 16 strings; paths: required 1 to 256 strings; exclude: optional 0 to 256 strings; head_lines: optional integer 1 to 10000; complete_listing: optional boolean; fresh_for_s: required integer 1 to 315360000",&[]);
    invalid_source_option_unlisted => ("invalid_source_option","/sources/input/unlisted","696",false,"file_source",json!({"/sources/input/unlisted": true}),"markdown_fields allows only glob, exclude, head_lines, complete_listing, fresh_for_s; file_digest allows only paths, exclude, complete_listing, fresh_for_s. glob: required string or 1 to 16 strings; paths: required 1 to 256 strings; exclude: optional 0 to 256 strings; head_lines: optional integer 1 to 10000; complete_listing: optional boolean; fresh_for_s: required integer 1 to 315360000",&[]);
    invalid_glob_glob => ("invalid_glob","/sources/input/glob","719",false,"file_source",json!({"/sources/input/glob": "relative"}),"absolute glob at most 4096 UTF-8 bytes; no NUL, empty, '.' or '..' segment, or { } [ ] \\; '*' and '?' wildcards allowed; '**' must occupy a whole segment, occur at most 8 times and not be the last segment",&[]);
    invalid_glob_glob_array => ("invalid_glob","/sources/input/glob/0","732",false,"file_source",json!({"/sources/input/glob": ["relative"]}),"absolute glob at most 4096 UTF-8 bytes; no NUL, empty, '.' or '..' segment, or { } [ ] \\; '*' and '?' wildcards allowed; '**' must occupy a whole segment, occur at most 8 times and not be the last segment",&[]);
    invalid_glob_paths => ("invalid_glob","/sources/input/paths/0","712",false,"digest_source",json!({"/sources/input/paths": ["relative"]}),"absolute glob at most 4096 UTF-8 bytes; no NUL, empty, '.' or '..' segment, or { } [ ] \\; '*' and '?' wildcards allowed; '**' must occupy a whole segment, occur at most 8 times and not be the last segment",&[]);
    invalid_glob_exclude => ("invalid_glob","/sources/input/exclude/0","712",false,"file_source",json!({"/sources/input/exclude": ["relative"]}),"absolute glob at most 4096 UTF-8 bytes; no NUL, empty, '.' or '..' segment, or { } [ ] \\; '*' and '?' wildcards allowed; '**' must occupy a whole segment, occur at most 8 times and not be the last segment",&[]);
    unknown_field_root => ("unknown_field","/unlisted","1283",false,"plain",json!({"/unlisted": true}),"field is not allowed in this object",&[]);
    unknown_field_property => ("unknown_field","/object_types/item/properties/label/unlisted","1283",false,"plain",json!({"/object_types/item/properties/label/unlisted": true}),"field is not allowed in this object",&[]);
    unknown_field_parameter => ("unknown_field","/action_types/change/parameters/payload/unlisted","1283",false,"action",json!({"/action_types/change/parameters/payload/unlisted": true}),"field is not allowed in this object",&[]);
    missing_field_root => ("missing_field","/format","1287",false,"plain",json!({"@remove": ["/format"]}),"required field",&[]);
    missing_field_property => ("missing_field","/object_types/item/properties/label/type","1287",true,"plain",json!({"@remove": ["/object_types/item/properties/label/type"]}),"required field",&[]);
    missing_field_parameter => ("missing_field","/action_types/change/parameters/payload/type","1287",true,"action",json!({"@remove": ["/action_types/change/parameters/payload/type"]}),"required field",&[]);
    duplicate_field_root => ("duplicate_field","","1265,1285",false,"plain",json!({"@mode": "duplicate", "@duplicate": "/format"}),"field must occur only once",&[]);
    duplicate_field_property => ("duplicate_field","/object_types/item/properties/label","1265,1285",false,"plain",json!({"@mode": "duplicate", "@duplicate": "/object_types/item/properties/label/type"}),"field must occur only once",&[]);
    duplicate_field_parameter => ("duplicate_field","/action_types/change/parameters/payload","1265,1285",false,"action",json!({"@mode": "duplicate", "@duplicate": "/action_types/change/parameters/payload/type"}),"field must occur only once",&[]);
    wrong_value_type_value => ("wrong_value_type","","1307,1336,1341,2706,2753,2758,2763,2768",true,"plain",json!({"@mode": "value", "@value": false}),"value must satisfy the supplied schema type",&[]);
    wrong_value_type_fields => ("wrong_value_type","/fields/label","1307,1336,1341,2706,2753,2758,2763,2768",true,"plain",json!({"@mode": "fields", "@value": false}),"value must satisfy the declared property type",&[]);
    value_constraint_violation_value => ("value_constraint_violation","","1307,1336,1341,2706,2753,2758,2763,2768",true,"plain",json!({"@mode": "value", "@value": "bad", "/value_types": {"word": {"base": "string", "enum": ["ok"]}}, "/object_types/item/properties/label/type": {"value_type": "word"}}),"value must satisfy the declared value-type enum/pattern/min/max constraints",&[]);
    value_constraint_violation_fields => ("value_constraint_violation","/fields/label","1307,1336,1341,2706,2753,2758,2763,2768",true,"plain",json!({"@mode": "fields", "@value": "bad", "/value_types": {"word": {"base": "string", "enum": ["ok"]}}, "/object_types/item/properties/label/type": {"value_type": "word"}}),"value must satisfy the declared value-type enum/pattern/min/max and property max_bytes constraints",&[]);
    invalid_reference_property_set_reference => ("invalid_reference_property","/action_types/change/rules/0/set_reference/property","2058",true,"action",json!({"/action_types/change/rules": [{"set_reference": {"object_type": "item", "property": "label"}}]}),"reference rule property must be a ref or array of refs",&[]);
    invalid_reference_property_clear_reference => ("invalid_reference_property","/action_types/change/rules/0/clear_reference/property","2058",true,"action",json!({"/action_types/change/rules": [{"clear_reference": {"object_type": "item", "property": "label"}}]}),"reference rule property must be a ref or array of refs",&[]);
    invalid_schema_property_type => ("invalid_schema","/object_types/item/properties/label/type","1289",false,"plain",json!({"/object_types/item/properties/label/type": "unlisted"}),"type must be string, integer, boolean, decimal, timestamp, date, {ref:type_name}, {array:type}, {struct:{field:type}} or {value_type:name}",&[]);
    invalid_schema_parameter_type => ("invalid_schema","/action_types/change/parameters/payload/type","1289",false,"action",json!({"/action_types/change/parameters/payload/type": "unlisted"}),"type must be string, integer, boolean, decimal, timestamp, date, {ref:type_name}, {array:type}, {struct:{field:type}} or {value_type:name}",&[]);
}
#[test]
fn primary_key_first_word_is_accepted() {
    let mut schema = base("source");
    schema["object_types"]["item"]["primary_key"]["transform"] = json!("first_word");
    assert!(Schema::check_bytes(schema.to_string().as_bytes(), &BTreeSet::new()).is_ok());
}
