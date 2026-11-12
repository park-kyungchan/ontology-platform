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
            "views":{"all_nodes":{"object_type":"node","args":[],"order":[{"property":"text","direction":"asc"}],"columns":[{"header":"Text","property":"text"}]}},
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
fn get_150_observations_returns_latest_100_and_omitted_50() {
    let t = Temp::new();
    let mut item = row("history", "ok");
    let mut rows = vec![item.clone()];
    for i in 1..150 {
        item.observation.supersedes = Some(item.observation.revision.clone());
        item.observation.revision = format!("r{i}");
        item.observation.change = Change::Update;
        rows.push(item.clone());
    }
    t.put(&rows);
    // Even undecodable older rows must never be loaded by the recent window.
    let db = rusqlite::Connection::open(t.path()).unwrap();
    db.execute(
        "UPDATE observations SET payload='invalid' WHERE ordinal<50",
        [],
    )
    .unwrap();
    drop(db);
    // Repair the projection after synthetic corruption, without reading old rows.
    t.store()
        .install_schema(t.schema.to_string().as_bytes(), Default::default())
        .unwrap();
    let answer = t.cli("get", json!({"id":item.id}));
    assert_eq!(answer["status"], "ok", "{answer}");
    let object = &answer["result"]["objects"][0];
    let observations = object["observations"].as_array().unwrap();
    assert_eq!(observations.len(), 100);
    assert_eq!(object["head"], "r149");
    let revisions: Vec<_> = observations
        .iter()
        .map(|o| o["revision"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        revisions,
        (50..150).map(|i| format!("r{i}")).collect::<Vec<_>>()
    );
    assert_eq!(
        answer["result"]["basis"],
        json!({"older_omitted":50,"oversized_excluded":0})
    );
    let db = rusqlite::Connection::open(t.path()).unwrap();
    db.execute(
        "UPDATE observations SET payload=?1 WHERE ordinal=75",
        ["!".repeat(BYTES + 1)],
    )
    .unwrap();
    drop(db);
    t.store()
        .install_schema(t.schema.to_string().as_bytes(), Default::default())
        .unwrap();
    let answer = t.cli("get", json!({"id":item.id}));
    assert_eq!(answer["status"], "ok", "{answer}");
    assert_eq!(
        answer["result"]["objects"][0]["observations"]
            .as_array()
            .unwrap()
            .len(),
        99
    );
    assert_eq!(
        answer["result"]["basis"],
        json!({"older_omitted":50,"oversized_excluded":1})
    );
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
fn get_excludes_superseded_oversized_observation() {
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
    let answer = t.cli("get", json!({"id":item.id}));
    assert_eq!(answer["status"], "ok", "{answer}");
    assert_eq!(
        answer["result"]["basis"],
        json!({"older_omitted":0,"oversized_excluded":1})
    );
    let object = &answer["result"]["objects"][0];
    assert_eq!(object["head"], "new");
    let revisions: Vec<_> = object["observations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["revision"].as_str().unwrap())
        .collect();
    assert_eq!(revisions, ["r0", "new"]);
}

#[test]
fn render_refuses_oversized_legacy_heads_instead_of_omitting_rows() {
    for count in [1, 1001] {
        let t = Temp::new();
        let mut rows: Vec<_> = (0..count).map(|i| row(&format!("a{i:04}"), "ok")).collect();
        rows.push(row("z-poison", ""));
        t.put(&rows);
        t.legacy("z-poison", &"!".repeat(BYTES + 1));
        // The 1001-row case places poison after the first search page;
        // render must fail rather than publish a quietly incomplete board.
        let answer = t.cli("render", json!({"view":"all_nodes","args":{}}));
        assert_eq!(answer["code"], "observation_too_large", "{answer}");
    }
}

#[test]
fn array_bound_is_per_array_not_total_items() {
    let mut observation = row("two-arrays", "").observation;
    observation.fields.insert(
        "values".into(),
        Knowledge::Value(json!({
            "left": vec![0; ITEMS], "right": vec![0; ITEMS]
        })),
    );
    assert!(ont_core::observation_payload(&observation).is_ok());
}

#[test]
fn hidden_oversized_head_changes_no_set_read_answer_or_counter() {
    let mut t = Temp::new();
    t.schema["version"] = json!(2);
    // This fixture needs only Ontology-owned types; file-source schemas have
    // no scope-key mapping and must remain unscoped.
    t.schema["sources"] = json!({});
    t.schema["object_types"]
        .as_object_mut()
        .unwrap()
        .remove("indexed");
    t.schema["grants"][0]["read"] = json!(["node", "sentinel"]);
    t.schema["grants"][0]["derive"] = json!(["node"]);
    t.schema["scope_keys"] = json!(["tenant"]);
    t.schema["grants"][0]["scope"] = json!({"tenant":"public"});
    t.store()
        .install_schema(t.schema.to_string().as_bytes(), Default::default())
        .unwrap();
    let mut good = row("good", "ok");
    good.scope = Scope::domain(
        good.id.namespace.clone(),
        "test",
        BTreeMap::from([("tenant".into(), "public".into())]),
    )
    .unwrap();
    let mut hidden = row("hidden", "");
    hidden.scope = Scope::domain(
        hidden.id.namespace.clone(),
        "test",
        BTreeMap::from([("tenant".into(), "private".into())]),
    )
    .unwrap();
    good.observation
        .fields
        .insert("edges".into(), Knowledge::Value(json!([hidden.id])));
    t.put(&[good]);
    let reads = [
        ("search", json!({"object_type":"node"})),
        (
            "around",
            json!({"from":{"object_type":"node"},"path":["next"]}),
        ),
        (
            "aggregate",
            json!({"object_type":"node","metrics":["count"]}),
        ),
    ];
    let baseline: Vec<_> = reads
        .iter()
        .map(|(cmd, req)| t.cli(cmd, req.clone()))
        .collect();
    t.put(&[hidden]);
    t.legacy("hidden", &"!".repeat(BYTES + 1));
    for ((cmd, req), expected) in reads.into_iter().zip(baseline) {
        let answer = t.cli(cmd, req);
        assert_eq!(answer["status"], "ok", "{cmd}: {answer}");
        assert_eq!(answer["result"]["basis"]["oversized_excluded"], 0);
        assert_eq!(answer, expected, "hidden head changed {cmd}");
    }
}
