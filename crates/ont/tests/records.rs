//! Synthetic tests of the source kind `records`: every line of the files
//! under a glob is one record that carries its own time. Temporary
//! directories and stores only; every value is invented.
#![allow(clippy::unwrap_used)]
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const FRESH: u64 = 100;

struct Env(PathBuf);
impl Env {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "ont-rec-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("data/side")).unwrap();
        Self(dir)
    }
    fn setup(complete: bool) -> Self {
        let e = Self::new();
        e.install(&e.schema(1, complete));
        e
    }
    fn store(&self) -> String {
        self.0.join("store.sqlite").display().to_string()
    }
    fn data(&self, name: &str) -> PathBuf {
        self.0.join("data").join(name)
    }
    /// `lines` joined by newlines, with a final newline.
    fn put(&self, name: &str, lines: &[String]) {
        let mut text = lines.join("\n");
        text.push('\n');
        std::fs::write(self.data(name), text).unwrap();
    }
    fn put_raw(&self, name: &str, bytes: &[u8]) {
        std::fs::write(self.data(name), bytes).unwrap();
    }
    fn remove(&self, name: &str) {
        std::fs::remove_file(self.data(name)).unwrap();
    }
    fn schema(&self, version: u64, complete: bool) -> Value {
        let dir = self.0.join("data");
        json!({
            "format":"ontology-schema/v1","domain":"acad","version":version,
            "namespace":{"instance":"vps://test","account":"root"},
            "scope_keys":[],"value_types":{},
            "sources":{
                "rec":{"kind":"records","glob":format!("{}/*.jsonl", dir.display()),
                    "time_member":"observed_at","type_member":"type",
                    "complete_listing":complete,"fresh_for_s":FRESH},
                "rec2":{"kind":"records","glob":format!("{}/side/*.jsonl", dir.display()),
                    "time_member":"observed_at","complete_listing":true,"fresh_for_s":FRESH}
            },
            "object_types":{
                "attempt":{"owner":{"source":"rec"},"primary_key":{"from":"/key"},"title":"label",
                    "properties":{
                        "label":{"type":"string","required":false,"from":"/label"},
                        "score":{"type":"integer","required":false,"from":"/score"}}},
                "side":{"owner":{"source":"rec2"},"primary_key":{"from":"/key"},
                    "properties":{"label":{"type":"string","required":false,"from":"/label"}}}
            },
            "views":{"attempts":{"object_type":"attempt","limit":1000,
                "order":[{"property":"$key","direction":"asc"}],
                "columns":[{"header":"Key","property":"$key"},
                    {"header":"Label","property":"label"},
                    {"header":"Seen","property":"$observed_at"}]}},
            "link_types":{},"action_types":{},"functions":{},
            "grants":[{"principal":"agent","scope":{},"read":["attempt","side"],"derive":[],"actions":[]}]
        })
    }
    fn file(&self, name: &str, bytes: &[u8]) -> String {
        let p = self.0.join(name);
        std::fs::write(&p, bytes).unwrap();
        p.display().to_string()
    }
    fn install(&self, schema: &Value) {
        let file = self.file("schema.json", &serde_json::to_vec(schema).unwrap());
        let (code, ans) = self.run(0, &["schema", "install", &file], b"");
        assert_eq!(code, 0, "{ans}");
    }
    fn run(&self, now: u64, args: &[&str], stdin: &[u8]) -> (i32, Value) {
        let mut all: Vec<String> = vec![
            "--store".into(),
            self.store(),
            "--now".into(),
            now.to_string(),
        ];
        all.extend(args.iter().map(|s| s.to_string()));
        let out = ont::run(&all, stdin, 0, 0);
        assert_eq!(out.stderr, "", "stderr: {}", out.stderr);
        (out.code, serde_json::from_str(out.stdout.trim()).unwrap())
    }
    fn index(&self, now: u64) -> Value {
        self.run(now, &["index"], b"").1
    }
    fn count(&self) -> i64 {
        let c = rusqlite::Connection::open_with_flags(
            self.store(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        c.query_row("SELECT count(*) FROM observations", [], |r| r.get(0))
            .unwrap()
    }
    fn get(&self, now: u64, ty: &str, key: &str) -> (i32, Value) {
        let req = json!({"id":{"object_type":ty,
            "namespace":{"instance":"vps://test","account":"root"},"native_key":key}});
        self.run(now, &["get"], &serde_json::to_vec(&req).unwrap())
    }
    /// The head observation of an attempt and the length of its history.
    fn head(&self, key: &str) -> (Value, usize) {
        let (code, ans) = self.get(5000, "attempt", key);
        assert_eq!(code, 0, "{ans}");
        let object = &ans["result"]["objects"][0];
        let all = object["observations"].as_array().unwrap();
        let head = all
            .iter()
            .find(|o| o["revision"] == object["head"])
            .unwrap()
            .clone();
        (head, all.len())
    }
    fn times(&self, key: &str) -> (u64, u64) {
        let (h, _) = self.head(key);
        (
            h["observed_at"].as_u64().unwrap(),
            h["fresh_until"].as_u64().unwrap(),
        )
    }
    fn search(&self, now: u64, req: &Value) -> (i32, Value) {
        self.run(now, &["search"], &serde_json::to_vec(req).unwrap())
    }
}
impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn rec(key: &str, label: &str, t: u64) -> String {
    json!({"type":"attempt","key":key,"observed_at":t,"label":label}).to_string()
}
fn source<'a>(ans: &'a Value, name: &str) -> &'a Value {
    ans["result"]["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["source"] == name)
        .unwrap()
}
fn appended(entry: &Value) -> [u64; 4] {
    ["initial", "update", "refresh", "retraction"].map(|k| entry["appended"][k].as_u64().unwrap())
}
fn cause(ans: &Value) -> String {
    source(ans, "rec")["cause"].as_str().unwrap().to_string()
}

