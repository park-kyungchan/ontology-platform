//! O21: synthetic stores only; legacy poison is written directly through SQL.
#![allow(clippy::unwrap_used)]
use ont_core::{Change, Coverage, Id, Knowledge, Lineage, Observation, Scope};
use ont_store::{Append, Store};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
const BYTES: usize = 65_536;
const ITEMS: usize = 1_024;
struct Temp {
    root: PathBuf,
    schema: Value,
}
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "ont-o21-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let schema = json!({"format":"ontology-schema/v1","domain":"test","version":1,
            "namespace":{"instance":"test","account":"root"},"scope_keys":[],"value_types":{},
            "sources":{"docs":{"kind":"markdown_fields","glob":format!("{}/*.txt",root.display()),"head_lines":40,"complete_listing":true,"fresh_for_s":90}},
            "object_types":{
                "node":{"owner":"ontology","primary_key":{"from":"$action_id"},"properties":{"text":{"type":"string"},"values":{"type":{"array":"integer"},"required":false},"edges":{"type":{"array":{"ref":"node"}},"required":false}}},
                "sentinel":{"owner":"ontology","primary_key":{"from":"$action_id"},"properties":{"text":{"type":"string"}}},
                "indexed":{"owner":{"source":"docs"},"primary_key":{"from":"/path"},"properties":{"text":{"type":"string","from":"/fields/Text"}}}},
            "link_types":{"edge":{"from":"node","via":"edges","to":"node","cardinality":"many_to_many","names":{"forward":"next","reverse":"previous"}}},
            "action_types":{"write":{"parameters":{"text":{"type":"string","required":true}},"rules":[
                {"create_object":"sentinel","set":{"text":"small"}},
                {"create_object":"node","set":{"text":"$text"}}]}},
            "functions":{},"grants":[{"principal":"agent","scope":{},"read":["node","indexed","sentinel"],"derive":["node","indexed"],"actions":["write"]}]});
        let t = Self { root, schema };
        Store::open_schema(&t.path())
            .unwrap()
            .install_schema(t.schema.to_string().as_bytes(), Default::default())
            .unwrap();
        t
    }
    fn path(&self) -> PathBuf {
        self.root.join("store.sqlite")
    }
    fn store(&self) -> Store {
        Store::open_schema_existing_writable(&self.path()).unwrap()
    }
    fn cli(&self, command: &str, request: Value) -> Value {
        let mut args = vec![
            "--store".into(),
            self.path().display().to_string(),
            "--now".into(),
            "10".into(),
        ];
        args.extend(command.split_whitespace().map(str::to_string));
        let out = ont::run(&args, request.to_string().as_bytes(), 0, 0);
        serde_json::from_str(&out.stdout).unwrap()
    }
    fn put(&self, rows: &[Append]) {
        self.store().append_schema(rows, 10).unwrap();
    }
    fn legacy(&self, key: &str, payload: &str) {
        let db = rusqlite::Connection::open(self.path()).unwrap();
        db.execute("INSERT INTO observations SELECT object_type,instance,account,native_key,'legacy',1,?1,10,schema_domain,schema_version,'update' FROM observations WHERE native_key=?2 AND ordinal=0",rusqlite::params![payload,key]).unwrap();
        db.execute(
            "UPDATE objects SET head='legacy' WHERE native_key=?1",
            [key],
        )
        .unwrap();
        drop(db);
        // Supported identical-schema repair also has to tolerate legacy poison.
        self.store()
            .install_schema(self.schema.to_string().as_bytes(), Default::default())
            .unwrap();
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn row(key: &str, text: &str) -> Append {
    let id:Id=serde_json::from_value(json!({"object_type":"node","namespace":{"instance":"test","account":"root"},"native_key":key})).unwrap();
    Append {
        scope: Scope::domain(id.namespace.clone(), "test", BTreeMap::new()).unwrap(),
        id,
        observation: Observation {
            revision: "r0".into(),
            source_revision: "s0".into(),
            observed_at: 10,
            fresh_until: 100,
            change: Change::Initial,
            supersedes: None,
            coverage: Coverage::Complete,
            fields: BTreeMap::from([("text".into(), Knowledge::Value(json!(text)))]),
            lineage: Lineage {
                artifact_sha256: "a".repeat(64),
                pointer: String::new(),
            },
        },
    }
}
fn pad(obs: &mut Observation, bytes: usize) {
    let base = serde_json::to_vec(obs).unwrap().len();
    let Knowledge::Value(Value::String(text)) = obs.fields.get_mut("text").unwrap() else {
        panic!()
    };
    text.push_str(&"x".repeat(bytes - base));
    assert_eq!(serde_json::to_vec(obs).unwrap().len(), bytes);
}
#[test]
fn ingest_exact_byte_bound_and_batch_rollback() {
    let t = Temp::new();
    let mut exact = row("exact", "");
    pad(&mut exact.observation, BYTES);
    t.put(&[exact]);
    let mut big = row("large", "");
    pad(&mut big.observation, BYTES + 1);
    let e = t
        .store()
        .append_schema(&[row("first", "small"), big], 10)
        .unwrap_err();
    assert_eq!(e.code, "observation_too_large");
    assert!(
        t.store()
            .head_access(&row("first", "").id)
            .unwrap()
            .is_none()
    );
    assert!(
        t.store()
            .head_access(&row("large", "").id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        t.cli("get", json!({"id":row("exact","").id}))["status"],
        "ok"
    );
}
#[test]
fn index_exact_byte_bound_and_source_rollback() {
    for extra in [0, 1] {
        let t = Temp::new();
        let mut obs = row("probe", "").observation;
        obs.revision = "r0@10".into();
        obs.source_revision = "a".repeat(64);
        let n = BYTES - serde_json::to_vec(&obs).unwrap().len() + extra;
        std::fs::write(t.root.join("a.txt"), "Text: small\n").unwrap();
        std::fs::write(t.root.join("z.txt"), format!("Text: {}\n", "x".repeat(n))).unwrap();
        let answer = t.cli("index", json!({}));
        if extra == 0 {
            assert_eq!(answer["result"]["sources"][0]["status"], "ok", "{answer}");
            let db = rusqlite::Connection::open(t.path()).unwrap();
            let size: usize = db
                .query_row(
                    "SELECT max(length(CAST(payload AS BLOB))) FROM observations",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(size, BYTES);
        } else {
            assert_eq!(
                answer["result"]["sources"][0]["code"], "observation_too_large",
                "{answer}"
            );
            let db = rusqlite::Connection::open(t.path()).unwrap();
            assert_eq!(
                db.query_row("SELECT count(*) FROM objects", [], |r| r.get::<_, u64>(0))
                    .unwrap(),
                0
            );
        }
    }
}
#[test]
fn action_exact_byte_bound_and_all_edits_rollback() {
    let probe = Temp::new();
    let request = |text: String| json!({"action_type":"write","request_id":"fixed","parameters":{"text":text}});
    let applied = probe.cli("action apply", request(String::new()));
    assert_eq!(applied["status"], "ok", "{applied}");
    let mut target = row("", "").id;
    target.native_key = applied["result"]["action_id"].as_str().unwrap().into();
    let base = probe
        .store()
        .head(&target)
        .unwrap()
        .unwrap()
        .head
        .observation;
    let overhead = serde_json::to_vec(&base).unwrap().len();
    for extra in [0, 1] {
        let t = Temp::new();
        let answer = t.cli(
            "action apply",
            request("x".repeat(BYTES - overhead + extra)),
        );
        if extra == 0 {
            assert_eq!(answer["status"], "ok", "{answer}");
            let obs = t.store().head(&target).unwrap().unwrap().head.observation;
            assert_eq!(serde_json::to_vec(&obs).unwrap().len(), BYTES);
        } else {
            assert_eq!(answer["code"], "observation_too_large", "{answer}");
            let db = rusqlite::Connection::open(t.path()).unwrap();
            for table in ["objects", "observations", "action_log", "head_candidates"] {
                assert_eq!(
                    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                        .get::<_, u64>(0))
                        .unwrap(),
                    0,
                    "{table}"
                );
            }
            assert!(!t.root.join("edits.jsonl").exists());
        }
    }
}
#[test]
fn array_bound_includes_nested_values() {
    let mut item = row("arrays", "");
    item.observation
        .fields
        .insert("values".into(), Knowledge::Value(json!(vec![0; ITEMS])));
    let t = Temp::new();
    t.put(&[item.clone()]);
    item.id.native_key = "above".into();
    item.observation
        .fields
        .insert("values".into(), Knowledge::Value(json!(vec![0; ITEMS + 1])));
    assert_eq!(
        t.store()
            .append_schema(&[item.clone()], 10)
            .unwrap_err()
            .code,
        "observation_too_large"
    );
    item.observation.fields.insert(
        "values".into(),
        Knowledge::Value(json!({"nested":[vec![0;ITEMS+1]]})),
    );
    assert_eq!(
        ont_core::observation_payload(&item.observation)
            .unwrap_err()
            .code(),
        "observation_too_large"
    );
}
#[test]
fn legacy_byte_array_and_utf8_poison_excluded_by_set_reads_get_refuses() {
    for kind in ["bytes", "array", "nested", "utf8"] {
        let t = Temp::new();
        t.put(&[row("bad", ""), row("good", "ok")]);
        let payload = match kind {
            // Invalid JSON additionally proves the over-byte guard precedes JSON decoding.
            "bytes" => "!".repeat(BYTES + 1),
            "array" => {
                let mut o = row("bad", "").observation;
                o.fields
                    .insert("values".into(), Knowledge::Value(json!(vec![0; ITEMS + 1])));
                serde_json::to_string(&o).unwrap()
            }
            "nested" => {
                let mut o = row("bad", "").observation;
                o.fields.insert(
                    "values".into(),
                    Knowledge::Value(json!({"inner":[vec![0; ITEMS + 1]]})),
                );
                serde_json::to_string(&o).unwrap()
            }
            _ => {
                let o = row("bad", &"é".repeat(BYTES / 2)).observation;
                let s = serde_json::to_string(&o).unwrap();
                assert!(s.chars().count() < BYTES && s.len() > BYTES);
                s
            }
        };
        t.legacy("bad", &payload);
        for (command, request) in [
            (
                "search",
                json!({"object_type":"node","where":{"eq":["text","not present"]}}),
            ),
            (
                "around",
                json!({"from":{"object_type":"node"},"path":["next"]}),
            ),
            (
                "aggregate",
                json!({"object_type":"node","metrics":["count"]}),
            ),
        ] {
            let answer = t.cli(command, request);
            assert_eq!(answer["status"], "ok", "{kind} {command}: {answer}");
            assert_eq!(
                answer["result"]["basis"]["oversized_excluded"], 1,
                "{answer}"
            );
            assert_eq!(answer["result"]["basis"]["qualified"], false);
            assert_eq!(
                answer["result"]["basis"]["scanned"],
                if command == "search" { 1 } else { 2 }
            );
            if command == "aggregate" {
                assert_eq!(answer["result"]["groups"][0]["count"], 1);
            }
        }
        assert_eq!(
            t.cli("get", json!({"id":row("bad","").id}))["code"],
            "observation_too_large"
        );
        // Hiding the type must conceal the refusal and history-size information.
        let mut schema = t.schema.clone();
        schema["version"] = json!(2);
        schema["grants"][0]["read"] = json!(["indexed"]);
        schema["grants"][0]["derive"] = json!(["indexed"]);
        t.store()
            .install_schema(schema.to_string().as_bytes(), Default::default())
            .unwrap();
        assert_eq!(
            t.cli("get", json!({"id":row("bad","").id})),
            t.cli("get", json!({"id":row("missing","").id}))
        );
    }
}
#[test]
fn get_history_bound_precedes_payload_loading() {
    let t = Temp::new();
    let mut item = row("history", "ok");
    t.put(&[item.clone()]);
    for i in 1..100 {
        item.observation.supersedes = Some(item.observation.revision.clone());
        item.observation.revision = format!("r{i}");
        item.observation.change = Change::Update;
        t.put(&[item.clone()]);
    }
    let req = json!({"id":item.id});
    let answer = t.cli("get", req.clone());
    assert_eq!(
        answer["result"]["objects"][0]["observations"]
            .as_array()
            .unwrap()
            .len(),
        100
    );
    item.observation.supersedes = Some(item.observation.revision.clone());
    item.observation.revision = "r100".into();
    t.put(&[item]);
    let db = rusqlite::Connection::open(t.path()).unwrap();
    db.execute(
        "UPDATE observations SET payload='invalid' WHERE ordinal=0",
        [],
    )
    .unwrap();
    drop(db);
    // A decode-first or count-after-copy implementation gets store_failure here.
    assert_eq!(t.cli("get", req)["code"], "result_too_large");
}

#[test]
fn repeated_links_count_one_oversized_head_once() {
    let t = Temp::new();
    let mut root = row("root", "root");
    root.observation.fields.insert(
        "edges".into(),
        Knowledge::Value(json!([row("bad", "").id, row("bad", "").id])),
    );
    t.put(&[root, row("bad", "")]);
    t.legacy("bad", &"!".repeat(BYTES + 1));
    for path in [json!(["next"]), json!(["previous"])] {
        let answer = t.cli(
            "around",
            json!({"from":{"object_type":"node","where":{"eq":["text","root"]}},"path":path}),
        );
        assert_eq!(answer["status"], "ok", "{answer}");
        assert_eq!(answer["result"]["basis"]["oversized_excluded"], 1);
        assert_eq!(answer["result"]["basis"]["qualified"], false);
    }
}

#[test]
fn get_checks_non_head_historical_observations() {
    let t = Temp::new();
    t.put(&[row("history", "old")]);
    t.legacy("history", &"!".repeat(BYTES + 1));
    let mut item = row("history", "restored");
    item.observation.revision = "new".into();
    item.observation.supersedes = Some("legacy".into());
    item.observation.change = Change::Update;
    t.put(&[item.clone()]);
    let answer = t.cli("search", json!({"object_type":"node"}));
    assert_eq!(answer["result"]["basis"]["qualified"], true);
    assert_eq!(answer["result"]["basis"]["oversized_excluded"], 0);
    assert_eq!(
        t.cli("get", json!({"id":item.id}))["code"],
        "observation_too_large"
    );
}
