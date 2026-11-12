//! Observe the history count/load boundary in the production get evaluator.
#![allow(clippy::unwrap_used)]
use super::*;
use ont_core::{Change, Coverage, Lineage, Namespace, Observation, Scope};
use ont_store::{Append, Store};
use serde_json::json;
use std::cell::Cell;

struct Probe<'a, 'b> {
    tx: &'a SchemaRead<'b>,
    history_reads: Cell<usize>,
}
impl GetSource for Probe<'_, '_> {
    fn schema_set(&self) -> &SchemaSet {
        self.tx.schema_set()
    }
    fn head_access(&self, id: &Id) -> Result<Option<HeadAccess>, StoreRefusal> {
        self.tx.head_access(id)
    }
    fn history(&self, id: &Id) -> Result<Option<SchemaHistory>, StoreRefusal> {
        self.history_reads.set(self.history_reads.get() + 1);
        self.tx.history(id)
    }
}
fn id(key: &str) -> Id {
    Id {
        object_type: "node".into(),
        namespace: Namespace {
            instance: "test".into(),
            account: "root".into(),
        },
        native_key: key.into(),
    }
}
#[test]
fn hidden_101_history_is_unavailable_without_count_before_admission() {
    let mut store = Store::memory_schema().unwrap();
    let schema = json!({"format":"ontology-schema/v1","domain":"test","version":1,
        "namespace":{"instance":"test","account":"root"},"scope_keys":["tenant"],
        "object_types":{"node":{"owner":"ontology","primary_key":{"from":"$action_id"},"properties":{}}},
        "grants":[{"principal":"agent","scope":{"tenant":"public"},"read":["node"],"derive":[],"actions":[]}]});
    store
        .install_schema(&serde_json::to_vec(&schema).unwrap(), Default::default())
        .unwrap();
    let mut rows = vec![];
    for (key, tenant) in [("hidden", "private"), ("visible", "public")] {
        for i in 0..101 {
            rows.push(Append {
                id: id(key),
                scope: Scope::domain(
                    id(key).namespace,
                    "test",
                    BTreeMap::from([("tenant".into(), tenant.into())]),
                )
                .unwrap(),
                observation: Observation {
                    revision: format!("r{i}"),
                    source_revision: "s0".into(),
                    observed_at: 1,
                    fresh_until: 100,
                    change: if i == 0 {
                        Change::Initial
                    } else {
                        Change::Update
                    },
                    supersedes: (i > 0).then(|| format!("r{}", i - 1)),
                    coverage: Coverage::Complete,
                    fields: BTreeMap::new(),
                    lineage: Lineage {
                        artifact_sha256: "a".repeat(64),
                        pointer: String::new(),
                    },
                },
            });
        }
    }
    store.append_schema(&rows, 10).unwrap();
    store
        .read_schema(|tx| {
            let probe = Probe {
                tx,
                history_reads: Cell::new(0),
            };
            let ctx = Context::new("unused.sqlite".into(), 10);
            let request = |key| GetRequest {
                op: None,
                id: id(key),
            };
            let render = |key| crate::cli::render(read_get(&ctx, &request(key), &probe), 10);
            let hidden = render("hidden");
            assert_eq!(
                probe.history_reads.get(),
                0,
                "history metadata was counted before admission"
            );
            let absent = render("missing");
            assert_eq!(hidden.stdout, absent.stdout);
            let answer: Value = serde_json::from_str(&hidden.stdout).unwrap();
            assert_eq!(answer["code"], "unavailable");
            assert!(answer.get("result").is_none());
            assert!(!hidden.stdout.contains("omitted") && !hidden.stdout.contains("oversized"));
            assert_eq!(probe.history_reads.get(), 0);
            let visible = render("visible");
            assert_eq!(
                probe.history_reads.get(),
                1,
                "probe must observe the real history boundary"
            );
            let answer: Value = serde_json::from_str(&visible.stdout).unwrap();
            assert_eq!(answer["status"], "ok", "{answer}");
            assert_eq!(answer["result"]["basis"]["older_omitted"], 1);
        })
        .unwrap();
}