// ---- A1: one test per branch of the time rule (I3) ----

#[test]
fn rule_1_a_time_later_than_now_fails_the_source() {
    let e = Env::setup(true);
    e.put("a.jsonl", &[rec("k1", "x", 1001)]);
    let ans = e.index(1000);
    assert_eq!(source(&ans, "rec")["status"], "source_failed");
    let path = e.data("a.jsonl").display().to_string();
    assert_eq!(cause(&ans), format!("record_time_in_future@{path}:1"));
    assert_eq!(e.count(), 0);
    // The time of the run itself is not later than now.
    e.put("a.jsonl", &[rec("k1", "x", 1000)]);
    assert_eq!(source(&e.index(1000), "rec")["status"], "ok");
}

#[test]
fn rule_2_no_head_is_an_initial_observation_at_the_record_time() {
    let e = Env::setup(true);
    e.put("a.jsonl", &[rec("k1", "x", 900)]);
    let ans = e.index(1000);
    assert_eq!(appended(source(&ans, "rec")), [1, 0, 0, 0]);
    let (head, len) = e.head("k1");
    assert_eq!(len, 1);
    assert_eq!(head["change"], "initial");
    assert_eq!(e.times("k1"), (900, 1000 + FRESH));
}

#[test]
fn rule_2_a_retracted_head_is_initial_again_whatever_the_retraction_time() {
    let e = Env::setup(true);
    e.put("a.jsonl", &[rec("k1", "x", 900)]);
    e.index(1000);
    e.remove("a.jsonl");
    let ans = e.index(1010);
    assert_eq!(appended(source(&ans, "rec")), [0, 0, 0, 1]);
    // T (950) is earlier than the retraction (1010): still an initial one.
    e.put("a.jsonl", &[rec("k1", "x", 950)]);
    let ans = e.index(1020);
    assert_eq!(source(&ans, "rec")["status"], "ok", "{ans}");
    assert_eq!(appended(source(&ans, "rec")), [1, 0, 0, 0]);
    let (head, len) = e.head("k1");
    assert_eq!((head["change"].as_str().unwrap(), len), ("initial", 3));
    assert_eq!(e.times("k1"), (950, 1020 + FRESH));
}

