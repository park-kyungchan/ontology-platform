//! Revision-2 key contract; synthetic temporary stores only.
#![allow(clippy::unwrap_used)]
use ont_core::schema::{PropertyType, ScalarType};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "ont-key-rule-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn call(&self, cmd: &[&str], body: Value) -> Value {
        let mut args = vec![
            "--store".to_string(),
            self.0.join("store.sqlite").display().to_string(),
            "--now".into(),
            "1000".into(),
        ];
        args.extend(cmd.iter().map(|s| s.to_string()));
        let out = ont::run(&args, body.to_string().as_bytes(), 0, 0);
        assert!(out.stderr.is_empty(), "{}", out.stderr);
        serde_json::from_str(&out.stdout).unwrap()
    }
    fn file(&self, s: &Value) -> String {
        let p = self.0.join("schema.json");
        std::fs::write(&p, s.to_string()).unwrap();
        p.display().to_string()
    }
    fn check(&self, s: &Value) -> Value {
        self.call(&["schema", "check", &self.file(s)], json!({}))
    }
    fn install(&self, s: &Value) {
        let a = self.call(&["schema", "install", &self.file(s)], json!({}));
        assert_eq!(a["status"], "ok", "{a}");
    }
    fn planted(&self, s: &Value) {
        self.install(&schema());
        let bytes = s.to_string().into_bytes();
        let sha = format!("{:x}", Sha256::digest(&bytes));
        let db = rusqlite::Connection::open(self.0.join("store.sqlite")).unwrap();
        db.execute(
            "UPDATE schema_installs SET bytes=?1,sha256=?2",
            rusqlite::params![bytes, sha],
        )
        .unwrap();
        // These fixture changes affect action declarations / primary-key source,
        // not properties or existing projections (the fixture is empty).
        db.execute(
            "UPDATE candidate_generation SET projected=mutation,semantics_projected=mutation",
            [],
        )
        .unwrap();
    }
    fn heads(&self) -> Value {
        self.call(&["search"], json!({"object_type":"item"}))["result"].clone()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn id(ty: &str, key: &str) -> Value {
    json!({"object_type":ty,"namespace":{"instance":"synthetic","account":"test"},"native_key":key})
}
fn schema() -> Value {
    json!({"format":"ontology-schema/v1","domain":"keys","version":1,"namespace":{"instance":"synthetic","account":"test"},
        "value_types":{"label":{"base":"string"}},
        "object_types":{"item":{"owner":"ontology","primary_key":{"from":"/native_key"},"properties":{"title":{"type":"string","required":true}}},"other":{"owner":"ontology","primary_key":{"from":"$action_id"},"properties":{"title":{"type":"string","required":true}}}},
        "action_types":{"write":{"parameters":{"key":{"type":{"ref":"item"},"required":true}},"rules":[{"create_or_modify_object":"item","set":{"title":"written"}}]},"seed_other":{"rules":[{"create_object":"other","set":{"title":"other"}}]}},
        "grants":[{"principal":"agent","read":["item","other"],"derive":["item","other"],"actions":["write","seed_other"]}]})
}
// No wildcard: adding a PropertyType or ScalarType forces a classification here.
fn sample(ty: &PropertyType) -> (Value, bool) {
    match ty {
        PropertyType::Scalar(s) => (
            match s {
                ScalarType::String => json!("K"),
                ScalarType::Integer => json!(7),
                ScalarType::Boolean => json!(true),
                ScalarType::Decimal => json!("1.0"),
                ScalarType::Timestamp => json!(1000),
                ScalarType::Date => json!("2026-11-13"),
            },
            false,
        ),
        PropertyType::Ref(r) => (id(&r.target, "K"), r.target == "item"),
        PropertyType::Array(a) => (json!([sample(&a.array).0]), false),
        PropertyType::Struct(st) => (
            Value::Object(
                st.struct_
                    .iter()
                    .map(|(k, t)| (k.clone(), sample(t).0))
                    .collect(),
            ),
            false,
        ),
        PropertyType::ValueType(_) => (json!("K"), false),
    }
}
#[test]
fn key_parameter_type_table() {
    let mut failures = Vec::new();
    for raw in [
        json!("string"),
        json!("integer"),
        json!("boolean"),
        json!("decimal"),
        json!("timestamp"),
        json!("date"),
        json!({"ref":"item"}),
        json!({"ref":"other"}),
        json!({"array":"string"}),
        json!({"struct":{"native_key":"string"}}),
        json!({"struct":{"native_key":"integer"}}),
        json!({"struct":{"native_key":"boolean"}}),
        json!({"struct":{"native_key":{"array":"string"}}}),
        json!({"struct":{"native_key":{"ref":"item"}}}),
        json!({"value_type":"label"}),
    ] {
        let ty: PropertyType = serde_json::from_value(raw.clone()).unwrap();
        let (value, eligible) = sample(&ty);
        let t = Temp::new();
        let mut s = schema();
        s["action_types"]["write"]["parameters"]["key"]["type"] = raw.clone();
        let checked = t.check(&s);
        let accepted = checked["status"] == "ok";
        println!(
            "type {raw}: check {} expected {eligible}",
            checked["status"]
        );
        if accepted != eligible {
            failures.push(format!("type {raw}: {checked}"));
        }
        if eligible {
            t.install(&s);
        } else {
            t.planted(&s);
        }
        let before = t.heads();
        let req = json!({"action_type":"write","parameters":{"key":value}});
        let v = t.call(&["action", "validate"], req.clone());
        let a = t.call(&["action", "apply"], req);
        if eligible {
            assert_eq!(v["result"]["valid"], true, "{v}");
            assert_eq!(a["status"], "ok", "{a}");
            assert_eq!(v["result"]["edits"][0]["id"], id("item", "K"));
            assert_eq!(a["result"]["edits"][0]["id"], id("item", "K"));
        } else {
            for result in [&v, &a] {
                if result["code"] != "ambiguous_create_key"
                    || result["pointer"] != "/action_types/write"
                {
                    failures.push(format!("type {raw} runtime: {result}"));
                }
            }
            if t.heads() != before {
                failures.push(format!("type {raw}: wrote heads"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
#[test]
fn key_parameter_name_order_and_other_type() {
    for other_name in ["a_other", "zz_other"] {
        for same_type in [true, false] {
            let t = Temp::new();
            let mut s = schema();
            s["action_types"]["write"]["parameters"][other_name] =
                json!({"type":{"ref":if same_type {"item"} else {"other"}},"required":!same_type});
            t.install(&s);
            let other = if same_type {
                let r = t.call(
                    &["action", "apply"],
                    json!({"action_type":"write","parameters":{"key":id("item","other")}}),
                );
                assert_eq!(r["status"], "ok", "{r}");
                id("item", "other")
            } else {
                let r = t.call(
                    &["action", "apply"],
                    json!({"action_type":"seed_other","parameters":{}}),
                );
                r["result"]["edits"][0]["id"].clone()
            };
            let before = t.call(&["get"], json!({"id":other}));
            let req =
                json!({"action_type":"write","parameters":{"key":id("item","K"),other_name:other}});
            let v = t.call(&["action", "validate"], req.clone());
            let a = t.call(&["action", "apply"], req);
            assert_eq!(v["result"]["valid"], true, "{v}");
            assert_eq!(a["status"], "ok", "{a}");
            assert_eq!(v["result"]["edits"][0]["id"], id("item", "K"));
            assert_eq!(a["result"]["edits"][0]["id"], id("item", "K"));
            assert_eq!(t.call(&["get"], json!({"id":other})), before);
        }
    }
}
#[test]
fn duplicate_key_parameters_and_create_rules_refuse() {
    let mut failures = Vec::new();
    for mode in [
        "parameters",
        "create_then_upsert",
        "upsert_then_create",
        "two_creates",
        "two_upserts",
    ] {
        let t = Temp::new();
        let mut s = schema();
        if mode == "parameters" {
            s["action_types"]["write"]["parameters"]["other"] =
                json!({"type":{"ref":"item"},"required":true});
        } else {
            let kinds = match mode {
                "create_then_upsert" => ["create_object", "create_or_modify_object"],
                "upsert_then_create" => ["create_or_modify_object", "create_object"],
                "two_creates" => ["create_object", "create_object"],
                _ => ["create_or_modify_object", "create_or_modify_object"],
            };
            s["action_types"]["write"]["rules"] = json!([{kinds[0]:"item","set":{"title":"one"}},{kinds[1]:"item","set":{"title":"two"}}]);
        }
        let c = t.check(&s);
        println!("duplicate {mode}: {}", c["status"]);
        if c["status"] != "refused"
            || !c["violations"].as_array().is_some_and(|vs| {
                vs.iter().any(|v| {
                    v["code"] == "ambiguous_create_key" && v["pointer"] == "/action_types/write"
                })
            })
        {
            failures.push(format!("{mode}: {c}"));
        }
        t.planted(&s);
        let before = t.heads();
        for op in ["validate", "apply"] {
            let params = if mode == "parameters" {
                json!({"key":id("item","K"),"other":id("item","Q")})
            } else {
                json!({"key":id("item","K")})
            };
            let a = t.call(
                &["action", op],
                json!({"action_type":"write","parameters":params}),
            );
            if a["code"] != "ambiguous_create_key" || a["pointer"] != "/action_types/write" {
                failures.push(format!("{mode} {op}: {a}"));
            }
            if t.heads() != before {
                failures.push(format!("{mode} {op}: wrote heads"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
#[test]
fn other_ontology_key_pointers_refuse_but_installed_bytes_open() {
    let mut failures = Vec::new();
    for pointer in [
        "/key",
        "/object_type",
        "/namespace/instance",
        "/namespace/account",
        "/nested/native_key",
    ] {
        let t = Temp::new();
        let mut s = schema();
        s["object_types"]["item"]["primary_key"]["from"] = json!(pointer);
        // The type-level constraint also holds without an action creating it.
        s["action_types"].as_object_mut().unwrap().remove("write");
        s["grants"][0]["actions"] = json!(["seed_other"]);
        let c = t.check(&s);
        println!("pointer {pointer}: {}", c["status"]);
        if c["status"] != "refused" {
            failures.push(format!("{pointer}: {c}"));
        }
        t.planted(&s);
        let d = t.call(&["describe"], json!({}));
        assert_eq!(d["status"], "ok", "{d}");
        assert!(t.heads().is_object());
        s["version"] = json!(2);
        s["object_types"]["item"]["primary_key"]["from"] = json!("/native_key");
        t.install(&s);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
#[test]
fn key_namespace_and_id_bounds_agree_without_writes() {
    let mut failures = Vec::new();
    for mode in [
        "instance",
        "account",
        "empty",
        "oversized",
        "integer",
        "missing",
        "null",
    ] {
        let t = Temp::new();
        t.install(&schema());
        let mut key = id("item", "K");
        match mode {
            "instance" => key["namespace"]["instance"] = json!("foreign"),
            "account" => key["namespace"]["account"] = json!("foreign"),
            "empty" => key["native_key"] = json!(""),
            "oversized" => key["native_key"] = json!("X".repeat(513)),
            "integer" => key["native_key"] = json!(7),
            "missing" => {
                key.as_object_mut().unwrap().remove("native_key");
            }
            _ => key["native_key"] = Value::Null,
        }
        let before = t.heads();
        for op in ["validate", "apply"] {
            let a = t.call(
                &["action", op],
                json!({"action_type":"write","parameters":{"key":key}}),
            );
            let field = if mode == "instance" || mode == "account" {
                "/parameters/key/namespace"
            } else {
                "/parameters/key"
            };
            println!("invalid id {mode} {op}: {}", a["code"]);
            if a["code"] != "invalid_parameter" || a["field"] != field {
                failures.push(format!("{mode} {op}: {a}"));
            }
            if t.heads() != before {
                failures.push(format!("{mode} {op}: wrote heads"));
            }
        }
        if t.0.join("edits.jsonl").exists() {
            failures.push(format!("{mode}: edits file written"));
        }
    }
    // Boundary values accepted by Id validation must work at both entry points.
    for key in ["K".to_string(), "X".repeat(512), "\u{1f642}".repeat(128)] {
        let t = Temp::new();
        t.install(&schema());
        let req = json!({"action_type":"write","parameters":{"key":id("item",&key)}});
        let v = t.call(&["action", "validate"], req.clone());
        let a = t.call(&["action", "apply"], req);
        assert_eq!(v["result"]["valid"], true, "{v}");
        assert_eq!(a["status"], "ok", "{a}");
        assert_eq!(a["result"]["edits"][0]["id"], id("item", &key));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn key_preview_matches_create_update_and_resurrection() {
    let t = Temp::new();
    let mut s = schema();
    s["action_types"]["delete"] = json!({"parameters":{"key":{"type":{"ref":"item"},"required":true}},"rules":[{"delete_object":"item"}]});
    s["grants"][0]["actions"]
        .as_array_mut()
        .unwrap()
        .push(json!("delete"));
    t.install(&s);
    for (action, change) in [
        ("write", "initial"),
        ("write", "update"),
        ("delete", "retraction"),
        ("write", "initial"),
    ] {
        let req = json!({"action_type":action,"parameters":{"key":id("item","K")}});
        let v = t.call(&["action", "validate"], req.clone());
        let a = t.call(&["action", "apply"], req);
        assert_eq!(v["result"]["valid"], true, "{v}");
        assert_eq!(a["status"], "ok", "{a}");
        for r in [&v, &a] {
            assert_eq!(r["result"]["edits"][0]["change"], change, "{r}");
            assert_eq!(r["result"]["edits"][0]["id"], id("item", "K"));
        }
    }
}
