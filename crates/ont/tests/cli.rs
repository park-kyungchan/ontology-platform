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
    let out = ont::run(&args, stdin, 0, 0);
    assert_eq!(out.stderr, "", "stderr: {}", out.stderr);
    let value: Value = serde_json::from_str(out.stdout.trim()).unwrap();
    (out.code, value)
}
fn run_text(args: &[&str], stdin: &[u8]) -> (i32, String) {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let out = ont::run(&args, stdin, 0, 0);
    (out.code, out.stdout)
}
fn install_example(t: &Temp) -> (i32, Value) {
    let file = t.file("example.json", EXAMPLE);
    run(&["--store", &t.store(), "schema", "install", &file], b"")
}
fn append_task(store_path: &str) {
    append_object(store_path, "task", "ontology-operational-base");
}
fn append_object(store_path: &str, object_type: &str, key: &str) {
    use ont_core::{Coverage, Id, Knowledge, Lineage, Namespace, Observation, Scope};
    let ns = Namespace {
        instance: "vps://palantirkc".into(),
        account: "root".into(),
    };
    let mut store = Store::open_schema(std::path::Path::new(store_path)).unwrap();
    let fields: BTreeMap<String, Knowledge> = if object_type == "task" {
        BTreeMap::from([
            (
                "directory".to_string(),
                Knowledge::Value(json!("/tmp/task")),
            ),
            ("state_sha256".to_string(), Knowledge::Value(json!("abc"))),
            (
                "checkpoint_line".to_string(),
                Knowledge::Value(json!("line")),
            ),
        ])
    } else {
        BTreeMap::from([
            ("task".to_string(), Knowledge::Value(task_id())),
            ("effect".to_string(), Knowledge::Value(json!("merge"))),
            ("until".to_string(), Knowledge::Value(json!("later"))),
            ("status".to_string(), Knowledge::Value(json!("active"))),
        ])
    };
    store
        .append_schema(
            &[ont_store::Append {
                id: Id {
                    object_type: object_type.into(),
                    namespace: ns.clone(),
                    native_key: key.into(),
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
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
fn mode_of(path: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
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
    assert_eq!(mode_of(std::path::Path::new(&t.store())), 0o600);
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
    // Modes: the store file is 0600 and no backup directory exists yet.
    assert!(!t.0.join("backup").exists());
    // A second, different install takes a backup; retention keeps the newest
    // three of the names this command writes and leaves other files alone.
    let backup_dir = t.0.join("backup");
    for version in [2u64, 3, 4, 5, 6] {
        let mut v: Value = serde_json::from_slice(EXAMPLE).unwrap();
        v["version"] = json!(version);
        let file = t.file(
            &format!("v{version}.json"),
            &serde_json::to_vec(&v).unwrap(),
        );
        let now = (version - 1) * 1000;
        let (code, ans) = run(
            &[
                "--store",
                &t.store(),
                "--now",
                &now.to_string(),
                "schema",
                "install",
                &file,
            ],
            b"",
        );
        assert_eq!(code, 0, "v{version}: {ans}");
        if version == 2 {
            let backup = ans["result"]["backup"].as_str().unwrap().to_string();
            assert!(std::path::Path::new(&backup).exists());
            assert!(
                backup.ends_with("ontology-19700101T001640Z.sqlite"),
                "{backup}"
            );
            assert_eq!(mode_of(&backup_dir), 0o700);
            assert_eq!(mode_of(std::path::Path::new(&backup)), 0o600);
            std::fs::write(backup_dir.join("notes.txt"), b"keep").unwrap();
            std::fs::write(backup_dir.join("ontology-foo.sqlite"), b"keep").unwrap();
        }
    }
    let mut names: Vec<String> = std::fs::read_dir(&backup_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "notes.txt",
            "ontology-19700101T005000Z.sqlite",
            "ontology-19700101T010640Z.sqlite",
            "ontology-19700101T012320Z.sqlite",
            "ontology-foo.sqlite"
        ],
        "newest three kept, unrelated files untouched"
    );
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
    assert_eq!(ans["result"]["sha256"], sha256_hex(&bytes));
}

fn get_req(id: &Value) -> Vec<u8> {
    serde_json::to_vec(&json!({"id": id})).unwrap()
}
fn hold_id(key: &str) -> Value {
    json!({"object_type":"hold","namespace":{"instance":"vps://palantirkc","account":"root"},"native_key":key})
}

#[test]
fn ungranted_type_is_absent_and_its_object_is_not_found() {
    let t = Temp::new();
    let mut schema: Value = serde_json::from_slice(EXAMPLE).unwrap();
    schema["grants"][0]["read"] = json!(["task"]);
    schema["grants"][0]["derive"] = json!(["task"]);
    schema["grants"][0]["actions"] = json!([]);
    let file = t.file("s.json", &serde_json::to_vec(&schema).unwrap());
    let (code, _) = run(&["--store", &t.store(), "schema", "install", &file], b"");
    assert_eq!(code, 0);
    let (code, ans) = run(&["--store", &t.store(), "describe"], b"{}");
    assert_eq!(code, 0, "{ans}");
    let names = |key: &str| -> Vec<String> {
        ans["result"][key]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["name"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(names("object_types"), ["task"]);
    // The link needs both ends admitted; the action is not granted.
    assert!(names("link_types").is_empty());
    assert!(names("action_types").is_empty());
    // A hold exists in the store but may not be read; the granted task can.
    append_task(&t.store());
    append_object(&t.store(), "hold", "h1");
    let (code, ans) = run(&["--store", &t.store(), "get"], &get_req(&task_id()));
    assert_eq!(code, 0, "{ans}");
    assert_eq!(ans["result"]["objects"][0]["head"], "r1");
    let (hidden_code, hidden) = run_text(&["--store", &t.store(), "get"], &get_req(&hold_id("h1")));
    let (missing_code, missing) =
        run_text(&["--store", &t.store(), "get"], &get_req(&hold_id("nope")));
    assert_eq!(hidden_code, 2);
    assert!(hidden.contains("\"code\":\"unavailable\""), "{hidden}");
    assert_eq!((hidden_code, &hidden), (missing_code, &missing));
    // Widening the grant (a new install) makes the same object readable.
    schema["version"] = json!(2);
    schema["grants"][0]["read"] = json!(["task", "hold"]);
    let file = t.file("s2.json", &serde_json::to_vec(&schema).unwrap());
    let (code, _) = run(&["--store", &t.store(), "schema", "install", &file], b"");
    assert_eq!(code, 0);
    let (code, _) = run(&["--store", &t.store(), "get"], &get_req(&hold_id("h1")));
    assert_eq!(code, 0);
}

#[test]
fn describe_lists_only_what_one_grant_view_admits() {
    let t = Temp::new();
    let mut schema: Value = serde_json::from_slice(EXAMPLE).unwrap();
    // Only `hold` (owned by the Ontology) and its action: no task, no link,
    // no source.
    schema["grants"][0]["read"] = json!(["hold"]);
    schema["grants"][0]["derive"] = json!([]);
    let file = t.file("s.json", &serde_json::to_vec(&schema).unwrap());
    assert_eq!(
        run(&["--store", &t.store(), "schema", "install", &file], b"").0,
        0
    );
    let (code, ans) = run(&["--store", &t.store(), "describe"], b"{}");
    assert_eq!(code, 0, "{ans}");
    assert_eq!(ans["result"]["domains"][0]["domain"], "work");
    assert_eq!(ans["result"]["object_types"].as_array().unwrap().len(), 1);
    assert_eq!(ans["result"]["object_types"][0]["name"], "hold");
    // A ref property keeps its declared target name (stated limit).
    assert!(
        ans["result"]["object_types"][0]["properties"]
            .to_string()
            .contains("task")
    );
    assert_eq!(ans["result"]["link_types"], json!([]));
    assert_eq!(ans["result"]["sources"], json!([]));
    assert_eq!(ans["result"]["action_types"][0]["name"], "set_hold");
    // `task` only: the source of the task is listed, the hold is not.
    let t2 = Temp::new();
    schema["grants"][0]["read"] = json!(["task"]);
    schema["grants"][0]["actions"] = json!([]);
    let file = t2.file("s.json", &serde_json::to_vec(&schema).unwrap());
    assert_eq!(
        run(&["--store", &t2.store(), "schema", "install", &file], b"").0,
        0
    );
    let (_, ans) = run(&["--store", &t2.store(), "describe"], b"{}");
    assert_eq!(ans["result"]["sources"][0]["name"], "task_states");
    assert_eq!(ans["result"]["action_types"], json!([]));
    // No read and no action: the domain is not listed and not found.
    let t3 = Temp::new();
    schema["grants"][0]["read"] = json!([]);
    let file = t3.file("s.json", &serde_json::to_vec(&schema).unwrap());
    assert_eq!(
        run(&["--store", &t3.store(), "schema", "install", &file], b"").0,
        0
    );
    let (_, ans) = run(&["--store", &t3.store(), "describe"], b"{}");
    assert_eq!(ans["result"]["domains"], json!([]));
    assert_eq!(ans["schema"], json!({}));
    let (code, ans) = run(
        &["--store", &t3.store(), "describe"],
        br#"{"domain":"work"}"#,
    );
    assert_eq!((code, ans["code"].as_str()), (2, Some("unknown_domain")));
}

#[test]
fn only_install_creates_or_initialises_a_store() {
    let t = Temp::new();
    let file = t.file("example.json", EXAMPLE);
    let empty = t.0.join("empty.sqlite");
    std::fs::write(&empty, b"").unwrap();
    let foreign = t.0.join("foreign.sqlite");
    std::fs::write(&foreign, b"this is not a database file").unwrap();
    let stdin_for = |c: &str| -> &'static [u8] {
        if c == "get" {
            br#"{"id":{"object_type":"task","namespace":{"instance":"i","account":"a"},"native_key":"k"}}"#
        } else {
            b"{}"
        }
    };
    for store in [&empty, &foreign] {
        let s = store.display().to_string();
        for argv in [
            vec!["--store", &s, "schema", "check", &file],
            vec!["--store", &s, "describe"],
            vec!["--store", &s, "get"],
            vec!["--store", &s, "backup"],
        ] {
            let args: Vec<String> = argv.iter().map(|x| x.to_string()).collect();
            let out = ont::run(&args, stdin_for(argv[argv.len() - 1]), 0, 0);
            // A damaged or foreign store is internal: exit 1, located code.
            assert_eq!(out.code, 1, "{argv:?}");
            assert!(out.stdout.is_empty());
            assert!(
                out.stderr.starts_with("store_failure@crates/ont/src/"),
                "{}",
                out.stderr
            );
        }
    }
    assert_eq!(std::fs::metadata(&empty).unwrap().len(), 0, "still 0 bytes");
    assert_eq!(
        std::fs::read(&foreign).unwrap(),
        b"this is not a database file"
    );
    assert!(!t.0.join("backup").exists());
    // A missing store is a refusal for every other command and is not created.
    let missing = t.0.join("missing.sqlite").display().to_string();
    for argv in [
        vec!["--store", &missing, "describe"],
        vec!["--store", &missing, "get"],
        vec!["--store", &missing, "backup"],
    ] {
        let args: Vec<String> = argv.iter().map(|x| x.to_string()).collect();
        let out = ont::run(&args, stdin_for(argv[argv.len() - 1]), 0, 0);
        assert_eq!(out.code, 2, "{argv:?}");
        assert!(out.stdout.contains("store_not_found"), "{}", out.stdout);
    }
    assert!(!std::path::Path::new(&missing).exists());
    // Install may initialise an existing empty file (and keeps its path).
    let (code, ans) = run(
        &[
            "--store",
            &empty.display().to_string(),
            "schema",
            "install",
            &file,
        ],
        b"",
    );
    assert_eq!(code, 0, "{ans}");
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
    assert_eq!(ans["field"], "--store");
    assert_eq!(
        ans["rule"],
        "must be explicitly supplied when --now is used"
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
    let out = ont::run(&args, &b"{}"[..], 1000, 1000);
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
        let out = ont::run(&args, &b"{}"[..], 1000, 1000);
        let v: Value = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(
            (out.code, v["code"].as_str()),
            (2, Some("principal_not_trusted"))
        );
    }
}

#[test]
fn request_size_op_and_source_option_rules() {
    let t = Temp::new();
    assert_eq!(install_example(&t).0, 0);
    let padded = |body: &str, total: usize| -> Vec<u8> {
        let mut v = body.as_bytes().to_vec();
        v.resize(total, b' ');
        v
    };
    // Exactly 65,536 bytes is accepted; one more is refused (describe, get).
    let (code, _) = run(
        &["--store", &t.store(), "describe"],
        &padded("{}", ont::MAX_REQUEST),
    );
    assert_eq!(code, 0);
    let (code, ans) = run(
        &["--store", &t.store(), "describe"],
        &padded("{}", ont::MAX_REQUEST + 1),
    );
    assert_eq!((code, ans["code"].as_str()), (2, Some("invalid_request")));
    let body = serde_json::to_string(&json!({"id": task_id()})).unwrap();
    let (code, ans) = run(
        &["--store", &t.store(), "get"],
        &padded(&body, ont::MAX_REQUEST),
    );
    assert_eq!((code, ans["code"].as_str()), (2, Some("unavailable")));
    let (code, ans) = run(
        &["--store", &t.store(), "get"],
        &padded(&body, ont::MAX_REQUEST + 1),
    );
    assert_eq!((code, ans["code"].as_str()), (2, Some("invalid_request")));
    // `op` mismatch on get and on describe.
    let wrong = serde_json::to_vec(&json!({"op":"describe","id":task_id()})).unwrap();
    let (code, ans) = run(&["--store", &t.store(), "get"], &wrong);
    assert_eq!((code, ans["code"].as_str()), (2, Some("invalid_request")));
    let (code, ans) = run(&["--store", &t.store(), "describe"], br#"{"op":"get"}"#);
    assert_eq!((code, ans["code"].as_str()), (2, Some("invalid_request")));
    // Malformed --source-repository, and --source-* on other commands.
    let file = t.file("example.json", EXAMPLE);
    let (code, ans) = run(
        &[
            "--store",
            &t.store(),
            "--source-repository",
            "no slash",
            "schema",
            "install",
            &file,
        ],
        b"",
    );
    assert_eq!((code, ans["code"].as_str()), (2, Some("invalid_argument")));
    for argv in [
        vec!["--store", &t.store(), "--source-commit", "a", "describe"],
        vec!["--store", &t.store(), "--source-repository", "o/r", "get"],
        vec!["--store", &t.store(), "--source-commit", "a", "backup"],
        vec![
            "--store",
            &t.store(),
            "--source-commit",
            "a",
            "schema",
            "check",
            &file,
        ],
    ] {
        let (code, ans) = run(&argv, b"{}");
        assert_eq!(
            (code, ans["code"].as_str()),
            (2, Some("invalid_argument")),
            "{argv:?}"
        );
    }
    // Well-formed source options are recorded.
    let mut v2: Value = serde_json::from_slice(EXAMPLE).unwrap();
    v2["version"] = json!(2);
    let file2 = t.file("v2.json", &serde_json::to_vec(&v2).unwrap());
    let (code, ans) = run(
        &[
            "--store",
            &t.store(),
            "--source-repository",
            "park-kyungchan/x",
            "--source-commit",
            &"a".repeat(40),
            "schema",
            "install",
            &file2,
        ],
        b"",
    );
    assert_eq!(code, 0, "{ans}");
}

fn git(repo: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("/usr/bin/git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn setup_git(t: &Temp) -> (String, String) {
    let repo = t.0.join("repo");
    std::fs::create_dir_all(repo.join("ontology")).unwrap();
    std::fs::write(repo.join("ontology/work.schema.json"), EXAMPLE).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.email", "test@example.invalid"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "user:pa/ss@example.com:park/repo.git",
        ],
    );
    git(&repo, &["add", "ontology/work.schema.json"]);
    git(&repo, &["commit", "-q", "-m", "one"]);
    let commit = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    (repo.display().to_string(), commit)
}

fn commit_install_args<'a>(
    store: &'a str,
    repo: &'a str,
    commit: &'a str,
    path: &'a str,
) -> Vec<&'a str> {
    vec![
        "--store", store, "schema", "install", "--from", repo, "--commit", commit, "--path", path,
    ]
}

#[test]
fn commit_install_installs_and_describe_shows_provenance() {
    let t = Temp::new();
    let (repo, commit) = setup_git(&t);
    let store = t.store();
    let (code, ans) = run(
        &commit_install_args(&store, &repo, &commit, "ontology/work.schema.json"),
        b"",
    );
    assert_eq!(code, 0, "{ans}");
    assert_eq!(
        ans["result"]["source_repository"],
        "example.com:park/repo.git"
    );
    assert_eq!(ans["result"]["source_commit"], commit);
    assert_eq!(ans["result"]["source_path"], "ontology/work.schema.json");
    // Same bytes installed twice by commit are idempotent.
    let (code, ans) = run(
        &commit_install_args(&store, &repo, &commit, "ontology/work.schema.json"),
        b"",
    );
    assert_eq!(code, 0, "{ans}");
    assert_eq!(ans["result"]["changed"], false);
    let (code, ans) = run(&["--store", &store, "describe"], b"{}");
    assert_eq!(code, 0, "{ans}");
    let domain = &ans["result"]["domains"][0];
    assert_eq!(domain["domain"], "work");
    assert_eq!(domain["version"], 1);
    assert_eq!(domain["sha256"], sha256_hex(EXAMPLE));
    assert_eq!(domain["repository"], "example.com:park/repo.git");
    assert_eq!(domain["commit"], commit);
    assert_eq!(domain["path"], "ontology/work.schema.json");
}

#[test]
fn commit_install_refusals_write_nothing() {
    let t = Temp::new();
    let (repo, commit) = setup_git(&t);
    let store = t.store();
    let store_path = std::path::Path::new(&store);
    // An unmerged commit: HEAD moves past the pinned origin/main.
    let mut v2: Value = serde_json::from_slice(EXAMPLE).unwrap();
    v2["version"] = json!(2);
    std::fs::write(
        t.0.join("repo/ontology/work.schema.json"),
        serde_json::to_vec(&v2).unwrap(),
    )
    .unwrap();
    let repo_path = std::path::Path::new(&repo);
    git(repo_path, &["add", "ontology/work.schema.json"]);
    git(repo_path, &["commit", "-q", "-m", "two"]);
    let unmerged = git(repo_path, &["rev-parse", "HEAD"]);
    for (commit_arg, path, expected) in [
        (
            unmerged.as_str(),
            "ontology/work.schema.json",
            "commit_not_merged",
        ),
        ("not-hex", "ontology/work.schema.json", "invalid_request"),
        (
            commit.to_uppercase().as_str(),
            "ontology/work.schema.json",
            "invalid_request",
        ),
        (
            commit.as_str(),
            "ontology/nope.json",
            "schema_path_not_found",
        ),
        (commit.as_str(), "../work.schema.json", "invalid_request"),
        (
            commit.as_str(),
            "ontology/../work.schema.json",
            "invalid_request",
        ),
        (
            commit.as_str(),
            "/ontology/work.schema.json",
            "invalid_request",
        ),
    ] {
        let (code, ans) = run(&commit_install_args(&store, &repo, commit_arg, path), b"");
        assert_eq!(
            (code, ans["code"].as_str()),
            (2, Some(expected)),
            "{commit_arg} {path}: {ans}"
        );
        assert!(!store_path.exists(), "refusal wrote a store: {path}");
    }
}

#[test]
fn file_install_on_default_store_path_is_refused_by_the_decision() {
    assert_eq!(
        ont::DEFAULT_STORE,
        "/var/lib/ontology-platform/ont/ontology.sqlite"
    );
    assert!(ont::schema::install_requires_commit(std::path::Path::new(
        ont::DEFAULT_STORE
    )));
    assert!(!ont::schema::install_requires_commit(std::path::Path::new(
        "/tmp/example/ontology.sqlite"
    )));
}

#[test]
fn schema_check_against_an_existing_store_reports_what_it_checked() {
    let t = Temp::new();
    assert_eq!(install_example(&t).0, 0);
    let file = t.file("example.json", EXAMPLE);
    let (code, ans) = run(&["--store", &t.store(), "schema", "check", &file], b"");
    assert_eq!(code, 0, "{ans}");
    assert_eq!(ans["store_exists"], true);
    assert_eq!(ans["checked_against"], json!(["work"]));
    // A check never predicts a version conflict: a lower version is clean.
    let mut v0: Value = serde_json::from_slice(EXAMPLE).unwrap();
    v0["version"] = json!(1);
    v0["scope_keys"] = json!([]);
    let before = std::fs::read(t.store()).unwrap();
    let (code, _) = run(&["--store", &t.store(), "schema", "check", &file], b"");
    assert_eq!(code, 0);
    assert_eq!(
        std::fs::read(t.store()).unwrap(),
        before,
        "check wrote nothing"
    );
}

#[test]
fn existing_store_commit_refusals_leave_no_backup_or_rows() {
    let t = Temp::new();
    assert_eq!(install_example(&t).0, 0);
    let (repo, first) = setup_git(&t);
    let store = t.store();
    let before = std::fs::read(&store).unwrap();
    let path = std::path::Path::new(&repo);
    let mut changed = EXAMPLE.to_vec();
    changed.push(b' ');
    std::fs::write(path.join("ontology/work.schema.json"), changed).unwrap();
    git(path, &["add", "."]);
    git(path, &["commit", "-qm", "same version different bytes"]);
    let commit = git(path, &["rev-parse", "HEAD"]);
    for (revision, expected) in [
        (&commit, "commit_not_merged"),
        (&commit, "schema_version_conflict"),
    ] {
        if expected == "schema_version_conflict" {
            git(path, &["update-ref", "refs/remotes/origin/main", &commit]);
        }
        let (code, ans) = run(
            &commit_install_args(&store, &repo, revision, "ontology/work.schema.json"),
            b"",
        );
        assert_eq!((code, ans["code"].as_str()), (2, Some(expected)), "{ans}");
        assert_eq!(
            std::fs::read(&store).unwrap(),
            before,
            "refusal changed rows"
        );
        assert!(!t.0.join("backup").exists());
    }
    let file = t.file("conflict.json", &[EXAMPLE, b" "].concat());
    let (code, ans) = run(&["--store", &store, "schema", "install", &file], b"");
    assert_eq!(
        (code, ans["code"].as_str()),
        (2, Some("schema_version_conflict"))
    );
    assert_eq!(std::fs::read(&store).unwrap(), before);
    assert!(!t.0.join("backup").exists());
    // Invalid checkout/missing remote ref is a Git execution failure, not ancestry status 1.
    let (code, ans) = run(
        &commit_install_args(&store, t.0.to_str().unwrap(), &first, "x"),
        b"",
    );
    assert_eq!((code, ans["code"].as_str()), (2, Some("git_unavailable")));
    assert_eq!(std::fs::read(&store).unwrap(), before);
    assert!(!t.0.join("backup").exists());
}

#[test]
fn commit_oversize_refuses_without_a_store() {
    let t = Temp::new();
    let (repo, _) = setup_git(&t);
    let path = std::path::Path::new(&repo);
    std::fs::write(
        path.join("large.json"),
        vec![b' '; ont::gitread::MAX_SCHEMA_BYTES + 1],
    )
    .unwrap();
    git(path, &["add", "."]);
    git(path, &["commit", "-qm", "oversize"]);
    let commit = git(path, &["rev-parse", "HEAD"]);
    git(path, &["update-ref", "refs/remotes/origin/main", &commit]);
    let (code, ans) = run(
        &commit_install_args(&t.store(), &repo, &commit, "large.json"),
        b"",
    );
    assert_eq!((code, ans["code"].as_str()), (2, Some("invalid_request")));
    assert!(!std::path::Path::new(&t.store()).exists());
}
