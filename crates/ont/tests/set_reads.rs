//! O7: synthetic object sets only, with byte-equal hidden/missing answers.
#![allow(clippy::unwrap_used)]
use ont_core::{Change, Coverage, Id, Knowledge, Lineage, Observation, Scope};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
struct Temp(PathBuf);
fn id(key: &str) -> Id {
    serde_json::from_value(json!({"object_type":"node","namespace":{"instance":"test","account":"root"},"native_key":key})).unwrap()
}
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let t = Self(std::env::temp_dir().join(format!(
            "ont-sets-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )));
        std::fs::create_dir(&t.0).unwrap();
        let s = json!({"format":"ontology-schema/v1","domain":"test","version":1,
            "namespace":{"instance":"test","account":"root"},"scope_keys":["tenant"],"value_types":{},"sources":{},
            "object_types":{"node":{"owner":"ontology","primary_key":{"from":"$action_id"},"title":"name",
                "properties":{"name":{"type":"string","required":false},"edges":{"type":{"array":{"ref":"node"}},"required":false},"leaf":{"type":{"ref":"leaf"},"required":false}}},"leaf":{"owner":"ontology","primary_key":{"from":"$action_id"},"properties":{}}},
            "link_types":{"edge":{"from":"node","via":"edges","to":"node","cardinality":"many_to_many","names":{"forward":"next","reverse":"previous"}},"node_leaf":{"from":"node","via":"leaf","to":"leaf","cardinality":"many_to_one","names":{"forward":"leaf","reverse":"parents"}}},
            "action_types":{},"functions":{},"grants":[{"principal":"agent","scope":{"tenant":"public"},"read":["node","leaf"],"derive":[],"actions":[]}]});
        std::fs::write(t.0.join("schema.json"), s.to_string()).unwrap();
        let out = ont::run(
            &[
                "--store".into(),
                t.store().display().to_string(),
                "schema".into(),
                "install".into(),
                t.0.join("schema.json").display().to_string(),
            ],
            &b""[..],
            0,
            0,
        );
        assert_eq!(out.code, 0, "{}", out.stdout);
        t
    }
    fn store(&self) -> PathBuf {
        self.0.join("store.sqlite")
    }
    fn put(&self, key: &str, targets: &[&str], hidden: bool) {
        self.append(vec![row(key, targets, hidden)]);
    }
    fn append(&self, rows: Vec<ont_store::Append>) {
        ont_store::Store::open_schema_existing_writable(&self.store())
            .unwrap()
            .append_schema(&rows, 10)
            .unwrap();
    }
    fn cli(&self, command: &str, request: &Value) -> String {
        let out = ont::run(
            &[
                "--store".into(),
                self.store().display().to_string(),
                "--now".into(),
                "10".into(),
                command.into(),
            ],
            request.to_string().as_bytes(),
            0,
            0,
        );
        assert!(out.stderr.is_empty(), "{}", out.stderr);
        out.stdout
    }
    fn both(&self, request: Value) -> Value {
        let text = self.cli(
            if request["op"] == "aggregate" {
                "aggregate"
            } else {
                "around"
            },
            &request,
        );
        let config = ont::mcp::Config {
            store: self.store(),
            now: Some(10),
            client: "test".into(),
        };
        let mut server = ont::mcp::Server::new(config, &ont::cli::execute);
        let out = server.handle_line(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ont_search","arguments":request}}).to_string().as_bytes()).unwrap();
        assert_eq!(
            out["result"]["content"][0]["text"].as_str().unwrap(),
            text.trim_end()
        );
        serde_json::from_str(&text).unwrap()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn row(key: &str, targets: &[&str], hidden: bool) -> ont_store::Append {
    ont_store::Append {
        id: id(key),
        scope: Scope::domain(
            id(key).namespace,
            "test",
            BTreeMap::from([(
                "tenant".into(),
                if hidden { "private" } else { "public" }.into(),
            )]),
        )
        .unwrap(),
        observation: Observation {
            revision: "r0".into(),
            source_revision: "s0".into(),
            observed_at: 1,
            fresh_until: 100,
            change: Change::Initial,
            supersedes: None,
            coverage: Coverage::Complete,
            fields: BTreeMap::from([
                ("name".into(), Knowledge::Value(json!(key))),
                (
                    "edges".into(),
                    Knowledge::Value(json!(targets.iter().map(|s| id(s)).collect::<Vec<_>>())),
                ),
            ]),
            lineage: Lineage {
                artifact_sha256: "a".repeat(64),
                pointer: "/synthetic".into(),
            },
        },
    }
}
fn req(name: &str, path: &[&str]) -> Value {
    json!({"op":"search_around","from":{"object_type":"node","where":{"eq":["name",name]}},"path":path})
}
fn keys(answer: &Value) -> Vec<&str> {
    answer["result"]["objects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["id"]["native_key"].as_str().unwrap())
        .collect()
}
#[test]
fn around_one_two_three_hops_both_directions_dedup_and_limit() {
    let t = Temp::new();
    t.put("a", &["b", "b", "c"], false);
    t.put("b", &["c"], false);
    t.put("c", &["d"], false);
    t.put("d", &[], false);
    for (name, path, expected) in [
        ("a", vec!["next"], vec!["b", "c"]),
        ("a", vec!["next", "next"], vec!["c", "d"]),
        ("a", vec!["next", "next", "next"], vec!["d"]),
        ("d", vec!["previous"], vec!["c"]),
        ("d", vec!["previous", "previous"], vec!["a", "b"]),
        ("d", vec!["previous", "previous", "previous"], vec!["a"]),
        ("a", vec!["next", "previous"], vec!["a", "b"]),
    ] {
        let answer = t.both(req(name, &path));
        assert_eq!(keys(&answer), expected, "{answer}");
        assert_eq!(answer["result"]["basis"]["hops"], path.len());
        assert_eq!(answer["result"]["basis"]["qualified"], true);
        let scanned = if path == ["next"] { 3 } else { 4 };
        assert_eq!(answer["result"]["basis"]["scanned"], scanned);
    }
    let mut r = req("a", &["next"]);
    r["limit"] = json!(1);
    let a = t.both(r);
    assert_eq!(keys(&a), ["b"]);
    assert_eq!(a["result"]["basis"]["matched"], 2);
    let (exit, describe) = {
        let out = ont::run(
            &[
                "--store".into(),
                t.store().display().to_string(),
                "describe".into(),
            ],
            &b"{}"[..],
            0,
            0,
        );
        (
            out.code,
            serde_json::from_str::<Value>(&out.stdout).unwrap(),
        )
    };
    assert_eq!(exit, 0);
    assert_eq!(
        describe["result"]["link_types"][0]["names"],
        json!({"forward":"next","reverse":"previous"})
    );
}
#[test]
fn around_hidden_missing_retracted_parity_forward_and_reverse() {
    let missing = Temp::new();
    let hidden = Temp::new();
    let retracted = Temp::new();
    for t in [&missing, &hidden, &retracted] {
        t.put("a", &["z"], false);
    }
    hidden.put("z", &["a"], true);
    let mut r = row("z", &["a"], false);
    r.observation.change = Change::Retraction;
    r.observation.fields.clear();
    retracted.put("z", &["a"], false);
    r.observation.revision = "r1".into();
    r.observation.supersedes = Some("r0".into());
    retracted.append(vec![r]);
    for path in [vec!["next"], vec!["previous"], vec!["next", "previous"]] {
        let r = req("a", &path);
        let a = missing.cli("around", &r);
        assert_eq!(a, hidden.cli("around", &r));
        assert_eq!(a, retracted.cli("around", &r));
        let answer = hidden.both(r);
        assert!(keys(&answer).is_empty());
        if path[0] == "next" {
            assert_eq!(answer["result"]["basis"]["dangling"], 1);
            assert_eq!(answer["result"]["basis"]["qualified"], false);
        }
    }
}
#[test]
fn around_located_path_limit_and_command_refusals() {
    let t = Temp::new();
    for (path, code, field) in [
        (vec![], "invalid_path", "/path"),
        (vec!["next"; 4], "invalid_path", "/path"),
        (vec!["unknown"], "invalid_link", "/path/0"),
    ] {
        let answer = t.both(req("a", &path));
        assert_eq!(answer["code"], code);
        assert_eq!(answer["field"], field);
        assert!(answer["rule"].is_string());
        if code == "invalid_link" {
            assert_eq!(answer["available"], json!(["leaf", "next", "previous"]));
        }
    }
    let a = t.both(req("a", &["leaf", "next"]));
    assert_eq!(a["code"], "invalid_link");
    assert_eq!(a["field"], "/path/1");
    assert_eq!(a["available"], json!(["parents"]));
    for limit in [0, 1001] {
        let mut r = req("a", &["next"]);
        r["limit"] = json!(limit);
        let a = t.both(r);
        assert_eq!(a["field"], "/limit");
        assert!(a["rule"].as_str().unwrap().contains("1000"));
    }
    let text = t.cli("search", &req("a", &["next"]));
    let a: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(a["code"], "invalid_request");
    // op is checked before required search-only fields, to teach the command.
    assert!(a["rule"].as_str().unwrap().contains("around"), "{a}");
}
#[test]
fn around_qualification_includes_filters_links_partial_and_stale() {
    for state in ["filter", "filter_unresolved", "link", "partial", "stale"] {
        let t = Temp::new();
        let mut a = row("a", &["b"], false);
        let b = row("b", &[], false);
        match state {
            "filter" | "filter_unresolved" => {
                a.observation
                    .fields
                    .insert("name".into(), Knowledge::Missing);
            }
            "link" => {
                a.observation
                    .fields
                    .insert("edges".into(), Knowledge::Unknown);
            }
            "partial" => a.observation.coverage = Coverage::Partial,
            "stale" => a.observation.fresh_until = 9,
            _ => unreachable!(),
        }
        t.append(vec![a, b]);
        let mut request = req("a", &["next"]);
        if state == "filter_unresolved" {
            request["from"]["where"] = json!({"unresolved":"name"});
        }
        let answer = t.both(request);
        assert_eq!(
            answer["result"]["basis"]["qualified"], false,
            "{state}: {answer}"
        );
        if state == "filter" {
            assert_eq!(answer["result"]["basis"]["unresolved_excluded"], 1);
        }
    }
}
#[test]
fn around_result_default_maximum_and_admitted_scan_bound() {
    let t = Temp::new();
    let targets: Vec<String> = (0..1001).map(|n| format!("n{n:05}")).collect();
    let refs: Vec<&str> = targets.iter().map(String::as_str).collect();
    let mut rows = vec![row("a", &refs, false)];
    rows.extend(targets.iter().map(|key| row(key, &[], false)));
    t.append(rows);
    for (limit, expected) in [(None, 100), (Some(1000), 1000)] {
        let mut r = req("a", &["next"]);
        if let Some(n) = limit {
            r["limit"] = json!(n);
        }
        let a: Value = serde_json::from_str(&t.cli("around", &r)).unwrap();
        assert_eq!(keys(&a).len(), expected);
        assert_eq!(a["result"]["basis"]["matched"], 1001);
    }
    let mut rows: Vec<_> = (1001..9999)
        .map(|n| row(&format!("n{n:05}"), &[], false))
        .collect();
    rows.push(row("hidden", &[], true));
    t.append(rows);
    let mut r = req("absent", &["next"]);
    assert_eq!(t.both(r.clone())["result"]["basis"]["scanned"], 0);
    // An unfiltered first hop still consumes the whole admitted-head allowance.
    r["from"]["where"] = Value::Null;
    let a = t.both(r.clone());
    assert_eq!(a["result"]["basis"]["scanned"], 10000);
    t.put("one_above", &[], false);
    let a = t.both(r);
    assert_eq!(a["code"], "result_too_large");
}

#[test]
fn around_single_reference_forward_and_reverse() {
    let t = Temp::new();
    let mut a = row("a", &[], false);
    let mut leaf = row("z", &[], false);
    leaf.id.object_type = "leaf".into();
    leaf.observation.fields.clear();
    a.observation
        .fields
        .insert("leaf".into(), Knowledge::Value(json!(leaf.id)));
    t.append(vec![a, leaf]);
    let a = t.both(req("a", &["leaf"]));
    assert_eq!(keys(&a), ["z"]);
    assert_eq!(a["result"]["basis"]["qualified"], true);
    let a = t.both(json!({"op":"search_around","from":{"object_type":"leaf"},"path":["parents"]}));
    assert_eq!(keys(&a), ["a"]);
    assert_eq!(a["result"]["basis"]["qualified"], true);
}

#[test]
fn around_reference_entries_at_and_above_bound_forward_and_reverse() {
    let bound = ont::around::MAX_REFERENCE_ENTRIES;
    for reverse in [false, true] {
        for missing in [false, true] {
            for count in [bound, bound + 1] {
                let t = Temp::new();
                let targets = vec![
                    if missing {
                        "z"
                    } else if reverse {
                        "a"
                    } else {
                        "b"
                    };
                    count
                ];
                if reverse {
                    t.put("a", &[], false);
                    t.put("b", &targets, false);
                } else {
                    t.put("a", &targets, false);
                    t.put("b", &[], false);
                }
                let request = req("a", &[if reverse { "previous" } else { "next" }]);
                // Reverse success returns the large array: above the MCP cap.
                let answer = if reverse && !missing && count == bound {
                    serde_json::from_str(&t.cli("around", &request)).unwrap()
                } else {
                    t.both(request)
                };
                if count > bound {
                    assert_eq!(answer["code"], "result_too_large", "{answer}");
                    assert_eq!(answer["max_reference_entries"], bound);
                    assert!(
                        answer["rule"]
                            .as_str()
                            .unwrap()
                            .contains(&bound.to_string())
                    );
                } else {
                    assert_eq!(answer["status"], "ok", "{answer}");
                    assert_eq!(keys(&answer), if missing { vec![] } else { vec!["b"] });
                    assert_eq!(
                        answer["result"]["basis"]["dangling"],
                        if missing && !reverse { bound } else { 0 }
                    );
                }
            }
        }
    }
}

#[test]
fn around_reference_budget_is_shared_across_hops_and_heads() {
    let bound = ont::around::MAX_REFERENCE_ENTRIES;
    for reverse in [false, true] {
        for extra in [0, 1] {
            let t = Temp::new();
            if reverse {
                t.put("a", &vec!["a"; bound / 2 + extra], false);
            } else {
                t.put("a", &vec!["b"; bound / 2], false);
                t.put("b", &vec!["z"; bound / 2 + extra], false);
            }
            let hop = if reverse { "previous" } else { "next" };
            let request = req("a", &[hop, hop]);
            let answer = if reverse && extra == 0 {
                serde_json::from_str(&t.cli("around", &request)).unwrap()
            } else {
                t.both(request)
            };
            assert_eq!(
                answer["status"],
                if extra == 0 { "ok" } else { "refused" },
                "{answer}"
            );
            if extra != 0 {
                assert_eq!(answer["max_reference_entries"], bound);
            }
        }
    }
}

#[test]
fn around_reference_bound_hidden_missing_parity_ignores_hidden_source_entries() {
    let bound = ont::around::MAX_REFERENCE_ENTRIES;
    for reverse in [false, true] {
        let missing = Temp::new();
        let hidden = Temp::new();
        for t in [&missing, &hidden] {
            if reverse {
                t.put("a", &[], false);
                t.put("b", &vec!["z"; bound], false);
            } else {
                t.put("a", &vec!["z"; bound], false);
            }
        }
        hidden.put("z", &vec!["a"; bound + 1], true);
        let request = req("a", &[if reverse { "previous" } else { "next" }]);
        let a = missing.cli("around", &request);
        assert_eq!(a, hidden.cli("around", &request));
        assert_eq!(hidden.both(request)["status"], "ok");
    }
}

#[test]
fn around_reverse_scan_bound_on_type_different_from_start() {
    let t = Temp::new();
    let mut leaf = row("start", &[], false);
    leaf.id.object_type = "leaf".into();
    leaf.observation.fields.clear();
    let mut rows = vec![leaf];
    rows.extend((0..ont::search::SCAN_LIMIT).map(|i| row(&format!("n{i:05}"), &[], false)));
    t.append(rows);
    let request = json!({"op":"search_around","from":{"object_type":"leaf"},"path":["parents"]});
    let a = t.both(request.clone());
    assert_eq!(a["status"], "ok", "{a}");
    assert_eq!(a["result"]["basis"]["scanned"], ont::search::SCAN_LIMIT + 1);
    t.put("one_above", &[], false);
    let a = t.both(request);
    assert_eq!(a["code"], "result_too_large");
    assert!(
        a.get("max_reference_entries").is_none(),
        "must hit the head bound, not the reference budget: {a}"
    );
}

#[test]
fn around_wrong_op_hints_only_name_existing_subcommands() {
    let t = Temp::new();
    for (command, request) in [
        ("search", json!({"op":"aggregate","object_type":"node"})),
        (
            "around",
            json!({"op":"aggregate","from":{"object_type":"node"},"path":["next"]}),
        ),
    ] {
        let answer: Value = serde_json::from_str(&t.cli(command, &request)).unwrap();
        assert_eq!(answer["code"], "invalid_request");
        let rule = answer["rule"].as_str().unwrap();
        assert!(rule.contains("aggregate"), "{answer}");
        assert!(rule.contains("search"), "{answer}");
    }
}

fn upgrade_schema(t: &Temp, change: impl FnOnce(&mut Value)) {
    let path = t.0.join("schema.json");
    let mut schema: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    schema["version"] = json!(schema["version"].as_u64().unwrap() + 1);
    change(&mut schema);
    let bytes = serde_json::to_vec(&schema).unwrap();
    ont_store::Store::open_schema_existing_writable(&t.store())
        .unwrap()
        .install_schema(&bytes, Default::default())
        .unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn around_links_require_both_readable_endpoint_types_in_both_directions() {
    for reverse in [false, true] {
        for both_grants in [false, true] {
            let t = Temp::new();
            let mut a = row("a", &[], false);
            let mut leaf = row("z", &[], false);
            leaf.id.object_type = "leaf".into();
            leaf.observation.fields.clear();
            a.observation
                .fields
                .insert("leaf".into(), Knowledge::Value(json!(leaf.id)));
            t.append(vec![a, leaf]);
            if !both_grants {
                upgrade_schema(&t, |s| {
                    s["grants"][0]["read"] = json!([if reverse { "leaf" } else { "node" }])
                });
            }
            let name = if reverse { "parents" } else { "leaf" };
            let request = if reverse {
                json!({"op":"search_around","from":{"object_type":"leaf"},"path":[name]})
            } else {
                req("a", &[name])
            };
            let answer = t.both(request.clone());
            if both_grants {
                assert_eq!(keys(&answer), if reverse { vec!["a"] } else { vec!["z"] });
            } else {
                assert_eq!(answer["code"], "invalid_link");
                assert!(
                    !answer["available"]
                        .as_array()
                        .unwrap()
                        .contains(&json!(name))
                );
                let mut unknown = request.clone();
                unknown["path"] = json!(["no_such_link"]);
                assert_eq!(t.cli("around", &request), t.cli("around", &unknown));
                assert_eq!(answer, t.both(unknown));
                let describe: Value = serde_json::from_str(&t.cli("describe", &json!({}))).unwrap();
                assert!(
                    describe["result"]["link_types"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|l| l["name"] != "node_leaf")
                );
            }
        }
    }
}

#[test]
fn around_upgraded_reference_targets_are_excluded_and_keep_hidden_missing_parity() {
    for count in [
        1,
        ont::around::MAX_REFERENCE_ENTRIES,
        ont::around::MAX_REFERENCE_ENTRIES + 1,
    ] {
        let mut prior = None;
        for visibility in ["visible", "hidden", "missing"] {
            let t = Temp::new();
            upgrade_schema(&t, |s| {
                s["object_types"]["legacy"] =
                    json!({"owner":"ontology","primary_key":{"from":"$action_id"},"properties":{}});
                s["grants"][0]["read"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!("legacy"));
                s["object_types"]["node"]["properties"]["edges"]["type"] =
                    json!({"array":{"ref":"legacy"}});
                s["link_types"]["edge"]["to"] = json!("legacy");
            });
            let mut old = row("z", &[], visibility == "hidden");
            old.id.object_type = "legacy".into();
            old.observation.fields.clear();
            let mut a = row("a", &[], false);
            a.observation.fields.insert(
                "edges".into(),
                Knowledge::Value(json!(vec![old.id.clone(); count])),
            );
            let mut leaf = row("z", &[], false);
            leaf.id.object_type = "leaf".into();
            leaf.observation.fields.clear();
            t.append(vec![a, leaf]);
            if visibility != "missing" {
                t.append(vec![old]);
            }
            upgrade_schema(&t, |s| {
                s["object_types"]["node"]["properties"]["edges"]["type"] =
                    json!({"array":{"ref":"leaf"}});
                s["link_types"]["edge"]["to"] = json!("leaf");
            });
            let mut store = ont_store::Store::open_schema_existing(&t.store()).unwrap();
            assert_eq!(store.head(&id("a")).unwrap().unwrap().schema_pin.version, 2);
            assert_eq!(store.schema_set().unwrap().pin("test").unwrap().version, 3);
            let mut answers = Vec::new();
            for reverse in [false, true] {
                let request = if reverse {
                    json!({"op":"search_around","from":{"object_type":"leaf"},"path":["previous"]})
                } else {
                    req("a", &["next"])
                };
                let answer = t.both(request.clone());
                if count > ont::around::MAX_REFERENCE_ENTRIES {
                    assert_eq!(answer["code"], "result_too_large");
                    assert_eq!(
                        answer["max_reference_entries"],
                        ont::around::MAX_REFERENCE_ENTRIES
                    );
                } else {
                    assert_eq!(answer["status"], "ok", "{answer}");
                    assert!(keys(&answer).is_empty());
                    assert_eq!(answer["result"]["basis"]["qualified"], false);
                    assert_eq!(answer["result"]["basis"]["unresolved_excluded"], count);
                    assert_eq!(answer["result"]["basis"]["dangling"], 0);
                    assert_eq!(
                        answer["result"]["basis"]["scanned"],
                        if reverse { 2 } else { 1 }
                    );
                }
                answers.push(t.cli("around", &request));
            }
            if let Some(expected) = &prior {
                assert_eq!(&answers, expected);
            } else {
                prior = Some(answers);
            }
        }
    }
}

#[test]
fn aggregate_cli_mcp_count_filter_refusals_and_hints() {
    let t = Temp::new();
    upgrade_schema(&t, |schema| {
        schema["grants"][0]["derive"] = json!(["node"]);
        schema["object_types"]["node"]["properties"]["name"]["max_bytes"] = json!(170);
    });
    t.put("a", &[], false);
    t.put("hidden", &[], true);
    let req = json!({"op":"aggregate","object_type":"node","where":{"eq":["name","a"]},"group_by":["name"],"metrics":["count"]});
    let a = t.both(req.clone());
    assert_eq!(
        a["result"]["groups"],
        json!([{"key":{"name":"a"},"count":1}])
    );
    assert_eq!(a["result"]["basis"]["scanned"], 1);
    for metrics in [
        json!([]),
        json!(["count", "count"]),
        json!([{"min":"name"}]),
        json!([{"max":"name"}]),
        json!([{"sum":"name"}]),
    ] {
        let mut r = req.clone();
        r["metrics"] = metrics;
        let a = t.both(r);
        assert_eq!(a["code"], "unsupported_metric");
        assert_eq!(a["field"], "/metrics");
        assert!(a["rule"].as_str().unwrap().contains("count"));
    }
}
