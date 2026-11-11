//! Synthetic CLI tests through the library layer (uids are passed in, so
//! the test runner does not need to be root) plus binary spawn checks.
#![allow(clippy::unwrap_used)]
use ont_store::Store;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const EXAMPLE: &[u8] = include_bytes!("../../ont-core/tests/schema-fixtures/example.json");

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "ont-cli-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn store(&self) -> String {
        self.0.join("store.sqlite").display().to_string()
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
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let out = ont::run(&args, stdin, 0, 0).unwrap();
    assert_eq!(out.stderr, "", "stderr: {}", out.stderr);
    let value: Value = serde_json::from_str(out.stdout.trim()).unwrap();
    (out.code, value)
}
fn run_text(args: &[&str], stdin: &[u8]) -> (i32, String) {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let out = ont::run(&args, stdin, 0, 0).unwrap();
    (out.code, out.stdout)
}
fn install_example(t: &Temp) -> (i32, Value) {
    let file = t.file("example.json", EXAMPLE);
    run(&["--store", &t.store(), "schema", "install", &file], b"")
}
fn append_task(store_path: &str) {
    use ont_core::{Coverage, Id, Knowledge, Lineage, Namespace, Observation, Scope};
    let ns = Namespace {
        instance: "vps://palantirkc".into(),
        account: "root".into(),
    };
    let mut store = Store::open_schema(std::path::Path::new(store_path)).unwrap();
    let fields = BTreeMap::from([
        (
            "directory".to_string(),
            Knowledge::Value(json!("/tmp/task")),
        ),
        ("state_sha256".to_string(), Knowledge::Value(json!("abc"))),
        (
            "checkpoint_line".to_string(),
            Knowledge::Value(json!("line")),
        ),
    ]);
    store
        .append_schema(
            &[ont_store::Append {
                id: Id {
                    object_type: "task".into(),
                    namespace: ns.clone(),
                    native_key: "ontology-operational-base".into(),
                },
                scope: Scope::domain(ns, "work", BTreeMap::new()).unwrap(),
                observation: Observation {
                    revision: "r1".into(),
                    source_revision: "s1".into(),
                    observed_at: 10,
                    fresh_until: 20,
                    change: ont_core::Change::Initial,
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
fn task_id() -> Value {
    json!({"object_type":"task","namespace":{"instance":"vps://palantirkc","account":"root"},"native_key":"ontology-operational-base"})
}

#[test]
fn check_example_and_two_violations() {
    let t = Temp::new();
    let file = t.file("example.json", EXAMPLE);
    let (code, ans) = run(&["--store", &t.store(), "schema", "check", &file], b"");
    assert_eq!(code, 0, "{ans}");
    assert_eq!(
        ans["pin"]["sha256"],
        "1f7c885dc163d8d6ffbaceb573f178bc239d890069effe93c8f1626ee0132ad0"
    );
    assert!(
        !std::path::Path::new(&t.store()).exists(),
        "check must not create a store"
    );
    // Two violations are both listed, exit 2, still no store.
    let mut bad: Value = serde_json::from_slice(EXAMPLE).unwrap();
    bad["domain"] = json!("Work");
    bad["version"] = json!(0);
    let file = t.file("bad.json", &serde_json::to_vec(&bad).unwrap());
    let (code, ans) = run(&["--store", &t.store(), "schema", "check", &file], b"");
    assert_eq!(code, 2, "{ans}");
    let codes: Vec<&str> = ans["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"invalid_name"), "{codes:?}");
    assert!(codes.contains(&"invalid_version"), "{codes:?}");
    assert!(!std::path::Path::new(&t.store()).exists());
}

#[test]
fn install_describe_get_and_backup_flow() {
    let t = Temp::new();
    let (code, ans) = install_example(&t);
    assert_eq!(code, 0, "{ans}");
    assert_eq!(ans["result"]["changed"], true);
    assert_eq!(ans["result"]["previous_version"], Value::Null);
    assert_eq!(ans["result"]["backup"], Value::Null);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(t.store()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    // Same bytes again: a no-op.
    let (code, ans) = install_example(&t);
    assert_eq!(code, 0, "{ans}");
    assert_eq!(ans["result"]["changed"], false);
    // Describe lists both types, the link and the action.
    let (code, ans) = run(&["--store", &t.store(), "describe"], b"{}");
    assert_eq!(code, 0, "{ans}");
    assert_eq!(ans["status"], "ok");
    assert!(ans["trust_limit"].as_str().unwrap().contains("OS-account"));
    assert_eq!(ans["schema"]["work"]["version"], 1);
    let types: Vec<&str> = ans["result"]["object_types"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["name"].as_str().unwrap())
        .collect();
    assert_eq!(types, ["hold", "task"], "{types:?}");
    assert_eq!(ans["result"]["link_types"][0]["name"], "hold_task");
    assert_eq!(ans["result"]["action_types"][0]["name"], "set_hold");
    // Get with pins and stale at two values of --now.
    append_task(&t.store());
    for (now, stale) in [(10u64, false), (30u64, true)] {
        let req = serde_json::to_vec(&json!({"id": task_id()})).unwrap();
        let (code, ans) = run(
            &["--store", &t.store(), "--now", &now.to_string(), "get"],
            &req,
        );
        assert_eq!(code, 0, "{ans}");
        let obs = &ans["result"]["objects"][0]["observations"][0];
        assert_eq!(obs["stale"], stale, "now={now}");
        assert_eq!(
            obs["schema"]["sha256"],
            "1f7c885dc163d8d6ffbaceb573f178bc239d890069effe93c8f1626ee0132ad0"
        );
    }
    // A second, different install takes a backup; rotation keeps at most 3.
    for version in [2u64, 3, 4, 5] {
        let mut v: Value = serde_json::from_slice(EXAMPLE).unwrap();
        v["version"] = json!(version);
        let file = t.file(
            &format!("v{version}.json"),
            &serde_json::to_vec(&v).unwrap(),
        );
        let (code, ans) = run(&["--store", &t.store(), "schema", "install", &file], b"");
        assert_eq!(code, 0, "v{version}: {ans}");
        if version == 2 {
            let backup = ans["result"]["backup"].as_str().unwrap().to_string();
            assert!(std::path::Path::new(&backup).exists());
        }
    }
    let backup_dir = t.0.join("backup");
    let count = std::fs::read_dir(&backup_dir).unwrap().count();
    assert!(count <= 3, "backups kept: {count}");
    // The older version now refuses.
    let file = t.file("example.json", EXAMPLE);
    let (code, ans) = run(&["--store", &t.store(), "schema", "install", &file], b"");
    assert_eq!(code, 2, "{ans}");
    assert_eq!(ans["code"], "schema_version_conflict");
    // Backup command prints a path and the file's sha256.
    let (code, ans) = run(&["--store", &t.store(), "backup"], b"");
    assert_eq!(code, 0, "{ans}");
    let path = ans["result"]["path"].as_str().unwrap();
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(ans["result"]["sha256"], ont::sha256_of(&bytes));
}

#[test]
fn ungranted_type_is_absent_and_its_object_is_not_found() {
    let t = Temp::new();
    let mut schema: Value = serde_json::from_slice(EXAMPLE).unwrap();
    schema["grants"][0]["read"] = json!(["task"]);
    schema["grants"][0]["derive"] = json!(["task"]);
    let file = t.file("s.json", &serde_json::to_vec(&schema).unwrap());
    let (code, _) = run(&["--store", &t.store(), "schema", "install", &file], b"");
    assert_eq!(code, 0);
    let (code, ans) = run(&["--store", &t.store(), "describe"], b"{}");
    assert_eq!(code, 0, "{ans}");
    let types: Vec<&str> = ans["result"]["object_types"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["name"].as_str().unwrap())
        .collect();
    assert_eq!(types, ["task"]);
    // A hold object exists in the store but may not be read.
    append_task(&t.store());
    let hold_id = json!({"object_type":"hold","namespace":{"instance":"vps://palantirkc","account":"root"},"native_key":"h1"});
    let req = serde_json::to_vec(&json!({"id": hold_id})).unwrap();
    let (code, ans) = run(&["--store", &t.store(), "get"], &req);
    assert_eq!(code, 2, "{ans}");
    assert_eq!(ans["code"], "unavailable");
    // And a missing object is indistinguishable from it.
    let missing = json!({"object_type":"task","namespace":{"instance":"vps://palantirkc","account":"root"},"native_key":"nope"});
    let req = serde_json::to_vec(&json!({"id": missing})).unwrap();
    let (code2, ans2) = run(&["--store", &t.store(), "get"], &req);
    assert_eq!((code2, ans2["code"].as_str()), (2, Some("unavailable")));
}

#[test]
fn refusal_codes() {
    let t = Temp::new();
    // Unknown argument and missing command.
    let (code, ans) = run(&["--bogus"], b"");
    assert_eq!((code, ans["code"].as_str()), (2, Some("invalid_argument")));
    let (code, ans) = run(&[], b"");
    assert_eq!((code, ans["code"].as_str()), (2, Some("invalid_argument")));
    // --now without --store.
    let (code, ans) = run(&["--now", "5", "describe"], b"{}");
    assert_eq!(
        (code, ans["code"].as_str()),
        (2, Some("now_requires_store"))
    );
    // Store not found, unreadable schema file, unknown domain.
    let (code, ans) = run(&["--store", &t.store(), "describe"], b"{}");
    assert_eq!((code, ans["code"].as_str()), (2, Some("store_not_found")));
    let (code, ans) = run(
        &[
            "--store",
            &t.store(),
            "schema",
            "check",
            "/nonexistent/x.json",
        ],
        b"",
    );
    assert_eq!(
        (code, ans["code"].as_str()),
        (2, Some("schema_file_unreadable"))
    );
    // Oversized request and unknown request field.
    let (code, _) = install_example(&t);
    assert_eq!(code, 0);
    let big = vec![b' '; ont::MAX_REQUEST + 1];
    let (code, ans) = run(&["--store", &t.store(), "describe"], &big);
    assert_eq!((code, ans["code"].as_str()), (2, Some("invalid_request")));
    let (code, ans) = run(&["--store", &t.store(), "describe"], br#"{"bogus":1}"#);
    assert_eq!((code, ans["code"].as_str()), (2, Some("invalid_request")));
    let (code, ans) = run(
        &["--store", &t.store(), "describe"],
        br#"{"domain":"nope"}"#,
    );
    assert_eq!((code, ans["code"].as_str()), (2, Some("unknown_domain")));
    // Malformed source options.
    let file = t.file("example.json", EXAMPLE);
    let (code, ans) = run(
        &[
            "--store",
            &t.store(),
            "--source-commit",
            "xyz",
            "schema",
            "install",
            &file,
        ],
        b"",
    );
    assert_eq!((code, ans["code"].as_str()), (2, Some("invalid_argument")));
    // Missing store parent names the directory and creates nothing.
    let missing_store = t.0.join("missing-dir/store.sqlite");
    let (code, ans) = run(
        &[
            "--store",
            missing_store.to_str().unwrap(),
            "schema",
            "install",
            &file,
        ],
        b"",
    );
    assert_eq!(code, 2, "{ans}");
    assert_eq!(ans["code"], "store_parent_missing");
    assert!(!t.0.join("missing-dir").exists());
}

#[test]
fn uid_and_version_contracts() {
    assert_eq!(ont::check_uid(0, 0), Ok(()));
    assert_eq!(ont::check_uid(1000, 1000), Err("principal_not_trusted"));
    assert_eq!(ont::check_uid(0, 1000), Err("principal_not_trusted"));
    // The library refuses a non-root principal for describe.
    let t = Temp::new();
    let args: Vec<String> = ["--store", &t.store(), "describe"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let out = ont::run(&args, b"{}", 1000, 1000).unwrap();
    assert_eq!(out.code, 2);
    let v: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(v["code"], "principal_not_trusted");
    let (code, text) = run_text(&["--version"], b"");
    assert_eq!(code, 0);
    let parts: Vec<&str> = text.trim().split(' ').collect();
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[0], "ont");
}

#[test]
fn install_refusals_get_request_forms_and_uid_for_every_command() {
    let t = Temp::new();
    // A bad schema file refuses install with the violation list; no store.
    let mut bad: Value = serde_json::from_slice(EXAMPLE).unwrap();
    bad["domain"] = json!("Work");
    bad["version"] = json!(0);
    let file = t.file("bad.json", &serde_json::to_vec(&bad).unwrap());
    let (code, ans) = run(&["--store", &t.store(), "schema", "install", &file], b"");
    assert_eq!(code, 2, "{ans}");
    assert_eq!(ans["code"], "schema_violation");
    assert!(ans["violations"].as_array().unwrap().len() >= 2);
    assert!(!std::path::Path::new(&t.store()).exists());
    // Request forms: op tag accepted when it matches, refused otherwise.
    let (code, _) = install_example(&t);
    assert_eq!(code, 0);
    let (code, _) = run(
        &["--store", &t.store(), "describe"],
        br#"{"op":"describe"}"#,
    );
    assert_eq!(code, 0);
    let (code, ans) = run(&["--store", &t.store(), "describe"], br#"{"op":"get"}"#);
    assert_eq!((code, ans["code"].as_str()), (2, Some("invalid_request")));
    let req = serde_json::to_vec(&json!({"id": task_id(), "extra": 1})).unwrap();
    let (code, ans) = run(&["--store", &t.store(), "get"], &req);
    assert_eq!((code, ans["code"].as_str()), (2, Some("invalid_request")));
    // Every command refuses a non-root principal.
    let file = t.file("example.json", EXAMPLE);
    for argv in [
        vec!["--store", &t.store(), "schema", "check", &file],
        vec!["--store", &t.store(), "schema", "install", &file],
        vec!["--store", &t.store(), "get"],
        vec!["--store", &t.store(), "backup"],
    ] {
        let args: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
        let out = ont::run(&args, b"{}", 1000, 1000).unwrap();
        let v: Value = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(
            (out.code, v["code"].as_str()),
            (2, Some("principal_not_trusted"))
        );
    }
}
