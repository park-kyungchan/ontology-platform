//! Render contracts, using synthetic objects and temporary stores/files only.
#![allow(clippy::unwrap_used)]
use ont_core::{Change, Coverage, Id, Knowledge, Lineage, Namespace, Observation, Scope};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
const FIXTURE: &[u8] = include_bytes!("fixtures/render-board.json");
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "ont-render-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).unwrap();
        Self(dir)
    }
    fn file(&self, name: &str, bytes: &[u8]) -> String {
        let p = self.0.join(name);
        std::fs::write(&p, bytes).unwrap();
        p.to_str().unwrap().into()
    }
    fn run(&self, cmd: &[&str], input: Value) -> (i32, Value) {
        let mut args = vec![
            "--store".into(),
            self.0.join("store.sqlite").display().to_string(),
            "--now".into(),
            "1000".into(),
        ];
        args.extend(cmd.iter().map(|s| s.to_string()));
        let out = ont::run(&args, input.to_string().as_bytes(), 0, 0);
        assert!(out.stderr.is_empty(), "{}", out.stderr);
        (out.code, serde_json::from_str(out.stdout.trim()).unwrap())
    }
    fn install(&self, schema: Value) {
        let file = self.file("schema.json", &serde_json::to_vec(&schema).unwrap());
        let (code, ans) = self.run(&["schema", "install", &file], json!({}));
        assert_eq!(code, 0, "{ans}");
    }
    fn setup() -> Self {
        let t = Self::new();
        t.install(serde_json::from_slice(FIXTURE).unwrap());
        t
    }
    fn render(&self, view: &str, args: Value) -> (i32, Value) {
        self.run(&["render"], json!({"op":"render","view":view,"args":args}))
    }
    fn create(&self, title: &str, rank: i64) -> Value {
        let (code, ans) = self.run(
            &["action", "apply"],
            json!({"action_type":"create_item","parameters":{"title":title,"rank":rank}}),
        );
        assert_eq!(code, 0, "{ans}");
        ans
    }
    fn write(&self, file: &str, view: &str) -> (i32, Value) {
        self.run(
            &[
                "render",
                "--write",
                file,
                "--section",
                view,
                "--preimage-dir",
                self.0.to_str().unwrap(),
            ],
            json!({}),
        )
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn schema() -> Value {
    serde_json::from_slice(FIXTURE).unwrap()
}
fn ns() -> Namespace {
    Namespace {
        instance: "test".into(),
        account: "root".into(),
    }
}
fn id(key: &str) -> Id {
    Id {
        object_type: "item".into(),
        namespace: ns(),
        native_key: key.into(),
    }
}
fn append(
    t: &Temp,
    key: &str,
    fields: BTreeMap<String, Knowledge>,
    change: Change,
    fresh_until: u64,
) {
    let mut store = ont_store::Store::open_schema(&t.0.join("store.sqlite")).unwrap();
    store
        .append_schema(
            &[ont_store::Append {
                id: id(key),
                scope: Scope::domain(ns(), "board", BTreeMap::new()).unwrap(),
                observation: Observation {
                    revision: "r1".into(),
                    source_revision: "s1".into(),
                    observed_at: 10,
                    fresh_until,
                    change,
                    supersedes: None,
                    coverage: Coverage::Complete,
                    fields,
                    lineage: Lineage {
                        artifact_sha256: "a".repeat(64),
                        pointer: "/0".into(),
                    },
                },
            }],
            15,
        )
        .unwrap();
}
fn fields(title: &str, rank: i64) -> BTreeMap<String, Knowledge> {
    BTreeMap::from([
        ("title".into(), Knowledge::Value(json!(title))),
        ("rank".into(), Knowledge::Value(json!(rank))),
    ])
}
#[test]
fn deterministic_order_ties_arguments_and_actions() {
    let t = Temp::setup();
    append(&t, "z", fields("last", 10), Change::Initial, 2000);
    append(&t, "b", fields("equal", 2), Change::Initial, 2000);
    append(&t, "a", fields("equal", 2), Change::Initial, 2000);
    let (code, a) = t.render("items", json!({}));
    assert_eq!(code, 0, "{a}");
    assert_eq!(a, t.render("items", json!({})).1);
    assert_eq!(
        a["markdown"],
        "<!-- ont:render items schema=board@1 -->\n| Key | Title | Rank |\n| --- | --- | --- |\n| a | equal | 2 |\n| b | equal | 2 |\n| z | last | 10 |\n<!-- /ont:render -->"
    );
    let (_, filtered) = t.render("named_items", json!({"title":"equal"}));
    assert_eq!(filtered["rows"], 2);
    assert!(
        filtered["markdown"]
            .as_str()
            .unwrap()
            .starts_with("<!-- ont:render named_items title=equal schema=board@1 -->")
    );
    for args in [
        json!({}),
        json!({"title":"equal","extra":"x"}),
        json!({"title":"bad value"}),
        json!({"title":""}),
        json!({"title":"<!--"}),
    ] {
        let (code, ans) = t.render("named_items", args);
        assert_eq!((code, ans["code"].as_str()), (2, Some("invalid_argument")));
        assert!(ans["argument"].is_string());
    }
    t.create("created", 1);
    let (_, created) = t.render("items", json!({}));
    assert_eq!(created["rows"], 4);
    let (code, ans) = t.run(
        &["action", "apply"],
        json!({"action_type":"modify_item","parameters":{"item":id("a"),"title":"modified"}}),
    );
    assert_eq!(code, 0, "{ans}");
    assert!(
        t.render("items", json!({})).1["markdown"]
            .as_str()
            .unwrap()
            .contains("| a | modified | 2 |")
    );
    let (code, ans) = t.run(
        &["action", "apply"],
        json!({"action_type":"delete_item","parameters":{"item":id("a")}}),
    );
    assert_eq!(code, 0, "{ans}");
    let (_, after) = t.render("items", json!({}));
    assert_eq!(after["rows"], 3);
    assert!(!after["markdown"].as_str().unwrap().contains("| a |"));
}
#[test]
fn every_cell_form_and_escaping_and_stale() {
    let t = Temp::new();
    let mut s = schema();
    let names = [
        "$key",
        "title",
        "rank",
        "flag",
        "ref",
        "tags",
        "meta",
        "missing",
        "null",
        "blank",
        "unknown",
        "not_performed",
    ];
    s["views"]["items"]["columns"] = Value::Array(
        names
            .iter()
            .map(|p| json!({"header":p,"property":p}))
            .collect(),
    );
    t.install(s);
    let mut f = fields("pipe|\n<!--x-->", 42);
    f.extend([
        ("flag".into(), Knowledge::Value(json!(true))),
        ("ref".into(), Knowledge::Value(json!(id("self")))),
        ("tags".into(), Knowledge::Value(json!(["one", "two"]))),
        ("meta".into(), Knowledge::Value(json!({"x":"v"}))),
        ("missing".into(), Knowledge::Missing),
        ("null".into(), Knowledge::Null),
        ("blank".into(), Knowledge::Blank),
        ("unknown".into(), Knowledge::Unknown),
        ("not_performed".into(), Knowledge::NotPerformed),
    ]);
    append(&t, "self", f, Change::Initial, 20);
    let (code, ans) = t.render("items", json!({}));
    assert_eq!(code, 0, "{ans}");
    assert!(ans["markdown"].as_str().unwrap().contains("| self | pipe\\| <\\!--x--\\> (stale) | 42 (stale) | true (stale) | self (stale) | one, two (stale) | {\"x\":\"v\"} (stale) | (missing) | (null) | (blank) | (unknown) | (not_performed) |"), "{ans}");
}
#[test]
fn exact_truncation_and_pagination() {
    let t = Temp::new();
    let mut s = schema();
    s["views"]["items"]["limit"] = json!(1);
    t.install(s);
    append(&t, "a", fields("first", 1), Change::Initial, 2000);
    append(&t, "b", fields("second", 2), Change::Initial, 2000);
    append(&t, "c", fields("third", 3), Change::Initial, 2000);
    let (_, a) = t.render("items", json!({}));
    assert_eq!(a["rows"], 1);
    assert_eq!(a["truncated"], true);
    assert!(
        a["markdown"]
            .as_str()
            .unwrap()
            .contains("| (+2 more; limit 1) |  |  |\n<!-- /ont:render -->")
    );
}
#[test]
fn write_check_preserves_outside_mode_and_preimage_and_is_idempotent() {
    let t = Temp::setup();
    append(&t, "a", fields("equal", 2), Change::Initial, 2000);
    let original = b"before\r\n\xff\n<!-- ont:render items schema=board@0 -->\nold\n<!-- /ont:render -->\nafter\r\n";
    let file = t.file("board.md", original);
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();
    let (code, ans) = t.write(&file, "items");
    assert_eq!(code, 0, "{ans}");
    assert_eq!(ans["changed"], true);
    assert_eq!(
        std::fs::read(ans["preimage"].as_str().unwrap()).unwrap(),
        original
    );
    assert_eq!(
        std::fs::metadata(ans["preimage"].as_str().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o640
    );
    let updated = std::fs::read(&file).unwrap();
    assert!(updated.starts_with(b"before\r\n\xff\n"));
    assert!(updated.ends_with(b"\nafter\r\n"));
    let (code, ans) = t.run(&["render", "--check", &file], json!({}));
    assert_eq!(code, 0, "{ans}");
    assert_eq!(ans["blocks"], 1);
    let entries = std::fs::read_dir(&t.0).unwrap().count();
    let (code, ans) = t.write(&file, "items");
    assert_eq!(code, 0);
    assert_eq!(ans["changed"], false);
    assert_eq!(ans["preimage"], Value::Null);
    assert_eq!(std::fs::read_dir(&t.0).unwrap().count(), entries);
    let edit = String::from_utf8_lossy(&updated).replace("| a | equal", "| a | edited");
    std::fs::write(&file, edit).unwrap();
    let (code, ans) = t.run(&["render", "--check", &file], json!({}));
    assert_eq!((code, ans["code"].as_str()), (2, Some("render_drift")));
    assert_eq!(ans["drift"][0], json!({"view":"items","args":{},"line":3}));
}
#[test]
fn missing_duplicate_unclosed_unknown_symlink_and_preimage_failure() {
    let t = Temp::setup();
    let block = "<!-- ont:render items schema=board@0 -->\nstale\n<!-- /ont:render -->";
    for (text, expected) in [
        ("no blocks".to_string(), "render_block_missing"),
        (format!("{block}\n{block}"), "render_block_ambiguous"),
        (
            "<!-- ont:render items schema=board@0 -->\nunclosed".into(),
            "render_block_missing",
        ),
    ] {
        let file = t.file("board.md", text.as_bytes());
        let (code, ans) = t.write(&file, "items");
        assert_eq!((code, ans["code"].as_str()), (2, Some(expected)), "{ans}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
    }
    for text in [
        "<!-- ont:render items schema=board@0 -->",
        "<!-- ont:render unknown schema=board@1 -->\n<!-- /ont:render -->",
        "<!-- ont:render items BAD -->\n<!-- /ont:render -->",
    ] {
        let file = t.file("check.md", text.as_bytes());
        let (code, ans) = t.run(&["render", "--check", &file], json!({}));
        assert_eq!((code, ans["code"].as_str()), (2, Some("render_drift")));
        assert_eq!(ans["drift"][0]["line"], 1);
    }
    let file = t.file("board.md", block.as_bytes());
    let link = t.0.join("link.md");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    assert_eq!(
        t.write(link.to_str().unwrap(), "items").1["code"],
        "render_target_invalid"
    );
    let (code, ans) = t.run(
        &[
            "render",
            "--write",
            &file,
            "--section",
            "items",
            "--preimage-dir",
            t.0.join("missing").to_str().unwrap(),
        ],
        json!({}),
    );
    assert_eq!(
        (code, ans["code"].as_str()),
        (2, Some("render_preimage_failed"))
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), block);
}
#[test]
fn forbidden_type_and_old_schema() {
    let t = Temp::new();
    let mut s = schema();
    s["grants"][0]["read"] = json!([]);
    t.install(s);
    assert_eq!(t.render("items", json!({})).1["code"], "unavailable");
    let old = Temp::new();
    let mut s = schema();
    s.as_object_mut().unwrap().remove("views");
    old.install(s);
    assert_eq!(old.render("items", json!({})).1["code"], "unavailable");
    let file = old.file("plain.md", b"no generated blocks\n");
    assert_eq!(
        old.run(&["render", "--check", &file], json!({})),
        (0, json!({"status":"ok","blocks":0,"drift":[]}))
    );
}
#[test]
fn schema_refuses_each_invalid_view_and_duplicate_domain_names() {
    let changes = [
        ("/views/items/extra", json!(true)),
        ("/views/items/object_type", json!("absent")),
        ("/views/items/columns/0/property", json!("absent")),
        ("/views/items/order/0/property", json!("absent")),
        ("/views/items/columns", json!([])),
        ("/views/items/order", json!([])),
        ("/views/items/columns/0/header", json!("bad|header")),
        ("/views/items/columns/0/header", json!("bad\nheader")),
        ("/views/items/columns/0/header", json!("bad\rheader")),
        ("/views/items/limit", json!(0)),
        ("/views/items/limit", json!(1001)),
        (
            "/views/named_items/filter",
            json!({"eq":["title","$undeclared"]}),
        ),
        ("/views/items/filter", json!({"eq":["absent","x"]})),
        ("/views/items/filter", json!({"wrong":["title","x"]})),
        ("/views/items/order/0/direction", json!("up")),
    ];
    for (pointer, value) in changes {
        let t = Temp::new();
        let mut s = schema();
        if pointer.ends_with("/extra") {
            s["views"]["items"]["extra"] = value;
        } else if pointer.ends_with("/filter") {
            let view = if pointer.contains("named_items") {
                "named_items"
            } else {
                "items"
            };
            s["views"][view]["filter"] = value;
        } else {
            let (parent, key) = pointer.rsplit_once('/').unwrap();
            s.pointer_mut(parent)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert(key.into(), value);
        }
        let file = t.file("invalid.json", &serde_json::to_vec(&s).unwrap());
        let (code, ans) = t.run(&["schema", "check", &file], json!({}));
        assert_eq!(
            (code, ans["code"].as_str()),
            (2, Some("schema_violation")),
            "{pointer}: {ans}"
        );
    }
    let t = Temp::setup();
    let mut s = schema();
    s["domain"] = json!("other");
    // Give the other domain a different type name while retaining the view name.
    let item = s["object_types"]
        .as_object_mut()
        .unwrap()
        .remove("item")
        .unwrap();
    s["object_types"]["other_item"] = item;
    s["object_types"]["other_item"]["properties"]
        .as_object_mut()
        .unwrap()
        .remove("ref");
    s["action_types"] = json!({});
    s["grants"] = json!([]);
    for view in ["items", "named_items"] {
        s["views"][view]["object_type"] = json!("other_item");
    }
    let file = t.file("other.json", &serde_json::to_vec(&s).unwrap());
    assert_eq!(
        t.run(&["schema", "check", &file], json!({})).1["code"],
        "schema_violation"
    );
    assert_eq!(t.run(&["schema", "install", &file], json!({})).0, 2);
}

#[test]
fn sorted_arguments_write_only_matching_section_and_check_all_blocks() {
    let t = Temp::new();
    let mut s = schema();
    s["views"]["named_items"]["args"] = json!(["z", "a"]);
    s["views"]["named_items"]["filter"] =
        json!({"any":[{"eq":["title","$z"]},{"eq":["title","$a"]}]});
    t.install(s);
    append(&t, "a", fields("equal", 2), Change::Initial, 2000);
    let (_, answer) = t.render("named_items", json!({"z":"equal","a":"last"}));
    assert!(
        answer["markdown"]
            .as_str()
            .unwrap()
            .starts_with("<!-- ont:render named_items a=last z=equal schema=board@1 -->")
    );
    let old = "before\n<!-- ont:render named_items z=equal a=last schema=board@0 -->\nold\n<!-- /ont:render -->\n";
    let (_, other) = t.render("named_items", json!({"z":"other","a":"other"}));
    let outside = format!("{}\nafter\n", other["markdown"].as_str().unwrap());
    let file = t.file("args.md", format!("{old}{outside}").as_bytes());
    let (code, ans) = t.run(
        &[
            "render",
            "--write",
            &file,
            "--section",
            "named_items",
            "--arg",
            "z=equal",
            "--arg",
            "a=last",
            "--preimage-dir",
            t.0.to_str().unwrap(),
        ],
        json!({}),
    );
    assert_eq!(code, 0, "{ans}");
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.ends_with(&outside));
    assert_eq!(
        t.run(&["render", "--check", &file], json!({})),
        (0, json!({"status":"ok","blocks":2,"drift":[]}))
    );
    // A named section with different arguments does not match.
    let (code, ans) = t.run(
        &[
            "render",
            "--write",
            &file,
            "--section",
            "named_items",
            "--arg",
            "z=missing",
            "--arg",
            "a=last",
            "--preimage-dir",
            t.0.to_str().unwrap(),
        ],
        json!({}),
    );
    assert_eq!(
        (code, ans["code"].as_str()),
        (2, Some("render_block_missing"))
    );
}

#[test]
fn pagination_counts_all_matches_before_sort_and_limit() {
    let t = Temp::new();
    let mut s = schema();
    s["views"]["items"]["limit"] = json!(2);
    t.install(s);
    let mut store = ont_store::Store::open_schema(&t.0.join("store.sqlite")).unwrap();
    let entries: Vec<_> = (0..1002)
        .map(|i| ont_store::Append {
            id: id(&format!("k{i:04}")),
            scope: Scope::domain(ns(), "board", BTreeMap::new()).unwrap(),
            observation: Observation {
                revision: "r1".into(),
                source_revision: "s1".into(),
                observed_at: 10,
                fresh_until: 2000,
                change: Change::Initial,
                supersedes: None,
                coverage: Coverage::Complete,
                fields: fields("page", 1002 - i),
                lineage: Lineage {
                    artifact_sha256: "a".repeat(64),
                    pointer: "/0".into(),
                },
            },
        })
        .collect();
    store.append_schema(&entries, 15).unwrap();
    drop(store);
    let (code, ans) = t.render("items", json!({}));
    assert_eq!(code, 0, "{ans}");
    assert!(
        ans["markdown"].as_str().unwrap().contains(
            "| k1001 | page | 1 |\n| k1000 | page | 2 |\n| (+1000 more; limit 2) |  |  |"
        )
    );
}

#[test]
fn schema_view_names_and_nested_unknown_keys_and_filter_types() {
    for (name, value) in [
        ("Bad", json!({})),
        (
            "items",
            json!({"columns":[{"header":"Key","property":"$key","extra":true}]}),
        ),
        (
            "items",
            json!({"order":[{"property":"rank","direction":"asc","extra":true}]}),
        ),
        ("items", json!({"filter":{"eq":["rank","text"]}})),
        ("items", json!({"filter":{"prefix":["rank",1]}})),
        ("items", json!({"args":["x","x"]})),
    ] {
        let t = Temp::new();
        let mut s = schema();
        let mut view = s["views"]["items"].clone();
        for (k, v) in value.as_object().unwrap() {
            view[k] = v.clone();
        }
        s["views"][name] = view;
        let file = t.file("invalid.json", &serde_json::to_vec(&s).unwrap());
        assert_eq!(
            t.run(&["schema", "check", &file], json!({})).1["code"],
            "schema_violation",
            "{s}"
        );
    }
}
