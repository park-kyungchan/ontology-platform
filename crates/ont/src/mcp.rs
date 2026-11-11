//! `ont mcp`: a Model Context Protocol server over stdio for `describe`,
//! `get`, `search` and `index`.
//!
//! Wire: newline-delimited JSON-RPC 2.0, one request per line on stdin, one
//! JSON line per response on stdout (nothing else is ever written there).
//! No listener, no threads. State kept between calls: the negotiated
//! protocol version, `--client` and the client's `clientInfo.name`
//! ([`Session`], held by [`Server`]; they are not written anywhere yet: the
//! action log of a later packet reads them).
//!
//! A `tools/call` runs the same operation function as the subcommand through
//! [`crate::cli::execute`] with a [`Context`] built for that call (the store
//! is opened and closed inside the operation; `now` is read per call unless
//! `--now` was given) and renders it with the same [`crate::cli::render`], so
//! the `text` of a result is the subcommand's output line.
use crate::cli::{Op, execute, now_seconds, render};
use crate::{Answer, Context, Failure, Refusal};
use serde_json::{Map, Value, json};
use std::io::{BufRead, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Mutex;

/// Longest request line (bytes, newline excluded) that is parsed.
pub const MAX_LINE: usize = 1_048_576;
/// Longest `text` a tool result may carry.
pub const MAX_RESPONSE: usize = 262_144;
/// Protocol versions this server answers with, newest first (the list of
/// `edu-academy-mcp`, `protocol.rs`, without `2024-10-07`).
pub const SUPPORTED_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// What `ont mcp` was started with.
#[derive(Debug, Clone)]
pub struct Config {
    pub store: PathBuf,
    /// `--now`; `None` reads the clock per call.
    pub now: Option<u64>,
    /// `--client`.
    pub client: String,
}

/// `^[a-z0-9][a-z0-9-]{0,31}$`.
pub fn valid_client(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// What the server remembers between calls.
#[derive(Debug, Clone, Default)]
pub struct Session {
    client: String,
    client_info_name: Option<String>,
    protocol_version: Option<String>,
}
impl Session {
    /// The `--client` value.
    pub fn client(&self) -> &str {
        &self.client
    }
    /// `clientInfo.name` of the last `initialize`, if it gave one.
    pub fn client_info_name(&self) -> Option<&str> {
        self.client_info_name.as_deref()
    }
    /// The protocol version answered by the last `initialize`.
    pub fn protocol_version(&self) -> Option<&str> {
        self.protocol_version.as_deref()
    }
}

/// How a tool call runs its operation (a seam: `serve` passes
/// [`crate::cli::execute`]).
pub type Exec<'a> = dyn Fn(Op, &[u8], &Context) -> Result<Answer, Failure> + 'a;

static LAST_PANIC: Mutex<Option<String>> = Mutex::new(None);

/// Replace the panic hook by one that records where a panic happened
/// (`file:line`) and prints nothing, so a panic inside one tool call is
/// answered as that call's internal failure with a located code. The server
/// calls it once; it is process-wide.
pub fn install_panic_recorder() {
    std::panic::set_hook(Box::new(|info| {
        let at = info.location().map_or("unknown:0".to_string(), |l| {
            format!("{}:{}", l.file(), l.line())
        });
        if let Ok(mut last) = LAST_PANIC.lock() {
            *last = Some(at);
        }
    }));
}

fn take_panic_location() -> String {
    LAST_PANIC
        .lock()
        .ok()
        .and_then(|mut last| last.take())
        .unwrap_or_else(|| format!("{}:{}", file!(), line!()))
}

/// The four tools, in `tools/list` order.
const TOOLS: [(&str, Op); 4] = [
    ("ont_describe", Op::Describe),
    ("ont_get", Op::Get),
    ("ont_search", Op::Search),
    ("ont_index", Op::Index),
];

/// The constant `tools/list` result: it never depends on installed schemas.
pub fn tools_list() -> Value {
    let s = json!({"type":"string"});
    let id = json!({"type":"object","required":["object_type","namespace","native_key"],
        "properties":{"object_type":s,"native_key":s,
            "namespace":{"type":"object","required":["instance","account"],
                "properties":{"instance":s,"account":s}}}});
    json!({"tools":[
        {"name":"ont_describe",
         "description":"List the installed domains, object and link types, properties, actions, functions and sources this caller may read. Always carries schema pins and authorization_revision.",
         "inputSchema":{"type":"object","properties":{"domain":s,"object_type":s}}},
        {"name":"ont_get",
         "description":"One object with its full observation history. A hidden and a missing object answer alike (unavailable). Always carries schema pins and authorization_revision.",
         "inputSchema":{"type":"object","required":["id"],"properties":{"id":id}}},
        {"name":"ont_search",
         "description":"Filter objects of one type. where: {eq|ne|lt|le|gt|ge|prefix:[property,value]}, {in:[property,[values]]}, {unresolved:property}, or {all|any|none:[filters]}. limit up to 1000 (default 100); pass continuation for the next page. Always carries schema pins, authorization_revision and basis (scanned, matched, stale, unresolved_excluded, qualified).",
         "inputSchema":{"type":"object","required":["object_type"],
            "properties":{"object_type":s,"where":{"type":"object"},
                "limit":{"type":"integer","minimum":0,"maximum":1000},"continuation":s}}},
        {"name":"ont_index",
         "description":"Read the sources of the installed schemas and append what changed. sources: names to run (default all). One run at a time per store (index_busy). Always carries schema pins and authorization_revision.",
         "inputSchema":{"type":"object","properties":{"sources":{"type":"array","items":s,"minItems":1}}}}
    ]})
}

enum Read {
    Eof,
    Complete,
    TooLarge,
}

/// Read one line into `buf` (newline excluded). A line over `max` bytes is
/// discarded as it is read, never buffered, and reported `TooLarge`.
fn read_line(r: &mut impl BufRead, buf: &mut Vec<u8>, max: usize) -> std::io::Result<Read> {
    buf.clear();
    let (mut too_large, mut any) = (false, false);
    loop {
        let chunk = match r.fill_buf() {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if chunk.is_empty() {
            return Ok(if too_large {
                Read::TooLarge
            } else if any {
                Read::Complete
            } else {
                Read::Eof
            });
        }
        any = true;
        let newline = chunk.iter().position(|b| *b == b'\n');
        let take = newline.unwrap_or(chunk.len());
        if !too_large {
            if buf.len() + take > max {
                too_large = true;
                buf.clear();
            } else {
                buf.extend_from_slice(&chunk[..take]);
            }
        }
        let used = newline.map_or(take, |i| i + 1);
        r.consume(used);
        if newline.is_some() {
            return Ok(if too_large {
                Read::TooLarge
            } else {
                Read::Complete
            });
        }
    }
}

fn rpc_error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}
fn rpc_result(id: Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}

/// The server: configuration plus the [`Session`].
pub struct Server<'a> {
    config: Config,
    session: Session,
    exec: &'a Exec<'a>,
}

impl<'a> Server<'a> {
    pub fn new(config: Config, exec: &'a Exec<'a>) -> Self {
        let session = Session {
            client: config.client.clone(),
            ..Session::default()
        };
        Self {
            config,
            session,
            exec,
        }
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    /// The response to one request line, `None` for a notification.
    pub fn handle_line(&mut self, line: &[u8]) -> Option<Value> {
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            return Some(rpc_error(Value::Null, -32700, "Parse error"));
        };
        let Value::Object(object) = value else {
            return Some(rpc_error(Value::Null, -32600, "Invalid Request"));
        };
        let id = match object.get("id") {
            None => None,
            Some(v @ (Value::String(_) | Value::Number(_))) => Some(v.clone()),
            Some(_) => return Some(rpc_error(Value::Null, -32600, "Invalid Request")),
        };
        let method = match (object.get("jsonrpc"), object.get("method")) {
            (Some(Value::String(v)), Some(Value::String(m))) if v == "2.0" => m.as_str(),
            _ => {
                return Some(rpc_error(
                    id.unwrap_or(Value::Null),
                    -32600,
                    "Invalid Request",
                ));
            }
        };
        // Every notification, `notifications/initialized` included, gets no
        // response and changes nothing.
        let id = id?;
        let params = object.get("params");
        Some(match method {
            "initialize" => self.initialize(id, params),
            "ping" => rpc_result(id, json!({})),
            "tools/list" => rpc_result(id, tools_list()),
            "tools/call" => self.tools_call(id, params),
            _ => rpc_error(id, -32601, "Method not found"),
        })
    }

    fn initialize(&mut self, id: Value, params: Option<&Value>) -> Value {
        let Some(Value::String(asked)) = params.and_then(|p| p.get("protocolVersion")) else {
            return rpc_error(id, -32602, "Invalid params");
        };
        let version = SUPPORTED_VERSIONS
            .iter()
            .find(|v| **v == asked)
            .unwrap_or(&SUPPORTED_VERSIONS[0]);
        self.session.protocol_version = Some((*version).to_string());
        self.session.client_info_name = params
            .and_then(|p| p.get("clientInfo"))
            .and_then(|c| c.get("name"))
            .and_then(Value::as_str)
            .map(str::to_string);
        rpc_result(
            id,
            json!({
                "protocolVersion":version,
                "capabilities":{"tools":{}},
                "serverInfo":{"name":"ont","version":env!("ONT_BUILD_REVISION")}
            }),
        )
    }

    fn tools_call(&mut self, id: Value, params: Option<&Value>) -> Value {
        let invalid = |id: Value| rpc_error(id, -32602, "Invalid params");
        let Some(Value::Object(params)) = params else {
            return invalid(id);
        };
        let Some(Value::String(name)) = params.get("name") else {
            return invalid(id);
        };
        let Some((_, op)) = TOOLS.iter().find(|(n, _)| n == name) else {
            return invalid(id);
        };
        let arguments = match params.get("arguments") {
            None => Value::Object(Map::new()),
            Some(v @ Value::Object(_)) => v.clone(),
            Some(_) => return invalid(id),
        };
        let (text, is_error) = self.call(*op, &arguments);
        rpc_result(
            id,
            json!({"content":[{"type":"text","text":text}],"isError":is_error}),
        )
    }

    /// One tool call: the operation of the subcommand, rendered as the
    /// subcommand renders it. Returns the `text` and `isError`. A panic is
    /// contained here and answered as an internal failure of this call.
    fn call(&self, op: Op, arguments: &Value) -> (String, bool) {
        let now = self.config.now.unwrap_or_else(now_seconds);
        let ctx = Context {
            store: self.config.store.clone(),
            now,
        };
        let request = arguments.to_string();
        let ran = catch_unwind(AssertUnwindSafe(|| {
            render((self.exec)(op, request.as_bytes(), &ctx), now)
        }));
        let outcome = match ran {
            Ok(o) => o,
            Err(_) => {
                let text =
                    json!({"status":"internal","code":format!("panic@{}", take_panic_location())});
                return (text.to_string(), true);
            }
        };
        let is_error = outcome.code != 0;
        // An internal failure prints no stdout; its code is the stderr line.
        let text = if outcome.stdout.is_empty() {
            json!({"status":"internal","code":outcome.stderr.trim_end()}).to_string()
        } else {
            outcome.stdout.trim_end_matches('\n').to_string()
        };
        if text.len() > MAX_RESPONSE {
            let mut refusal = Refusal::new("response_too_large")
                .with("bytes", json!(text.len()))
                .with("max_bytes", json!(MAX_RESPONSE));
            if op == Op::Search {
                refusal = refusal.with("retry_with_smaller_limit", json!(true));
            }
            let outcome = render(Err(Failure::Refused(refusal)), now);
            return (outcome.stdout.trim_end_matches('\n').to_string(), true);
        }
        (text, is_error)
    }
}

fn send(out: &mut impl Write, value: &Value) -> std::io::Result<()> {
    let mut line = value.to_string();
    line.push('\n');
    out.write_all(line.as_bytes())?;
    out.flush()
}

/// Serve `reader` until its end, answering on `writer`, with the real
/// operations. Returns the final [`Session`].
pub fn serve(
    config: &Config,
    reader: impl BufRead,
    writer: impl Write,
) -> std::io::Result<Session> {
    serve_with(config, reader, writer, &execute)
}

/// [`serve`] with the operation runner given (a test seam).
pub fn serve_with(
    config: &Config,
    mut reader: impl BufRead,
    mut writer: impl Write,
    exec: &Exec<'_>,
) -> std::io::Result<Session> {
    let mut server = Server::new(config.clone(), exec);
    let mut buf = Vec::new();
    loop {
        let response = match read_line(&mut reader, &mut buf, MAX_LINE)? {
            Read::Eof => return Ok(server.session.clone()),
            Read::TooLarge => Some(rpc_error(Value::Null, -32600, "request_too_large")),
            Read::Complete => {
                let line = buf.strip_suffix(b"\r").unwrap_or(&buf);
                if line.iter().all(u8::is_ascii_whitespace) {
                    None
                } else {
                    server.handle_line(line)
                }
            }
        };
        if let Some(value) = response {
            send(&mut writer, &value)?;
        }
    }
}
