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
    let checked = Schema::check_bytes(EXAMPLE, &BTreeSet::new()).unwrap();
    let pin = checked.pin();
    assert_eq!(pin.domain, "work");
    assert_eq!(pin.version, 1);
    assert_eq!(pin.sha256, EXAMPLE_SHA256);
    let schema = Schema::parse(EXAMPLE).unwrap();
    assert!(schema.check(&BTreeSet::new()).is_empty());
    assert_eq!(checked.schema(), &schema);
}

#[test]
fn negative_fixtures_refuse_with_exact_violations() {
    type FixtureCase = (
        &'static str,
        &'static [(&'static str, &'static str)],
        &'static [&'static str],
    );
    let cases: &[FixtureCase] = &[
        ("invalid_format.json", &[("invalid_format", "/format")], &[]),
        ("unknown_field.json", &[("unknown_field", "/bogus")], &[]),
        (
            "invalid_name.json",
            &[("invalid_name", "/object_types/Thing")],
            &[],
        ),
        (
            "duplicate_type_name.json",
            &[("duplicate_type_name", "/object_types/thing")],
            &["thing"],
        ),
        (
            "ontology_property_from.json",
            &[(
                "ontology_property_from",
                "/object_types/thing/properties/name/from",
            )],
            &[],
        ),
        (
            "action_edits_source_owned.json",
            &[(
                "action_edits_source_owned",
                "/action_types/act/rules/0/create_object",
            )],
            &[],
        ),
        (
            "reference_edits_source_owned.json",
            &[(
                "action_edits_source_owned",
                "/action_types/act/rules/0/set_reference/object_type",
            )],
            &[],
        ),
        (
            "owner_unknown_key.json",
            &[("invalid_schema", "/object_types/thing/owner")],
            &[],
        ),
        (
            "unknown_type.json",
            &[("unknown_type", "/object_types/thing/properties/other/type")],
            &[],
        ),
        (
            "unknown_property.json",
            &[("unknown_property", "/object_types/thing/title")],
            &[],
        ),
        (
            "unknown_value_type.json",
            &[(
                "unknown_value_type",
                "/object_types/thing/properties/name/type",
            )],
            &[],
        ),
        (
            "unknown_source.json",
            &[("unknown_source", "/object_types/thing/owner/source")],
            &[],
        ),
        (
            "unknown_link.json",
            &[(
                "unknown_link",
                "/action_types/act/submission_criteria/0/all/0/linked/link",
            )],
            &[],
        ),
        (
            "unknown_parameter.json",
            &[("unknown_parameter", "/action_types/act/rules/0/set/name")],
            &[],
        ),
        (
            "invalid_primary_key.json",
            &[(
                "invalid_primary_key",
                "/object_types/thing/primary_key/from",
            )],
            &[],
        ),
        (
            "struct_in_struct.json",
            &[(
                "struct_in_struct",
                "/object_types/thing/properties/name/type/struct/deep",
            )],
            &[],
        ),
        (
            "required_property_without_from.json",
            &[(
                "required_property_without_from",
                "/object_types/thing/properties/name",
            )],
            &[],
        ),
    ];
    for (file, expected, others) in cases {
        let found_vec = violations(file, others);
        let found: Vec<(&str, &str)> = found_vec
            .iter()
            .map(|x| (x.code, x.pointer.as_str()))
            .collect();
        assert_eq!(&found, expected, "{file}");
    }
}

fn base_value() -> Value {
    json!({
        "format":"ontology-schema/v1","domain":"work","version":1,
        "namespace":{"instance":"vps://palantirkc","account":"root"},
        "scope_keys":["class_key"],
        "value_types":{},
        "sources":{"src":{"kind":"json_records","path":"x.json"}},
        "object_types":{"thing":{"owner":{"source":"src"},"primary_key":{"from":"/id"},"title":"name",
            "properties":{"name":{"type":"string","required":true,"from":"/name"}}}},
        "link_types":{},"action_types":{},"functions":{},
        "grants":[{"principal":"agent","scope":{"class_key":"C1"},"read":["thing"],"derive":[],"actions":[]}]
    })
}
fn check_of(v: Value) -> Vec<ont_core::schema::Violation> {
    let schema: Schema = serde_json::from_value(v).unwrap();
    schema.check(&BTreeSet::new())
}
fn pairs(v: &[ont_core::schema::Violation]) -> Vec<(&str, &str)> {
    v.iter().map(|x| (x.code, x.pointer.as_str())).collect()
}

