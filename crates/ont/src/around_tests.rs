//! Ordering probes exercise the same snapshot evaluator as the public read.
#![allow(clippy::unwrap_used)]
use super::*;
use ont_core::{Change, Coverage, Lineage, Namespace, Observation};
use ont_store::{Append, Store};
use std::cell::{Cell, RefCell};

fn id(ty: &str, key: &str) -> Id {
    Id {
        object_type: ty.into(),
        namespace: Namespace {
            instance: "test".into(),
            account: "root".into(),
        },
        native_key: key.into(),
    }
}
fn row(ty: &str, key: &str, hidden: bool) -> Append {
    let id = id(ty, key);
    Append {
        scope: Scope::domain(
            id.namespace.clone(),
            "test",
            BTreeMap::from([(
                "tenant".into(),
                if hidden { "private" } else { "public" }.into(),
            )]),
        )
        .unwrap(),
        id,
        observation: Observation {
            revision: "r0".into(),
            source_revision: "s0".into(),
            observed_at: 1,
            fresh_until: 100,
            change: Change::Initial,
            supersedes: None,
            coverage: Coverage::Complete,
            fields: if ty == "node" {
                BTreeMap::from([(
                    "edges".into(),
                    Knowledge::Value(json!([self::id("node", "z")])),
                )])
            } else {
                BTreeMap::new()
            },
            lineage: Lineage {
                artifact_sha256: "a".repeat(64),
                pointer: "/probe".into(),
            },
        },
    }
}
fn memory(hidden: bool) -> Store {
    let schema = json!({"format":"ontology-schema/v1","domain":"test","version":1,
        "namespace":{"instance":"test","account":"root"},"scope_keys":["tenant"],
        "object_types":{"node":{"owner":"ontology","primary_key":{"from":"$action_id"},"properties":{"edges":{"type":{"array":{"ref":"node"}},"required":false}}},
            "leaf":{"owner":"ontology","primary_key":{"from":"$action_id"},"properties":{}}},
        "link_types":{"edge":{"from":"node","via":"edges","to":"node","cardinality":"many_to_many","names":{"forward":"next","reverse":"previous"}}},
        "grants":[{"principal":"agent","scope":{"tenant":"public"},"read":["node","leaf"],"derive":[],"actions":[]}]});
    let mut store = Store::memory_schema().unwrap();
    store
        .install_schema(&serde_json::to_vec(&schema).unwrap(), Default::default())
        .unwrap();
    let mut rows = vec![row("node", "a", false)];
    if hidden {
        rows.push(row("node", "z", true));
    }
    store.append_schema(&rows, 10).unwrap();
    store
}
struct Probe<'a, 'b> {
    tx: &'a SchemaRead<'b>,
    poison: Option<Id>,
    metadata_reads: Cell<usize>,
    named_reads: Cell<usize>,
    payload_reads: RefCell<Vec<Id>>,
}
impl<'a, 'b> Probe<'a, 'b> {
    fn new(tx: &'a SchemaRead<'b>, poison: Option<Id>) -> Self {
        Self {
            tx,
            poison,
            metadata_reads: Cell::new(0),
            named_reads: Cell::new(0),
            payload_reads: RefCell::new(vec![]),
        }
    }
}
impl AroundSource for Probe<'_, '_> {
    fn schema_set(&self) -> &SchemaSet {
        self.tx.schema_set()
    }
    fn head_access(&self, id: &Id) -> Result<Option<HeadAccess>, StoreRefusal> {
        self.metadata_reads.set(self.metadata_reads.get() + 1);
        self.named_reads.set(self.named_reads.get() + 1);
        self.tx.head_access(id)
    }
    fn candidate_access(
        &self,
        ty: &str,
        condition: Option<&ont_store::schema_mode::CandidateCondition>,
        now: u64,
        limit: usize,
        after: Option<&Id>,
        scopes: &[Scope],
    ) -> Result<Vec<HeadAccess>, StoreRefusal> {
        self.metadata_reads.set(self.metadata_reads.get() + 1);
        self.tx
            .candidate_access(ty, condition, now, limit, after, scopes)
    }
    fn head_after_access(&self, access: &HeadAccess) -> Result<Option<SchemaHead>, StoreRefusal> {
        self.payload_reads.borrow_mut().push(access.id.clone());
        if self.poison.as_ref() == Some(&access.id) {
            return Err(StoreRefusal::new("store_failure"));
        }
        self.tx.head_after_access(access)
    }
}
fn request(path: &[&str]) -> AroundRequest {
    AroundRequest::parse(
        json!({"from":{"object_type":"node"},"path":path})
            .to_string()
            .as_bytes(),
    )
    .unwrap()
}
fn ctx() -> Context {
    Context::new("unused.sqlite".into(), 10)
}

