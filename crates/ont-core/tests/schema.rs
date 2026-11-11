//! Synthetic `ontology-schema/v1` tests.
#![allow(clippy::unwrap_used)]
use ont_core::schema::{Owner, Schema, SchemaSet};
use ont_core::{Change, Coverage, Error, Knowledge, Lineage, Observation};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const EXAMPLE: &[u8] = include_bytes!("schema-fixtures/example.json");
const EXAMPLE_SHA256: &str = "1f7c885dc163d8d6ffbaceb573f178bc239d890069effe93c8f1626ee0132ad0";

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/schema-fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}
fn violations(name: &str, others: &[&str]) -> Vec<ont_core::schema::Violation> {
    let others: BTreeSet<String> = others.iter().map(|s| (*s).to_string()).collect();
    Schema::check_bytes(&fixture(name), &others).unwrap_err()
}

#[test]
fn example_parses_checks_clean_and_pin_matches_sha256sum() {
    let pin = Schema::check_bytes(EXAMPLE, &BTreeSet::new()).unwrap();
    assert_eq!(pin.domain, "work");
    assert_eq!(pin.version, 1);
    assert_eq!(pin.sha256, EXAMPLE_SHA256);
    let schema = Schema::parse(EXAMPLE).unwrap();
    assert!(schema.check(&BTreeSet::new()).is_empty());
    assert_eq!(schema.pin(EXAMPLE), pin);
}

#[test]
fn negative_fixtures_refuse_with_code_and_pointer() {
    let cases: &[(&str, &str, &str, &[&str])] = &[
        ("invalid_format.json", "invalid_format", "/format", &[]),
        ("unknown_field.json", "unknown_field", "", &[]),
        (
            "invalid_name.json",
            "invalid_name",
            "/object_types/Thing",
            &[],
        ),
        (
            "duplicate_type_name.json",
            "duplicate_type_name",
            "/object_types/thing",
            &["thing"],
        ),
        (
            "ontology_property_from.json",
            "ontology_property_from",
            "/object_types/thing/properties/name/from",
            &[],
        ),
        (
            "action_edits_source_owned.json",
            "action_edits_source_owned",
            "/action_types/act/rules/0",
            &[],
        ),
        (
            "unknown_type.json",
            "unknown_type",
            "/object_types/thing/properties/other",
            &[],
        ),
        (
            "unknown_property.json",
            "unknown_property",
            "/object_types/thing/title",
            &[],
        ),
        (
            "unknown_value_type.json",
            "unknown_value_type",
            "/object_types/thing/properties/name",
            &[],
        ),
        (
            "unknown_source.json",
            "unknown_source",
            "/object_types/thing/owner",
            &[],
        ),
        (
            "unknown_link.json",
            "unknown_link",
            "/action_types/act/submission_criteria/0",
            &[],
        ),
        (
            "unknown_parameter.json",
            "unknown_parameter",
            "/action_types/act/rules/0",
            &[],
        ),
        (
            "invalid_primary_key.json",
            "invalid_primary_key",
            "/object_types/thing/primary_key/from",
            &[],
        ),
        (
            "struct_in_struct.json",
            "struct_in_struct",
            "/object_types/thing/properties/name",
            &[],
        ),
        (
            "required_property_without_from.json",
            "required_property_without_from",
            "/object_types/thing/properties/name",
            &[],
        ),
    ];
    for (file, code, pointer, others) in cases {
        let found = violations(file, others);
        assert!(
            found
                .iter()
                .any(|x| &x.code == code && &x.pointer == pointer),
            "{file}: expected {code}@{pointer}, got {found:?}"
        );
    }
}

fn rich_schema() -> Schema {
    serde_json::from_value(json!({
        "format":"ontology-schema/v1","domain":"work","version":1,
        "namespace":{"instance":"vps://palantirkc","account":"root"},
        "scope_keys":[],
        "value_types":{"status":{"base":"string","enum":["active","lifted"],"pattern":"^a.*$"},
                         "score":{"base":"integer","min":0,"max":10}},
        "sources":{},
        "object_types":{
          "other":{"owner":"ontology","primary_key":{"from":"$action_id"},
            "properties":{"name":{"type":"string","required":true}}},
          "thing":{"owner":"ontology","primary_key":{"from":"$action_id"},
            "properties":{
              "s":{"type":"string","required":true},
              "i":{"type":"integer","required":true},
              "b":{"type":"boolean","required":true},
              "d":{"type":"decimal","required":true},
              "t":{"type":"timestamp","required":true},
              "day":{"type":"date","required":true},
              "r":{"type":{"ref":"other"},"required":true},
              "arr":{"type":{"array":"integer"},"required":true},
              "st":{"type":{"struct":{"x":"string","n":"integer"}},"required":true},
              "vt":{"type":{"value_type":"status"},"required":true},
              "num":{"type":{"value_type":"score"},"required":false}
            }}},
        "link_types":{},"action_types":{},"functions":{},"grants":[]
    }))
    .unwrap()
}
fn value_fields() -> BTreeMap<String, Knowledge> {
    let id = json!({"object_type":"other","namespace":{"instance":"vps://palantirkc","account":"root"},"native_key":"k1"});
    BTreeMap::from([
        ("s".into(), Knowledge::Value(json!("x"))),
        ("i".into(), Knowledge::Value(json!(3))),
        ("b".into(), Knowledge::Value(json!(true))),
        ("d".into(), Knowledge::Value(json!("1.50"))),
        ("t".into(), Knowledge::Value(json!(1700000000))),
        ("day".into(), Knowledge::Value(json!("2026-02-28"))),
        ("r".into(), Knowledge::Value(id)),
        ("arr".into(), Knowledge::Value(json!([1, 2]))),
        ("st".into(), Knowledge::Value(json!({"x":"a","n":1}))),
        ("vt".into(), Knowledge::Value(json!("active"))),
    ])
}

