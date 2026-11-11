//! Synthetic snapshot probes and real schema upgrades for count aggregation.
#![allow(clippy::unwrap_used)]
use super::*;
use ont_core::{Id, Scope};
use ont_store::schema_mode::{HeadAccess, SchemaHead, SchemaRead, StoreRefusal};
use ont_store::{Append, Store};
use std::cell::Cell;

fn schema() -> Value {
    json!({"format":"ontology-schema/v1","domain":"test","version":1,
        "namespace":{"instance":"test","account":"root"},"scope_keys":["tenant"],
        "value_types":{"status":{"base":"string","enum":["open","resolved"]}},
        "object_types":{"node":{"owner":"ontology","primary_key":{"from":"$action_id"},"properties":{
            "status":{"type":{"value_type":"status"}},"name":{"type":"string"},"link":{"type":{"ref":"node"}}}},
            "other":{"owner":"ontology","primary_key":{"from":"$action_id"},"properties":{}}},
        "grants":[{"principal":"agent","scope":{"tenant":"public"},"read":["node"],"derive":["node"],"actions":[]}]})
}
fn install(store: &mut Store, s: &Value) {
    store
        .install_schema(&serde_json::to_vec(s).unwrap(), Default::default())
        .unwrap();
}
fn memory(s: &Value) -> Store {
    let mut store = Store::memory_schema().unwrap();
    install(&mut store, s);
    store
}
fn row(key: &str, hidden: bool) -> Append {
    let id: Id = serde_json::from_value(json!({"object_type":"node","namespace":{"instance":"test","account":"root"},"native_key":key})).unwrap();
    Append {
        scope: Scope::domain(id.namespace.clone(), "test", BTreeMap::from([("tenant".into(), if hidden {"private"} else {"public"}.into())])).unwrap(),
        id: id.clone(),
        observation: serde_json::from_value(json!({
            "revision":"r0","source_revision":"s0","observed_at":1,"fresh_until":100,"change":"initial","coverage":"complete",
            "fields":{"status":{"kind":"value","value":"resolved"},"name":{"kind":"value","value":key},"link":{"kind":"value","value":id}},
            "lineage":{"artifact_sha256":"a".repeat(64),"pointer":"/synthetic"}
        })).unwrap(),
    }
}

