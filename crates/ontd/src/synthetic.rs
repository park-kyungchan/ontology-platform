//! S06 demonstrator adapter. Account provenance is explicitly synthetic.
//! Frozen S05 frames are INPUT evidence here, separate from S06 output expectations.
use ont_core::{
    Authority, Change, Coverage, Error, Grant, Id, Knowledge, Lineage, Link, Namespace, Object,
    Observation, Policy, Scope, Session,
};
use ont_store::Store;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const NOW: u64 = 1_793_577_600;
pub const PRINCIPAL: &str = "synthetic-domain-principal";
pub const S05_REVISION: &str = "049f7c3da0af6c320c818d2afdab3da34bccd37a";
const RECORDED: &str = include_str!("../../../conformance/fixtures/s05/expected.json");
const AUDIT: &str = include_str!("../../../conformance/fixtures/s05/audit.json");
const SOURCE: &str = include_str!("../../../conformance/fixtures/synthetic-source.json");
const SYNTHETIC: &str = include_str!("../../../conformance/fixtures/synthetic-v2.json");
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureObject {
    id: Id,
    scope: Scope,
    observations: Vec<Observation>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    objects: Vec<FixtureObject>,
    links: Vec<Link>,
}
pub fn namespace() -> Namespace {
    Namespace {
        instance: "synthetic://same-host".into(),
        account: "A".into(),
    }
}
pub fn recorded_namespace() -> Namespace {
    Namespace {
        instance: "recording://s05-synthetic".into(),
        account: "fixture-A".into(),
    }
}
pub fn student() -> Id {
    Id {
        object_type: "student".into(),
        namespace: namespace(),
        native_key: "001".into(),
    }
}
pub fn policy() -> Policy {
    let types = [
        "student",
        "student|a",
        "textbook",
        "progress",
        "section",
        "attempt",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    let grant = Grant {
        scope: Scope {
            namespace: namespace(),
            class_key: "C1".into(),
            student_key: "001".into(),
        },
        object_types: types,
        read: true,
        derive: true,
        actions: Default::default(),
    };
    let mut recorded_grant = grant.clone();
    recorded_grant.scope = Scope {
        namespace: recorded_namespace(),
        class_key: "saved-subset".into(),
        student_key: "synthetic-student-1".into(),
    };
    Policy {
        principals: BTreeMap::from([(PRINCIPAL.into(), vec![grant, recorded_grant])]),
    }
}
pub fn graph() -> Result<(Vec<Object>, Vec<Link>), Error> {
    let mut input: Value = serde_json::from_str(SYNTHETIC).map_err(|_| Error::InvalidEvidence)?;
    let source: Value = serde_json::from_str(SOURCE).map_err(|_| Error::InvalidEvidence)?;
    let digest = format!("{:x}", Sha256::digest(SOURCE.as_bytes()));
    for object in input
        .get_mut("objects")
        .and_then(Value::as_array_mut)
        .ok_or(Error::InvalidEvidence)?
    {
        for observation in object
            .get_mut("observations")
            .and_then(Value::as_array_mut)
            .ok_or(Error::InvalidEvidence)?
        {
            if observation
                .pointer("/lineage/artifact_sha256")
                .and_then(Value::as_str)
                != Some(digest.as_str())
            {
                return Err(Error::InvalidEvidence);
            }
            let pointer = observation
                .pointer("/lineage/pointer")
                .and_then(Value::as_str)
                .ok_or(Error::InvalidEvidence)?
                .to_string();
            observation
                .as_object_mut()
                .ok_or(Error::InvalidEvidence)?
                .remove("lineage");
            if source.pointer(&pointer) != Some(&*observation) {
                return Err(Error::InvalidEvidence);
            }
        }
    }
    if input != source {
        return Err(Error::InvalidEvidence);
    }
    let fixture: Fixture = serde_json::from_str(SYNTHETIC).map_err(|_| Error::InvalidEvidence)?;
    let mut objects = Vec::new();
    for o in fixture.objects {
        let head = o
            .observations
            .last()
            .ok_or(Error::InvalidEvidence)?
            .revision
            .clone();
        objects.push(Object {
            id: o.id,
            scope: o.scope,
            head,
            observations: o.observations,
        });
    }
    let mut links = fixture.links;
    let (recorded_objects, recorded_links) = recorded_graph()?;
    objects.extend(recorded_objects);
    links.extend(recorded_links);
    Ok((objects, links))
}
pub fn setup() -> Result<(crate::Service, Session), Error> {
    let (objects, links) = graph()?;
    let mut store = Store::memory()?;
    store.seed(&objects, &links)?;
    let authority = Authority::new(policy());
    let session = authority.bind_trusted(PRINCIPAL)?;
    Ok((crate::Service::new(store, authority), session))
}
fn text(v: &Value) -> Result<&str, Error> {
    v.as_str().ok_or(Error::InvalidEvidence)
}
fn recorded_observation(fields: BTreeMap<String, Knowledge>, pointer: &str) -> Observation {
    Observation {
        revision: "saved-s05".into(),
        source_revision: "9b5a3c565f4aa9296ba98cdd7460138aa0642f38ded31470e9c8327cda12ee9c".into(),
        observed_at: 1_791_964_800,
        fresh_until: 1_791_964_800,
        change: Change::Initial,
        supersedes: None,
        coverage: Coverage::Partial,
        fields,
        lineage: Lineage {
            artifact_sha256: "4bdf5431cf2823cf707f9b73d2f7b07b47122369fde7ba7a0831bd9700922c3e"
                .into(),
            pointer: pointer.into(),
        },
    }
}
fn recorded_object(
    t: &str,
    key: &str,
    fields: BTreeMap<String, Knowledge>,
    pointer: &str,
) -> Object {
    Object {
        id: Id {
            object_type: t.into(),
            namespace: recorded_namespace(),
            native_key: key.into(),
        },
        scope: Scope {
            namespace: recorded_namespace(),
            class_key: "saved-subset".into(),
            student_key: "synthetic-student-1".into(),
        },
        head: "saved-s05".into(),
        observations: vec![recorded_observation(fields, pointer)],
    }
}
fn recorded_graph() -> Result<(Vec<Object>, Vec<Link>), Error> {
    if format!("{:x}", Sha256::digest(RECORDED.as_bytes()))
        != "aae41a0f4d79752a0b14d2d4504d5112d4bc576ba6827412675f6789eb6ee234"
        || format!("{:x}", Sha256::digest(AUDIT.as_bytes()))
            != "4bdf5431cf2823cf707f9b73d2f7b07b47122369fde7ba7a0831bd9700922c3e"
    {
        return Err(Error::InvalidEvidence);
    }
    let frames: Value = serde_json::from_str(RECORDED).map_err(|_| Error::InvalidEvidence)?;
    let audit: Value = serde_json::from_str(AUDIT).map_err(|_| Error::InvalidEvidence)?;
    let root = recorded_object(
        "student",
        "synthetic-student-1",
        BTreeMap::from([
            (
                "teacher_authenticated".into(),
                Knowledge::Value(false.into()),
            ),
            (
                "complete_roster_verified".into(),
                Knowledge::Value(false.into()),
            ),
            (
                "absence_proves_nonperformance".into(),
                Knowledge::Value(false.into()),
            ),
        ]),
        "/students/0",
    );
    let mut objects = vec![root.clone()];
    let mut links = Vec::new();
    for (case, kind) in [
        ("sections-edition-a", "section"),
        ("attempt-knowledge-states", "attempt"),
    ] {
        let frame = frames.get(case).ok_or(Error::InvalidEvidence)?;
        if frame.pointer("/value/status").and_then(Value::as_str) != Some("partial")
            || frame
                .pointer("/value/scope/completeTeacherRosterVerified")
                .and_then(Value::as_bool)
                != Some(false)
            || frame
                .pointer("/value/currentAcademyServerQueried")
                .and_then(Value::as_bool)
                != Some(false)
        {
            return Err(Error::InvalidEvidence);
        }
        let rows = frame
            .pointer("/value/records")
            .and_then(Value::as_array)
            .ok_or(Error::InvalidEvidence)?;
        for row in rows {
            let raw = row.get("value").ok_or(Error::InvalidEvidence)?;
            let pointer = text(
                row.pointer("/source/jsonPointer")
                    .ok_or(Error::InvalidEvidence)?,
            )?;
            if audit.pointer(pointer) != Some(raw) {
                return Err(Error::InvalidEvidence);
            }
            let key = text(raw.get("id").ok_or(Error::InvalidEvidence)?)?;
            let mut fields =
                BTreeMap::from([("source_record".into(), Knowledge::Value(raw.clone()))]);
            if kind == "attempt" {
                let score = if raw.get("assessmentState").and_then(Value::as_str)
                    == Some("not_performed")
                {
                    Knowledge::NotPerformed
                } else {
                    match raw.get("score") {
                        None => Knowledge::Missing,
                        Some(Value::Null) => Knowledge::Null,
                        Some(Value::String(s)) if s.is_empty() => Knowledge::Blank,
                        Some(Value::String(s)) if s == "unknown" => Knowledge::Unknown,
                        Some(v) => Knowledge::Value(v.clone()),
                    }
                };
                fields.insert("score".into(), score);
            }
            let object = recorded_object(kind, key, fields, pointer);
            links.push(Link {
                source: root.id.clone(),
                relation: kind.into(),
                target: object.id.clone(),
            });
            objects.push(object);
        }
    }
    Ok((objects, links))
}
