//! Tests of `ont mcp`. Temporary directories and temporary stores only: no
//! test opens the default store path or indexes a directory outside its own
//! temporary one. The peer tests spawn the built `ont` binary and talk to it
//! over its standard input and output.
//!
//! Two peers exist. As uid 0 (this host) the peer is the spawned binary, and
//! only then do these assertions mean something: the exit code and the empty
//! stderr returned by `Peer::finish`, the `cli::run_mcp` plumbing (argument
//! parsing, panic-hook installation, stdout), and `serverInfo.version` coming
//! from the build. Otherwise (CI runs as a non-root user, and the binary
//! refuses a non-root principal) the peer is `ont::mcp::serve` on a thread
//! over pipes: the framing and every protocol assertion run, `finish` returns
//! a fixed `(Some(0), "")`, and the three items above are not exercised.
#![allow(clippy::unwrap_used)]
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Cursor, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
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

/// Real = effective uid 0 (as `main.rs` reads it): only then does the built
/// binary serve; otherwise it refuses with `principal_not_trusted`.
fn is_root() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|l| l.strip_prefix("Uid:"))
        .is_some_and(|f| f.split_whitespace().take(2).all(|u| u == "0"))
}

enum Backend {
    /// The built `ont` binary (uid 0 only).
    Child {
        child: Child,
        stdin: Option<ChildStdin>,
    },
    /// `ont::mcp::serve` on a thread over two pipes: the same framing, run
    /// in the test process (used when the test is not uid 0, as in CI).
    Thread {
        stdin: Option<std::io::PipeWriter>,
        handle: std::thread::JoinHandle<std::io::Result<ont::mcp::Session>>,
    },
}

