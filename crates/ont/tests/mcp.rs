//! Tests of `ont mcp`. Temporary directories and temporary stores only: no
//! test opens the default store path or indexes a directory outside its own
//! temporary one. The peer tests spawn the built `ont` binary and talk to it
//! over its standard input and output.
#![allow(clippy::unwrap_used)]
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Cursor, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

const NOW: u64 = 1000;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "ont-mcp-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("tasks")).unwrap();
        Self(dir)
    }
    fn store(&self, name: &str) -> String {
        self.0.join(name).display().to_string()
    }
    fn tasks(&self) -> PathBuf {
        self.0.join("tasks")
    }
    fn task(&self, name: &str, state: &str) {
        let dir = self.tasks().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("STATE.md"), state).unwrap();
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn schema(t: &Temp) -> Value {
    json!({
        "format":"ontology-schema/v1","domain":"work","version":1,
        "namespace":{"instance":"vps://palantirkc","account":"root"},
        "scope_keys":[],"value_types":{},
        "sources":{"task_states":{
            "kind":"markdown_fields",
            "glob":format!("{}/*/STATE.md", t.tasks().display()),
            "head_lines":40,"complete_listing":true,"fresh_for_s":3600}},
        "object_types":{
            "task":{"owner":{"source":"task_states"},"primary_key":{"from":"/dir"},"title":"directory",
                "properties":{
                    "directory":{"type":"string","required":true,"from":"/dir_path"},
                    "owner_line":{"type":"string","required":false,"from":"/fields/Owner"}}}
        },
        "link_types":{},"action_types":{},"functions":{},
        "grants":[{"principal":"agent","scope":{},"read":["task"],"derive":[],"actions":[]}]
    })
}

/// The subcommand, in process: (exit code, stdout, stderr).
fn sub(store: &str, args: &[&str], stdin: &[u8]) -> (i32, String, String) {
    let now = NOW.to_string();
    let mut a: Vec<String> = ["--store", store, "--now", &now]
        .iter()
        .map(|s| s.to_string())
        .collect();
    a.extend(args.iter().map(|s| s.to_string()));
    let out = ont::run(&a, stdin, 0, 0);
    (out.code, out.stdout, out.stderr)
}

/// A store at `name` with the schema of `t` installed (not indexed).
fn install(t: &Temp, name: &str) -> String {
    let store = t.store(name);
    let file = t.0.join("schema.json");
    std::fs::write(&file, serde_json::to_vec(&schema(t)).unwrap()).unwrap();
    let (code, out, _) = sub(&store, &["schema", "install", file.to_str().unwrap()], b"");
    assert_eq!(code, 0, "{out}");
    store
}