#[test]
fn one_test_per_check_code() {
    assert!(check_of(base_value()).is_empty());
    type Case = (Value, Vec<(&'static str, &'static str)>);
    let cases: Vec<Case> = {
        let mut v = Vec::new();
        let mut m = base_value();
        m["version"] = json!(0);
        v.push((m, vec![("invalid_version", "/version")]));
        let mut m = base_value();
        m["namespace"]["account"] = json!("");
        v.push((m, vec![("invalid_namespace", "/namespace/account")]));
        let mut m = base_value();
        m["domain"] = json!("program");
        v.push((m, vec![("reserved_domain", "/domain")]));
        let mut m = base_value();
        m["namespace"]["instance"] = json!("x".repeat(513));
        v.push((m, vec![("invalid_namespace", "/namespace/instance")]));
        let mut m = base_value();
        m["scope_keys"] = json!(["class_key", "class_key"]);
        v.push((m, vec![("duplicate_scope_key", "/scope_keys/1")]));
        let mut m = base_value();
        m["sources"]["src"]["kind"] = json!("bogus");
        v.push((m, vec![("unknown_source_kind", "/sources/src/kind")]));
        let mut m = base_value();
        m["grants"][0]["actions"] = json!(["missing"]);
        v.push((m, vec![("unknown_action", "/grants/0/actions/0")]));
        let mut m = base_value();
        m["grants"][0]["scope"] = json!({"bogus": "x"});
        v.push((m, vec![("unknown_scope_key", "/grants/0/scope/bogus")]));
        let mut m = base_value();
        m["grants"][0]["principal"] = json!("");
        v.push((m, vec![("invalid_principal", "/grants/0/principal")]));
        let mut m = base_value();
        m["object_types"]["thing"]["properties"]["name"]["from"] = json!("name");
        v.push((
            m,
            vec![(
                "invalid_from_pointer",
                "/object_types/thing/properties/name/from",
            )],
        ));
        let mut m = base_value();
        m["object_types"]["thing"]["properties"]["other"] =
            json!({"type":"string","required":false,"from":"/other"});
        m["link_types"] = json!({"l":{"from":"thing","via":"other","to":"thing",
            "cardinality":"many_to_one","names":{"forward":"to_thing","reverse":"from_thing"}}});
        v.push((m, vec![("invalid_link_backing", "/link_types/l/via")]));
        let mut m = base_value();
        m["object_types"]["thing"]["properties"]["other"] =
            json!({"type":{"ref":"thing"},"required":false,"from":"/other"});
        m["link_types"] = json!({"l":{"from":"thing","via":"other","to":"thing",
            "cardinality":"many_to_one","names":{"forward":"same","reverse":"same"}}});
        v.push((
            m,
            vec![("duplicate_link_name", "/link_types/l/names/reverse")],
        ));
        // Ontology-owned thing for rule/criterion cases.
        let owned = |m: &mut Value| {
            m["object_types"]["thing"]["owner"] = json!("ontology");
            m["object_types"]["thing"]["primary_key"] = json!({"from":"$action_id"});
            m["object_types"]["thing"]["properties"]["name"] =
                json!({"type":"string","required":true});
        };
        let mut m = base_value();
        owned(&mut m);
        m["action_types"] = json!({"act":{"parameters":{},
            "submission_criteria":[],"rules":[{"set_reference":{"object_type":"thing","property":"name"}}]}});
        v.push((
            m,
            vec![(
                "invalid_reference_property",
                "/action_types/act/rules/0/set_reference/property",
            )],
        ));
        let mut m = base_value();
        owned(&mut m);
        m["action_types"] = json!({"act":{"parameters":{"x":{"type":{"ref":"thing"},"required":true}},
            "submission_criteria":[
                {"code":"c1","message":"m","all":[{"exists":"$x"}],"any":[{"exists":"$x"}]}],
            "rules":[]}});
        v.push((
            m,
            vec![(
                "invalid_criterion",
                "/action_types/act/submission_criteria/0",
            )],
        ));
        let mut m = base_value();
        owned(&mut m);
        m["action_types"] = json!({"act":{"parameters":{"x":{"type":{"ref":"thing"},"required":true}},
            "submission_criteria":[
                {"code":"c1","message":"m","all":[{"exists":"$x"}]},
                {"code":"c1","message":"m","all":[{"exists":"$x"}]}],
            "rules":[]}});
        v.push((
            m,
            vec![(
                "duplicate_criterion_code",
                "/action_types/act/submission_criteria/1/code",
            )],
        ));
        let mut m = base_value();
        owned(&mut m);
        m["action_types"] = json!({"act":{"parameters":{},"submission_criteria":[],"rules":[{}]}});
        v.push((m, vec![("invalid_rule", "/action_types/act/rules/0")]));
        v
    };
    for (input, expected) in &cases {
        let found = check_of(input.clone());
        assert_eq!(&pairs(&found), expected, "input {input}");
        for (code, _) in expected {
            assert!(
                ont_core::schema::codes::ALL.contains(code),
                "code {code} not in codes::ALL"
            );
        }
    }
    // duplicate_domain via SchemaSet; missing_field via check_bytes.
    let checked = Schema::check_bytes(EXAMPLE, &BTreeSet::new()).unwrap();
    let err = SchemaSet::new(vec![checked.clone(), checked]).unwrap_err();
    assert_eq!(pairs(&err), vec![("duplicate_domain", "/domain")]);
    let mut obj = base_value();
    obj.as_object_mut().unwrap().remove("version");
    let err =
        Schema::check_bytes(&serde_json::to_vec(&obj).unwrap(), &BTreeSet::new()).unwrap_err();
    assert_eq!(pairs(&err), vec![("missing_field", "")]);
}