struct Peer {
    backend: Backend,
    out: BufReader<Box<dyn Read>>,
}
impl Peer {
    fn spawn(store: &str) -> Self {
        if is_root() {
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
            let out = BufReader::new(Box::new(child.stdout.take().unwrap()) as Box<dyn Read>);
            return Self {
                backend: Backend::Child { child, stdin },
                out,
            };
        }
        let (request_reader, request_writer) = std::io::pipe().unwrap();
        let (response_reader, response_writer) = std::io::pipe().unwrap();
        let config = ont::mcp::Config {
            store: store.into(),
            now: Some(NOW),
            client: "test".into(),
        };
        let handle = std::thread::spawn(move || {
            ont::mcp::serve(&config, BufReader::new(request_reader), response_writer)
        });
        Self {
            backend: Backend::Thread {
                stdin: Some(request_writer),
                handle,
            },
            out: BufReader::new(Box::new(response_reader) as Box<dyn Read>),
        }
    }
    fn send_raw(&mut self, bytes: &[u8]) {
        let stdin: &mut dyn Write = match &mut self.backend {
            Backend::Child { stdin, .. } => stdin.as_mut().unwrap(),
            Backend::Thread { stdin, .. } => stdin.as_mut().unwrap(),
        };
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
        match &mut self.backend {
            Backend::Child { stdin, .. } => drop(stdin.take()),
            Backend::Thread { stdin, .. } => drop(stdin.take()),
        }
        let mut rest = String::new();
        self.out.read_to_string(&mut rest).unwrap();
        assert_eq!(rest, "", "output after the last response");
        match self.backend {
            Backend::Child { mut child, .. } => {
                let status = child.wait().unwrap();
                let mut stderr = String::new();
                child
                    .stderr
                    .take()
                    .unwrap()
                    .read_to_string(&mut stderr)
                    .unwrap();
                (status.code(), stderr)
            }
            Backend::Thread { handle, .. } => {
                handle.join().unwrap().unwrap();
                (Some(0), String::new())
            }
        }
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
fn tools_list_is_constant_small_and_has_six_tools() {
    let t = Temp::new();
    let mut p = Peer::spawn(&t.store("absent.sqlite"));
    p.send_raw(br#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#);
    let line = p.recv_line();
    let r: Value = serde_json::from_str(&line).unwrap();
    let result = r["result"].to_string();
    eprintln!("tools/list result bytes: {}", result.len());
    assert!(result.len() <= 5000, "{}", result.len());
    let tools = r["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "ont_describe",
            "ont_get",
            "ont_search",
            "ont_index",
            "ont_action_validate",
            "ont_action_apply"
        ]
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

/// Serve `input` to its end with no store behind it; the response lines.
fn serve_bytes(input: &[u8]) -> Vec<Value> {
    let config = ont::mcp::Config {
        store: "/nonexistent-ont-mcp-test/s.sqlite".into(),
        now: Some(NOW),
        client: "unit".into(),
    };
    let mut out = Vec::new();
    ont::mcp::serve(&config, Cursor::new(input.to_vec()), &mut out).unwrap();
    String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn framing_edges_eof_blank_null_id_and_bad_utf8() {
    let ping = br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    // (b) A last line without a newline is answered.
    let r = serve_bytes(ping);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["result"], json!({}));
    // (c) Blank lines get no response; the next request is answered.
    let mut input = b"\n\r\n   \n".to_vec();
    input.extend_from_slice(ping);
    input.push(b'\n');
    let r = serve_bytes(&input);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["id"], 1);
    // (d) `"id":null` is -32600 with a null id.
    let r = serve_bytes(br#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#);
    assert_eq!(r[0]["error"]["code"], -32600);
    assert_eq!(r[0]["id"], Value::Null);
    // (e) A line that is not valid UTF-8 is -32700, and the server goes on.
    let mut input = b"\xff\xfe{\n".to_vec();
    input.extend_from_slice(ping);
    let r = serve_bytes(&input);
    assert_eq!(r.len(), 2);
    assert_eq!(
        (r[0]["error"]["code"].clone(), r[0]["id"].clone()),
        (json!(-32700), Value::Null)
    );
    assert_eq!(r[1]["result"], json!({}));
}

#[test]
fn an_internal_failure_is_a_tool_result_with_a_located_code() {
    use ont::cli::Op;
    let config = ont::mcp::Config {
        store: "/nonexistent-ont-mcp-test/s.sqlite".into(),
        now: Some(NOW),
        client: "unit".into(),
    };
    let exec = |_op: Op, _request: &[u8], _ctx: &ont::Context| {
        Err(ont::Failure::Internal(ont::internal!("seam_failure")))
    };
    let line =
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ont_describe"}});
    let mut out = Vec::new();
    ont::mcp::serve_with(
        &config,
        Cursor::new(format!("{line}\n").into_bytes()),
        &mut out,
        &exec,
    )
    .unwrap();
    let r: Value = serde_json::from_slice(&out).unwrap();
    let (text, is_error) = text_of(&r["result"]);
    assert!(is_error);
    let answer: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(answer["status"], "internal");
    let code = answer["code"].as_str().unwrap();
    assert!(
        code.starts_with("seam_failure@") && code.contains("mcp.rs:"),
        "{code}"
    );
}

#[test]
fn every_example_in_a_description_is_accepted_by_the_parser() {
    let t = Temp::new();
    t.task("a", "# a\nOwner: lead\n");
    let store = install(&t, "ex.sqlite");
    assert_eq!(sub(&store, &["index"], b"").0, 0);
    let list = ont::mcp::tools_list();
    let description = |name: &str| -> String {
        list["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .unwrap()["description"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let mut p = Peer::spawn(&store);
    // ont_search: the `where` after "e.g. ".
    let d = description("ont_search");
    let rest = d.split("e.g. ").nth(1).unwrap();
    let example: Value = serde_json::Deserializer::from_str(rest)
        .into_iter::<Value>()
        .next()
        .unwrap()
        .unwrap();
    let (text, _) = p.call("ont_search", json!({"object_type":"task","where":example}));
    assert!(!text.contains("invalid_request"), "{text}");
    // ont_get: `id` is an objects[].id from ont_search.
    assert!(description("ont_get").contains("objects[].id"));
    let (text, _) = p.call("ont_search", json!({"object_type":"task"}));
    let answer: Value = serde_json::from_str(&text).unwrap();
    let id = answer["result"]["objects"][0]["id"].clone();
    let (text, is_error) = p.call("ont_get", json!({"id":id}));
    assert!(!is_error, "{text}");
    // ont_describe is called first with no arguments; ont_index returns
    // counts per source.
    assert!(description("ont_describe").contains("Call first"));
    let (text, is_error) = p.call("ont_describe", json!({}));
    assert!(!is_error && text.contains("\"task\""), "{text}");
    let (text, is_error) = p.call("ont_index", json!({}));
    assert!(!is_error && text.contains("\"appended\""), "{text}");
    // The limit schema matches the parser: 0 is refused, 1 is accepted.
    assert_eq!(
        ont::mcp::tools_list()["tools"][2]["inputSchema"]["properties"]["limit"]["minimum"],
        1
    );
    let (text, is_error) = p.call("ont_search", json!({"object_type":"task","limit":0}));
    assert!(is_error && text.contains("invalid_request"), "{text}");
    let (_, is_error) = p.call("ont_search", json!({"object_type":"task","limit":1}));
    assert!(!is_error);
    assert_eq!(p.finish(), (Some(0), String::new()));
}

/// Reuse the schema already shared by the action tests, without copying it.
mod action_fixture {
    use super::*;

    pub fn install(t: &Temp) -> String {
        let store = t.store("action.sqlite");
        let file = t.0.join("action-schema.json");
        std::fs::write(&file, include_bytes!("fixtures/action-work.json")).unwrap();
        let (code, out, _) = sub(&store, &["schema", "install", file.to_str().unwrap()], b"");
        assert_eq!(code, 0, "{out}");
        store
    }

    pub fn counts(store: &str) -> Vec<usize> {
        let mut store =
            ont_store::Store::open_schema_existing(std::path::Path::new(store)).unwrap();
        let mut counts = vec![store.action_log().unwrap().len()];
        for ty in ["task", "hold", "note", "tag", "widget", "probe"] {
            counts.push(store.scan_heads(ty, 100, None).unwrap().len());
        }
        counts
    }
}

fn action_call(server: &mut ont::mcp::Server<'_>, name: &str, arguments: Value) -> Value {
    let response = server
        .handle_line(
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
        "params":{"name":name,"arguments":arguments}})
            .to_string()
            .as_bytes(),
        )
        .unwrap();
    assert!(response.get("error").is_none(), "{response}");
    let body: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        response["result"]["isError"],
        body["status"] != "ok",
        "{response}"
    );
    body
}

fn action_config(store: &str, client: &str) -> ont::mcp::Config {
    ont::mcp::Config {
        store: store.into(),
        now: Some(NOW),
        client: client.into(),
    }
}

#[test]
fn action_validate_apply_passthrough_and_refusals_write_nothing() {
    let t = Temp::new();
    let store = action_fixture::install(&t);
    let mut server = ont::mcp::Server::new(action_config(&store, "test"), &ont::cli::execute);
    let body =
        json!({"action_type":"create_tag","parameters":{"label":"once"},"request_id":"tag-1"});
    let preview = action_call(&mut server, "ont_action_validate", body.clone());
    assert_eq!(preview["result"]["valid"], true, "{preview}");
    let (code, stdout, stderr) = sub(
        &store,
        &["--client", "test", "action", "validate"],
        body.to_string().as_bytes(),
    );
    assert_eq!((code, stderr.as_str()), (0, ""));
    assert_eq!(preview, serde_json::from_str::<Value>(&stdout).unwrap());
    let mut ctx = ont::Context::new(store.clone().into(), NOW);
    ctx.client = "test".into();
    let req = ont::action::ValidateActionRequest::parse(body.to_string().as_bytes()).unwrap();
    let ont::Answer::Enveloped { result, .. } = ont::action::validate_action(&ctx, &req).unwrap()
    else {
        panic!("enveloped")
    };
    assert_eq!(preview["result"], result);
    assert_eq!(action_fixture::counts(&store), vec![0, 0, 0, 0, 0, 0, 0]);
    let applied = action_call(&mut server, "ont_action_apply", body.clone());
    assert_eq!(applied["result"]["outcome"], "applied", "{applied}");
    assert_eq!(applied["result"]["edits"], preview["result"]["edits"]);
    assert_eq!(applied["result"]["actor_trust"], "self_reported");
    let req = ont::action::ApplyActionRequest::parse(body.to_string().as_bytes()).unwrap();
    let ont::Answer::Enveloped { result, .. } = ont::action::apply_action(&ctx, &req).unwrap()
    else {
        panic!("enveloped")
    };
    let retried = action_call(&mut server, "ont_action_apply", body);
    assert_eq!(
        retried["result"], result,
        "library result is passed through"
    );
    assert_eq!(retried["result"]["outcome"], "already_applied");
    assert_eq!(action_fixture::counts(&store), vec![1, 0, 0, 0, 1, 0, 0]);

    let before = action_fixture::counts(&store);
    let edits = std::fs::read(t.0.join("edits.jsonl")).unwrap();
    let bad = json!({"action_type":"set_hold","parameters":{"task":{"object_type":"task",
        "namespace":{"instance":"vps://palantirkc","account":"root"},"native_key":"absent"},
        "effect":"merge","until":"later"},"request_id":"hold-1"});
    let failed = action_call(&mut server, "ont_action_validate", bad.clone());
    assert_eq!(failed["result"]["valid"], false, "{failed}");
    assert!(
        failed["result"]["criteria"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["code"] == "task_not_current" && c["passed"] == false)
    );
    let refused = action_call(&mut server, "ont_action_apply", bad);
    assert_eq!(refused["code"], "submission_criteria");
    for request_id in [None, Some(Value::Null)] {
        let mut body = json!({"action_type":"create_tag","parameters":{"label":"missing"}});
        if let Some(id) = request_id {
            body["request_id"] = id;
        }
        let refused = action_call(&mut server, "ont_action_apply", body);
        assert_eq!(refused["code"], "invalid_argument");
        assert_eq!(refused["argument"], "request_id");
    }
    let unknown = action_call(
        &mut server,
        "ont_action_apply",
        json!({"action_type":"create_tag",
        "parameters":{"label":"x"},"request_id":"unknown-1","surprise":true}),
    );
    assert_eq!(unknown["code"], "invalid_request");
    assert_eq!(action_fixture::counts(&store), before);
    assert_eq!(std::fs::read(t.0.join("edits.jsonl")).unwrap(), edits);
}

#[test]
fn cancellation_between_apply_and_retry_yields_two_answers_and_one_log_row() {
    let t = Temp::new();
    let store = action_fixture::install(&t);
    let request = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
        "name":"ont_action_apply","arguments":{"action_type":"create_tag",
        "parameters":{"label":"once"},"request_id":"cancel-1"}}});
    let cancel =
        json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}});
    let input = format!("{request}\n{cancel}\n{request}\n");
    let mut output = Vec::new();
    ont::mcp::serve(
        &action_config(&store, "test"),
        Cursor::new(input),
        &mut output,
    )
    .unwrap();
    let output = String::from_utf8(output).unwrap();
    let answers: Vec<Value> = output
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(answers.len(), 2);
    for (answer, expected) in answers.iter().zip(["applied", "already_applied"]) {
        let text: Value =
            serde_json::from_str(answer["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text["result"]["outcome"], expected);
    }
    assert_eq!(action_fixture::counts(&store), vec![1, 0, 0, 0, 1, 0, 0]);
}

#[test]
fn action_actor_uses_initialize_fallback_and_explicit_client() {
    // Set the environment only in a child, never mutate the threaded test process.
    if std::env::var_os("ONT_MCP_ACTOR_TEST").is_none() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "action_actor_uses_initialize_fallback_and_explicit_client",
                "--nocapture",
            ])
            .env("ONT_AGENT", "environment-agent")
            .env("ONT_MCP_ACTOR_TEST", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let t = Temp::new();
    let store = action_fixture::install(&t);
    for (client, expected, request_id) in [
        ("", "peer-client", "fallback"),
        ("explicit", "explicit", "explicit"),
    ] {
        let mut server = ont::mcp::Server::new(action_config(&store, client), &ont::cli::execute);
        server
            .handle_line(
                json!({"jsonrpc":"2.0","id":0,"method":"initialize",
            "params":initialize("2025-11-25")})
                .to_string()
                .as_bytes(),
            )
            .unwrap();
        let answer = action_call(
            &mut server,
            "ont_action_apply",
            json!({"action_type":"create_tag",
            "parameters":{"label":"actor"},"request_id":request_id,"agent":"argument-agent","session":"session-1"}),
        );
        assert_eq!(answer["result"]["outcome"], "applied", "{answer}");
        let mut db = ont_store::Store::open_schema_existing(std::path::Path::new(&store)).unwrap();
        let log = db.action_log().unwrap();
        let actor = &log.last().unwrap().1.actor;
        assert_eq!(actor["client"], expected);
        assert_eq!(actor["agent"], "argument-agent");
        assert_eq!(actor["session"], "session-1");
        drop(db);
        let answer = action_call(
            &mut server,
            "ont_action_apply",
            json!({"action_type":"create_tag",
            "parameters":{"label":"environment"},"request_id":format!("{request_id}-env")}),
        );
        assert_eq!(answer["result"]["outcome"], "applied", "{answer}");
        let mut db = ont_store::Store::open_schema_existing(std::path::Path::new(&store)).unwrap();
        let log = db.action_log().unwrap();
        assert_eq!(log.last().unwrap().1.actor["agent"], "environment-agent");
    }
}

#[test]
fn every_listed_tool_is_callable_and_unknown_names_are_not_callable() {
    let t = Temp::new();
    let mut server = ont::mcp::Server::new(
        action_config(&t.store("absent"), "test"),
        &ont::cli::execute,
    );
    let listing = ont::mcp::tools_list();
    for tool in listing["tools"].as_array().unwrap() {
        let name = tool["name"].as_str().unwrap();
        for (candidate, callable) in [(name.to_string(), true), (format!("{name}_unknown"), false)]
        {
            let answer = server
                .handle_line(
                    json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
                "params":{"name":candidate,"arguments":{}}})
                    .to_string()
                    .as_bytes(),
                )
                .unwrap();
            assert_eq!(answer.get("result").is_some(), callable, "{answer}");
            if !callable {
                assert_eq!(answer["error"]["code"], -32602);
            }
        }
    }
    // The held tool is neither advertised nor callable.
    assert!(
        !listing["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "ont_action_list")
    );
}