struct Peer {
    child: Child,
    stdin: Option<ChildStdin>,
    out: BufReader<ChildStdout>,
}
impl Peer {
    fn spawn(store: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ont"))
            .args([
                "--store",
                store,
                "--now",
                &NOW.to_string(),
                "mcp",
                "--client",
                "test",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let out = BufReader::new(child.stdout.take().unwrap());
        Self { child, stdin, out }
    }
    fn send_raw(&mut self, bytes: &[u8]) {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(bytes).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
    }
    fn send(&mut self, value: &Value) {
        self.send_raw(value.to_string().as_bytes());
    }
    fn recv_line(&mut self) -> String {
        let mut line = String::new();
        self.out.read_line(&mut line).unwrap();
        assert!(
            line.ends_with('\n'),
            "server closed or cut a line: {line:?}"
        );
        line
    }
    fn recv(&mut self) -> Value {
        serde_json::from_str(&self.recv_line()).unwrap()
    }
    fn request(&mut self, id: Value, method: &str, params: Value) -> Value {
        self.send(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        let response = self.recv();
        assert_eq!(response["id"], id);
        response
    }
    /// A `tools/call`; returns `(text, isError)`.
    fn call(&mut self, tool: &str, arguments: Value) -> (String, bool) {
        let r = self.request(
            json!(1),
            "tools/call",
            json!({"name":tool,"arguments":arguments}),
        );
        text_of(&r["result"])
    }
    /// Close stdin, expect no more output, and return (exit code, stderr).
    fn finish(mut self) -> (Option<i32>, String) {
        drop(self.stdin.take());
        let mut rest = String::new();
        self.out.read_to_string(&mut rest).unwrap();
        assert_eq!(rest, "", "output after the last response");
        let status = self.child.wait().unwrap();
        let mut stderr = String::new();
        self.child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        (status.code(), stderr)
    }
}

fn text_of(result: &Value) -> (String, bool) {
    let content = result["content"].as_array().unwrap();
    assert_eq!(content.len(), 1, "{result}");
    assert_eq!(content[0]["type"], "text");
    assert!(result.get("structuredContent").is_none());
    (
        content[0]["text"].as_str().unwrap().to_string(),
        result["isError"].as_bool().unwrap(),
    )
}

fn initialize(version: &str) -> Value {
    json!({"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"peer-client","version":"1"}})
}

#[test]
fn initialize_notification_and_ping() {
    let t = Temp::new();
    let mut p = Peer::spawn(&t.store("absent.sqlite"));
    let r = p.request(json!(0), "initialize", initialize("2025-06-18"));
    assert_eq!(r["jsonrpc"], "2.0");
    assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(r["result"]["capabilities"], json!({"tools":{}}));
    assert_eq!(r["result"]["serverInfo"]["name"], "ont");
    let version = sub(&t.store("x"), &["--version"], b"").1;
    assert_eq!(
        r["result"]["serverInfo"]["version"],
        version.split_whitespace().last().unwrap()
    );
    let r = p.request(json!(1), "initialize", initialize("1999-01-01"));
    assert_eq!(r["result"]["protocolVersion"], "2025-11-25");
    // A notification gets no line: the next line is the answer to the ping.
    p.send(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    p.send(&json!({"jsonrpc":"2.0","method":"notifications/anything","params":{}}));
    let r = p.request(json!(2), "ping", json!({}));
    assert_eq!(r["result"], json!({}));
    assert_eq!(p.finish(), (Some(0), String::new()));
}

#[test]
fn tools_list_is_constant_small_and_has_four_tools() {
    let t = Temp::new();
    let mut p = Peer::spawn(&t.store("absent.sqlite"));
    p.send_raw(br#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#);
    let line = p.recv_line();
    let r: Value = serde_json::from_str(&line).unwrap();
    let result = r["result"].to_string();
    eprintln!("tools/list result bytes: {}", result.len());
    assert!(result.len() <= 2000, "{}", result.len());
    let tools = r["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        ["ont_describe", "ont_get", "ont_search", "ont_index"]
    );
    for tool in tools {
        assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
        assert!(!tool["description"].as_str().unwrap().is_empty());
    }
    assert_eq!(ont::mcp::tools_list()["tools"], r["result"]["tools"]);
    assert_eq!(p.finish(), (Some(0), String::new()));
}

/// The text of a tool call equals the subcommand's stdout line.
fn assert_same(
    p: &mut Peer,
    tool: &str,
    arguments: &Value,
    store: &str,
    sub_args: &[&str],
    is_error: bool,
) -> String {
    let (text, flag) = p.call(tool, arguments.clone());
    let stdin = if sub_args == ["index"] {
        Vec::new()
    } else {
        arguments.to_string().into_bytes()
    };
    let (code, stdout, stderr) = sub(store, sub_args, &stdin);
    assert_eq!(stderr, "");
    assert_eq!(format!("{text}\n"), stdout, "{tool} {arguments}");
    assert_eq!(flag, code != 0);
    assert_eq!(flag, is_error, "{text}");
    text
}

#[test]
fn each_tool_answers_what_its_subcommand_prints() {
    let t = Temp::new();
    for n in ["alpha", "beta", "gamma"] {
        t.task(n, &format!("# State of {n}\nOwner: lead\n"));
    }
    // Two stores with the same content: the subcommand index runs on one,
    // the tool index on the other (a second run on one store would differ).
    let cmd_store = install(&t, "cmd.sqlite");
    let tool_store = install(&t, "tool.sqlite");
    let mut p = Peer::spawn(&tool_store);

    // index: success, a refusal, and a source that fails (still isError).
    let text = assert_same(
        &mut p,
        "ont_index",
        &json!({}),
        &cmd_store,
        &["index"],
        false,
    );
    assert!(text.contains(r#""status":"ok""#), "{text}");
    let text = assert_same(
        &mut p,
        "ont_index",
        &json!({"sources":["nope"]}),
        &cmd_store,
        &["index", "--source", "nope"],
        true,
    );
    assert!(text.contains("unknown_source"), "{text}");

    // describe, get, search on the indexed stores (same content, same now).
    let text = assert_same(
        &mut p,
        "ont_describe",
        &json!({}),
        &cmd_store,
        &["describe"],
        false,
    );
    assert!(text.contains("authorization_revision"));
    let text = assert_same(
        &mut p,
        "ont_describe",
        &json!({"domain":"nope"}),
        &cmd_store,
        &["describe"],
        true,
    );
    assert!(text.contains("unknown_domain"));

    let request = json!({"object_type":"task","where":{"eq":["directory", format!("{}/alpha", t.tasks().display())]}});
    let text = assert_same(
        &mut p,
        "ont_search",
        &request,
        &cmd_store,
        &["search"],
        false,
    );
    let answer: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(answer["result"]["basis"]["qualified"], true, "{text}");
    let id = answer["result"]["objects"][0]["id"].clone();
    let text = assert_same(
        &mut p,
        "ont_search",
        &json!({"object_type":"nope"}),
        &cmd_store,
        &["search"],
        true,
    );
    assert!(text.contains("unavailable"));

    assert_same(
        &mut p,
        "ont_get",
        &json!({"id":id}),
        &cmd_store,
        &["get"],
        false,
    );
    let mut missing = id.clone();
    missing["native_key"] = json!("absent");
    let text = assert_same(
        &mut p,
        "ont_get",
        &json!({"id":missing}),
        &cmd_store,
        &["get"],
        true,
    );
    assert!(text.contains("unavailable"));

    // A refusal of the request itself.
    assert_same(
        &mut p,
        "ont_get",
        &json!({"id":1}),
        &cmd_store,
        &["get"],
        true,
    );

    // A source that fails: the root is gone (complete_listing). isError.
    std::fs::rename(t.tasks(), t.0.join("moved")).unwrap();
    let text = assert_same(
        &mut p,
        "ont_index",
        &json!({}),
        &cmd_store,
        &["index"],
        true,
    );
    assert!(
        text.contains("source_failed") && text.contains("glob_root_missing"),
        "{text}"
    );
    assert_eq!(p.finish(), (Some(0), String::new()));
}

#[test]
fn protocol_errors_and_framing() {
    let t = Temp::new();
    let store = install(&t, "s.sqlite");
    let mut p = Peer::spawn(&store);
    let code = |r: &Value| r["error"]["code"].as_i64().unwrap();

    let r = p.request(
        json!(1),
        "tools/call",
        json!({"name":"ont_nope","arguments":{}}),
    );
    assert_eq!(code(&r), -32602);
    let r = p.request(
        json!(2),
        "tools/call",
        json!({"name":"ont_get","arguments":[1]}),
    );
    assert_eq!(code(&r), -32602);
    let r = p.request(json!(3), "tools/call", json!({"arguments":{}}));
    assert_eq!(code(&r), -32602);
    let r = p.request(json!(4), "no/such/method", json!({}));
    assert_eq!(code(&r), -32601);

    // Not JSON: -32700 with a null id, and the server continues.
    p.send_raw(b"this is not json");
    let r = p.recv();
    assert_eq!((code(&r), r["id"].clone()), (-32700, Value::Null));
    let r = p.request(json!(5), "ping", json!({}));
    assert_eq!(r["result"], json!({}));

    // Not a request object: a batch, a number, an object without a method.
    p.send_raw(br#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#);
    let r = p.recv();
    assert_eq!((code(&r), r["id"].clone()), (-32600, Value::Null));
    p.send_raw(b"42");
    assert_eq!(code(&p.recv()), -32600);
    p.send_raw(br#"{"jsonrpc":"2.0","id":9}"#);
    let r = p.recv();
    assert_eq!((code(&r), r["id"].clone()), (-32600, json!(9)));

    // A string id and a number id come back exactly.
    let r = p.request(json!("req-α-1"), "ping", json!({}));
    assert_eq!(r["id"], "req-α-1");
    p.send_raw(br#"{"jsonrpc":"2.0","id":123456789012,"method":"ping"}"#);
    assert_eq!(
        p.recv_line(),
        "{\"id\":123456789012,\"jsonrpc\":\"2.0\",\"result\":{}}\n"
    );

    // A request line over 1,048,576 bytes: -32600, the line is discarded,
    // and the next request is served.
    let mut long = br#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{"x":""#.to_vec();
    long.resize(ont::mcp::MAX_LINE + 1, b'a');
    p.send_raw(&long);
    let r = p.recv();
    assert_eq!(
        (code(&r), r["error"]["message"].clone()),
        (-32600, json!("request_too_large"))
    );
    assert_eq!(r["id"], Value::Null);
    let r = p.request(json!(6), "ping", json!({}));
    assert_eq!(r["result"], json!({}));

    // End of input: exit 0, nothing on stderr.
    assert_eq!(p.finish(), (Some(0), String::new()));
}

#[test]
fn a_line_at_the_bound_is_parsed() {
    // Exactly 1,048,576 bytes is accepted (it is valid JSON here).
    let t = Temp::new();
    let mut p = Peer::spawn(&t.store("absent.sqlite"));
    let mut line = br#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{"x":""#.to_vec();
    let tail = br#""}}"#;
    line.resize(ont::mcp::MAX_LINE - tail.len(), b'a');
    line.extend_from_slice(tail);
    assert_eq!(line.len(), ont::mcp::MAX_LINE);
    p.send_raw(&line);
    assert_eq!(p.recv()["result"], json!({}));
    assert_eq!(p.finish(), (Some(0), String::new()));
}

#[test]
fn a_response_over_the_bound_is_response_too_large() {
    let t = Temp::new();
    for i in 0..1000 {
        t.task(
            &format!("task-{i:04}"),
            "# State of a task with a title that adds bytes to every object\nOwner: lead\n",
        );
    }
    let store = install(&t, "big.sqlite");
    let (code, out, _) = sub(&store, &["index"], b"");
    assert_eq!(code, 0, "{out}");
    let request = json!({"object_type":"task","limit":1000});
    let (code, full, _) = sub(&store, &["search"], request.to_string().as_bytes());
    assert_eq!(code, 0);
    assert!(full.len() - 1 > ont::mcp::MAX_RESPONSE, "{}", full.len());
    let mut p = Peer::spawn(&store);
    let (text, is_error) = p.call("ont_search", request);
    assert!(is_error);
    let answer: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(answer["status"], "refused");
    assert_eq!(answer["code"], "response_too_large");
    assert_eq!(answer["bytes"], full.len() - 1);
    assert_eq!(answer["retry_with_smaller_limit"], true);
    // A smaller limit answers.
    let (text, is_error) = p.call("ont_search", json!({"object_type":"task","limit":10}));
    assert!(!is_error, "{text}");
    assert_eq!(p.finish(), (Some(0), String::new()));
}

#[test]
fn a_store_that_does_not_exist_is_a_tool_result_and_is_not_created() {
    let t = Temp::new();
    let store = t.store("absent.sqlite");
    let mut p = Peer::spawn(&store);
    let id = json!({"object_type":"task","namespace":{"instance":"vps://palantirkc","account":"root"},"native_key":"a"});
    for (tool, args, sub_args) in [
        ("ont_describe", json!({}), vec!["describe"]),
        ("ont_get", json!({"id":id}), vec!["get"]),
        ("ont_search", json!({"object_type":"task"}), vec!["search"]),
        ("ont_index", json!({}), vec!["index"]),
    ] {
        let text = assert_same(&mut p, tool, &args, &store, &sub_args, true);
        let answer: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(answer["code"], "store_not_found", "{text}");
    }
    assert_eq!(p.finish(), (Some(0), String::new()));
    let created: Vec<_> = std::fs::read_dir(&t.0)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        !created.iter().any(|n| n.starts_with("absent")),
        "created: {created:?}"
    );
}

#[test]
fn two_servers_on_one_store_and_the_run_lock() {
    let t = Temp::new();
    t.task("a", "# a\nOwner: lead\n");
    let store = install(&t, "shared.sqlite");
    let (code, _, _) = sub(&store, &["index"], b"");
    assert_eq!(code, 0);
    let mut a = Peer::spawn(&store);
    let mut b = Peer::spawn(&store);
    let search = json!({"object_type":"task"});
    let (ta, ea) = a.call("ont_search", search.clone());
    let (tb, eb) = b.call("ont_search", search);
    assert!(!ea && !eb);
    assert_eq!(ta, tb);
    // The run lock is held through a second file description (as the
    // library test does): one `ont_index` is refused as busy, a search is
    // not blocked, and after the release the index runs.
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .open(format!("{store}.index.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    let (text, is_error) = b.call("ont_index", json!({}));
    assert!(is_error);
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap()["code"],
        "index_busy"
    );
    assert!(!a.call("ont_search", json!({"object_type":"task"})).1);
    drop(lock);
    let (text, is_error) = b.call("ont_index", json!({}));
    assert!(!is_error, "{text}");
    assert_eq!(a.finish(), (Some(0), String::new()));
    assert_eq!(b.finish(), (Some(0), String::new()));
}

#[test]
fn a_panic_in_one_call_answers_that_call_and_the_server_goes_on() {
    use ont::cli::Op;
    ont::mcp::install_panic_recorder();
    let t = Temp::new();
    let store = install(&t, "s.sqlite");
    let config = ont::mcp::Config {
        store: store.clone().into(),
        now: Some(NOW),
        client: "unit".into(),
    };
    let exec = |op: Op, request: &[u8], ctx: &ont::Context| {
        if op == Op::Get {
            panic!("seam");
        }
        ont::cli::execute(op, request, ctx)
    };
    let lines = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":initialize("2025-03-26")}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"ont_get","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"ont_describe"}}),
    ]
    .map(|v| v.to_string())
    .join("\n");
    let mut out = Vec::new();
    let session =
        ont::mcp::serve_with(&config, Cursor::new(lines.into_bytes()), &mut out, &exec).unwrap();
    let responses: Vec<Value> = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(responses.len(), 3);
    let (text, is_error) = text_of(&responses[1]["result"]);
    assert!(is_error);
    let answer: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(answer["status"], "internal");
    let code = answer["code"].as_str().unwrap();
    assert!(
        code.starts_with("panic@") && code.contains("mcp.rs:"),
        "{code}"
    );
    let (text, is_error) = text_of(&responses[2]["result"]);
    assert!(!is_error, "{text}");
    // Where the client names are held.
    assert_eq!(session.client(), "unit");
    assert_eq!(session.client_info_name(), Some("peer-client"));
    assert_eq!(session.protocol_version(), Some("2025-03-26"));
}

#[test]
fn client_name_uid_and_now_rules() {
    let run = |args: &[&str], uid: u32| {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        ont::run(&args, &b""[..], uid, uid)
    };
    for bad in ["", "Bad", "-x", "a_b", "a b", &"a".repeat(33)] {
        let out = run(&["mcp", "--client", bad], 0);
        assert_eq!(out.code, 2, "{bad:?}");
        assert!(
            out.stdout.contains(r#""code":"invalid_request""#),
            "{}",
            out.stdout
        );
    }
    let out = run(&["mcp"], 0);
    assert!(out.stdout.contains("invalid_request"), "{}", out.stdout);
    let out = run(&["mcp", "--client", "claude"], 1000);
    assert!(
        out.stdout.contains("principal_not_trusted"),
        "{}",
        out.stdout
    );
    let out = run(&["--now", "5", "mcp", "--client", "claude"], 0);
    assert!(out.stdout.contains("now_requires_store"), "{}", out.stdout);
    let out = run(&["describe", "--client", "claude"], 0);
    assert!(out.stdout.contains("invalid_argument"), "{}", out.stdout);
    for ok in ["a", "claude", "codex-2", "0x", &"a".repeat(32)] {
        assert!(ont::mcp::valid_client(ok), "{ok}");
    }
}
