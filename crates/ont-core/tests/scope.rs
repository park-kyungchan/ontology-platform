//! Synthetic literal compatibility and exact-domain authorization challenges.
#![allow(clippy::unwrap_used)]
use ont_core::action::{ActionInput, Basis, Review, TargetSnapshot};
use ont_core::{Authority, Error, Grant, Id, Namespace, Object, Policy, Query, Scope, Snapshot};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

const EDUCATION: &str = r#"{"namespace":{"instance":"synthetic://same-host","account":"A"},"class_key":"C1","student_key":"001"}"#;
const PROGRAM: &str = r#"{"namespace":{"instance":"synthetic://same-host","account":"A"},"domain":"program","program_key":"001"}"#;
const OBSERVATION: &str = r#"{"revision":"r1","source_revision":"s1","observed_at":10,"fresh_until":20,"change":"initial","supersedes":null,"coverage":"complete","fields":{"physical_record":{"kind":"value","value":"R1"},"recommendation":{"kind":"value","value":"next"}},"lineage":{"artifact_sha256":"0000000000000000000000000000000000000000000000000000000000000000","pointer":"/0"}}"#;
const ID: &str = r#"{"object_type":"progress","namespace":{"instance":"synthetic://same-host","account":"A"},"native_key":"001"}"#;

fn namespace() -> Namespace {
    Namespace {
        instance: "synthetic://same-host".into(),
        account: "A".into(),
    }
}

