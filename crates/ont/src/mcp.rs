//! `ont mcp`: a Model Context Protocol server over stdio for `describe`,
//! `get`, `search`, `index` and actions.
//!
//! Wire: newline-delimited JSON-RPC 2.0, one request per line on stdin, one
//! JSON line per response on stdout (nothing else is ever written there).
//! No listener, no threads. State kept between calls: the negotiated
//! protocol version, `--client` and the client's `clientInfo.name`
//! ([`Session`], held by [`Server`]); actions record caller attribution.
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
#[doc(hidden)]
pub type Exec<'a> = dyn Fn(Op, &[u8], &Context) -> Result<Answer, Failure> + 'a;

thread_local! {
    static LAST_PANIC: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    static IN_CALL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Marks the thread as inside a tool call until dropped.
struct InCall;
impl InCall {
    fn enter() -> Self {
        IN_CALL.with(|c| c.set(true));
        Self
    }
}
impl Drop for InCall {
    fn drop(&mut self) {
        IN_CALL.with(|c| c.set(false));
    }
}

/// The part of a panic location's file from `crates/` on (the file is
/// repository-relative, or absolute with a `/crates/` component); any other
/// location (a dependency, the standard library) is `external`.
fn trim_panic_file(file: &str) -> &str {
    if file.starts_with("crates/") {
        file
    } else if let Some(i) = file.find("/crates/") {
        &file[i + 1..]
    } else {
        "external"
    }
}

fn panic_location(file: &str, line: u32) -> String {
    format!("{}:{line}", trim_panic_file(file))
}

/// Replace the panic hook. Inside a tool call it records where the panic
/// happened (`file:line`, trimmed by [`trim_panic_file`]) and prints
/// nothing, so the call is answered as an internal failure with a located
/// code. Anywhere else (framing, `initialize`, the writer) it prints one
/// line `panic@<file>:<line>` to stderr and the process ends nonzero as a
/// panic does. Process-wide; the server installs it once.
pub fn install_panic_recorder() {
    std::panic::set_hook(Box::new(|info| {
        let at = info.location().map_or("external:0".to_string(), |l| {
            panic_location(l.file(), l.line())
        });
        if IN_CALL.with(std::cell::Cell::get) {
            LAST_PANIC.with(|last| *last.borrow_mut() = Some(at));
        } else {
            eprintln!("panic@{at}");
        }
    }));
}

fn take_panic_location() -> String {
    LAST_PANIC
        .with(|last| last.borrow_mut().take())
        .unwrap_or_else(|| panic_location(file!(), line!()))
}

#[derive(Clone, Copy)]
enum ToolOp {
    Read(Op),
    ListActions,
    Validate,
    Apply,
}

struct Tool {
    op: ToolOp,
    spec: Value,
}

/// The single table used for discovery and dispatch.
fn tools() -> Vec<Tool> {
    let s = json!({"type":"string"});
    let request_id = json!({"type":"string","pattern":"^[A-Za-z0-9._-]{1,64}$"});
    let actor = json!({"type":"string","pattern":"^[A-Za-z0-9._:-]{1,64}$"});
    let id = json!({"type":"object","required":["object_type","namespace","native_key"],
        "properties":{"object_type":s,"native_key":s,
            "namespace":{"type":"object","required":["instance","account"],
                "properties":{"instance":s,"account":s}}}});
    let expected = json!({"type":"array","items":{"type":"object","required":["id","revision"],"properties":{"id":id,"revision":s}}});
    vec![
        Tool {
            op: ToolOp::Read(Op::Describe),
            spec: json!({"name":"ont_describe",
         "description":"Call first: object types and properties, links, actions, views, functions and sources this caller may read. Includes schema pins and authorization_revision.",
         "inputSchema":{"type":"object","properties":{"domain":s,"object_type":s}}}),
        },
        Tool {
            op: ToolOp::Read(Op::Get),
            spec: json!({"name":"ont_get",
         "description":"One object's full history. id: objects[].id from ont_search. Hidden and missing both return unavailable. Includes schema pins and authorization_revision.",
         "inputSchema":{"type":"object","required":["id"],"properties":{"id":id}}}),
        },
        Tool {
            op: ToolOp::Read(Op::Search),
            spec: json!({"name":"ont_search",
         "description":"Filter objects of one type. where, one operator key per object, e.g. {\"eq\":[\"status\",\"open\"]}; also ne lt le gt ge prefix [property,value], in [property,[values]], unresolved property, all|any|none [filters]. limit 1-1000 (default 100); continuation pages. Always carries schema pins, authorization_revision and basis (scanned, matched, stale, unresolved_excluded, qualified).",
         "inputSchema":{"type":"object","required":["object_type"],
            "properties":{"object_type":s,"where":{"type":"object"},
                "limit":{"type":"integer","minimum":1,"maximum":1000},"continuation":s}}}),
        },
        Tool {
            op: ToolOp::Read(Op::Index),
            spec: json!({"name":"ont_index",
         "description":"Index installed sources and append what changed; counts per source; failed source gives isError. sources: names (default all). One run at a time (index_busy). Includes schema pins and authorization_revision.",
         "inputSchema":{"type":"object","properties":{"sources":{"type":"array","items":s,"minItems":1}}}}),
        },
        Tool {
            op: ToolOp::ListActions,
            spec: json!({
                "name":"ont_action_list",
                "description":"List action types with parameters and criteria codes; optional domain from ont_describe. Use this list for action_type and parameter names in ont_action_validate and ont_action_apply. Object ids come from ont_search.",
                "inputSchema":{"type":"object","additionalProperties":false,"properties":{"domain":s}}
            }),
        },
        Tool {
            op: ToolOp::Validate,
            spec: json!({
                "name":"ont_action_validate",
                "description":"Preview without writing. action_type/parameters: ont_action_list; reference values: objects[].id from ont_search. If result.valid, apply the same arguments with result.basis as expected; otherwise fix failed criteria. agent/session are optional.",
                "inputSchema":{"type":"object","additionalProperties":false,"required":["action_type","parameters"],
                    "properties":{"action_type":s,"parameters":{"type":"object"},"request_id":request_id,"agent":actor,"session":actor,"expected":expected}}
            }),
        },
        Tool {
            op: ToolOp::Apply,
            spec: json!({
                "name":"ont_action_apply",
                "description":"Apply action_type/parameters from ont_action_list; reference values: objects[].id from ont_search; expected: validate result.basis. Every edits_log value means the change was made. On applied or already_applied continue; on refusal fix its code.",
                "inputSchema":{"type":"object","additionalProperties":false,"required":["action_type","parameters","request_id"],
                    "properties":{"action_type":s,"parameters":{"type":"object"},"request_id":request_id,"agent":actor,"session":actor,"expected":expected}}
            }),
        },
    ]
}

/// Constant discovery, independent of installed schemas.
pub fn tools_list() -> Value {
    json!({"tools":tools().into_iter().map(|t| t.spec).collect::<Vec<_>>()})
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
#[doc(hidden)]
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
        let Some(tool) = tools().into_iter().find(|t| t.spec["name"] == *name) else {
            return invalid(id);
        };
        let arguments = match params.get("arguments") {
            None => Value::Object(Map::new()),
            Some(v @ Value::Object(_)) => v.clone(),
            Some(_) => return invalid(id),
        };
        let (text, is_error) = self.call(tool.op, &arguments);
        rpc_result(
            id,
            json!({"content":[{"type":"text","text":text}],"isError":is_error}),
        )
    }

    /// One tool call: the operation of the subcommand, rendered as the
    /// subcommand renders it. Returns the `text` and `isError`. A panic is
    /// contained here and answered as an internal failure of this call.
    fn call(&self, op: ToolOp, arguments: &Value) -> (String, bool) {
        let now = self.config.now.unwrap_or_else(now_seconds);
        let mut ctx = Context::new(self.config.store.clone(), now);
        ctx.client = if self.config.client.is_empty() {
            self.session
                .client_info_name
                .as_deref()
                .filter(|name| valid_client(name))
                .unwrap_or("mcp")
                .to_string()
        } else {
            self.config.client.clone()
        };
        // Keep the CLI action namespace distinct even for a literal "cli" name.
        if ctx.client == "cli" {
            ctx.client = "mcp".to_string();
        }
        ctx.agent = arguments
            .get("agent")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| std::env::var("ONT_AGENT").ok().filter(|s| !s.is_empty()));
        ctx.session = arguments
            .get("session")
            .and_then(Value::as_str)
            .map(str::to_string);
        let request = arguments.to_string();
        let guard = InCall::enter();
        let ran = catch_unwind(AssertUnwindSafe(|| {
            let result = match op {
                ToolOp::Read(op) => (self.exec)(op, request.as_bytes(), &ctx),
                ToolOp::ListActions => crate::action::ListActionsRequest::parse(request.as_bytes())
                    .map_err(Failure::from)
                    .and_then(|req| crate::action::list_actions(&ctx, &req)),
                ToolOp::Validate => check_request_id(arguments, false)
                    .and_then(|()| {
                        crate::action::ValidateActionRequest::parse(request.as_bytes())
                            .map_err(Failure::from)
                    })
                    .and_then(|mut req| {
                        req.agent = None;
                        req.session = None;
                        crate::action::validate_action(&ctx, &req)
                    }),
                ToolOp::Apply => check_request_id(arguments, true).and_then(|()| {
                    crate::action::ApplyActionRequest::parse(request.as_bytes())
                        .map_err(Failure::from)
                        .and_then(|mut req| {
                            req.agent = None;
                            req.session = None;
                            crate::action::apply_action(&ctx, &req)
                        })
                }),
            };
            render(result, now)
        }));
        drop(guard);
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
            if matches!(op, ToolOp::Apply)
                && !is_error
                && let Ok(answer) = serde_json::from_str::<Value>(&text)
            {
                for field in ["action_id", "outcome"] {
                    if let Some(value) = answer["result"].get(field) {
                        refusal = refusal.with(field, value.clone());
                    }
                }
            }
            if matches!(op, ToolOp::Read(Op::Search)) {
                refusal = refusal.with("retry_with_smaller_limit", json!(true));
            }
            let outcome = render(Err(Failure::Refused(refusal)), now);
            return (outcome.stdout.trim_end_matches('\n').to_string(), true);
        }
        (text, is_error)
    }
}

