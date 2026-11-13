//! Compaction controls through the command and trusted store boundaries.
#![allow(clippy::unwrap_used)]
use ont_store::Store;
use serde_json::{Value, json};

struct Temp(std::path::PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ont-compact-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> std::path::PathBuf {
        self.0.join("store.sqlite")
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn old_store(t: &Temp) -> Store {
    let sql = rusqlite::Connection::open(t.path()).unwrap();
    sql.execute_batch("PRAGMA page_size=4096").unwrap();
    sql.execute_batch(include_str!("../../ont-store/schema-v3.sql"))
        .unwrap();
    drop(sql);
    let mut store = Store::open_schema_existing_writable(&t.path()).unwrap();
    assert_eq!(store.page_size().unwrap(), 4096);
    let task = t.0.join("tasks/one");
    std::fs::create_dir_all(&task).unwrap();
    std::fs::write(task.join("STATE.md"), "# Synthetic task\nOwner: worker\n").unwrap();
    let mut schema: Value =
        serde_json::from_slice(include_bytes!("fixtures/action-work.json")).unwrap();
    schema["sources"]["task_states"]["glob"] =
        json!(format!("{}/*/STATE.md", t.0.join("tasks").display()));
    store
        .install_schema(&serde_json::to_vec(&schema).unwrap(), Default::default())
        .unwrap();
    store
}
fn run(t: &Temp, command: &[&str], input: Value) -> (i32, Value) {
    let mut args = vec![
        "--store".into(),
        t.path().display().to_string(),
        "--now".into(),
        "1000".into(),
    ];
    args.extend(command.iter().map(|v| v.to_string()));
    let out = ont::run(&args, serde_json::to_vec(&input).unwrap().as_slice(), 0, 0);
    assert!(out.stderr.is_empty(), "{}", out.stderr);
    (out.code, serde_json::from_str(&out.stdout).unwrap())
}
fn ok(t: &Temp, command: &[&str], input: Value) -> Value {
    let (code, answer) = run(t, command, input);
    assert_eq!(code, 0, "{answer}");
    answer
}
fn create_tag(t: &Temp, label: &str) -> Value {
    ok(
        t,
        &["action", "apply"],
        json!({"action_type":"create_tag","agent":"worker","parameters":{"label":label}}),
    )
}
fn backups(t: &Temp) -> Vec<std::path::PathBuf> {
    let mut paths = std::fs::read_dir(t.0.join("backup"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect::<Vec<_>>();
    paths.sort();
    paths
}
#[test]
fn compact_keeps_every_row() {
    use ont_store::schema_mode::CandidateCondition;
    use sha2::{Digest, Sha256};
    use std::collections::{BTreeMap, BTreeSet};
    let t = Temp::new();
    drop(old_store(&t));
    ok(&t, &["index"], json!({}));
    let tag = create_tag(&t, "ready");
    assert_eq!(tag["result"]["outcome"], "applied");
    let sql = rusqlite::Connection::open(t.path()).unwrap();
    // Reconstruct the postings of an older writer for the never-stale tag.
    let expiry = format!("{:016x}", ont_store::NEVER_STALE);
    for n in 1..=16 {
        sql.execute("INSERT INTO head_candidates SELECT object_type,visibility,'$stale',?1,instance,account,native_key,scope_keys FROM head_candidates WHERE object_type='tag' AND property=''", [&expiry[..n]]).unwrap();
    }
    let stale = || {
        sql.query_row(
            "SELECT count(*) FROM head_candidates WHERE property='$stale' AND object_type='tag'",
            [],
            |r| r.get::<_, u64>(0),
        )
        .unwrap()
    };
    assert_eq!(stale(), 16);
    let definitions = || -> Vec<(String, Option<String>)> {
        sql.prepare("SELECT name,sql FROM sqlite_master ORDER BY name")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let markers = || -> (u32, u32) {
        (
            sql.query_row("PRAGMA user_version", [], |r| r.get(0))
                .unwrap(),
            sql.query_row("PRAGMA application_id", [], |r| r.get(0))
                .unwrap(),
        )
    };
    let before_definitions = definitions();
    let before_markers = markers();
    let mut store = Store::open_schema_existing(&t.path()).unwrap();
    let checksum = ont::compact::checksum(&mut store).unwrap();
    let condition = CandidateCondition {
        property: "label".into(),
        values: vec![json!("ready")],
        used: BTreeSet::new(),
    };
    let scope = ont_core::Scope::domain(
        ont_core::Namespace {
            instance: "vps://palantirkc".into(),
            account: "root".into(),
        },
        "work",
        BTreeMap::new(),
    )
    .unwrap();
    let candidates = store
        .candidate_access(
            "tag",
            Some(&condition),
            1000,
            100,
            None,
            std::slice::from_ref(&scope),
        )
        .unwrap();
    assert_eq!(candidates.len(), 1);
    drop(store);
    let answer = ok(&t, &["compact"], json!({}));
    let result = &answer["result"];
    assert_eq!(result["compacted"], true);
    assert_eq!(result["page_size_before"], 4096);
    assert_eq!(result["page_size"], 16384);
    assert_eq!(definitions(), before_definitions);
    assert_eq!(markers(), before_markers);
    assert_eq!(
        stale(),
        0,
        "never-stale tag has no dead expiry postings after rebuild"
    );
    let mut store = Store::open_schema_existing(&t.path()).unwrap();
    assert_eq!(store.page_size().unwrap(), 16384);
    assert_eq!(ont::compact::checksum(&mut store).unwrap(), checksum);
    assert_eq!(
        store
            .candidate_access("tag", Some(&condition), 1000, 100, None, &[scope])
            .unwrap(),
        candidates
    );
    let backup = std::path::Path::new(result["backup"].as_str().unwrap());
    let bytes = std::fs::read(backup).unwrap();
    assert_eq!(
        result["backup_sha256"],
        format!("{:x}", Sha256::digest(bytes))
    );
    let mut copy = Store::open_schema_existing(backup).unwrap();
    assert_eq!(copy.page_size().unwrap(), 4096);
    assert_eq!(ont::compact::checksum(&mut copy).unwrap(), checksum);
    let search = ok(
        &t,
        &["search"],
        json!({"object_type":"tag","where":{"eq":["label","ready"]}}),
    );
    assert_eq!(search["result"]["objects"].as_array().unwrap().len(), 1);
    assert_eq!(create_tag(&t, "after")["result"]["outcome"], "applied");
    let backups_before = backups(&t);
    let file_before = std::fs::read(t.path()).unwrap();
    let answer = ok(&t, &["compact"], json!({}));
    assert_eq!(
        answer["result"],
        json!({"compacted":false,"page_size":16384,"bytes":file_before.len()})
    );
    assert_eq!(backups(&t), backups_before);
    assert_eq!(std::fs::read(t.path()).unwrap(), file_before);
}
#[test]
fn compact_refuses_while_busy() {
    let t = Temp::new();
    let store = old_store(&t);
    let before = std::fs::read(t.path()).unwrap();
    let sql = rusqlite::Connection::open(t.path()).unwrap();
    sql.execute_batch("BEGIN IMMEDIATE").unwrap();
    drop(store);
    let (code, answer) = run(&t, &["compact"], json!({}));
    assert_eq!(code, 2, "{answer}");
    assert_eq!(answer["code"], "store_busy");
    assert_eq!(std::fs::read(t.path()).unwrap(), before);
    sql.execute_batch("ROLLBACK").unwrap();
    assert!(Store::open_schema_existing(&t.path()).is_ok());
}
#[test]
fn search_clock_above_never_stale_is_refused() {
    let t = Temp::new();
    drop(old_store(&t));
    let args = [
        "--store".to_string(),
        t.path().display().to_string(),
        "--now".into(),
        "253402300800".into(),
        "search".into(),
    ];
    let out = ont::run(
        &args,
        serde_json::to_vec(&json!({"object_type":"task"}))
            .unwrap()
            .as_slice(),
        0,
        0,
    );
    let answer: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(out.code, 2, "{answer}");
    assert_eq!(answer["code"], "clock_out_of_range");
}