#[test]
fn exact_legacy_and_program_bytes_and_old_reader_refusal() {
    let education = Scope::education(namespace(), "C1", "001").unwrap();
    let program = Scope::program(namespace(), "001").unwrap();
    for (scope, literal) in [(education, EDUCATION), (program, PROGRAM)] {
        assert_eq!(serde_json::to_string(&scope).unwrap(), literal);
        assert_eq!(serde_json::from_str::<Scope>(literal).unwrap(), scope);
        assert_eq!(scope.namespace(), &namespace());
    }
    // This is the unchanged S06 Scope reader shape, including strict fields.
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct LegacyScope {
        namespace: Namespace,
        class_key: String,
        student_key: String,
    }
    let old = serde_json::from_str::<LegacyScope>(EDUCATION).unwrap();
    assert_eq!(old.namespace, namespace());
    assert_eq!(
        (old.class_key.as_str(), old.student_key.as_str()),
        ("C1", "001")
    );
    assert!(serde_json::from_str::<LegacyScope>(PROGRAM).is_err());
    assert_eq!(
        serde_json::from_str::<Scope>(r#"{"program_key":"001","domain":"program","namespace":{"account":"A","instance":"synthetic://same-host"}}"#).unwrap(),
        Scope::program(namespace(), "001").unwrap()
    );
}

#[test]
fn rejects_duplicate_mixed_unknown_missing_null_and_invalid_fields() {
    // Raw JSON is essential: a Value intermediary would erase duplicate keys.
    let ns = r#""namespace":{"instance":"synthetic://same-host","account":"A"}"#;
    let education = r#""class_key":"C1","student_key":"001""#;
    let program = r#""domain":"program","program_key":"001""#;
    let mut invalid = vec!["null".into(), "[]".into(), "{}".into()];
    for fields in [education, program] {
        invalid.push(format!("{{{fields}}}"));
        invalid.push(format!("{{{ns},{ns},{fields}}}"));
        invalid.push(format!("{{{ns},{fields},\"unknown\":0}}"));
        invalid.push(format!("{{{ns},{fields},\"unknown\":null}}"));
        for bad_ns in [
            "null".into(),
            "[]".into(),
            r#"["s","A"]"#.into(),
            r#"{"instance":"","account":"A"}"#.into(),
            r#"{"instance":"s","account":""}"#.into(),
            r#"{"instance":"s"}"#.into(),
            r#"{"account":"A"}"#.into(),
            r#"{"instance":"s","account":"A","extra":0}"#.into(),
            r#"{"instance":"s","instance":"s","account":"A"}"#.into(),
            r#"{"instance":"s","account":"A","account":"A"}"#.into(),
            format!(r#"{{"instance":"{}","account":"A"}}"#, "x".repeat(513)),
            format!(r#"{{"instance":"s","account":"{}"}}"#, "x".repeat(513)),
        ] {
            invalid.push(format!("{{\"namespace\":{bad_ns},{fields}}}"));
        }
    }
    for duplicate in [r#""class_key":"C1""#, r#""student_key":"001""#] {
        invalid.push(format!("{{{ns},{education},{duplicate}}}"));
    }
    for duplicate in [r#""domain":"program""#, r#""program_key":"001""#] {
        invalid.push(format!("{{{ns},{program},{duplicate}}}"));
    }
    for fields in [
        r#""class_key":"C1""#,
        r#""student_key":"001""#,
        r#""class_key":"","student_key":"001""#,
        r#""class_key":"C1","student_key":"""#,
        r#""class_key":null,"student_key":"001""#,
        r#""class_key":"C1","student_key":null"#,
        r#""class_key":1,"student_key":"001""#,
        r#""domain":"program""#,
        r#""program_key":"001""#,
        r#""domain":"other","program_key":"001""#,
        r#""domain":"Program","program_key":"001""#,
        r#""domain":"","program_key":"001""#,
        r#""domain":null,"program_key":"001""#,
        r#""domain":"program","program_key":null"#,
        r#""domain":"program","program_key":"""#,
        r#""domain":"program","program_key":1"#,
    ] {
        invalid.push(format!("{{{ns},{fields}}}"));
    }
    // Mixed sets must fail in both directions, including null-valued extras.
    for extra in [
        program,
        r#""domain":"program""#,
        r#""domain":"other""#,
        r#""domain":null"#,
        r#""program_key":"001""#,
        r#""program_key":null"#,
    ] {
        invalid.push(format!("{{{ns},{education},{extra}}}"));
    }
    for extra in [
        education,
        r#""class_key":"C1""#,
        r#""student_key":"001""#,
        r#""class_key":null"#,
        r#""student_key":null"#,
    ] {
        invalid.push(format!("{{{ns},{program},{extra}}}"));
    }
    for raw in invalid {
        assert!(
            serde_json::from_str::<Scope>(&raw).is_err(),
            "accepted {raw}"
        );
    }
}

#[test]
fn constructors_validate_without_normalizing_identity() {
    assert_eq!(
        Scope::education(namespace(), "", "001"),
        Err(Error::InvalidRequest)
    );
    assert_eq!(
        Scope::education(namespace(), "C1", ""),
        Err(Error::InvalidRequest)
    );
    assert_eq!(Scope::program(namespace(), ""), Err(Error::InvalidRequest));
    for ns in [
        Namespace {
            instance: "".into(),
            ..namespace()
        },
        Namespace {
            account: "".into(),
            ..namespace()
        },
        Namespace {
            instance: "x".repeat(513),
            ..namespace()
        },
        Namespace {
            account: "x".repeat(513),
            ..namespace()
        },
    ] {
        assert!(Scope::education(ns.clone(), "C1", "001").is_err());
        assert!(Scope::program(ns, "001").is_err());
    }
    for key in [" ", "\t", " 001 ", "학생|프로그램", "é", "e\u{301}"] {
        let ns = Namespace {
            instance: " 인스턴스 ".into(),
            account: " A ".into(),
        };
        let education = Scope::education(ns.clone(), key, key).unwrap();
        let program = Scope::program(ns, key).unwrap();
        for scope in [education, program] {
            let raw = serde_json::to_string(&scope).unwrap();
            assert_eq!(serde_json::from_str::<Scope>(&raw).unwrap(), scope);
        }
    }
    // Legacy class/student keys were nonempty-only, with no new length limit.
    assert!(Scope::education(namespace(), "x".repeat(513), "001").is_ok());
    assert_ne!(
        Scope::program(namespace(), "é"),
        Scope::program(namespace(), "e\u{301}")
    );
    assert_ne!(
        Scope::program(namespace(), "001"),
        Scope::program(namespace(), " 001 ")
    );
    let invalid = Scope::Program {
        namespace: namespace(),
        program_key: String::new(),
    };
    assert!(serde_json::to_string(&invalid).is_err());
}

#[test]
fn serialized_s07_basis_target_and_review_are_byte_stable() {
    // Hand-authored legacy bytes; no candidate output is used to set expectations.
    let basis = format!(r#"{{"id":{ID},"scope":{EDUCATION},"observation":{OBSERVATION}}}"#);
    let target = format!(
        r#"{{"identity":"fake-1","canonical_path":"/synthetic/target.sqlite","physical":{{"namespace":{{"instance":"synthetic://same-host","account":"A"}},"record":"R1","field":"note"}},"scope":{EDUCATION},"value":"old","version":0}}"#
    );
    let review = format!(
        r#"{{"input":{{"request_id":"r1","lesson_date":"2026-11-05","object":{ID}}},"basis":{basis},"target":{target},"desired":"next","owner":"synthetic-owner","authorization_generation":1}}"#
    );
    let decoded: Basis = serde_json::from_str(&basis).unwrap();
    assert_eq!(serde_json::to_string(&decoded).unwrap(), basis);
    let decoded: TargetSnapshot = serde_json::from_str(&target).unwrap();
    assert_eq!(serde_json::to_string(&decoded).unwrap(), target);
    let decoded: Review = serde_json::from_str(&review).unwrap();
    assert_eq!(serde_json::to_string(&decoded).unwrap(), review);
}

struct Graph(Vec<Object>);
impl Snapshot for Graph {
    fn get(&self, id: &Id) -> Result<Option<Object>, Error> {
        Ok(self.0.iter().find(|o| &o.id == id).cloned())
    }
    fn targets(&self, root: &Id, _: &str) -> Result<Vec<Id>, Error> {
        Ok(self
            .0
            .iter()
            .filter(|o| &o.id != root)
            .map(|o| o.id.clone())
            .collect())
    }
}
fn object(scope: Scope, key: &str) -> Object {
    Object {
        id: Id {
            object_type: "progress".into(),
            namespace: scope.namespace().clone(),
            native_key: key.into(),
        },
        scope,
        head: "r1".into(),
        observations: vec![serde_json::from_str(OBSERVATION).unwrap()],
    }
}
fn authority(scope: Scope, derive: bool) -> Authority {
    Authority::new(Policy {
        principals: BTreeMap::from([(
            "owner".into(),
            vec![Grant {
                scope,
                object_types: BTreeSet::from(["progress".into()]),
                read: true,
                derive,
                actions: BTreeSet::new(),
            }],
        )]),
    })
}

#[test]
fn domain_scope_round_trips_and_refusals() {
    let empty = Scope::domain(namespace(), "work", BTreeMap::new()).unwrap();
    let literal = r#"{"namespace":{"instance":"synthetic://same-host","account":"A"},"domain":"work","keys":{}}"#;
    assert_eq!(serde_json::to_string(&empty).unwrap(), literal);
    assert_eq!(serde_json::from_str::<Scope>(literal).unwrap(), empty);
    let keyed = Scope::domain(
        namespace(),
        "academy",
        BTreeMap::from([
            ("class_key".to_string(), "C1".to_string()),
            ("student_key".to_string(), "001".to_string()),
        ]),
    )
    .unwrap();
    let keyed_literal = r#"{"namespace":{"instance":"synthetic://same-host","account":"A"},"domain":"academy","keys":{"class_key":"C1","student_key":"001"}}"#;
    assert_eq!(serde_json::to_string(&keyed).unwrap(), keyed_literal);
    assert_eq!(serde_json::from_str::<Scope>(keyed_literal).unwrap(), keyed);
    let ns = r#""namespace":{"instance":"synthetic://same-host","account":"A"}"#;
    for raw in [
        format!(r#"{{{ns},"domain":"program","keys":{{}}}}"#),
        format!(r#"{{{ns},"domain":"program","program_key":"001","keys":{{}}}}"#),
        format!(r#"{{{ns},"domain":"work"}}"#),
        format!(r#"{{{ns},"keys":{{}}}}"#),
        format!(r#"{{{ns},"domain":"","keys":{{}}}}"#),
        format!(r#"{{{ns},"domain":"work","keys":{{"":"x"}}}}"#),
        format!(r#"{{{ns},"domain":"work","keys":{{"k":""}}}}"#),
        format!(r#"{{{ns},"domain":"work","keys":{{}},"class_key":"C1"}}"#),
    ] {
        assert!(
            serde_json::from_str::<Scope>(&raw).is_err(),
            "accepted {raw}"
        );
    }
    assert_eq!(
        Scope::domain(namespace(), "program", BTreeMap::new()),
        Err(Error::InvalidRequest)
    );
    assert_eq!(
        Scope::domain(namespace(), "", BTreeMap::new()),
        Err(Error::InvalidRequest)
    );
}

#[test]
fn education_and_program_scopes_with_keys_are_refused() {
    let ns = r#""namespace":{"instance":"synthetic://same-host","account":"A"}"#;
    for raw in [
        format!(r#"{{{ns},"class_key":"C1","student_key":"001","keys":{{}}}}"#),
        format!(r#"{{{ns},"domain":"program","program_key":"001","keys":{{}}}}"#),
    ] {
        assert!(
            serde_json::from_str::<Scope>(&raw).is_err(),
            "accepted {raw}"
        );
    }
}

#[test]
fn conformance_fixture_scopes_round_trip() {
    let dir = format!("{}/../../conformance/fixtures", env!("CARGO_MANIFEST_DIR"));
    let mut found = 0;
    for name in [
        "synthetic.json",
        "synthetic-v2.json",
        "synthetic-source.json",
    ] {
        let text = std::fs::read_to_string(format!("{dir}/{name}")).unwrap();
        let root: serde_json::Value = serde_json::from_str(&text).unwrap();
        let mut stack = vec![&root];
        while let Some(item) = stack.pop() {
            match item {
                serde_json::Value::Object(map) => {
                    if let Some(scope) = map.get("scope") {
                        let parsed: Scope = serde_json::from_value(scope.clone()).unwrap();
                        assert_eq!(serde_json::to_value(&parsed).unwrap(), *scope);
                        found += 1;
                    }
                    stack.extend(map.values());
                }
                serde_json::Value::Array(items) => stack.extend(items.iter()),
                _ => {}
            }
        }
    }
    assert!(found >= 12, "found {found} fixture scopes");
}

#[test]
fn domain_grant_matching_named_omitted_and_wrong_domain() {
    let candidate = Scope::domain(
        namespace(),
        "academy",
        BTreeMap::from([
            ("class_key".to_string(), "C1".to_string()),
            ("student_key".to_string(), "001".to_string()),
        ]),
    )
    .unwrap();
    let named = Scope::domain(
        namespace(),
        "academy",
        BTreeMap::from([("class_key".to_string(), "C1".to_string())]),
    )
    .unwrap();
    let omitted = Scope::domain(namespace(), "academy", BTreeMap::new()).unwrap();
    let wrong_value = Scope::domain(
        namespace(),
        "academy",
        BTreeMap::from([("class_key".to_string(), "C2".to_string())]),
    )
    .unwrap();
    let wrong_domain = Scope::domain(namespace(), "work", BTreeMap::new()).unwrap();
    for (grant, allowed) in [
        (named, true),
        (omitted, true),
        (wrong_value, false),
        (wrong_domain, false),
    ] {
        assert_eq!(grant.matches(&candidate), allowed);
        let authority = authority(grant.clone(), true);
        let session = authority.bind_trusted("owner").unwrap();
        let admission = authority.admit(&session).unwrap();
        let obj = object(candidate.clone(), "001");
        let graph = Graph(vec![obj.clone()]);
        let result = admission.query(&graph, Query::Get { id: obj.id.clone() }, 15);
        assert_eq!(result.is_ok(), allowed, "grant {grant:?}");
    }
}

#[test]
fn grants_isolate_account_instance_domain_program_and_derive() {
    let scopes = vec![
        Scope::program(namespace(), "001").unwrap(),
        Scope::program(namespace(), "002").unwrap(),
        Scope::program(
            Namespace {
                account: "B".into(),
                ..namespace()
            },
            "001",
        )
        .unwrap(),
        Scope::program(
            Namespace {
                instance: "synthetic://other".into(),
                ..namespace()
            },
            "001",
        )
        .unwrap(),
        Scope::education(namespace(), "001", "001").unwrap(),
    ];
    for granted in &scopes {
        let authority = authority(granted.clone(), true);
        let session = authority.bind_trusted("owner").unwrap();
        let admission = authority.admit(&session).unwrap();
        for candidate in &scopes {
            let object = object(candidate.clone(), "001");
            let graph = Graph(vec![object.clone()]);
            for query in [
                Query::Get {
                    id: object.id.clone(),
                },
                Query::Decide {
                    root: object.id.clone(),
                    relation: "linked".into(),
                },
            ] {
                let result = admission.query(&graph, query, 15);
                if candidate == granted {
                    assert!(result.is_ok());
                } else {
                    assert_eq!(result, Err(Error::Unavailable));
                }
            }
        }
    }
    let root = object(scopes[0].clone(), "root");
    let graph = Graph(
        std::iter::once(root.clone())
            .chain(
                scopes[1..]
                    .iter()
                    .enumerate()
                    .map(|(i, s)| object(s.clone(), &format!("hidden-{i}"))),
            )
            .collect(),
    );
    let authority = authority(root.scope.clone(), true);
    let session = authority.bind_trusted("owner").unwrap();
    let admission = authority.admit(&session).unwrap();
    let decide = Query::Decide {
        root: root.id.clone(),
        relation: "linked".into(),
    };
    assert_eq!(
        serde_json::to_string(&admission.query(&graph, decide.clone(), 15).unwrap()).unwrap(),
        serde_json::to_string(
            &admission
                .query(&Graph(vec![root.clone()]), decide.clone(), 15)
                .unwrap()
        )
        .unwrap()
    );
    let input = ActionInput {
        request_id: "r1".into(),
        lesson_date: "2026-11-05".into(),
        object: root.id.clone(),
    };
    assert_eq!(
        admission.action_basis(&graph, &input, 15),
        Err(Error::ActionForbidden)
    );
    let read_only = self::authority(root.scope.clone(), false);
    let session = read_only.bind_trusted("owner").unwrap();
    assert_eq!(
        read_only.admit(&session).unwrap().query(&graph, decide, 15),
        Err(Error::DeriveForbidden)
    );
}