/// MCP argument feedback for the library's request-id form.
fn check_request_id(arguments: &Value, required: bool) -> Result<(), Failure> {
    match arguments.get("request_id") {
        None if !required => Ok(()),
        Some(Value::String(id))
            if !id.is_empty()
                && id.len() <= 64
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')) =>
        {
            Ok(())
        }
        _ => Err(Refusal::input(
            "invalid_argument",
            "/request_id",
            "1 to 64 ASCII characters: letters, digits, '.', '_', '-'",
        )
        .with("argument", json!("request_id"))
        .with(
            "message",
            json!(
                "request_id must be 1-64 characters of A-Z a-z 0-9 . _ - (^[A-Za-z0-9._-]{1,64}$)"
            ),
        )
        .into()),
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
#[doc(hidden)]
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn panic_files_are_trimmed_to_the_repository_or_external() {
        assert_eq!(
            panic_location("crates/ont/src/get.rs", 7),
            "crates/ont/src/get.rs:7"
        );
        assert_eq!(
            panic_location("/home/x/wt/repo/crates/ont/src/get.rs", 8),
            "crates/ont/src/get.rs:8"
        );
        assert_eq!(
            panic_location("/root/.cargo/registry/src/a-1/src/lib.rs", 9),
            "external:9"
        );
    }

    #[test]
    fn an_over_long_line_is_consumed_to_its_newline_without_being_buffered() {
        let mut data = vec![b'a'; 10_000];
        data.extend_from_slice(b"\nnext\nlast");
        // A reader that hands out 8 bytes at a time.
        let mut reader = std::io::BufReader::with_capacity(8, Cursor::new(data));
        let mut buf = Vec::new();
        assert!(matches!(
            read_line(&mut reader, &mut buf, 16).unwrap(),
            Read::TooLarge
        ));
        assert!(buf.is_empty());
        assert!(buf.capacity() <= 16, "buffered {} bytes", buf.capacity());
        assert!(matches!(
            read_line(&mut reader, &mut buf, 16).unwrap(),
            Read::Complete
        ));
        assert_eq!(buf, b"next");
        assert!(matches!(
            read_line(&mut reader, &mut buf, 16).unwrap(),
            Read::Complete
        ));
        assert_eq!(buf, b"last");
        assert!(matches!(
            read_line(&mut reader, &mut buf, 16).unwrap(),
            Read::Eof
        ));
    }
}
