//! Independent controls for G2b-1's store boundary; command work requires a SHA-256 dependency grant.
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
    store
        .install_schema(
            include_bytes!("fixtures/action-work.json"),
            Default::default(),
        )
        .unwrap();
    store
}
#[test]
fn store_compact_changes_pages_and_keeps_schema() {
    let t = Temp::new();
    let mut store = old_store(&t);
    let sql = rusqlite::Connection::open(t.path()).unwrap();
    let definitions = || -> Vec<(String, Option<String>)> {
        sql.prepare("SELECT name,sql FROM sqlite_master ORDER BY name")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let before = definitions();
    let pins = store.schema_pins().unwrap();
    store.compact().unwrap();
    assert_eq!(store.page_size().unwrap(), 16384);
    assert_eq!(definitions(), before);
    assert_eq!(store.schema_pins().unwrap(), pins);
    assert!(Store::open_schema_existing(&t.path()).is_ok());
}
#[test]
fn compact_refuses_while_busy() {
    let t = Temp::new();
    let mut store = old_store(&t);
    let before = std::fs::read(t.path()).unwrap();
    let sql = rusqlite::Connection::open(t.path()).unwrap();
    sql.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert_eq!(store.compact().unwrap_err().code, "store_busy");
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