#[test]
fn value_validation_accepts_each_type_and_non_value_states() {
    let schema = rich_schema();
    assert!(
        schema.check(&BTreeSet::new()).is_empty(),
        "{:?}",
        schema.check(&BTreeSet::new())
    );
    assert_eq!(schema.validate_fields("thing", &value_fields()), Ok(()));
    for state in [
        Knowledge::Missing,
        Knowledge::Null,
        Knowledge::Blank,
        Knowledge::Unknown,
        Knowledge::NotPerformed,
    ] {
        let mut fields = value_fields();
        fields.insert("s".into(), state);
        assert_eq!(schema.validate_fields("thing", &fields), Ok(()));
    }
}

#[test]
fn value_validation_refusals() {
    let schema = rich_schema();
    let mut fields = value_fields();
    fields.insert("nope".into(), Knowledge::Value(json!(1)));
    assert_eq!(
        schema.validate_fields("thing", &fields).unwrap_err().code,
        "undeclared_property"
    );
    let mut fields = value_fields();
    fields.remove("s");
    assert_eq!(
        schema.validate_fields("thing", &fields).unwrap_err().code,
        "missing_required_property"
    );
    let mut fields = value_fields();
    fields.insert("i".into(), Knowledge::Value(json!("three")));
    assert_eq!(
        schema.validate_fields("thing", &fields).unwrap_err().code,
        "wrong_value_type"
    );
    let mut fields = value_fields();
    fields.insert("vt".into(), Knowledge::Value(json!("bogus")));
    assert_eq!(
        schema.validate_fields("thing", &fields).unwrap_err().code,
        "value_constraint_violation"
    );
    let mut fields = value_fields();
    fields.insert("num".into(), Knowledge::Value(json!(99)));
    assert_eq!(
        schema.validate_fields("thing", &fields).unwrap_err().code,
        "value_constraint_violation"
    );
}

#[test]
fn schema_set_answers_declaration_properties_owner_and_links() {
    let schema = Schema::parse(EXAMPLE).unwrap();
    let pin = schema.pin(EXAMPLE);
    let set = SchemaSet::with_pins(vec![(schema, pin.clone())]).unwrap();
    assert!(set.object_type("task").is_some());
    assert!(set.properties("hold").unwrap().contains_key("status"));
    assert_eq!(set.owner("task"), Some(Owner::Source("task_states".into())));
    assert_eq!(set.owner("hold"), Some(Owner::Ontology));
    assert_eq!(set.link_types_from("hold").len(), 1);
    assert_eq!(set.link_types_to("task").len(), 1);
    assert_eq!(set.link_types_from("task").len(), 0);
    assert_eq!(set.pin("work"), Some(&pin));
    assert_eq!(
        set.validate_fields("hold", &BTreeMap::new())
            .unwrap_err()
            .code,
        "missing_required_property"
    );
}

#[test]
fn duplicate_type_across_domains_refused_by_schema_set() {
    let a = Schema::parse(EXAMPLE).unwrap();
    let mut b = a.clone();
    b.domain = "other".into();
    let err = SchemaSet::new(vec![a, b]).unwrap_err();
    assert!(
        err.iter().any(|x| x.code == "duplicate_type_name"),
        "{err:?}"
    );
}

fn observation(change: Change, supersedes: Option<&str>, empty: bool) -> Observation {
    Observation {
        revision: "r2".into(),
        source_revision: "s2".into(),
        observed_at: 10,
        fresh_until: 20,
        change,
        supersedes: supersedes.map(str::to_string),
        coverage: Coverage::Complete,
        fields: if empty {
            BTreeMap::new()
        } else {
            BTreeMap::from([("x".into(), Knowledge::Value(json!(1)))])
        },
        lineage: Lineage {
            artifact_sha256: "0".repeat(64),
            pointer: "/0".into(),
        },
    }
}

#[test]
fn new_change_kinds_serialize_and_validate() {
    for (change, wire) in [
        (Change::Update, "update"),
        (Change::Refresh, "refresh"),
        (Change::Retraction, "retraction"),
    ] {
        assert_eq!(
            serde_json::to_value(&change).unwrap(),
            Value::String(wire.into())
        );
        assert_eq!(
            serde_json::from_value::<Change>(Value::String(wire.into())).unwrap(),
            change
        );
    }
    assert!(
        observation(Change::Update, Some("r1"), false)
            .validate()
            .is_ok()
    );
    assert!(
        observation(Change::Refresh, Some("r1"), false)
            .validate()
            .is_ok()
    );
    assert!(observation(Change::Update, None, false).validate().is_err());
    assert!(
        observation(Change::Refresh, None, false)
            .validate()
            .is_err()
    );
    assert!(
        observation(Change::Retraction, Some("r1"), true)
            .validate()
            .is_ok()
    );
    assert!(
        observation(Change::Retraction, Some("r1"), false)
            .validate()
            .is_err()
    );
    assert!(
        observation(Change::Retraction, None, true)
            .validate()
            .is_err()
    );
    assert_eq!(Error::SchemaViolation.code(), "schema_violation");
    assert_eq!(Schema::parse(b"{\"nope\":1}"), Err(Error::SchemaViolation));
}