#[test]
fn around_admits_hidden_target_before_poison_payload_read() {
    let mut answers = vec![];
    for hidden in [false, true] {
        let mut store = memory(hidden);
        store
            .read_schema(|tx| {
                let probe = Probe::new(tx, Some(id("node", "z")));
                if hidden {
                    let access = tx.head_access(&id("node", "z")).unwrap().unwrap();
                    assert!(
                        probe.head_after_access(&access).is_err(),
                        "poison must be active"
                    );
                    probe.payload_reads.borrow_mut().clear();
                }
                let answer =
                    crate::cli::render(read_around(&ctx(), &request(&["next"]), &probe), 10);
                assert_eq!(answer.code, 0, "{}", answer.stderr);
                assert_eq!(*probe.payload_reads.borrow(), vec![id("node", "a")]);
                answers.push(answer.stdout);
            })
            .unwrap();
    }
    assert_eq!(answers[0], answers[1]);
}

#[test]
fn around_validates_entire_path_before_any_head_metadata_or_payload_read() {
    let mut store = memory(false);
    store
        .read_schema(|tx| {
            let probe = Probe::new(tx, Some(id("node", "a")));
            for (path, code) in [
                (vec!["bad"], "invalid_link"),
                (vec!["next", "bad"], "invalid_link"),
                (vec!["next"; 4], "invalid_path"),
            ] {
                let answer = crate::cli::render(read_around(&ctx(), &request(&path), &probe), 10);
                let answer: serde_json::Value = serde_json::from_str(&answer.stdout).unwrap();
                assert_eq!(answer["code"], code);
                assert_eq!(probe.metadata_reads.get(), 0);
                assert!(probe.payload_reads.borrow().is_empty());
            }
            // The same nonempty store hits the poison on a valid path.
            let answer = crate::cli::render(read_around(&ctx(), &request(&["next"]), &probe), 10);
            assert_eq!(answer.code, 1);
            assert!(answer.stderr.contains("store_failure"));
            assert!(probe.metadata_reads.get() > 0);
            assert_eq!(*probe.payload_reads.borrow(), vec![id("node", "a")]);
        })
        .unwrap();
}

#[test]
fn around_named_target_type_head_cap_is_independent_of_start_and_reference_budget() {
    let mut store = memory(false);
    store
        .write(10, |tx| {
            for i in 0..=SCAN_LIMIT {
                tx.append(&row("leaf", &i.to_string(), false))?;
            }
            Ok(())
        })
        .unwrap();
    store.read_schema(|tx| {
        let principal=Principal::bind(tx.schema_set()).unwrap();
        let admission=principal.authority().admit(principal.session()).unwrap();
        let mut reader=Reader {tx,admission:&admission,now:10,loaded:BTreeMap::new(),counts:BTreeMap::new(),basis:Basis::default()};
        assert!(reader.named(&id("node","a")).unwrap().is_some());
        for i in 0..SCAN_LIMIT { assert!(reader.named(&id("leaf",&i.to_string())).unwrap().is_some()); }
        assert_eq!(reader.basis.scanned,SCAN_LIMIT+1);
        assert!(matches!(reader.named(&id("leaf",&SCAN_LIMIT.to_string())),Err(Failure::Refused(r)) if r.code=="result_too_large"));
    }).unwrap();
}

#[test]
fn around_reference_limit_refuses_before_missing_target_lookups() {
    let mut store = memory(false);
    let mut update = row("node", "a", false);
    update.observation.revision = "r1".into();
    update.observation.change = Change::Update;
    update.observation.supersedes = Some("r0".into());
    update.observation.fields.insert(
        "edges".into(),
        Knowledge::Value(json!(vec![id("node", "z"); MAX_REFERENCE_ENTRIES + 1])),
    );
    store.append_schema(&[update], 10).unwrap();
    store
        .read_schema(|tx| {
            let probe = Probe::new(tx, None);
            let answer = crate::cli::render(read_around(&ctx(), &request(&["next"]), &probe), 10);
            let answer: serde_json::Value = serde_json::from_str(&answer.stdout).unwrap();
            assert_eq!(answer["code"], "result_too_large");
            assert_eq!(answer["max_reference_entries"], MAX_REFERENCE_ENTRIES);
            assert_eq!(probe.named_reads.get(), 0);
            assert_eq!(*probe.payload_reads.borrow(), vec![id("node", "a")]);
        })
        .unwrap();
}

#[test]
fn around_historical_reference_type_is_excluded_before_named_lookup() {
    let mut store = memory(false);
    let mut schema =
        serde_json::to_value(store.schema_set().unwrap().schema("test").unwrap()).unwrap();
    schema["version"] = json!(2);
    schema["object_types"]["node"]["properties"]["edges"]["type"] = json!({"array":{"ref":"leaf"}});
    schema["link_types"]["edge"]["to"] = json!("leaf");
    store
        .install_schema(&serde_json::to_vec(&schema).unwrap(), Default::default())
        .unwrap();
    store
        .read_schema(|tx| {
            let probe = Probe::new(tx, Some(id("node", "z")));
            let answer = crate::cli::render(read_around(&ctx(), &request(&["next"]), &probe), 10);
            let answer: serde_json::Value = serde_json::from_str(&answer.stdout).unwrap();
            assert_eq!(answer["result"]["basis"]["unresolved_excluded"], 1);
            assert_eq!(answer["result"]["basis"]["qualified"], false);
            assert_eq!(probe.named_reads.get(), 0);
            assert_eq!(*probe.payload_reads.borrow(), vec![id("node", "a")]);
        })
        .unwrap();
}