#[test]
fn value_type_definitions_checked_completely() {
    let cases: Vec<(Value, &str)> = vec![
        (
            {
                let mut m = base_value();
                m["value_types"] = json!({"vt":{"base":"integer","pattern":"[0-9]+"}});
                m
            },
            "/value_types/vt/pattern",
        ),
        (
            {
                let mut m = base_value();
                m["value_types"] = json!({"vt":{"base":"boolean","min":true}});
                m
            },
            "/value_types/vt/min",
        ),
        (
            {
                let mut m = base_value();
                m["value_types"] = json!({"vt":{"base":"integer","min":"0"}});
                m
            },
            "/value_types/vt/min",
        ),
        (
            {
                let mut m = base_value();
                m["value_types"] = json!({"vt":{"base":"integer","enum":["a"]}});
                m
            },
            "/value_types/vt/enum",
        ),
        (
            {
                let mut m = base_value();
                m["value_types"] = json!({"vt":{"base":"integer","min":10,"max":0}});
                m
            },
            "/value_types/vt/max",
        ),
    ];
    for (input, pointer) in cases {
        let found = check_of(input);
        assert_eq!(
            pairs(&found),
            vec![("invalid_value_type_definition", pointer)]
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
              "num":{"type":{"value_type":"score"},"required":false},
              "short":{"type":"string","required":false,"max_bytes":3}
            }}},
        "link_types":{},"action_types":{},"functions":{},"grants":[]
    }))
    .unwrap()
}
fn set_of(schema: Schema) -> SchemaSet {
    let bytes = serde_json::to_vec(&schema).unwrap();
    let checked = Schema::check_bytes(&bytes, &BTreeSet::new()).unwrap();
    SchemaSet::new(vec![checked]).unwrap()
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
    let set = set_of(schema);
    assert_eq!(set.validate_fields("thing", &value_fields()), Ok(()));
    // Accepted min/max-bounded value and max_bytes-conforming value.
    let mut fields = value_fields();
    fields.insert("num".into(), Knowledge::Value(json!(5)));
    fields.insert("short".into(), Knowledge::Value(json!("abc")));
    assert_eq!(set.validate_fields("thing", &fields), Ok(()));
    for state in [
        Knowledge::Missing,
        Knowledge::Null,
        Knowledge::Blank,
        Knowledge::Unknown,
        Knowledge::NotPerformed,
    ] {
        let mut fields = value_fields();
        fields.insert("s".into(), state);
        assert_eq!(set.validate_fields("thing", &fields), Ok(()));
    }
}