fn request(keys: &[&str]) -> AggregateRequest {
    AggregateRequest::parse(
        json!({"op":"aggregate","object_type":"node","group_by":keys,"metrics":["count"]})
            .to_string()
            .as_bytes(),
    )
    .unwrap()
}
fn read(req: &AggregateRequest, tx: &dyn AroundSource) -> crate::cli::Outcome {
    let ctx = Context::new("unused".into(), 10);
    crate::cli::render(read_aggregate(&ctx, req, tx), 10)
}
fn text(store: &mut Store, req: &AggregateRequest) -> String {
    store.read_schema(|tx| read(req, tx).stdout).unwrap()
}
fn answer(store: &mut Store, req: &AggregateRequest) -> Value {
    serde_json::from_str(&text(store, req)).unwrap()
}
struct Probe<'a, 'b> {
    tx: &'a SchemaRead<'b>,
    metadata: Cell<usize>,
    payload: Cell<usize>,
    poison: bool,
}
impl AroundSource for Probe<'_, '_> {
    fn schema_set(&self) -> &ont_core::schema::SchemaSet {
        self.tx.schema_set()
    }
    fn head_access(&self, id: &Id) -> Result<Option<HeadAccess>, StoreRefusal> {
        self.tx.head_access(id)
    }
    fn scan_head_access(
        &self,
        ty: &str,
        limit: usize,
        after: Option<&Id>,
    ) -> Result<Vec<HeadAccess>, StoreRefusal> {
        self.metadata.set(self.metadata.get() + 1);
        self.tx.scan_head_access(ty, limit, after)
    }
    fn head_after_access(&self, a: &HeadAccess) -> Result<Option<SchemaHead>, StoreRefusal> {
        self.payload.set(self.payload.get() + 1);
        if self.poison && a.id.native_key == "hidden" {
            return Err(StoreRefusal::new("poison_payload"));
        }
        self.tx.head_after_access(a)
    }
}
#[test]
fn example_zero_buckets_two_keys_and_unresolved() {
    let mut s = schema();
    s["object_types"]["node"]["properties"]["name"]["type"] = json!({"value_type":"status"});
    let mut store = memory(&s);
    let mut r = row("a", false);
    r.observation
        .fields
        .insert("name".into(), Knowledge::Value(json!("open")));
    store.append_schema(&[r.clone()], 10).unwrap();
    assert_eq!(
        answer(&mut store, &request(&["status"]))["result"]["groups"],
        json!([{"key":{"status":"open"},"count":0},{"key":{"status":"resolved"},"count":1}])
    );
    let a = answer(&mut store, &request(&["status", "name"]));
    assert_eq!(a["result"]["groups"].as_array().unwrap().len(), 4);
    assert_eq!(a["result"]["basis"]["qualified"], true);
    r.id.native_key = "missing".into();
    r.observation
        .fields
        .insert("status".into(), Knowledge::Missing);
    store.append_schema(&[r], 10).unwrap();
    let a = answer(&mut store, &request(&["status"]));
    assert_eq!(a["result"]["basis"]["unresolved_excluded"], 1);
    assert_eq!(a["result"]["basis"]["qualified"], false);
    assert_eq!(
        answer(&mut store, &request(&[]))["result"]["groups"],
        json!([{"key":{},"count":2}])
    );
}
#[test]
fn hidden_poison_parity_and_preflight_reads_no_heads() {
    let mut outputs = vec![];
    for hidden in [false, true] {
        let mut store = memory(&schema());
        store.append_schema(&[row("a", false)], 10).unwrap();
        if hidden {
            store.append_schema(&[row("hidden", true)], 10).unwrap();
        }
        store
            .read_schema(|tx| {
                let probe = Probe {
                    tx,
                    metadata: Cell::new(0),
                    payload: Cell::new(0),
                    poison: true,
                };
                if hidden {
                    assert!(
                        probe
                            .head_after_access(
                                &tx.head_access(&row("hidden", true).id).unwrap().unwrap()
                            )
                            .is_err()
                    );
                    probe.payload.set(0);
                }
                for mutate in 0..5 {
                    let mut req = request(&["status"]);
                    match mutate {
                        0 => req.group_by = vec!["secret".into()],
                        1 => req.filter = Some(Filter::Eq("absent".into(), json!(1))),
                        2 => req.metrics = vec![json!({"max":"secret"})],
                        3 => req.object_type = "other".into(),
                        _ => req.group_by = vec!["status".into(); 3],
                    }
                    assert_eq!(read(&req, &probe).code, 2);
                    assert_eq!(probe.metadata.get(), 0);
                    assert_eq!(probe.payload.get(), 0);
                }
                outputs.push(read(&request(&["status"]), &probe).stdout);
                assert_eq!(probe.payload.get(), 1);
            })
            .unwrap();
    }
    assert_eq!(outputs[0], outputs[1]);
    let mut s = schema();
    s["grants"][0]["derive"] = json!([]);
    let mut empty = memory(&s);
    let denied = text(&mut empty, &request(&[]));
    empty.append_schema(&[row("a", false)], 10).unwrap();
    assert_eq!(denied, text(&mut empty, &request(&[])));
    assert!(denied.contains("derive_forbidden"));
    // A derive grant in another scope must not authorize readable public rows.
    s["grants"].as_array_mut().unwrap().push(json!({"principal":"agent","scope":{"tenant":"private"},"read":["node"],"derive":["node"],"actions":[]}));
    let mut scoped = memory(&s);
    scoped.append_schema(&[row("a", false)], 10).unwrap();
    assert_eq!(
        answer(&mut scoped, &request(&[]))["code"],
        "derive_forbidden"
    );
}
#[test]
fn historical_enum_reference_and_value_type_upgrades() {
    for (property, ty) in [
        ("status", json!({"value_type":"status"})),
        ("link", json!({"ref":"other"})),
        ("name", json!("integer")),
    ] {
        let mut s = schema();
        let mut store = memory(&s);
        store.append_schema(&[row("a", false)], 10).unwrap();
        s["version"] = json!(2);
        s["value_types"]["status"]["enum"] = json!(["open"]);
        s["object_types"]["node"]["properties"][property]["type"] = ty;
        install(&mut store, &s);
        let a = answer(&mut store, &request(&[property]));
        assert_eq!(a["result"]["basis"]["unresolved_excluded"], 1, "{a}");
        assert_eq!(a["result"]["basis"]["qualified"], false);
        assert!(
            a["result"]["groups"]
                .as_array()
                .unwrap()
                .iter()
                .all(|g| g["count"] == 0)
        );
    }
}
#[test]
fn keys_metrics_value_bytes_and_bucket_bounds() {
    let mut store = memory(&schema());
    assert_eq!(
        answer(&mut store, &request(&["status", "name"]))["status"],
        "ok"
    );
    for req in [
        json!({"group_by":["a","b","c"],"metrics":["count"]}),
        json!({"group_by":[],"metrics":[]}),
        json!({"group_by":[],"metrics":["count","count"]}),
        json!({"group_by":[],"metrics":[{"sum":"name"}]}),
        json!({"group_by":[],"metrics":["min"]}),
    ] {
        let mut req = req;
        req["object_type"] = json!("node");
        assert!(AggregateRequest::parse(req.to_string().as_bytes()).is_err());
    }
    for bytes in [MAX_GROUP_VALUE_BYTES, MAX_GROUP_VALUE_BYTES + 1] {
        let v = json!("x".repeat(bytes - 2));
        assert_eq!(bounded(&v).is_ok(), bytes == MAX_GROUP_VALUE_BYTES);
    }
    for two in [false, true] {
        for n in [MAX_BUCKETS, MAX_BUCKETS + 1] {
            let mut s = schema();
            let size = if two {
                if n == MAX_BUCKETS { 20 } else { 7 }
            } else {
                n
            };
            s["value_types"]["status"]["enum"] =
                json!((0..size).map(|i| i.to_string()).collect::<Vec<_>>());
            if two {
                s["object_types"]["node"]["properties"]["name"]["type"] =
                    json!({"value_type":"pair"});
                s["value_types"]["pair"] = json!({"base":"string","enum":(0..if n==MAX_BUCKETS {50}else{143}).map(|i| i.to_string()).collect::<Vec<_>>()});
            }
            let mut store = memory(&s);
            let a = answer(
                &mut store,
                &request(if two {
                    &["status", "name"]
                } else {
                    &["status"]
                }),
            );
            if n == MAX_BUCKETS {
                assert_eq!(a["result"]["groups"].as_array().unwrap().len(), MAX_BUCKETS);
            } else {
                assert_eq!(a["code"], "result_too_large");
            }
        }
    }
}
#[test]
fn admitted_scan_and_dynamic_bucket_limits_before_payload_or_growth() {
    let mut store = memory(&schema());
    store
        .write(10, |tx| {
            for i in 0..SCAN_LIMIT {
                tx.append(&row(&format!("{i:05}"), false))?;
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(
        answer(&mut store, &request(&[]))["result"]["basis"]["scanned"],
        SCAN_LIMIT
    );
    store.append_schema(&[row("extra", false)], 10).unwrap();
    store
        .read_schema(|tx| {
            let probe = Probe {
                tx,
                metadata: Cell::new(0),
                payload: Cell::new(0),
                poison: false,
            };
            let a = read(&request(&[]), &probe);
            assert!(a.stdout.contains("result_too_large"));
            assert_eq!(probe.payload.get(), SCAN_LIMIT);
        })
        .unwrap();
    let mut groups = BTreeMap::new();
    for i in 0..MAX_BUCKETS {
        bucket(&mut groups, json!({"name":i}), 1).unwrap();
    }
    assert!(bucket(&mut groups, json!({"name":"above"}), 1).is_err());
    assert_eq!(groups.len(), MAX_BUCKETS);
}