#[test]
fn rule_3_different_fields_update_at_the_record_time() {
    let e = Env::setup(true);
    e.put("a.jsonl", &[rec("k1", "x", 900)]);
    e.index(1000);
    e.put("a.jsonl", &[rec("k1", "y", 950)]);
    let ans = e.index(1010);
    assert_eq!(appended(source(&ans, "rec")), [0, 1, 0, 0]);
    assert_eq!(e.head("k1").0["change"], "update");
    assert_eq!(e.times("k1"), (950, 1010 + FRESH));
    // T equal to the head's time is still an update.
    e.put("a.jsonl", &[rec("k1", "z", 950)]);
    let ans = e.index(1020);
    assert_eq!(appended(source(&ans, "rec")), [0, 1, 0, 0]);
    assert_eq!(e.times("k1"), (950, 1020 + FRESH));
    assert_eq!(e.head("k1").1, 3);
}

#[test]
fn rule_3_older_content_over_newer_fails_the_source_and_appends_nothing() {
    let e = Env::setup(true);
    e.put("a.jsonl", &[rec("k1", "x", 900), rec("k2", "x", 900)]);
    e.index(1000);
    let before = e.count();
    // k2 is fine (a later time) but k1 is older than its head: all or nothing.
    e.put("a.jsonl", &[rec("k1", "y", 899), rec("k2", "y", 950)]);
    let ans = e.index(1010);
    let path = e.data("a.jsonl").display().to_string();
    assert_eq!(cause(&ans), format!("record_older_than_head@{path}:1"));
    assert_eq!(e.count(), before);
    assert_eq!(e.times("k2"), (900, 1000 + FRESH));
}

#[test]
fn rule_4_a_changed_digest_with_equal_fields_refreshes_with_the_later_time() {
    let e = Env::setup(true);
    let line = |t: u64, note: &str| {
        json!({"type":"attempt","key":"k1","observed_at":t,"label":"x","note":note}).to_string()
    };
    e.put("a.jsonl", &[line(900, "a")]);
    e.index(1000);
    // An unmapped member changes: the fields are equal, the digest differs.
    // T (800) is earlier than the head's time: the time stays at 900.
    e.put("a.jsonl", &[line(800, "b")]);
    let ans = e.index(1010);
    assert_eq!(appended(source(&ans, "rec")), [0, 0, 1, 0]);
    assert_eq!(e.head("k1").0["change"], "refresh");
    assert_eq!(e.times("k1"), (900, 1010 + FRESH));
    e.put("a.jsonl", &[line(960, "c")]);
    let ans = e.index(1020);
    assert_eq!(appended(source(&ans, "rec")), [0, 0, 1, 0]);
    assert_eq!(e.times("k1"), (960, 1020 + FRESH));
}

#[test]
fn rule_4_a_raised_schema_version_refreshes_with_the_later_time() {
    let e = Env::setup(true);
    e.put("a.jsonl", &[rec("k1", "x", 900)]);
    e.index(1000);
    e.install(&e.schema(2, true));
    let ans = e.index(1010);
    assert_eq!(appended(source(&ans, "rec")), [0, 0, 1, 0]);
    assert_eq!(e.times("k1"), (900, 1010 + FRESH));
    e.install(&e.schema(3, true));
    e.put("a.jsonl", &[rec("k1", "x", 960)]);
    let ans = e.index(1020);
    assert_eq!(appended(source(&ans, "rec")), [0, 0, 1, 0]);
    assert_eq!(e.times("k1"), (960, 1020 + FRESH));
}

#[test]
fn rule_5_a_later_time_renews_in_place() {
    let e = Env::setup(true);
    e.put("a.jsonl", &[rec("k1", "x", 900)]);
    e.index(1000);
    e.put("a.jsonl", &[rec("k1", "x", 950)]);
    let ans = e.index(1010);
    let entry = source(&ans, "rec");
    assert_eq!(
        (appended(entry), entry["renewed"].as_u64()),
        ([0; 4], Some(1))
    );
    assert_eq!(e.times("k1"), (950, 1010 + FRESH));
    assert_eq!(e.head("k1").1, 1);
}