#[test]
fn value_validation_refusals() {
    let schema = set_of(rich_schema());
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
    // Decimal as a JSON number, timestamp as a string, bad dates.
    for (key, val) in [
        ("d", json!(1.5)),
        ("t", json!("1700000000")),
        ("day", json!("2026-02-30")),
        ("day", json!("2026-+1-+1")),
        ("d", json!("+1")),
        ("d", json!(".5")),
        ("d", json!("1.")),
    ] {
        let mut fields = value_fields();
        fields.insert(key.into(), Knowledge::Value(val.clone()));
        assert_eq!(
            schema.validate_fields("thing", &fields).unwrap_err().code,
            "wrong_value_type",
            "{key}={val}"
        );
    }
    // Ref with the wrong object_type, struct missing/extra field, bad array
    // item, and a max_bytes violation.
    let mut fields = value_fields();
    fields.insert(
        "r".into(),
        Knowledge::Value(
            json!({"object_type":"thing","namespace":{"instance":"vps://palantirkc","account":"root"},"native_key":"k1"}),
        ),
    );
    assert_eq!(
        schema.validate_fields("thing", &fields).unwrap_err().code,
        "wrong_value_type"
    );
    for st in [json!({"x":"a"}), json!({"x":"a","n":1,"z":2})] {
        let mut fields = value_fields();
        fields.insert("st".into(), Knowledge::Value(st));
        assert_eq!(
            schema.validate_fields("thing", &fields).unwrap_err().code,
            "wrong_value_type"
        );
    }
    let mut fields = value_fields();
    fields.insert("arr".into(), Knowledge::Value(json!([1, "x"])));
    assert_eq!(
        schema.validate_fields("thing", &fields).unwrap_err().code,
        "wrong_value_type"
    );
    let mut fields = value_fields();
    fields.insert("short".into(), Knowledge::Value(json!("toolong")));
    assert_eq!(
        schema.validate_fields("thing", &fields).unwrap_err().code,
        "value_constraint_violation"
    );
}

fn pattern_schema(pattern: &str) -> Schema {
    serde_json::from_value(json!({
        "format":"ontology-schema/v1","domain":"work","version":1,
        "namespace":{"instance":"vps://palantirkc","account":"root"},
        "value_types":{"code":{"base":"string","pattern":pattern}},
        "object_types":{"thing":{"owner":"ontology","primary_key":{"from":"$action_id"},
            "properties":{"code":{"type":{"value_type":"code"},"required":true}}}}
    }))
    .unwrap()
}

#[test]
fn pattern_must_match_the_whole_value() {
    let schema = set_of(pattern_schema("[a-z]+"));
    let fields = |v: &str| BTreeMap::from([("code".to_string(), Knowledge::Value(json!(v)))]);
    // Matching value.
    assert_eq!(schema.validate_fields("thing", &fields("abc")), Ok(()));
    // Non-matching value.
    assert_eq!(
        schema
            .validate_fields("thing", &fields("ABC"))
            .unwrap_err()
            .code,
        "value_constraint_violation"
    );
    // A value that matches only a part must refuse (whole-value semantics).
    assert_eq!(
        schema
            .validate_fields("thing", &fields("abc1"))
            .unwrap_err()
            .code,
        "value_constraint_violation"
    );
}

#[test]
fn pattern_alternation_anchors_and_flags_are_whole_value() {
    let fields = |v: &str| BTreeMap::from([("code".to_string(), Knowledge::Value(json!(v)))]);
    let refuse = |set: &SchemaSet, v: &str| {
        assert_eq!(
            set.validate_fields("thing", &fields(v)).unwrap_err().code,
            "value_constraint_violation",
            "value {v:?}"
        );
    };
    // Alternation: accepts exactly `a` or `b`, refuses partial matches.
    let set = set_of(pattern_schema("a|b"));
    assert_eq!(set.validate_fields("thing", &fields("a")), Ok(()));
    assert_eq!(set.validate_fields("thing", &fields("b")), Ok(()));
    refuse(&set, "ax");
    refuse(&set, "xb");
    // A pattern with its own anchors still behaves as a whole-value match.
    let set = set_of(pattern_schema("^a$"));
    assert_eq!(set.validate_fields("thing", &fields("a")), Ok(()));
    refuse(&set, "aa");
    refuse(&set, "xa");
    // An inline flag applies to the whole value.
    let set = set_of(pattern_schema("(?i)ab"));
    assert_eq!(set.validate_fields("thing", &fields("AB")), Ok(()));
    assert_eq!(set.validate_fields("thing", &fields("ab")), Ok(()));
    refuse(&set, "xab");
    refuse(&set, "abx");
}

#[test]
fn pattern_compiling_only_inside_the_wrapper_is_refused() {
    // Before the repair this compiled (as `^(?:a)|(b)$`) and `ax` matched.
    let schema = pattern_schema("a)|(b");
    let found = schema.check(&BTreeSet::new());
    assert_eq!(
        pairs(&found),
        vec![("invalid_value_type_definition", "/value_types/code/pattern")]
    );
    let bytes = serde_json::to_vec(&schema).unwrap();
    assert!(Schema::check_bytes(&bytes, &BTreeSet::new()).is_err());
}

