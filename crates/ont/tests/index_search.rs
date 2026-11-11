//! Synthetic tests of `ont index` and `ont search`: temporary directories
//! only (no test reads `/home/palantirkc/work`, `/root` or the default store).
#![allow(clippy::unwrap_used)]
use serde_json::{Value, json};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "ont-idx-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("tasks")).unwrap();
        Self(dir)
    }
    fn store(&self) -> String {
        self.0.join("store.sqlite").display().to_string()
    }
    fn tasks(&self) -> PathBuf {
        self.0.join("tasks")
    }
    fn task(&self, name: &str, state: &str) {
        let dir = self.tasks().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("STATE.md"), state).unwrap();
    }
    fn file(&self, name: &str, bytes: &[u8]) -> String {
        let p = self.0.join(name);
        std::fs::write(&p, bytes).unwrap();
        p.display().to_string()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(args: &[&str], stdin: &[u8]) -> (i32, Value) {
    let (code, text) = run_text(args, stdin);
    (code, serde_json::from_str(text.trim()).unwrap())
}
fn run_text(args: &[&str], stdin: &[u8]) -> (i32, String) {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let out = ont::run(&args, stdin, 0, 0);
    assert_eq!(out.stderr, "", "stderr: {}", out.stderr);
    (out.code, out.stdout)
}

/// A schema of domain `work`: source `task_states` over `<tasks>/*/STATE.md`
/// with the object type `task`, an ontology-owned `note`, and a `secret`
/// type that no grant reads.
fn schema(t: &Temp) -> Value {
    json!({
        "format":"ontology-schema/v1","domain":"work","version":1,
        "namespace":{"instance":"vps://palantirkc","account":"root"},
        "scope_keys":[],
        "value_types":{},
        "sources":{"task_states":{
            "kind":"markdown_fields",
            "glob":format!("{}/*/STATE.md", t.tasks().display()),
            "head_lines":40,"complete_listing":true,"fresh_for_s":3600}},
        "object_types":{
            "task":{"owner":{"source":"task_states"},"primary_key":{"from":"/dir"},"title":"directory",
                "properties":{
                    "directory":{"type":"string","required":true,"from":"/dir_path"},
                    "state_sha256":{"type":"string","required":true,"from":"/sha256"},
                    "checkpoint_line":{"type":"string","required":true,"from":"/line1"},
                    "owner_line":{"type":"string","required":false,"from":"/fields/Owner"},
                    "first":{"type":"string","required":false,"from":"/fields/Owner","transform":"first_word"},
                    "count":{"type":"integer","required":false,"from":"/fields/Count","transform":"parse_integer"}}},
            "note":{"owner":"ontology","primary_key":{"from":"$action_id"},"title":"text",
                "properties":{"text":{"type":"string","required":true}}},
            "secret":{"owner":"ontology","primary_key":{"from":"$action_id"},
                "properties":{"text":{"type":"string","required":true}}}
        },
        "link_types":{},"action_types":{},"functions":{},
        "grants":[{"principal":"agent","scope":{},"read":["task","note"],"derive":[],"actions":[]}]
    })
}
fn install(t: &Temp, schema: &Value) {
    let file = t.file("schema.json", &serde_json::to_vec(schema).unwrap());
    let (code, ans) = run(&["--store", &t.store(), "schema", "install", &file], b"");
    assert_eq!(code, 0, "{ans}");
}
fn index(t: &Temp, now: u64, extra: &[&str]) -> (i32, Value) {
    let now = now.to_string();
    let store = t.store();
    let mut args = vec!["--store", &store, "--now", &now, "index"];
    args.extend_from_slice(extra);
    run(&args, b"")
}
fn search(t: &Temp, now: u64, request: &Value) -> (i32, Value) {
    let now = now.to_string();
    run(
        &["--store", &t.store(), "--now", &now, "search"],
        &serde_json::to_vec(request).unwrap(),
    )
}
fn keys(ans: &Value) -> Vec<String> {
    ans["result"]["objects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["id"]["native_key"].as_str().unwrap().to_string())
        .collect()
}
fn first_source(ans: &Value) -> &Value {
    &ans["result"]["sources"][0]
}
fn appended(entry: &Value) -> [u64; 4] {
    let a = &entry["appended"];
    ["initial", "update", "refresh", "retraction"].map(|k| a[k].as_u64().unwrap())
}

#[test]
fn three_tasks_then_one_removed() {
    let t = Temp::new();
    for n in ["alpha", "beta", "gamma"] {
        t.task(n, &format!("# State of {n}\nOwner: lead\n"));
    }
    install(&t, &schema(&t));
    let (code, ans) = index(&t, 1000, &[]);
    assert_eq!(code, 0, "{ans}");
    assert_eq!(ans["status"], "ok");
    let entry = first_source(&ans);
    assert_eq!(entry["source"], "task_states");
    assert_eq!(entry["kind"], "markdown_fields");
    assert_eq!(entry["records"], 3);
    assert_eq!(appended(entry), [3, 0, 0, 0]);
    assert_eq!(entry["status"], "ok");
    let (code, ans) = search(&t, 1000, &json!({"object_type":"task"}));
    assert_eq!(code, 0, "{ans}");
    assert_eq!(keys(&ans), ["alpha", "beta", "gamma"]);
    assert_eq!(ans["result"]["kind"], "object_set");
    assert_eq!(ans["result"]["basis"]["qualified"], true, "{ans}");
    assert_eq!(ans["result"]["basis"]["scanned"], 3);
    assert_eq!(ans["result"]["basis"]["stale_sources"], json!([]));
    let head = &ans["result"]["objects"][0]["head"];
    assert_eq!(head["change"], "initial");
    assert_eq!(head["revision"], "r0@1000");
    assert_eq!(head["coverage"], "complete");
    assert_eq!(head["lineage"]["pointer"], "");
    assert_eq!(head["fresh_until"], 4600);
    assert_eq!(head["fields"]["first"]["value"], "lead");
    // The title is the declared `title` property (`directory`).
    assert_eq!(
        ans["result"]["objects"][0]["title"],
        t.tasks().join("alpha").display().to_string()
    );
    // The same files at the same --now append nothing.
    let (code, ans) = index(&t, 1000, &[]);
    assert_eq!(code, 0);
    assert_eq!(appended(first_source(&ans)), [0, 0, 0, 0]);
    assert_eq!(first_source(&ans)["unchanged"], 3);
    // Remove one directory: the object is retracted.
    std::fs::remove_dir_all(t.tasks().join("beta")).unwrap();
    let (code, ans) = index(&t, 1001, &[]);
    assert_eq!(code, 0, "{ans}");
    assert_eq!(appended(first_source(&ans)), [0, 0, 0, 1]);
    assert_eq!(first_source(&ans)["records"], 2);
    let (_, ans) = search(&t, 1001, &json!({"object_type":"task"}));
    assert_eq!(keys(&ans), ["alpha", "gamma"]);
    assert_eq!(ans["result"]["basis"]["qualified"], true);
    // A returning directory is `initial` again, with a later revision number.
    t.task("beta", "# back\n");
    let (_, ans) = index(&t, 1002, &[]);
    assert_eq!(appended(first_source(&ans)), [1, 0, 0, 0]);
    let (_, ans) = search(
        &t,
        1002,
        &json!({"object_type":"task","where":{"eq":["directory",
            t.tasks().join("beta").display().to_string()]}}),
    );
    assert_eq!(keys(&ans), ["beta"]);
    assert_eq!(ans["result"]["objects"][0]["head"]["revision"], "r2@1002");
}

#[test]
fn change_refresh_and_stale_report() {
    let t = Temp::new();
    t.task("alpha", "# a\nOwner: lead\n");
    t.task("beta", "# b\nOwner: lead\n");
    install(&t, &schema(&t));
    assert_eq!(index(&t, 1000, &[]).0, 0);
    // A changed file is an `update`; the unchanged one is not touched.
    t.task("alpha", "# a changed\nOwner: lead\n");
    let (_, ans) = index(&t, 1100, &[]);
    assert_eq!(appended(first_source(&ans)), [0, 1, 0, 0]);
    assert_eq!(first_source(&ans)["unchanged"], 1);
    let (_, ans) = search(
        &t,
        1100,
        &json!({"object_type":"task","where":{"eq":["checkpoint_line","# a changed"]}}),
    );
    assert_eq!(keys(&ans), ["alpha"]);
    assert_eq!(ans["result"]["objects"][0]["head"]["change"], "update");
    assert_eq!(ans["result"]["objects"][0]["head"]["revision"], "r1@1100");
    // Beyond fresh_for_s: stale before the refresh, with the command named.
    let late = 1100 + 3600 + 1;
    let (code, ans) = search(&t, late, &json!({"object_type":"task"}));
    assert_eq!(code, 0, "{ans}");
    // alpha was observed at 1100 (fresh until 4700), beta at 1000 (4600).
    assert_eq!(ans["result"]["basis"]["stale"], 2, "{ans}");
    assert_eq!(ans["result"]["basis"]["qualified"], false);
    assert_eq!(
        ans["result"]["basis"]["stale_sources"],
        json!(["task_states: ont index --source task_states"])
    );
    assert_eq!(ans["result"]["objects"][0]["head"]["stale"], true);
    let (_, ans) = index(&t, late, &["--source", "task_states"]);
    assert_eq!(appended(first_source(&ans)), [0, 0, 2, 0]);
    let (_, ans) = search(&t, late, &json!({"object_type":"task"}));
    assert_eq!(ans["result"]["basis"]["stale"], 0);
    assert_eq!(ans["result"]["basis"]["qualified"], true);
    assert_eq!(ans["result"]["objects"][0]["head"]["change"], "refresh");
}

#[test]
fn exclude_symlinks_and_head_lines() {
    let t = Temp::new();
    t.task("keep", "# k\nOwner: x\n");
    t.task("skip-me", "# s\n");
    // A symlinked file and a symlinked directory are not followed.
    let other = t.0.join("elsewhere");
    std::fs::create_dir_all(other.join("deep")).unwrap();
    std::fs::write(other.join("deep/STATE.md"), "# d\n").unwrap();
    std::fs::create_dir_all(t.tasks().join("filelink")).unwrap();
    symlink(
        t.tasks().join("keep/STATE.md"),
        t.tasks().join("filelink/STATE.md"),
    )
    .unwrap();
    symlink(other.join("deep"), t.tasks().join("dirlink")).unwrap();
    // `head_lines` 1: the `Owner:` line 2 is not parsed.
    let mut s = schema(&t);
    s["sources"]["task_states"]["head_lines"] = json!(1);
    s["sources"]["task_states"]["exclude"] =
        json!([format!("{}/skip-*/STATE.md", t.tasks().display())]);
    install(&t, &s);
    let (code, ans) = index(&t, 50, &[]);
    assert_eq!(code, 0, "{ans}");
    let entry = first_source(&ans);
    assert_eq!(entry["records"], 1, "{ans}");
    assert_eq!(entry["skipped_symlinks"], 2, "{ans}");
    let (_, ans) = search(&t, 50, &json!({"object_type":"task"}));
    assert_eq!(keys(&ans), ["keep"]);
    assert_eq!(
        ans["result"]["objects"][0]["head"]["fields"]["owner_line"]["kind"],
        "missing"
    );
    // The same source with head_lines 5 parses the field.
    let t2 = Temp::new();
    t2.task("keep", "# k\nOwner: x\n");
    install(&t2, &schema(&t2));
    index(&t2, 50, &[]);
    let (_, ans) = search(
        &t2,
        50,
        &json!({"object_type":"task","where":{"eq":["owner_line","x"]}}),
    );
    assert_eq!(keys(&ans), ["keep"]);
}

#[test]
fn sixteen_mebibyte_bound_by_a_sparse_file() {
    let t = Temp::new();
    t.task("ok", "# fine\n");
    install(&t, &schema(&t));
    // Exactly 16 MiB is read; one byte more fails the source.
    let exact = t.tasks().join("ok/STATE.md");
    std::fs::File::options()
        .write(true)
        .open(&exact)
        .unwrap()
        .set_len(16 * 1024 * 1024)
        .unwrap();
    let (code, ans) = index(&t, 10, &[]);
    assert_eq!(code, 0, "{ans}");
    assert_eq!(ans["result"]["sources"][0]["records"], 1);
    std::fs::File::options()
        .write(true)
        .open(&exact)
        .unwrap()
        .set_len(16 * 1024 * 1024 + 1)
        .unwrap();
    let (code, ans) = index(&t, 11, &[]);
    assert_eq!(code, 2, "{ans}");
    assert_eq!(ans["status"], "source_failed");
    let entry = first_source(&ans);
    assert_eq!(entry["status"], "source_failed");
    assert_eq!(entry["code"], "file_too_large");
    assert!(
        entry["cause"]
            .as_str()
            .unwrap()
            .starts_with("file_too_large@/")
    );
    assert_eq!(appended(entry), [0, 0, 0, 0]);
    // The earlier object is untouched (nothing appended, nothing retracted).
    let (_, ans) = search(&t, 11, &json!({"object_type":"task"}));
    assert_eq!(keys(&ans), ["ok"]);
}

#[test]
fn file_digest_with_a_missing_listed_path() {
    let t = Temp::new();
    let a = t.file("a.txt", b"hello");
    std::fs::set_permissions(&a, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
    let b = t.0.join("b.txt").display().to_string();
    let mut s = schema(&t);
    s["sources"]["files"] = json!({"kind":"file_digest","paths":[a, b],
        "complete_listing":true,"fresh_for_s":60});
    s["object_types"]["digest"] = json!({"owner":{"source":"files"},"primary_key":{"from":"/path"},
        "properties":{
            "sha256":{"type":"string","required":true,"from":"/sha256"},
            "size":{"type":"integer","required":true,"from":"/size"},
            "mtime":{"type":"integer","required":true,"from":"/mtime"},
            "mode":{"type":"integer","required":true,"from":"/mode"}}});
    s["grants"][0]["read"] = json!(["task", "note", "digest"]);
    install(&t, &s);
    let (code, ans) = index(&t, 100, &["--source", "files"]);
    assert_eq!(code, 0, "{ans}");
    let entry = first_source(&ans);
    assert_eq!(entry["kind"], "file_digest");
    assert_eq!(entry["records"], 1);
    assert_eq!(appended(entry), [1, 0, 0, 0]);
    let (_, ans) = search(&t, 100, &json!({"object_type":"digest"}));
    let fields = &ans["result"]["objects"][0]["head"]["fields"];
    assert_eq!(
        fields["sha256"]["value"],
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
    assert_eq!(fields["size"]["value"], 5);
    assert_eq!(fields["mode"]["value"], 0o644);
    // The missing path appears: a second object. The first file goes: its
    // object is retracted (complete listing).
    std::fs::write(&b, "x").unwrap();
    std::fs::remove_file(t.0.join("a.txt")).unwrap();
    let (_, ans) = index(&t, 101, &["--source", "files"]);
    assert_eq!(appended(first_source(&ans)), [1, 0, 0, 1], "{ans}");
    let (_, ans) = search(&t, 101, &json!({"object_type":"digest"}));
    assert_eq!(keys(&ans), [b]);
}

#[test]
fn unresolved_required_property_is_missing_and_partial() {
    let t = Temp::new();
    t.task("a", "# a\n");
    let mut s = schema(&t);
    s["object_types"]["task"]["properties"]["owner_line"]["required"] = json!(true);
    install(&t, &s);
    let (code, ans) = index(&t, 5, &[]);
    assert_eq!(code, 0, "{ans}");
    let (_, ans) = search(&t, 5, &json!({"object_type":"task"}));
    let head = &ans["result"]["objects"][0]["head"];
    assert_eq!(head["coverage"], "partial");
    assert_eq!(head["fields"]["owner_line"]["kind"], "missing");
    assert_eq!(ans["result"]["basis"]["partial"], 1);
    assert_eq!(ans["result"]["basis"]["qualified"], false);
    // `unresolved` finds it; a comparison on it excludes and counts it.
    let (_, ans) = search(
        &t,
        5,
        &json!({"object_type":"task","where":{"unresolved":"owner_line"}}),
    );
    assert_eq!(keys(&ans), ["a"]);
    let (_, ans) = search(
        &t,
        5,
        &json!({"object_type":"task","where":{"eq":["owner_line","x"]}}),
    );
    assert_eq!(keys(&ans), Vec::<String>::new());
    assert_eq!(ans["result"]["basis"]["unresolved_excluded"], 1);
    assert_eq!(ans["result"]["basis"]["qualified"], false);
}

#[test]
fn one_failed_source_beside_a_good_one() {
    let t = Temp::new();
    t.task("a", "# a\n");
    let bad = t.0.join("bad");
    std::fs::create_dir_all(bad.join("x")).unwrap();
    let big = bad.join("x/STATE.md");
    std::fs::write(&big, "").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&big)
        .unwrap()
        .set_len(16 * 1024 * 1024 + 1)
        .unwrap();
    let mut s = schema(&t);
    s["sources"]["bad_states"] = json!({"kind":"markdown_fields",
        "glob":format!("{}/*/STATE.md", bad.display()),"fresh_for_s":60});
    s["object_types"]["other"] = json!({"owner":{"source":"bad_states"},"primary_key":{"from":"/dir"},
        "properties":{"sha":{"type":"string","required":true,"from":"/sha256"}}});
    s["grants"][0]["read"] = json!(["task", "note", "other"]);
    install(&t, &s);
    // `bad_states` sorts before `task_states`: the good one still runs.
    let (code, ans) = index(&t, 7, &[]);
    assert_eq!(code, 2, "{ans}");
    let sources = ans["result"]["sources"].as_array().unwrap();
    assert_eq!(sources.len(), 2);
    assert_eq!(sources[0]["source"], "bad_states");
    assert_eq!(sources[0]["status"], "source_failed");
    assert_eq!(sources[1]["source"], "task_states");
    assert_eq!(sources[1]["status"], "ok");
    assert_eq!(appended(&sources[1]), [1, 0, 0, 0]);
    let (_, ans) = search(&t, 7, &json!({"object_type":"task"}));
    assert_eq!(keys(&ans), ["a"]);
    let (_, ans) = search(&t, 7, &json!({"object_type":"other"}));
    assert_eq!(keys(&ans), Vec::<String>::new());
}

#[test]
fn source_failures_have_located_causes() {
    // key_unresolved, duplicate_key, schema_violation,
    // source_kind_unsupported, fresh_until_overflow, too_many_records.
    let t = Temp::new();
    t.task("a", "# a\n");
    t.task("b", "# b\n");
    let cause = |t: &Temp, s: &Value, now: u64| -> String {
        install(t, s);
        let (code, ans) = index(t, now, &[]);
        assert_eq!(code, 2, "{ans}");
        first_source(&ans)["cause"].as_str().unwrap().to_string()
    };
    let mut s = schema(&t);
    s["object_types"]["task"]["primary_key"]["from"] = json!("/fields/Nope");
    assert!(cause(&t, &s, 5).starts_with("key_unresolved@/"));
    let t = {
        let t2 = Temp::new();
        t2.task("a", "# a\n");
        t2.task("b", "# b\n");
        t2
    };
    let mut s = schema(&t);
    s["object_types"]["task"]["primary_key"]["from"] = json!("/stem");
    assert!(cause(&t, &s, 5).starts_with("duplicate_key@/"));
    let t = Temp::new();
    t.task("a", "# a\n");
    let mut s = schema(&t);
    s["object_types"]["task"]["properties"]["state_sha256"] =
        json!({"type":"integer","required":true,"from":"/sha256"});
    let c = cause(&t, &s, 5);
    assert!(
        c.starts_with("schema_violation:wrong_value_type@/fields/state_sha256"),
        "{c}"
    );
    let t = Temp::new();
    t.task("a", "# a\n");
    let mut s = schema(&t);
    s["sources"]["task_states"]["kind"] = json!("json_records");
    s["sources"]["task_states"]["path"] = json!("x.json");
    s["sources"]["task_states"]
        .as_object_mut()
        .unwrap()
        .remove("glob");
    s["sources"]["task_states"]
        .as_object_mut()
        .unwrap()
        .remove("head_lines");
    s["sources"]["task_states"]
        .as_object_mut()
        .unwrap()
        .remove("complete_listing");
    s["sources"]["task_states"]
        .as_object_mut()
        .unwrap()
        .remove("fresh_for_s");
    assert_eq!(
        cause(&t, &s, 5),
        "source_kind_unsupported@/sources/task_states/kind"
    );
    let t = Temp::new();
    t.task("a", "# a\n");
    let c = cause(&t, &schema(&t), u64::MAX - 10);
    assert!(c.starts_with("fresh_until_overflow@"), "{c}");
    // More records than one source may list.
    let t = Temp::new();
    for i in 0..10_001 {
        t.task(&format!("t{i:05}"), "x\n");
    }
    let c = cause(&t, &schema(&t), 5);
    assert!(c.starts_with("too_many_records@"), "{c}");
}

fn operator_fixture() -> Temp {
    let t = Temp::new();
    t.task("t1", "# one\nOwner: alpha one\nCount: 1\n");
    t.task("t2", "# two\nOwner: alpha two\nCount: 2\n");
    t.task("t3", "# three\nOwner: beta\nCount: 3\n");
    t.task("t4", "# four\n");
    install(&t, &schema(&t));
    assert_eq!(index(&t, 10, &[]).0, 0);
    t
}
fn matched(t: &Temp, filter: Value) -> (Vec<String>, Value) {
    let (code, ans) = search(t, 10, &json!({"object_type":"task","where":filter}));
    assert_eq!(code, 0, "{ans}");
    (keys(&ans), ans["result"]["basis"].clone())
}

#[test]
fn every_filter_operator_and_combinator() {
    let t = operator_fixture();
    let (k, b) = matched(&t, json!({"eq":["owner_line","beta"]}));
    assert_eq!(k, ["t3"]);
    assert_eq!(b["unresolved_excluded"], 1);
    assert_eq!(b["qualified"], false);
    assert_eq!(
        matched(&t, json!({"ne":["owner_line","beta"]})).0,
        ["t1", "t2"]
    );
    assert_eq!(matched(&t, json!({"in":["count",[1,3]]})).0, ["t1", "t3"]);
    assert_eq!(matched(&t, json!({"lt":["count",2]})).0, ["t1"]);
    assert_eq!(matched(&t, json!({"le":["count",2]})).0, ["t1", "t2"]);
    assert_eq!(matched(&t, json!({"gt":["count",2]})).0, ["t3"]);
    assert_eq!(matched(&t, json!({"ge":["count",2]})).0, ["t2", "t3"]);
    assert_eq!(
        matched(&t, json!({"prefix":["owner_line","alpha"]})).0,
        ["t1", "t2"]
    );
    let (k, b) = matched(&t, json!({"unresolved":"count"}));
    assert_eq!(k, ["t4"]);
    assert_eq!(b["qualified"], true);
    assert_eq!(
        matched(
            &t,
            json!({"all":[{"gt":["count",1]},{"prefix":["owner_line","alpha"]}]})
        )
        .0,
        ["t2"]
    );
    let (k, b) = matched(&t, json!({"any":[{"eq":["count",1]},{"eq":["count",3]}]}));
    assert_eq!(k, ["t1", "t3"]);
    assert_eq!(b["unresolved_excluded"], 1);
    let (k, _) = matched(&t, json!({"none":[{"eq":["count",1]}]}));
    assert_eq!(k, ["t2", "t3"]);
    // `first_word` and `parse_integer` produced typed values.
    let (_, ans) = search(
        &t,
        10,
        &json!({"object_type":"task","where":{"eq":["first","alpha"]}}),
    );
    assert_eq!(keys(&ans), ["t1", "t2"]);
    assert_eq!(
        ans["result"]["objects"][0]["head"]["fields"]["count"]["value"],
        1
    );
}

#[test]
fn continuation_over_two_pages() {
    let t = Temp::new();
    for i in 1..=5 {
        t.task(&format!("t{i}"), "x\n");
    }
    install(&t, &schema(&t));
    index(&t, 10, &[]);
    let mut seen: Vec<String> = Vec::new();
    let mut token = Value::Null;
    let mut pages = 0;
    loop {
        let mut req = json!({"object_type":"task","limit":2});
        if !token.is_null() {
            req["continuation"] = token.clone();
        }
        let (code, ans) = search(&t, 10, &req);
        assert_eq!(code, 0, "{ans}");
        pages += 1;
        seen.extend(keys(&ans));
        token = ans["result"]["continuation"].clone();
        if token.is_null() {
            break;
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(seen, ["t1", "t2", "t3", "t4", "t5"]);
    // Exactly one page of the whole set: no continuation.
    let (_, ans) = search(&t, 10, &json!({"object_type":"task","limit":5}));
    assert_eq!(ans["result"]["continuation"], Value::Null);
}

#[test]
fn scan_limit_is_ten_thousand_heads() {
    use ont_core::{Coverage, Id, Knowledge, Lineage, Namespace, Observation, Scope};
    use std::collections::BTreeMap;
    let t = Temp::new();
    install(&t, &schema(&t));
    let ns = Namespace {
        instance: "vps://palantirkc".into(),
        account: "root".into(),
    };
    let mut store = ont_store::Store::open_schema(Path::new(&t.store())).unwrap();
    let mut put = |from: usize, to: usize| {
        store
            .write(1, |tx| {
                for i in from..to {
                    tx.append(&ont_store::Append {
                        id: Id {
                            object_type: "note".into(),
                            namespace: ns.clone(),
                            native_key: format!("n{i:06}"),
                        },
                        scope: Scope::domain(ns.clone(), "work", BTreeMap::new()).unwrap(),
                        observation: Observation {
                            revision: "r0@1".into(),
                            source_revision: "s".into(),
                            observed_at: 1,
                            fresh_until: 1_000_000,
                            change: ont_core::Change::Initial,
                            supersedes: None,
                            coverage: Coverage::Complete,
                            fields: BTreeMap::from([(
                                "text".to_string(),
                                Knowledge::Value(json!("none")),
                            )]),
                            lineage: Lineage {
                                artifact_sha256: "a".repeat(64),
                                pointer: String::new(),
                            },
                        },
                    })?;
                }
                Ok(())
            })
            .unwrap();
    };
    put(0, 10_000);
    let req = json!({"object_type":"note","where":{"eq":["text","absent"]}});
    let (code, ans) = search(&t, 2, &req);
    assert_eq!(code, 0, "{ans}");
    assert_eq!(ans["result"]["basis"]["scanned"], 10_000);
    // One head more: the request is refused.
    store
        .write(1, |tx| {
            tx.append(&ont_store::Append {
                id: Id {
                    object_type: "note".into(),
                    namespace: ns.clone(),
                    native_key: "n999999".into(),
                },
                scope: Scope::domain(ns.clone(), "work", BTreeMap::new()).unwrap(),
                observation: Observation {
                    revision: "r0@1".into(),
                    source_revision: "s".into(),
                    observed_at: 1,
                    fresh_until: 1_000_000,
                    change: ont_core::Change::Initial,
                    supersedes: None,
                    coverage: Coverage::Complete,
                    fields: BTreeMap::from([("text".to_string(), Knowledge::Value(json!("none")))]),
                    lineage: Lineage {
                        artifact_sha256: "a".repeat(64),
                        pointer: String::new(),
                    },
                },
            })
        })
        .unwrap();
    drop(store);
    let (code, ans) = search(&t, 2, &req);
    assert_eq!(
        (code, ans["code"].as_str()),
        (2, Some("result_too_large")),
        "{ans}"
    );
    // A page that fills up inside the scan window is fine.
    let (code, ans) = search(&t, 2, &json!({"object_type":"note","limit":3}));
    assert_eq!(code, 0);
    assert_eq!(keys(&ans).len(), 3);
    assert!(ans["result"]["continuation"].is_string());
}

#[test]
fn types_outside_the_grants_look_absent() {
    let t = Temp::new();
    install(&t, &schema(&t));
    let (c1, hidden) = run_text(
        &["--store", &t.store(), "search"],
        br#"{"object_type":"secret"}"#,
    );
    let (c2, missing) = run_text(
        &["--store", &t.store(), "search"],
        br#"{"object_type":"nothing_here"}"#,
    );
    assert_eq!((c1, c2), (2, 2));
    assert_eq!(hidden, missing);
    assert!(hidden.contains("\"unavailable\""));
    // The filter of a hidden type is not even looked at.
    let (_, ans) = run(
        &["--store", &t.store(), "search"],
        br#"{"object_type":"secret","where":{"eq":["nope",1]}}"#,
    );
    assert_eq!(ans["code"], "unavailable");
}

#[test]
fn refusal_codes_of_index_and_search() {
    let t = Temp::new();
    t.task("a", "# a\nCount: 4\n");
    install(&t, &schema(&t));
    index(&t, 1, &[]);
    let code_of = |req: &[u8]| -> (i32, Value) { run(&["--store", &t.store(), "search"], req) };
    let expect = |req: &[u8], want: &str| {
        let (code, ans) = code_of(req);
        assert_eq!((code, ans["code"].as_str()), (2, Some(want)), "{ans}");
        ans
    };
    expect(b"not json", "invalid_request");
    expect(br#"{"object_type":"task","bogus":1}"#, "invalid_request");
    expect(br#"{"op":"get","object_type":"task"}"#, "invalid_request");
    expect(br#"{"object_type":"task","limit":0}"#, "invalid_request");
    expect(br#"{"object_type":"task","limit":1001}"#, "limit_too_large");
    let leaves = vec![json!({"eq":["count",1]}); 65];
    expect(
        &serde_json::to_vec(&json!({"object_type":"task","where":{"all":leaves}})).unwrap(),
        "filter_too_large",
    );
    let ans = expect(
        br#"{"object_type":"task","where":{"eq":["nope",1]}}"#,
        "undeclared_property",
    );
    assert_eq!(ans["property"], "nope");
    expect(
        br#"{"object_type":"task","where":{"eq":["count","x"]}}"#,
        "wrong_value_type",
    );
    expect(
        br#"{"object_type":"task","where":{"prefix":["count","1"]}}"#,
        "invalid_filter_operator",
    );
    expect(
        br#"{"object_type":"task","where":{"prefix":["owner_line",1]}}"#,
        "wrong_value_type",
    );
    expect(
        br#"{"object_type":"task","continuation":"zz"}"#,
        "invalid_continuation",
    );
    expect(
        br#"{"object_type":"task","where":{"eq":["count",1],"ne":["count",2]}}"#,
        "invalid_request",
    );
    // Index: unknown source, bad request, missing store (with its path).
    let (code, ans) = run(&["--store", &t.store(), "index", "--source", "nope"], b"");
    assert_eq!(
        (code, ans["code"].as_str(), ans["source"].as_str()),
        (2, Some("unknown_source"), Some("nope"))
    );
    let (code, ans) = run(
        &[
            "--store",
            &t.store(),
            "index",
            "--source",
            "task_states",
            "--source",
            "task_states",
        ],
        b"",
    );
    assert_eq!((code, ans["code"].as_str()), (2, Some("invalid_request")));
    let (code, ans) = run(&["--store", &t.store(), "describe", "--source", "x"], b"{}");
    assert_eq!((code, ans["code"].as_str()), (2, Some("invalid_argument")));
    let task_id = r#"{"id":{"object_type":"task","namespace":{"instance":"vps://palantirkc","account":"root"},"native_key":"a"}}"#;
    for (argv, stdin) in [
        (vec!["index"], ""),
        (vec!["search"], r#"{"object_type":"task"}"#),
        (vec!["describe"], "{}"),
        (vec!["get"], task_id),
        (vec!["backup"], ""),
    ] {
        let missing = t.0.join("none.sqlite").display().to_string();
        let mut args = vec!["--store", missing.as_str()];
        args.extend(argv.iter().copied());
        let (code, ans) = run(&args, stdin.as_bytes());
        assert_eq!(
            (code, ans["code"].as_str(), ans["path"].as_str()),
            (2, Some("store_not_found"), Some(missing.as_str())),
            "{argv:?}"
        );
        assert!(!Path::new(&missing).exists());
    }
    // Index opens a store that cannot be created: an empty file stays empty.
    let empty = t.file("empty.sqlite", b"");
    let args: Vec<String> = ["--store", &empty, "index"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let out = ont::run(&args, &b""[..], 0, 0);
    assert_eq!(out.code, 1);
    assert!(out.stderr.starts_with("store_failure@"), "{}", out.stderr);
    assert_eq!(std::fs::metadata(&empty).unwrap().len(), 0);
}

#[test]
fn schema_check_refuses_bad_source_options_globs_and_transforms() {
    let t = Temp::new();
    let base = schema(&t);
    let check = |s: &Value| -> Vec<(String, String)> {
        let file = t.file("c.json", &serde_json::to_vec(s).unwrap());
        let (_, ans) = run(&["--store", &t.store(), "schema", "check", &file], b"");
        ans["violations"]
            .as_array()
            .map(|v| {
                v.iter()
                    .map(|x| {
                        (
                            x["code"].as_str().unwrap().to_string(),
                            x["pointer"].as_str().unwrap().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    assert!(check(&base).is_empty());
    let with = |edit: &dyn Fn(&mut Value)| {
        let mut s = base.clone();
        edit(&mut s);
        check(&s)
    };
    let p = |c: &str, p: &str| vec![(c.to_string(), p.to_string())];
    assert_eq!(
        with(&|s| s["sources"]["task_states"]["glob"] = json!("relative/*.md")),
        p("invalid_glob", "/sources/task_states/glob")
    );
    for bad in [
        "/a/{x,y}/f",
        "/a/[ab]/f",
        "/a/**",
        "/a/b**/f",
        "/a//f",
        "/a/../f",
        "/a/\\x/f",
    ] {
        assert_eq!(
            with(&|s| s["sources"]["task_states"]["glob"] = json!(bad)),
            p("invalid_glob", "/sources/task_states/glob"),
            "{bad}"
        );
    }
    assert_eq!(
        with(&|s| s["sources"]["task_states"]["exclude"] = json!(["/ok/*.md", "x"])),
        p("invalid_glob", "/sources/task_states/exclude/1")
    );
    assert_eq!(
        with(&|s| s["sources"]["task_states"]["fresh_for_s"] = json!(0)),
        p("invalid_source_option", "/sources/task_states/fresh_for_s")
    );
    assert_eq!(
        with(&|s| s["sources"]["task_states"]["head_lines"] = json!("x")),
        p("invalid_source_option", "/sources/task_states/head_lines")
    );
    assert_eq!(
        with(&|s| s["sources"]["task_states"]["bogus"] = json!(1)),
        p("invalid_source_option", "/sources/task_states/bogus")
    );
    assert_eq!(
        with(&|s| s["sources"]["f"] = json!({"kind":"file_digest","fresh_for_s":5})),
        p("invalid_source_option", "/sources/f/paths")
    );
    assert_eq!(
        with(&|s| s["sources"]["f"] =
            json!({"kind":"file_digest","paths":["/a"],"fresh_for_s":5,"head_lines":3})),
        p("invalid_source_option", "/sources/f/head_lines")
    );
    assert_eq!(
        with(&|s| s["object_types"]["task"]["properties"]["count"]["transform"] = json!("upper")),
        p(
            "invalid_transform",
            "/object_types/task/properties/count/transform"
        )
    );
    assert_eq!(
        with(
            &|s| s["object_types"]["task"]["properties"]["owner_line"]["transform"] =
                json!("parse_integer")
        ),
        p(
            "invalid_transform",
            "/object_types/task/properties/owner_line/transform"
        )
    );
    assert_eq!(
        with(&|s| s["object_types"]["task"]["primary_key"]["transform"] = json!("parse_integer")),
        p(
            "invalid_transform",
            "/object_types/task/primary_key/transform"
        )
    );
    // Source-owned types: scope keys and unknown record fields are refused
    // at check time.
    assert_eq!(
        with(&|s| {
            s["scope_keys"] = json!(["class_key"]);
            s["grants"][0]["scope"] = json!({"class_key":"C1"});
        }),
        p(
            "source_scope_keys_unmapped",
            "/object_types/task/owner/source"
        )
    );
    assert_eq!(
        with(&|s| s["object_types"]["task"]["primary_key"]["from"] = json!("/nope")),
        p(
            "unknown_source_field",
            "/object_types/task/primary_key/from"
        )
    );
    assert_eq!(
        with(
            &|s| s["object_types"]["task"]["properties"]["owner_line"]["from"] = json!("/bogus/x")
        ),
        p(
            "unknown_source_field",
            "/object_types/task/properties/owner_line/from"
        )
    );
    assert_eq!(
        with(&|s| s["sources"]["task_states"]["glob"] = json!(format!("/a/{}f", "**/".repeat(9)))),
        p("invalid_glob", "/sources/task_states/glob")
    );
    // The new options are described.
    install(&t, &base);
    let (_, ans) = run(&["--store", &t.store(), "describe"], b"{}");
    assert_eq!(ans["result"]["sources"][0]["options"]["head_lines"], 40);
    assert_eq!(
        ans["result"]["object_types"][1]["properties"][0]["transform"],
        Value::Null
    );
}

/// A reader that fails the test when read.
struct Panicking;
impl std::io::Read for Panicking {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        panic!("stdin was read");
    }
}

#[test]
fn schema_check_and_backup_never_read_stdin() {
    let t = Temp::new();
    install(&t, &schema(&t));
    let file = t.file("c.json", &serde_json::to_vec(&schema(&t)).unwrap());
    for argv in [
        vec!["--store", &t.store(), "schema", "check", &file],
        vec!["--store", &t.store(), "backup"],
        vec!["--store", &t.store(), "index", "--source", "task_states"],
    ] {
        let args: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
        let out = ont::run(&args, Panicking, 0, 0);
        assert_eq!(out.code, 0, "{argv:?}: {}", out.stdout);
    }
}

#[test]
fn backup_keeps_three_and_reports_no_retention_failure() {
    // Four backups are taken; the newest three are kept and nothing fails.
    let t = Temp::new();
    install(&t, &schema(&t));
    for now in [1_000u64, 2_000, 3_000, 4_000] {
        let n = now.to_string();
        let (code, ans) = run(&["--store", &t.store(), "--now", &n, "backup"], b"");
        assert_eq!(code, 0, "{ans}");
        assert_eq!(ans["result"]["retention_failure"], Value::Null);
        assert_eq!(ans["authorization_revision"], 1);
    }
    let dir = t.0.join("backup");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 3);
}

#[test]
fn a_replaced_glob_root_fails_the_source_and_retracts_nothing() {
    let t = Temp::new();
    for n in ["a", "b", "c"] {
        t.task(n, "# x\n");
    }
    install(&t, &schema(&t));
    assert_eq!(index(&t, 10, &[]).0, 0);
    // `tasks` becomes a symlink to a directory that holds nothing.
    let moved = t.0.join("moved");
    std::fs::rename(t.tasks(), &moved).unwrap();
    let empty = t.0.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    symlink(&empty, t.tasks()).unwrap();
    let (code, ans) = index(&t, 11, &[]);
    assert_eq!(code, 2, "{ans}");
    assert_eq!(first_source(&ans)["code"], "glob_root_is_symlink");
    assert_eq!(appended(first_source(&ans)), [0, 0, 0, 0]);
    let (_, ans) = search(&t, 11, &json!({"object_type":"task"}));
    assert_eq!(keys(&ans), ["a", "b", "c"]);
}

#[test]
fn file_digest_literal_paths_symlinked_parent_and_fifo() {
    let t = Temp::new();
    let real = t.0.join("real");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::write(real.join("f.txt"), "x").unwrap();
    symlink(&real, t.0.join("linked")).unwrap();
    let mut s = schema(&t);
    s["sources"]["files"] = json!({"kind":"file_digest",
        "paths":[t.0.join("linked/f.txt").display().to_string()],"fresh_for_s":60});
    s["object_types"]["digest"] = json!({"owner":{"source":"files"},"primary_key":{"from":"/path"},
        "properties":{"sha256":{"type":"string","required":true,"from":"/sha256"}}});
    s["grants"][0]["read"] = json!(["task", "digest"]);
    install(&t, &s);
    let (code, ans) = index(&t, 1, &["--source", "files"]);
    assert_eq!(code, 2, "{ans}");
    assert_eq!(first_source(&ans)["code"], "glob_root_is_symlink");
    // A FIFO is not a regular file; the run does not block.
    let fifo = t.0.join("pipe");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let mut s2 = schema(&t);
    s2["version"] = json!(2);
    s2["sources"]["files"] = json!({"kind":"file_digest",
        "paths":[fifo.display().to_string()],"fresh_for_s":60});
    s2["object_types"]["digest"] = s["object_types"]["digest"].clone();
    s2["grants"][0]["read"] = json!(["task", "digest"]);
    let file = t.file("s2.json", &serde_json::to_vec(&s2).unwrap());
    let (code, _) = run(&["--store", &t.store(), "schema", "install", &file], b"");
    assert_eq!(code, 0);
    let (code, ans) = index(&t, 2, &["--source", "files"]);
    assert_eq!(code, 2, "{ans}");
    assert_eq!(first_source(&ans)["code"], "not_regular_file");
}

#[test]
fn bytes_that_differ_outside_the_mapped_fields_refresh_the_lineage() {
    let t = Temp::new();
    t.task("a", "# a\nOwner: x\n");
    let mut s = schema(&t);
    s["object_types"]["task"]["properties"]
        .as_object_mut()
        .unwrap()
        .remove("state_sha256");
    install(&t, &s);
    assert_eq!(index(&t, 10, &[]).0, 0);
    t.task("a", "# a\nOwner: x\nunrelated prose\n");
    let (_, ans) = index(&t, 11, &[]);
    assert_eq!(appended(first_source(&ans)), [0, 0, 1, 0], "{ans}");
    let (_, ans) = search(&t, 11, &json!({"object_type":"task"}));
    let head = &ans["result"]["objects"][0]["head"];
    assert_eq!(head["change"], "refresh");
    let bytes = std::fs::read(t.tasks().join("a/STATE.md")).unwrap();
    use sha2::{Digest, Sha256};
    assert_eq!(
        head["lineage"]["artifact_sha256"],
        format!("{:x}", Sha256::digest(bytes))
    );
    // Unchanged bytes: nothing.
    let (_, ans) = index(&t, 12, &[]);
    assert_eq!(appended(first_source(&ans)), [0, 0, 0, 0]);
}

#[test]
fn one_index_run_at_a_time_per_store() {
    let t = Temp::new();
    t.task("a", "# a\n");
    install(&t, &schema(&t));
    assert_eq!(index(&t, 10, &[]).0, 0);
    let lock_path = format!("{}.index.lock", t.store());
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&lock_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    let held = std::fs::OpenOptions::new()
        .write(true)
        .open(&lock_path)
        .unwrap();
    held.try_lock().unwrap();
    let (code, ans) = index(&t, 11, &[]);
    assert_eq!(
        (code, ans["code"].as_str()),
        (2, Some("index_busy")),
        "{ans}"
    );
    // Search is not blocked by the lock.
    assert_eq!(search(&t, 11, &json!({"object_type":"task"})).0, 0);
    drop(held);
    assert_eq!(index(&t, 12, &[]).0, 0);
}

#[test]
fn same_second_changes_get_distinct_revisions_and_double_star_globs_index() {
    let t = Temp::new();
    t.task("a", "# one\n");
    install(&t, &schema(&t));
    assert_eq!(index(&t, 1000, &[]).0, 0);
    for (text, rev) in [("# two\n", "r1@1000"), ("# one\n", "r2@1000")] {
        t.task("a", text);
        let (_, ans) = index(&t, 1000, &[]);
        assert_eq!(appended(first_source(&ans)), [0, 1, 0, 0], "{ans}");
        let (_, ans) = search(&t, 1000, &json!({"object_type":"task"}));
        assert_eq!(ans["result"]["objects"][0]["head"]["revision"], rev);
    }
    // `**`: zero or more directories.
    let t = Temp::new();
    t.task("a/b", "# nested\n");
    t.task("c", "# c\n");
    std::fs::write(t.tasks().join("STATE.md"), "# top\n").unwrap();
    let mut s = schema(&t);
    s["sources"]["task_states"]["glob"] = json!(format!("{}/**/STATE.md", t.tasks().display()));
    install(&t, &s);
    let (code, ans) = index(&t, 5, &[]);
    assert_eq!(code, 0, "{ans}");
    assert_eq!(first_source(&ans)["records"], 3);
    let (_, ans) = search(&t, 5, &json!({"object_type":"task"}));
    assert_eq!(keys(&ans), ["b", "c", "tasks"]);
}

#[test]
fn filter_limits_and_none_counts_unresolved() {
    let t = operator_fixture();
    let mut deep = json!({"eq":["count",1]});
    for _ in 0..9 {
        deep = json!({"all":[deep]});
    }
    let (code, ans) = search(&t, 10, &json!({"object_type":"task","where":deep}));
    assert_eq!((code, ans["code"].as_str()), (2, Some("filter_too_large")));
    let many: Vec<i64> = (0..101).collect();
    let (code, ans) = search(
        &t,
        10,
        &json!({"object_type":"task","where":{"in":["count",many]}}),
    );
    assert_eq!((code, ans["code"].as_str()), (2, Some("filter_too_large")));
    let (k, b) = matched(&t, json!({"none":[{"eq":["count",1]}]}));
    assert_eq!(k, ["t2", "t3"]);
    assert_eq!(b["unresolved_excluded"], 1);
    assert_eq!(b["qualified"], false);
}

#[test]
fn index_request_parse() {
    use ont::index::IndexRequest;
    let r = IndexRequest::parse(br#"{"op":"index","sources":["a","b"]}"#).unwrap();
    assert_eq!(r.sources, Some(vec!["a".to_string(), "b".to_string()]));
    assert!(IndexRequest::parse(b"{}").unwrap().sources.is_none());
    assert!(IndexRequest::parse(br#"{"op":"get"}"#).is_err());
    assert!(IndexRequest::parse(br#"{"sources":"a"}"#).is_err());
    assert!(IndexRequest::parse(br#"{"x":1}"#).is_err());
}

#[test]
fn a_missing_glob_root_with_complete_listing_fails_and_retracts_nothing() {
    let t = Temp::new();
    for n in ["a", "b", "c"] {
        t.task(n, "# x\n");
    }
    install(&t, &schema(&t));
    assert_eq!(index(&t, 10, &[]).0, 0);
    // The root goes away (an unmounted or renamed directory).
    std::fs::rename(t.tasks(), t.0.join("moved")).unwrap();
    let (code, ans) = index(&t, 11, &[]);
    assert_eq!(code, 2, "{ans}");
    assert_eq!(first_source(&ans)["code"], "glob_root_missing");
    assert_eq!(appended(first_source(&ans)), [0, 0, 0, 0]);
    let (_, ans) = search(&t, 11, &json!({"object_type":"task"}));
    assert_eq!(keys(&ans), ["a", "b", "c"]);
    // The root comes back: the run succeeds again.
    std::fs::rename(t.0.join("moved"), t.tasks()).unwrap();
    assert_eq!(index(&t, 12, &[]).0, 0);
}

#[test]
fn a_missing_literal_parent_fails_a_complete_listing_file_digest_source() {
    let t = Temp::new();
    let dir = t.0.join("mounted");
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("f.txt");
    std::fs::write(&f, "x").unwrap();
    let mut s = schema(&t);
    s["sources"]["files"] = json!({"kind":"file_digest","paths":[f.display().to_string()],
        "complete_listing":true,"fresh_for_s":60});
    s["object_types"]["digest"] = json!({"owner":{"source":"files"},"primary_key":{"from":"/path"},
        "properties":{"sha256":{"type":"string","required":true,"from":"/sha256"}}});
    s["grants"][0]["read"] = json!(["task", "note", "digest"]);
    install(&t, &s);
    let (code, ans) = index(&t, 1, &["--source", "files"]);
    assert_eq!(
        (code, appended(first_source(&ans))),
        (0, [1, 0, 0, 0]),
        "{ans}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
    let (code, ans) = index(&t, 2, &["--source", "files"]);
    assert_eq!(code, 2, "{ans}");
    assert_eq!(first_source(&ans)["code"], "glob_root_missing");
    let (_, ans) = search(&t, 2, &json!({"object_type":"digest"}));
    assert_eq!(keys(&ans), [f.display().to_string()]);
    // Without complete_listing a missing root is an empty listing.
    let mut s2 = s.clone();
    s2["version"] = json!(2);
    s2["sources"]["files"]["complete_listing"] = json!(false);
    let file = t.file("s2.json", &serde_json::to_vec(&s2).unwrap());
    let (code, _) = run(&["--store", &t.store(), "schema", "install", &file], b"");
    assert_eq!(code, 0);
    let (code, ans) = index(&t, 3, &["--source", "files"]);
    assert_eq!(
        (code, appended(first_source(&ans))),
        (0, [0, 0, 0, 0]),
        "{ans}"
    );
}

#[test]
fn a_run_lock_that_cannot_be_opened_is_a_refusal_not_an_internal_failure() {
    let t = Temp::new();
    t.task("a", "# a\n");
    install(&t, &schema(&t));
    // A symlink at the lock name is not followed (O_NOFOLLOW).
    let target = t.0.join("elsewhere");
    std::fs::write(&target, "keep").unwrap();
    symlink(&target, format!("{}.index.lock", t.store())).unwrap();
    let (code, ans) = index(&t, 10, &[]);
    assert_eq!(
        (code, ans["status"].as_str(), ans["code"].as_str()),
        (2, Some("refused"), Some("index_lock_unavailable")),
        "{ans}"
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
}

#[test]
fn a_literal_prefix_that_is_not_a_directory_fails_a_complete_listing_source() {
    let t = Temp::new();
    t.task("a", "# a\n");
    install(&t, &schema(&t));
    assert_eq!(index(&t, 10, &[]).0, 0);
    // `tasks` becomes a regular file.
    std::fs::rename(t.tasks(), t.0.join("moved")).unwrap();
    std::fs::write(t.tasks(), "x").unwrap();
    let (code, ans) = index(&t, 11, &[]);
    assert_eq!(code, 2, "{ans}");
    assert_eq!(first_source(&ans)["code"], "glob_root_not_directory");
    assert_eq!(appended(first_source(&ans)), [0, 0, 0, 0]);
    let (_, ans) = search(&t, 11, &json!({"object_type":"task"}));
    assert_eq!(keys(&ans), ["a"]);
    // The same for the parent of a literal file_digest path.
    let dir = t.0.join("mounted");
    std::fs::write(&dir, "x").unwrap();
    let f = dir.join("f.txt");
    let mut s = schema(&t);
    s["version"] = json!(2);
    s["sources"]["files"] = json!({"kind":"file_digest","paths":[f.display().to_string()],
        "complete_listing":true,"fresh_for_s":60});
    s["object_types"]["digest"] = json!({"owner":{"source":"files"},"primary_key":{"from":"/path"},
        "properties":{"sha256":{"type":"string","required":true,"from":"/sha256"}}});
    s["grants"][0]["read"] = json!(["task", "note", "digest"]);
    let file = t.file("s2.json", &serde_json::to_vec(&s).unwrap());
    assert_eq!(
        run(&["--store", &t.store(), "schema", "install", &file], b"").0,
        0
    );
    let (code, ans) = index(&t, 12, &["--source", "files"]);
    assert_eq!(code, 2, "{ans}");
    assert_eq!(first_source(&ans)["code"], "glob_root_not_directory");
    // Made a directory again: both sources run.
    std::fs::remove_file(&dir).unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&f, "x").unwrap();
    std::fs::remove_file(t.tasks()).unwrap();
    std::fs::rename(t.0.join("moved"), t.tasks()).unwrap();
    let (code, ans) = index(&t, 13, &[]);
    assert_eq!(code, 0, "{ans}");
}

fn get_obj(t: &Temp, object_type: &str, key: &str) -> (i32, Value) {
    let req = json!({"id":{"object_type":object_type,
        "namespace":{"instance":"vps://palantirkc","account":"root"},"native_key":key}});
    run(
        &["--store", &t.store(), "get"],
        &serde_json::to_vec(&req).unwrap(),
    )
}
fn ref_id(key: &str) -> Value {
    json!({"object_type":"task",
        "namespace":{"instance":"vps://palantirkc","account":"root"},"native_key":key})
}

#[test]
fn several_globs_union_dedup_exclude_retract_and_missing_root() {
    let t = Temp::new();
    let base = t.tasks();
    std::fs::create_dir_all(base.join("a/b/c")).unwrap();
    std::fs::create_dir_all(base.join("excluded")).unwrap();
    std::fs::write(base.join("a/STATE.md"), "# a\n").unwrap();
    std::fs::write(base.join("a/b/STATE.md"), "# b\n").unwrap();
    std::fs::write(base.join("a/b/c/STATE.md"), "# c\n").unwrap();
    std::fs::write(base.join("excluded/STATE.md"), "# x\n").unwrap();
    let mut s = schema(&t);
    s["sources"]["task_states"]["glob"] = json!([
        format!("{}/*/STATE.md", base.display()),
        format!("{}/*/*/STATE.md", base.display()),
        format!("{}/**/STATE.md", base.display())
    ]);
    s["sources"]["task_states"]["exclude"] = json!([format!("{}/excluded", base.display())]);
    install(&t, &s);
    let (code, ans) = index(&t, 100, &[]);
    assert_eq!(code, 0, "{ans}");
    // Union: three distinct files; `a` and `b` are also matched by `**`.
    assert_eq!(first_source(&ans)["records"], 3, "{ans}");
    assert_eq!(appended(first_source(&ans)), [3, 0, 0, 0]);
    let (_, ans) = search(&t, 100, &json!({"object_type":"task"}));
    assert_eq!(keys(&ans), ["a", "b", "c"]);
    // Remove the depth-2 file: retracted against the union.
    std::fs::remove_file(base.join("a/b/STATE.md")).unwrap();
    let (_, ans) = index(&t, 101, &[]);
    assert_eq!(appended(first_source(&ans)), [0, 0, 0, 1], "{ans}");
    let (_, ans) = search(&t, 101, &json!({"object_type":"task"}));
    assert_eq!(keys(&ans), ["a", "c"]);
    // A glob with a missing literal root fails the whole source: nothing
    // appended and nothing retracted.
    let mut s2 = s.clone();
    s2["version"] = json!(2);
    s2["sources"]["task_states"]["glob"] = json!([
        format!("{}/*/STATE.md", base.display()),
        format!("{}/gone/*/STATE.md", base.display())
    ]);
    let file = t.file("s2.json", &serde_json::to_vec(&s2).unwrap());
    assert_eq!(
        run(&["--store", &t.store(), "schema", "install", &file], b"").0,
        0
    );
    let (code, ans) = index(&t, 102, &[]);
    assert_eq!(code, 2, "{ans}");
    assert_eq!(first_source(&ans)["code"], "glob_root_missing");
    assert_eq!(appended(first_source(&ans)), [0, 0, 0, 0]);
    let (_, ans) = search(&t, 102, &json!({"object_type":"task"}));
    assert_eq!(keys(&ans), ["a", "c"]);
    // `glob_root_not_directory` from the second glob: the same rule.
    std::fs::write(t.0.join("afile"), "x").unwrap();
    let mut s3 = s2.clone();
    s3["version"] = json!(3);
    s3["sources"]["task_states"]["glob"] = json!([
        format!("{}/*/STATE.md", base.display()),
        format!("{}/afile/*/STATE.md", t.0.display())
    ]);
    let file = t.file("s3.json", &serde_json::to_vec(&s3).unwrap());
    assert_eq!(
        run(&["--store", &t.store(), "schema", "install", &file], b"").0,
        0
    );
    let (code, ans) = index(&t, 103, &[]);
    assert_eq!(code, 2, "{ans}");
    assert_eq!(first_source(&ans)["code"], "glob_root_not_directory");
    assert_eq!(appended(first_source(&ans)), [0, 0, 0, 0]);
    let (_, ans) = search(&t, 103, &json!({"object_type":"task"}));
    assert_eq!(keys(&ans), ["a", "c"]);
}

#[test]
fn key_template_and_references_from_record_strings() {
    let t = Temp::new();
    t.task("alpha", "# Alpha\nOwner: lead\n");
    let packets = t.0.join("packets");
    for (dir, file) in [
        ("alpha", "TASK-one.md"),
        ("alpha", "TASK.md"),
        ("ghost", "TASK-two.md"),
    ] {
        std::fs::create_dir_all(packets.join(dir)).unwrap();
        std::fs::write(packets.join(dir).join(file), format!("# {file}\n")).unwrap();
    }
    // An empty `Target:` field: an empty reference string (see below).
    std::fs::write(
        packets.join("alpha/TASK-one.md"),
        "# TASK-one.md\nTarget:\n",
    )
    .unwrap();
    let prefix = format!("{}/", packets.display());
    let mut s = schema(&t);
    s["sources"]["packet_files"] = json!({"kind":"markdown_fields",
        "glob":format!("{}/*/TASK*.md", packets.display()),
        "head_lines":40,"complete_listing":true,"fresh_for_s":3600});
    s["object_types"]["packet"] = json!({
    "owner":{"source":"packet_files"},
    "primary_key":{"template":"{dir}/{stem}"},
    "title":"title_line",
    "properties":{
        "title_line":{"type":"string","required":true,"from":"/line1"},
        "task_by_template":{"type":{"ref":"task"},"required":false,"template":"{dir}"},
        "task_by_segment":{"type":{"ref":"task"},"required":false,"from":"/dir_path",
            "transform":{"segment_after":prefix}},
        "task_by_replace":{"type":{"ref":"task"},"required":false,"from":"/dir_path",
            "transform":{"replace_prefix":[prefix,""]}},
        "task_by_template_unresolved":{"type":{"ref":"task"},"required":false,
            "template":"{stem_id}"},
        "task_by_segment_unresolved":{"type":{"ref":"task"},"required":false,
            "from":"/dir","transform":{"segment_after":prefix}},
        "task_by_replace_unresolved":{"type":{"ref":"task"},"required":false,
            "from":"/dir","transform":{"replace_prefix":[prefix,"x"]}},
        "task_empty":{"type":{"ref":"task"},"required":false,"from":"/fields/Target"}
    }});
    s["link_types"] = json!({"packet_task":{"from":"packet","via":"task_by_template",
        "to":"task","cardinality":"many_to_one","names":{"forward":"task","reverse":"packets"}}});
    s["grants"][0]["read"] = json!(["task", "note", "packet"]);
    install(&t, &s);
    let (code, ans) = index(&t, 200, &[]);
    assert_eq!(code, 0, "{ans}");
    let (_, ans) = search(&t, 200, &json!({"object_type":"packet"}));
    // Key template over two fields: `{dir}/{stem}`.
    assert_eq!(
        keys(&ans),
        ["alpha/TASK", "alpha/TASK-one", "ghost/TASK-two"]
    );
    let objs = ans["result"]["objects"].as_array().unwrap();
    let one = objs
        .iter()
        .find(|o| o["id"]["native_key"] == "alpha/TASK-one")
        .unwrap();
    let fields = &one["head"]["fields"];
    for prop in ["task_by_template", "task_by_segment", "task_by_replace"] {
        assert_eq!(fields[prop]["kind"], "value", "{prop}: {fields}");
        assert_eq!(fields[prop]["value"], ref_id("alpha"), "{prop}");
    }
    assert_eq!(
        fields["task_by_template_unresolved"]["value"],
        ref_id("one")
    );
    for prop in ["task_by_segment_unresolved", "task_by_replace_unresolved"] {
        assert_eq!(fields[prop]["kind"], "missing", "{prop}: {fields}");
    }
    // An empty reference string is stored as missing, never an empty Id.
    assert_eq!(fields["task_empty"]["kind"], "missing", "{fields}");
    // Follow a resolved reference: `get` on its Id returns the target.
    let target = fields["task_by_template"]["value"].clone();
    let (code, ans) = get_obj(
        &t,
        target["object_type"].as_str().unwrap(),
        target["native_key"].as_str().unwrap(),
    );
    assert_eq!(code, 0, "{ans}");
    assert_eq!(ans["result"]["objects"][0]["id"]["native_key"], "alpha");
    assert_eq!(
        ans["result"]["objects"][0]["observations"][0]["fields"]["directory"]["value"],
        t.tasks().join("alpha").display().to_string()
    );
    // No hyphen in `TASK`: `stem_id` is absent, so the template is unresolved.
    let plain = objs
        .iter()
        .find(|o| o["id"]["native_key"] == "alpha/TASK")
        .unwrap();
    assert_eq!(
        plain["head"]["fields"]["task_by_template_unresolved"]["kind"],
        "missing"
    );
    // Dangling reference: stored as it is; `get` returns the full Id even
    // though no `ghost` task exists, and `search` on tasks finds none.
    let (code, ans) = get_obj(&t, "packet", "ghost/TASK-two");
    assert_eq!(code, 0, "{ans}");
    let ghost_fields = &ans["result"]["objects"][0]["observations"][0]["fields"];
    assert_eq!(ghost_fields["task_by_template"]["value"], ref_id("ghost"));
    let (_, ans) = search(
        &t,
        200,
        &json!({"object_type":"task","where":{"eq":["directory",
            t.tasks().join("ghost").display().to_string()]}}),
    );
    assert_eq!(keys(&ans), Vec::<String>::new());
    // `search` compares a reference with `eq` (full Id) and `unresolved`.
    let (_, ans) = search(
        &t,
        200,
        &json!({"object_type":"packet","where":{"eq":["task_by_template", ref_id("alpha")]}}),
    );
    assert_eq!(keys(&ans), ["alpha/TASK", "alpha/TASK-one"]);
    let (_, ans) = search(
        &t,
        200,
        &json!({"object_type":"packet","where":{"unresolved":"task_by_segment_unresolved"}}),
    );
    assert_eq!(
        keys(&ans),
        ["alpha/TASK", "alpha/TASK-one", "ghost/TASK-two"]
    );
    // Rename a packet file: the key template's input changes, so in one
    // run the old object is retracted and the new one appended.
    std::fs::rename(
        packets.join("alpha/TASK-one.md"),
        packets.join("alpha/TASK-three.md"),
    )
    .unwrap();
    let (code, ans) = index(&t, 201, &[]);
    assert_eq!(code, 0, "{ans}");
    let packet_entry = ans["result"]["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["source"] == "packet_files")
        .unwrap()
        .clone();
    assert_eq!(appended(&packet_entry), [1, 0, 0, 1], "{packet_entry}");
    let (_, ans) = search(&t, 201, &json!({"object_type":"packet"}));
    assert_eq!(
        keys(&ans),
        ["alpha/TASK", "alpha/TASK-three", "ghost/TASK-two"]
    );
}

#[test]
fn schema_check_refuses_templates_glob_arrays_and_new_transforms() {
    let t = Temp::new();
    let base = schema(&t);
    let check = |s: &Value| -> Vec<(String, String)> {
        let file = t.file("c.json", &serde_json::to_vec(s).unwrap());
        let (_, ans) = run(&["--store", &t.store(), "schema", "check", &file], b"");
        ans["violations"]
            .as_array()
            .map(|v| {
                v.iter()
                    .map(|x| {
                        (
                            x["code"].as_str().unwrap().to_string(),
                            x["pointer"].as_str().unwrap().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let with = |edit: &dyn Fn(&mut Value)| {
        let mut s = base.clone();
        edit(&mut s);
        check(&s)
    };
    let p = |c: &str, p: &str| vec![(c.to_string(), p.to_string())];
    // Glob array forms.
    assert_eq!(
        with(&|s| s["sources"]["task_states"]["glob"] = json!([])),
        p("invalid_source_option", "/sources/task_states/glob")
    );
    assert_eq!(
        with(&|s| s["sources"]["task_states"]["glob"] = json!(vec!["/a/*/S.md"; 17])),
        p("invalid_source_option", "/sources/task_states/glob")
    );
    assert_eq!(
        with(&|s| s["sources"]["task_states"]["glob"] = json!(["/a/*/S.md", "rel/S.md"])),
        p("invalid_glob", "/sources/task_states/glob/1")
    );
    assert_eq!(
        with(&|s| s["sources"]["task_states"]["glob"] = json!(["/a/*/S.md", 1])),
        p("invalid_source_option", "/sources/task_states/glob/1")
    );
    // Templates.
    let tpl_ptr = "/object_types/task/properties/owner_line/template";
    assert_eq!(
        with(&|s| {
            let prop = &mut s["object_types"]["task"]["properties"]["owner_line"];
            prop.as_object_mut().unwrap().remove("from");
            prop["template"] = json!("{nope}");
        }),
        p("unknown_template_field", tpl_ptr)
    );
    for bad in [
        "{dir",
        "{}",
        "{dir}}",
        "{di r}",
        "{dir}{dir}{dir}{dir}{dir}{dir}{dir}{dir}{dir}",
    ] {
        assert_eq!(
            with(&|s| {
                let prop = &mut s["object_types"]["task"]["properties"]["owner_line"];
                prop.as_object_mut().unwrap().remove("from");
                prop["template"] = json!(bad);
            }),
            p("invalid_template", tpl_ptr),
            "{bad}"
        );
    }
    assert_eq!(
        with(&|s| {
            let prop = &mut s["object_types"]["task"]["properties"]["owner_line"];
            prop.as_object_mut().unwrap().remove("from");
            prop["template"] = json!("x".repeat(257));
        }),
        p("invalid_template", tpl_ptr)
    );
    assert_eq!(
        with(
            &|s| s["object_types"]["task"]["properties"]["owner_line"]["template"] = json!("{dir}")
        ),
        p("invalid_template", tpl_ptr)
    );
    assert_eq!(
        with(&|s| s["object_types"]["task"]["primary_key"] = json!({"template":"{nope}"})),
        p(
            "unknown_template_field",
            "/object_types/task/primary_key/template"
        )
    );
    assert_eq!(
        with(&|s| s["object_types"]["task"]["primary_key"] =
            json!({"from":"/dir","template":"{dir}"})),
        p(
            "invalid_template",
            "/object_types/task/primary_key/template"
        )
    );
    // A placeholder naming a field that is never a string, and duplicate
    // placeholder names giving one violation each.
    for field in ["size", "mtime", "fields"] {
        assert_eq!(
            with(&|s| {
                let prop = &mut s["object_types"]["task"]["properties"]["owner_line"];
                prop.as_object_mut().unwrap().remove("from");
                prop["template"] = json!(format!("{{{field}}}"));
            }),
            p("template_field_not_string", tpl_ptr),
            "{field}"
        );
    }
    assert_eq!(
        with(&|s| {
            let prop = &mut s["object_types"]["task"]["properties"]["owner_line"];
            prop.as_object_mut().unwrap().remove("from");
            prop["template"] = json!("{nope}{nope}");
        }),
        p("unknown_template_field", tpl_ptr)
    );
    assert_eq!(
        with(&|s| {
            let prop = &mut s["object_types"]["task"]["properties"]["owner_line"];
            prop.as_object_mut().unwrap().remove("from");
            prop["template"] = json!("{size}{size}");
        }),
        p("template_field_not_string", tpl_ptr)
    );
    // An array-of-references property filled from a source is refused.
    assert_eq!(
        with(&|s| s["object_types"]["task"]["properties"]["owner_line"] =
            json!({"type":{"array":{"ref":"task"}},"required":false,"from":"/dir"})),
        p(
            "reference_array_from_source_unsupported",
            "/object_types/task/properties/owner_line/from"
        )
    );
    assert_eq!(
        with(&|s| s["object_types"]["task"]["properties"]["owner_line"] =
            json!({"type":{"array":{"ref":"task"}},"required":false,"template":"{dir}"})),
        p(
            "reference_array_from_source_unsupported",
            "/object_types/task/properties/owner_line/template"
        )
    );
    // New transforms.
    let tr_ptr = "/object_types/task/properties/owner_line/transform";
    for bad in [
        json!({"segment_after": "relative/"}),
        json!({"segment_after": "/no-trailing-slash"}),
        json!({"segment_after": 123}),
        json!({"segment_after": format!("/{}/", "x".repeat(1024))}),
        json!({"replace_prefix": ["/a"]}),
        json!({"replace_prefix": ["x".repeat(1025), ""]}),
        json!({"replace_prefix": ["", "x"]}),
        json!({"unknown_transform": "/a/"}),
    ] {
        assert_eq!(
            with(
                &|s| s["object_types"]["task"]["properties"]["owner_line"]["transform"] =
                    bad.clone()
            ),
            p("invalid_transform", tr_ptr),
            "{bad}"
        );
    }
    // A reference property whose target type is not declared.
    assert_eq!(
        with(&|s| s["object_types"]["task"]["properties"]["owner_line"] =
            json!({"type":{"ref":"nope"},"required":false,"from":"/dir"})),
        p(
            "unknown_type",
            "/object_types/task/properties/owner_line/type"
        )
    );
    // Valid forms are described as declared.
    let mut good = base.clone();
    good["object_types"]["task"]["properties"]["owner_line"]["transform"] =
        json!({"segment_after": "/a/"});
    good["object_types"]["task"]["properties"]["directory"]["template"] = json!("{dir}");
    good["object_types"]["task"]["properties"]["directory"]
        .as_object_mut()
        .unwrap()
        .remove("from");
    assert!(check(&good).is_empty(), "{:?}", check(&good));
    install(&t, &good);
    let (_, ans) = run(&["--store", &t.store(), "describe"], b"{}");
    let task = ans["result"]["object_types"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["name"] == "task")
        .unwrap()
        .clone();
    let props = task["properties"].as_array().unwrap();
    let owner = props.iter().find(|x| x["name"] == "owner_line").unwrap();
    assert_eq!(owner["transform"], json!({"segment_after": "/a/"}));
    let dir = props.iter().find(|x| x["name"] == "directory").unwrap();
    assert_eq!(dir["template"], "{dir}");
}

#[test]
fn existing_example_fixture_keeps_its_pin() {
    const EXAMPLE: &[u8] = include_bytes!("../../ont-core/tests/schema-fixtures/example.json");
    let t = Temp::new();
    let file = t.file("example.json", EXAMPLE);
    let (code, ans) = run(&["--store", &t.store(), "schema", "check", &file], b"");
    assert_eq!(code, 0, "{ans}");
    assert_eq!(ans["clean"], true);
    assert_eq!(
        ans["pin"]["sha256"],
        "1f7c885dc163d8d6ffbaceb573f178bc239d890069effe93c8f1626ee0132ad0"
    );
    use sha2::{Digest, Sha256};
    assert_eq!(
        format!("{:x}", Sha256::digest(EXAMPLE)),
        "1f7c885dc163d8d6ffbaceb573f178bc239d890069effe93c8f1626ee0132ad0"
    );
}