#[test]
fn rule_5_staleness_renews_in_place_and_keeps_the_time() {
    let e = Env::setup(true);
    e.put("a.jsonl", &[rec("k1", "x", 900)]);
    e.index(1000);
    let ans = e.index(1000 + FRESH + 1);
    let entry = source(&ans, "rec");
    assert_eq!(
        (appended(entry), entry["renewed"].as_u64()),
        ([0; 4], Some(1))
    );
    assert_eq!(e.times("k1"), (900, 1000 + FRESH + 1 + FRESH));
    assert_eq!(e.head("k1").1, 1);
}

#[test]
fn rule_5_equal_and_fresh_changes_nothing() {
    let e = Env::setup(true);
    e.put("a.jsonl", &[rec("k1", "x", 900)]);
    e.index(1000);
    let ans = e.index(1050);
    let entry = source(&ans, "rec");
    assert_eq!(appended(entry), [0; 4]);
    assert_eq!(
        (entry["renewed"].as_u64(), entry["unchanged"].as_u64()),
        (Some(0), Some(1))
    );
    assert_eq!(e.times("k1"), (900, 1000 + FRESH));
    // An earlier time with equal fields changes nothing either.
    e.put("a.jsonl", &[rec("k1", "x", 800)]);
    let ans = e.index(1060);
    assert_eq!(source(&ans, "rec")["unchanged"], 1);
    assert_eq!(e.times("k1"), (900, 1000 + FRESH));
}

#[test]
fn removing_a_file_retracts_and_putting_it_back_is_initial_again() {
    let e = Env::setup(true);
    e.put("a.jsonl", &[rec("k1", "x", 900)]);
    e.put("b.jsonl", &[rec("k2", "y", 910)]);
    let ans = e.index(1000);
    assert_eq!(appended(source(&ans, "rec")), [2, 0, 0, 0]);
    let saved = std::fs::read(e.data("b.jsonl")).unwrap();
    e.remove("b.jsonl");
    let ans = e.index(1010);
    assert_eq!(appended(source(&ans, "rec")), [0, 0, 0, 1]);
    e.put_raw("b.jsonl", &saved);
    let ans = e.index(1020);
    assert_eq!(source(&ans, "rec")["status"], "ok", "{ans}");
    assert_eq!(appended(source(&ans, "rec")), [1, 0, 0, 0]);
    let (head, _) = e.head("k2");
    assert_eq!(head["change"], "initial");
    assert_eq!(e.times("k2"), (910, 1020 + FRESH));
    assert_eq!(e.times("k1"), (900, 1000 + FRESH));
}

// ---- A2: the digest (I4) ----

