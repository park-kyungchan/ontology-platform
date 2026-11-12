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
            "status":{"type":{"value_type":"status"}},"name":{"type":"string","max_bytes":170},"link":{"type":{"ref":"node"}}}},
            "other":{"owner":"ontology","primary_key":{"from":"$action_id"},"properties":{"private_name":{"type":"string"}}}},
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
            "revision":"r0","source_revision":"s0","observed_at":1,"fresh_until":if hidden {1}else{100},"change":"initial","coverage":if hidden {"partial"}else{"complete"},
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
                    let output = read(&req, &probe);
                    let a: Value = serde_json::from_str(&output.stdout).unwrap();
                    if mutate == 0 {
                        assert_eq!(a["available"], json!(["link", "name", "status"]));
                    }
                    if mutate == 3 {
                        req.object_type = "absent".into();
                        assert_eq!(output.stdout, read(&req, &probe).stdout);
                    }
                    assert_eq!(a["status"], "refused");
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
        let mut req = request(if property == "link" {
            &[]
        } else {
            std::slice::from_ref(&property)
        });
        if property == "link" {
            req.filter = Some(Filter::Unresolved(property.into()));
        }
        let a = answer(&mut store, &req);
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

#[test]
fn filter_membership_equals_search_and_historical_filter_is_excluded() {
    let root =
        std::path::PathBuf::from("/home/palantirkc/work/ontology-operational-base/o7b-scratch");
    let path = root.join(format!("r1-filter-{}.sqlite", std::process::id()));
    let mut store = Store::open_schema(&path).unwrap();
    let mut s = schema();
    install(&mut store, &s);
    let mut a = row("a", false);
    a.observation
        .fields
        .insert("status".into(), Knowledge::Missing);
    let mut b = row("b", false);
    b.observation
        .fields
        .insert("name".into(), Knowledge::Missing);
    store.append_schema(&[a, b], 10).unwrap();
    let ctx = Context::new(path.clone(), 10);
    for filter in [
        json!({"unresolved":"status"}),
        json!({"any":[{"eq":["status","resolved"]},{"eq":["name","a"]}]}),
        json!({"none":[{"all":[{"eq":["status","open"]},{"eq":["name","a"]}]}]}),
    ] {
        let req = AggregateRequest::parse(
            json!({"object_type":"node","where":filter,"metrics":["count"]})
                .to_string()
                .as_bytes(),
        )
        .unwrap();
        let search = crate::search::SearchRequest::parse(
            json!({"object_type":"node","where":filter})
                .to_string()
                .as_bytes(),
        )
        .unwrap();
        let matched: Value = serde_json::from_str(
            &crate::cli::render(crate::search::search(&ctx, &search), 10).stdout,
        )
        .unwrap();
        assert_eq!(
            answer(&mut store, &req)["result"]["groups"][0]["count"],
            matched["result"]["basis"]["matched"]
        );
    }
    s["version"] = json!(2);
    s["object_types"]["node"]["properties"]["name"]["type"] = json!("integer");
    install(&mut store, &s);
    let mut req = request(&[]);
    req.filter = Some(Filter::Unresolved("name".into()));
    assert_eq!(
        answer(&mut store, &req)["result"]["basis"]["unresolved_excluded"],
        1
    );
    drop(store);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn declared_types_and_stored_group_bytes_are_bounded_end_to_end() {
    for (ty, max, ok) in [
        (json!("boolean"), None, true),
        (json!("integer"), None, true),
        (json!("timestamp"), None, true),
        (json!("date"), None, true),
        (json!("decimal"), None, false),
        (json!("decimal"), Some(1022), true),
        (json!("string"), None, false),
        (json!("string"), Some(170), true),
        (json!("string"), Some(171), false),
        (json!({"ref":"node"}), None, false),
        (json!({"array":"integer"}), None, false),
        (json!({"struct":{"flag":"boolean"}}), None, true),
        (json!({"struct":{"text":"string"}}), None, false),
    ] {
        let mut s = schema();
        s["object_types"]["node"]["properties"]["name"] = json!({"type":ty,"max_bytes":max});
        let a = answer(&mut memory(&s), &request(&["name"]));
        assert_eq!(a["status"] == "ok", ok, "{a}");
        if !ok {
            assert_eq!(a["property"], "name");
            assert_eq!(a["field"], "/group_by/0");
        }
    }
    let mut s = schema();
    s["object_types"]["node"]["properties"]["name"]["max_bytes"] = Value::Null;
    let mut store = memory(&s);
    let denied = text(&mut store, &request(&["name"]));
    store.append_schema(&[row("a", false)], 10).unwrap();
    assert_eq!(denied, text(&mut store, &request(&["name"])));
    let mut s = schema();
    s["object_types"]["node"]["properties"]["name"] = json!({"type":"decimal","max_bytes":1023});
    let mut store = memory(&s);
    let mut rows = vec![];
    for (key, bytes) in [
        ("at", MAX_GROUP_VALUE_BYTES),
        ("above", MAX_GROUP_VALUE_BYTES + 1),
    ] {
        let mut r = row(key, false);
        r.observation.fields.insert(
            "status".into(),
            Knowledge::Value(json!(if key == "at" { "open" } else { "resolved" })),
        );
        r.observation.fields.insert(
            "name".into(),
            Knowledge::Value(json!("1".repeat(bytes - 2))),
        );
        rows.push(r);
    }
    store.append_schema(&rows, 10).unwrap();
    s["version"] = json!(2);
    s["object_types"]["node"]["properties"]["name"]["max_bytes"] = json!(1022);
    install(&mut store, &s);
    let mut req = request(&["name"]);
    let a = answer(&mut store, &req);
    assert_eq!(a["result"]["groups"][0]["count"], 1);
    assert_eq!(a["result"]["basis"]["unresolved_excluded"], 1);
    req.filter = Some(Filter::Eq("status".into(), json!("open")));
    let a = answer(&mut store, &req);
    assert_eq!(a["status"], "ok");
    assert_eq!(a["result"]["basis"]["unresolved_excluded"], 0);
}
#[test]
fn enum_product_precedes_expansion_order_and_key_identity_are_stable() {
    let mut s = schema();
    let values: Vec<_> = (0..1000).map(|i| i.to_string()).collect();
    s["value_types"]["status"]["enum"] = json!(values);
    s["object_types"]["node"]["properties"]["name"]["type"] = json!({"value_type":"status"});
    EXPANSIONS.set(0);
    assert_eq!(
        answer(&mut memory(&s), &request(&["status", "name"]))["code"],
        "result_too_large"
    );
    assert_eq!(EXPANSIONS.get(), 0);
    s["value_types"]["status"]["enum"] = json!(["resolved", "open"]);
    let a = answer(&mut memory(&s), &request(&["status"]));
    assert_eq!(a["result"]["groups"][0]["key"]["status"], "resolved");
    let pair = answer(&mut memory(&s), &request(&["status", "name"]));
    assert_eq!(
        pair["result"]["groups"][0]["key"],
        json!({"status":"resolved","name":"resolved"})
    );
    assert_eq!(
        pair["result"]["groups"][3]["key"],
        json!({"status":"open","name":"open"})
    );
    let mut groups = BTreeMap::new();
    let mut one = serde_json::Map::new();
    one.insert("b".into(), json!({"y":1,"x":2}));
    one.insert("a".into(), json!(0));
    let mut two = serde_json::Map::new();
    two.insert("a".into(), json!(0));
    two.insert("b".into(), json!({"x":2,"y":1}));
    bucket(&mut groups, Value::Object(one), 1).unwrap();
    bucket(&mut groups, Value::Object(two), 1).unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups.values().next().unwrap()["count"], 2);
}