#[test]
fn pattern_longer_than_limit_is_refused() {
    let schema = pattern_schema(&"a".repeat(1_025));
    let found = schema.check(&BTreeSet::new());
    assert_eq!(
        pairs(&found),
        vec![("invalid_value_type_definition", "/value_types/code/pattern")]
    );
    let ok = pattern_schema(&"a".repeat(1_024));
    assert!(ok.check(&BTreeSet::new()).is_empty());
}

#[test]
fn pattern_that_does_not_compile_refuses_at_check() {
    let schema = pattern_schema("(");
    let found = schema.check(&BTreeSet::new());
    assert!(
        found
            .iter()
            .any(|x| x.code == "invalid_value_type_definition"
                && x.pointer == "/value_types/code/pattern"),
        "{found:?}"
    );
}

#[test]
fn schema_set_answers_declaration_properties_owner_and_links() {
    let checked = Schema::check_bytes(EXAMPLE, &BTreeSet::new()).unwrap();
    let pin = checked.pin().clone();
    let set = SchemaSet::new(vec![checked]).unwrap();
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
    let a = Schema::check_bytes(EXAMPLE, &BTreeSet::new()).unwrap();
    let b_bytes = String::from_utf8(EXAMPLE.to_vec())
        .unwrap()
        .replacen("\"domain\": \"work\"", "\"domain\": \"other\"", 1)
        .into_bytes();
    let b = Schema::check_bytes(&b_bytes, &BTreeSet::new()).unwrap();
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

#[test]
fn duplicate_json_keys_are_refused_with_pointer() {
    let cases: &[(&[u8], &str)] = &[
        (
            br#"{"format":"ontology-schema/v1","domain":"work","version":1,"namespace":{"instance":"vps://palantirkc","account":"root"},"object_types":{"thing":{"owner":"ontology","primary_key":{"from":"$action_id"},"properties":{}},"thing":{"owner":"ontology","primary_key":{"from":"$action_id"},"properties":{}}}}"#,
            "/object_types/thing",
        ),
        // Inside an array of objects: the pointer carries the index.
        (
            br#"{"format":"ontology-schema/v1","domain":"work","version":1,"namespace":{"instance":"vps://palantirkc","account":"root"},"grants":[{"principal":"agent","principal":"agent"}]}"#,
            "/grants/0/principal",
        ),
        // A key containing `/` is escaped as `~1` in the pointer.
        (
            br#"{"format":"ontology-schema/v1","domain":"work","version":1,"namespace":{"instance":"vps://palantirkc","account":"root"},"sources":{"src":{"kind":"json_records","a/b":1,"a/b":2}}}"#,
            "/sources/src/a~1b",
        ),
        // A key containing `~` is escaped as `~0` in the pointer.
        (
            br#"{"format":"ontology-schema/v1","domain":"work","version":1,"namespace":{"instance":"vps://palantirkc","account":"root"},"sources":{"src":{"kind":"json_records","a~b":1,"a~b":2}}}"#,
            "/sources/src/a~0b",
        ),
        // An escaped duplicate: `"ab"` written with a unicode escape.
        (
            br#"{"format":"ontology-schema/v1","domain":"work","version":1,"namespace":{"instance":"vps://palantirkc","account":"root"},"sources":{"src":{"kind":"json_records","ab":1,"a\u0062":2}}}"#,
            "/sources/src/ab",
        ),
    ];
    for (bytes, pointer) in cases {
        let err = Schema::check_bytes(bytes, &BTreeSet::new()).unwrap_err();
        assert_eq!(err.len(), 1, "{err:?}");
        assert_eq!(err[0].code, "duplicate_field");
        assert_eq!(err[0].pointer, *pointer);
    }
}

#[test]
fn deeply_nested_document_is_refused_by_the_parser_not_the_scan() {
    // 100,000 opening brackets: the scan stops at its depth limit and
    // serde_json refuses; the test process must not abort.
    let bytes = vec![b'['; 100_000];
    let err = Schema::check_bytes(&bytes, &BTreeSet::new()).unwrap_err();
    assert_eq!(err.len(), 1, "{err:?}");
    assert!(
        [
            "invalid_schema",
            "unknown_field",
            "duplicate_field",
            "missing_field"
        ]
        .contains(&err[0].code),
        "parse-level code expected, got {err:?}"
    );
}

#[test]
fn valid_document_nested_100_deep_in_source_options_checks() {
    let mut nested = json!(1);
    for _ in 0..100 {
        nested = json!([nested]);
    }
    let mut doc = base_value();
    doc["sources"]["src"]["nested"] = nested;
    let bytes = serde_json::to_vec(&doc).unwrap();
    assert!(Schema::check_bytes(&bytes, &BTreeSet::new()).is_ok());
}