#[test]
fn member_order_other_files_and_later_times_append_nothing() {
    let e = Env::setup(false);
    e.put(
        "a.jsonl",
        &[r#"{"type":"attempt","key":"k1","observed_at":900,"label":"x","score":3,"extra":{"b":1,"a":[1,{"d":4,"c":3}]}}"#.into()],
    );
    e.index(1000);
    let before = e.count();
    e.remove("a.jsonl");
    e.put(
        "z.jsonl",
        &[r#"{"extra":{"a":[1,{"c":3,"d":4}],"b":1},"score":3,"label":"x","observed_at":950,"key":"k1","type":"attempt"}"#.into()],
    );
    let ans = e.index(1010);
    let entry = source(&ans, "rec");
    assert_eq!(entry["status"], "ok");
    assert_eq!(appended(entry), [0; 4]);
    assert_eq!(e.count(), before);
    assert_eq!(e.times("k1"), (950, 1010 + FRESH));
}

#[test]
fn a_repeated_member_or_an_integer_beyond_64_bits_fails_the_source() {
    let e = Env::setup(true);
    e.put("good.jsonl", &[rec("k0", "x", 900)]);
    e.index(1000);
    let before = e.count();
    let cases: [(&str, &str); 6] = [
        (
            r#"{"type":"attempt","key":"k1","observed_at":900,"t":1,"t":5}"#,
            "record_duplicate_member",
        ),
        (
            r#"{"type":"attempt","key":"k1","observed_at":900,"ab":1,"ab":2}"#,
            "record_duplicate_member",
        ),
        (
            r#"{"type":"attempt","key":"k1","observed_at":900,"n":{"x":[{"q":1,"q":2}]}}"#,
            "record_duplicate_member",
        ),
        (
            r#"{"type":"attempt","key":"k1","observed_at":900,"score":18446744073709551616}"#,
            "record_number_range",
        ),
        (
            r#"{"type":"attempt","key":"k1","observed_at":900,"score":-9223372036854775809}"#,
            "record_number_range",
        ),
        (
            r#"{"type":"attempt","key":"k1","observed_at":900,"score":[0,123456789012345678901234567890]}"#,
            "record_number_range",
        ),
    ];
    for (line, code) in cases {
        e.put("bad.jsonl", &[line.to_string()]);
        let ans = e.index(1010);
        let path = e.data("bad.jsonl").display().to_string();
        assert_eq!(cause(&ans), format!("{code}@{path}:1"), "{line}");
        assert_eq!(e.count(), before, "{line}");
    }
    // The edges of the 64-bit range and a float are accepted.
    e.put(
        "bad.jsonl",
        &[r#"{"type":"attempt","key":"k1","observed_at":900,"score":18446744073709551615,"f":1.5,"m":-9223372036854775808}"#.into()],
    );
    assert_eq!(source(&e.index(1020), "rec")["status"], "ok");
}

// ---- A3: failures (I5) ----

#[test]
fn every_failure_names_file_and_line_and_leaves_the_observation_count() {
    let e = Env::setup(true);
    e.put("a-good.jsonl", &[rec("k0", "x", 900)]);
    e.index(1000);
    let before = e.count();
    let good = rec("k1", "x", 900);
    let cases: Vec<(Vec<u8>, &str, usize)> = vec![
        (b"[1]\n".to_vec(), "record_not_json_object", 1),
        (b"\"text\"\n".to_vec(), "record_not_json_object", 1),
        (
            format!("{good}\n{{not json\n").into_bytes(),
            "record_not_json_object",
            2,
        ),
        (
            format!("{good}\n{good} trailing\n").into_bytes(),
            "record_not_json_object",
            2,
        ),
        (format!("{good}\n\n").into_bytes(), "record_line_blank", 2),
        (
            format!("{good}\n   \n{good}\n").into_bytes(),
            "record_line_blank",
            2,
        ),
        (
            br#"{"type":"attempt","key":"k1","label":"x"}"#.to_vec(),
            "record_time_missing",
            1,
        ),
        (
            br#"{"type":"attempt","key":"k1","observed_at":1.5}"#.to_vec(),
            "record_time_invalid",
            1,
        ),
        (
            br#"{"type":"attempt","key":"k1","observed_at":900.0}"#.to_vec(),
            "record_time_invalid",
            1,
        ),
        (
            br#"{"type":"attempt","key":"k1","observed_at":"900"}"#.to_vec(),
            "record_time_invalid",
            1,
        ),
        (
            br#"{"type":"attempt","key":"k1","observed_at":-1}"#.to_vec(),
            "record_time_invalid",
            1,
        ),
        (
            br#"{"type":"attempt","observed_at":900}"#.to_vec(),
            "key_unresolved",
            1,
        ),
        (
            br#"{"type":"attempt","key":7,"observed_at":900}"#.to_vec(),
            "key_unresolved",
            1,
        ),
        (format!("{good}\n{good}\n").into_bytes(), "duplicate_key", 2),
        (
            br#"{"type":"other","key":"k1","observed_at":900}"#.to_vec(),
            "record_type_mismatch",
            1,
        ),
        (
            br#"{"key":"k1","observed_at":900}"#.to_vec(),
            "record_type_mismatch",
            1,
        ),
        (
            br#"{"type":7,"key":"k1","observed_at":900}"#.to_vec(),
            "record_type_mismatch",
            1,
        ),
    ];
    for (bytes, code, line) in cases {
        e.put_raw("bad.jsonl", &bytes);
        let ans = e.index(1010);
        let path = e.data("bad.jsonl").display().to_string();
        assert_eq!(
            cause(&ans),
            format!("{code}@{path}:{line}"),
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        assert_eq!(e.count(), before, "{}", String::from_utf8_lossy(&bytes));
    }
    // A key that occurs in two files names the second file.
    e.put("bad.jsonl", &[rec("k0", "other", 900)]);
    let ans = e.index(1010);
    let path = e.data("bad.jsonl").display().to_string();
    assert_eq!(cause(&ans), format!("duplicate_key@{path}:1"));
    assert_eq!(e.count(), before);
}

#[test]
fn a_failed_source_does_not_stop_the_other_sources_of_the_command() {
    let e = Env::setup(true);
    e.put_raw("a.jsonl", b"not json\n");
    std::fs::write(
        e.data("side/s.jsonl"),
        "{\"key\":\"s1\",\"observed_at\":900,\"label\":\"x\"}\n",
    )
    .unwrap();
    let ans = e.index(1000);
    assert_eq!(source(&ans, "rec")["status"], "source_failed");
    assert_eq!(source(&ans, "rec2")["status"], "ok");
    assert_eq!(appended(source(&ans, "rec2")), [1, 0, 0, 0]);
    assert_eq!(e.count(), 1);
}

#[test]
fn an_empty_file_is_zero_records_and_a_last_line_without_newline_is_one() {
    let e = Env::setup(true);
    e.put_raw("a.jsonl", b"");
    let ans = e.index(1000);
    assert_eq!(source(&ans, "rec")["status"], "ok");
    assert_eq!(source(&ans, "rec")["records"], 0);
    assert_eq!(e.count(), 0);
    e.put_raw("a.jsonl", rec("k1", "x", 900).as_bytes());
    let ans = e.index(1010);
    assert_eq!(source(&ans, "rec")["records"], 1);
    assert_eq!(appended(source(&ans, "rec")), [1, 0, 0, 0]);
    // CRLF line ends are whitespace after the object.
    e.put_raw("a.jsonl", format!("{}\r\n", rec("k1", "x", 900)).as_bytes());
    let ans = e.index(1020);
    assert_eq!(source(&ans, "rec")["status"], "ok");
    assert_eq!(appended(source(&ans, "rec")), [0; 4]);
}

// ---- A4: retraction across two files (I6) ----

#[test]
fn complete_listing_retracts_what_no_record_of_the_run_names() {
    let e = Env::setup(true);
    e.put("a.jsonl", &[rec("k1", "x", 900), rec("k2", "x", 900)]);
    e.put("b.jsonl", &[rec("k3", "x", 900)]);
    e.index(1000);
    // k2 leaves a.jsonl, b.jsonl goes: k2 and k3 are retracted, k1 stays.
    e.put("a.jsonl", &[rec("k1", "x", 900)]);
    e.remove("b.jsonl");
    let ans = e.index(1010);
    assert_eq!(appended(source(&ans, "rec")), [0, 0, 0, 2]);
    let (_, found) = e.search(1020, &json!({"object_type":"attempt"}));
    let keys: Vec<&str> = found["result"]["objects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["id"]["native_key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["k1"]);
    // An existing empty directory is the erasure path.
    e.remove("a.jsonl");
    let ans = e.index(1030);
    assert_eq!(appended(source(&ans, "rec")), [0, 0, 0, 1]);
    // A missing directory fails the source and retracts nothing.
    e.put("a.jsonl", &[rec("k1", "x", 900)]);
    e.index(1040);
    let before = e.count();
    std::fs::remove_dir_all(e.0.join("data")).unwrap();
    let ans = e.index(1050);
    assert_eq!(source(&ans, "rec")["status"], "source_failed");
    assert_eq!(source(&ans, "rec")["code"], "glob_root_missing");
    assert_eq!(e.count(), before);
}

#[test]
fn without_complete_listing_nothing_is_retracted() {
    let e = Env::setup(false);
    e.put("a.jsonl", &[rec("k1", "x", 900), rec("k2", "x", 900)]);
    e.index(1000);
    e.put("a.jsonl", &[rec("k1", "x", 900)]);
    let ans = e.index(1010);
    assert_eq!(appended(source(&ans, "rec")), [0; 4]);
    e.remove("a.jsonl");
    let ans = e.index(1020);
    assert_eq!(appended(source(&ans, "rec")), [0; 4]);
    assert_eq!(e.count(), 2);
}

// ---- A5: `$observed_at` (I7) and a read over 12,000 objects ----

#[test]
fn the_view_column_observed_at_shows_the_head_time() {
    let e = Env::setup(true);
    e.put(
        "a.jsonl",
        &[rec("k1", "x", 1_794_580_000), rec("k2", "y", 0)],
    );
    e.index(1_794_580_100);
    let (code, ans) = e.run(
        1_794_580_100,
        &["render"],
        br#"{"op":"render","view":"attempts","args":{}}"#,
    );
    assert_eq!(code, 0, "{ans}");
    assert_eq!(
        ans["markdown"],
        "<!-- ont:render attempts schema=acad@1 -->\n| Key | Label | Seen |\n| --- | --- | --- |\n| k1 | x | 2026-11-13T14:26:40Z |\n| k2 | y | 1970-01-01T00:00:00Z |\n<!-- /ont:render -->"
    );
}

#[test]
fn an_equality_search_over_12000_objects_after_renewal_is_not_too_large() {
    let e = Env::setup(true);
    for f in 0..2 {
        let lines: Vec<String> = (0..6000)
            .map(|i| {
                let n = f * 6000 + i;
                rec(
                    &format!("k{n:05}"),
                    if n == 7777 { "target" } else { "plain" },
                    900,
                )
            })
            .collect();
        e.put(&format!("f{f}.jsonl"), &lines);
    }
    let ans = e.index(1000);
    assert_eq!(appended(source(&ans, "rec")), [12_000, 0, 0, 0], "{ans}");
    // A second run after the heads went stale renews them in place.
    let ans = e.index(1000 + FRESH + 50);
    assert_eq!(source(&ans, "rec")["renewed"], 12_000);
    let req = json!({"object_type":"attempt","where":{"eq":["label","target"]}});
    // Past the first run's `fresh_until` (1100) but inside the renewed one
    // (1250): the renewal re-posted the 12,000 heads as fresh, so the
    // equality filter reads one candidate and nothing is stale.
    let (code, found) = e.search(1000 + FRESH + 60, &req);
    assert_eq!(code, 0, "{found}");
    assert_eq!(found["result"]["objects"].as_array().unwrap().len(), 1);
    assert_eq!(found["result"]["objects"][0]["id"]["native_key"], "k07777");
    assert_eq!(found["result"]["basis"]["stale"], 0);
    assert!(found["result"]["basis"]["scanned"].as_u64().unwrap() < 10);
}

// ---- A6: the schema check (I1) ----

#[test]
fn the_schema_check_refuses_a_records_source_without_one_owner_or_a_time_member() {
    let e = Env::new();
    let check = |s: &Value| -> Vec<(String, String)> {
        let file = e.file("c.json", &serde_json::to_vec(s).unwrap());
        let (_, ans) = e.run(0, &["schema", "check", &file], b"");
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
    let base = e.schema(1, true);
    assert_eq!(check(&base), Vec::<(String, String)>::new());
    let pair = |c: &str, p: &str| (c.to_string(), p.to_string());

    // No owner, and two owners.
    let mut none = base.clone();
    none["object_types"]
        .as_object_mut()
        .unwrap()
        .remove("attempt");
    none["views"] = json!({});
    none["grants"][0]["read"] = json!(["side"]);
    assert_eq!(check(&none), [pair("records_source_owner", "/sources/rec")]);
    let mut two = base.clone();
    two["object_types"]["second"] =
        json!({"owner":{"source":"rec"},"primary_key":{"from":"/key"},"properties":{}});
    assert_eq!(check(&two), [pair("records_source_owner", "/sources/rec")]);

    // The time member: missing, not a string, empty; a type member equal to it.
    for (member, value, pointer) in [
        ("time_member", Value::Null, "/sources/rec/time_member"),
        ("time_member", json!(5), "/sources/rec/time_member"),
        ("time_member", json!(""), "/sources/rec/time_member"),
        (
            "type_member",
            json!("observed_at"),
            "/sources/rec/type_member",
        ),
        ("type_member", json!(["type"]), "/sources/rec/type_member"),
        ("bogus", json!(1), "/sources/rec/bogus"),
    ] {
        let mut s = base.clone();
        if value.is_null() {
            s["sources"]["rec"].as_object_mut().unwrap().remove(member);
        } else {
            s["sources"]["rec"][member] = value.clone();
        }
        assert_eq!(
            check(&s),
            [pair("invalid_source_option", pointer)],
            "{member} {value}"
        );
    }

    // Mappings point into the file's own members: no fixed member list.
    let mut free = base.clone();
    free["object_types"]["attempt"]["properties"]["any"] =
        json!({"type":"string","required":false,"from":"/nowhere_in_particular/x"});
    free["object_types"]["attempt"]["properties"]["tpl"] =
        json!({"type":"string","required":false,"template":"{label}-{key}"});
    assert_eq!(check(&free), Vec::<(String, String)>::new());

    // `$observed_at` is a column, not an order key.
    let mut order = base.clone();
    order["views"]["attempts"]["order"] = json!([{"property":"$observed_at","direction":"asc"}]);
    assert_eq!(
        check(&order),
        [pair("unknown_property", "/views/attempts/order")]
    );
}

// ---- I9: bounds ----

#[test]
fn twenty_thousand_records_in_two_hundred_files_are_read() {
    let e = Env::setup(true);
    for f in 0..200 {
        let lines: Vec<String> = (0..100)
            .map(|i| rec(&format!("k{f:03}-{i:03}"), "x", 900))
            .collect();
        e.put(&format!("f{f:03}.jsonl"), &lines);
    }
    let ans = e.index(1000);
    let entry = source(&ans, "rec");
    assert_eq!(entry["status"], "ok", "{ans}");
    assert_eq!(entry["records"], 20_000);
    assert_eq!(appended(entry), [20_000, 0, 0, 0]);
}

#[test]
fn over_the_bounds_the_source_fails_with_a_code_and_nothing_is_truncated() {
    let e = Env::setup(true);
    // 201 files.
    for f in 0..201 {
        e.put(
            &format!("f{f:03}.jsonl"),
            &[rec(&format!("k{f:03}"), "x", 900)],
        );
    }
    let ans = e.index(1000);
    assert_eq!(source(&ans, "rec")["code"], "too_many_files");
    assert_eq!(e.count(), 0);
    for f in 0..201 {
        e.remove(&format!("f{f:03}.jsonl"));
    }
    // 50,001 lines in five files.
    for f in 0..5 {
        let lines: Vec<String> = (0..10_001)
            .map(|i| format!("{{\"type\":\"attempt\",\"key\":\"k{f}-{i}\",\"observed_at\":900}}"))
            .collect();
        e.put(&format!("g{f}.jsonl"), &lines);
    }
    let ans = e.index(1000);
    assert_eq!(source(&ans, "rec")["code"], "too_many_records");
    assert_eq!(e.count(), 0);
    for f in 0..5 {
        e.remove(&format!("g{f}.jsonl"));
    }
    // A line of exactly 64 KiB passes; one byte more fails.
    let padded = |len: usize| {
        let head = r#"{"type":"attempt","key":"k1","observed_at":900,"pad":""#;
        let tail = r#""}"#;
        let pad = "p".repeat(len - head.len() - tail.len());
        format!("{head}{pad}{tail}")
    };
    e.put("h.jsonl", &[padded(65_536)]);
    assert_eq!(source(&e.index(1000), "rec")["status"], "ok");
    e.put("h.jsonl", &[padded(65_537)]);
    let ans = e.index(1010);
    let path = e.data("h.jsonl").display().to_string();
    assert_eq!(cause(&ans), format!("record_line_too_long@{path}:1"));
    assert_eq!(e.count(), 1);
    // A file over 16 MiB.
    e.put_raw("h.jsonl", &vec![b'\n'; 16 * 1024 * 1024 + 1]);
    let ans = e.index(1020);
    assert_eq!(source(&ans, "rec")["code"], "file_too_large");
    assert_eq!(e.count(), 1);
}
