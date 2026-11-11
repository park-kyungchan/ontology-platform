//! The command line: argument parsing, stdin, and the only place where the
//! answer line, the exit code and the error text are built.
use crate::backup::{BackupRequest, backup};
use crate::describe::{DescribeRequest, describe};
use crate::get::{GetRequest, get};
use crate::schema::{CheckRequest, InstallRequest, check, install};
use crate::{Answer, Context, Failure, MAX_REQUEST, Refusal, TRUST_LIMIT, check_uid};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::io::Read;
use std::path::PathBuf;

const DEFAULT_STORE: &str = "/var/lib/ontology-platform/ont/ontology.sqlite";
const PRINCIPAL: &str = crate::principal::PRINCIPAL;

/// What the process prints and returns.
#[derive(Debug)]
pub struct Outcome {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}
impl Outcome {
    fn line(value: &Value, code: i32) -> Self {
        Self {
            stdout: format!("{value}\n"),
            stderr: String::new(),
            code,
        }
    }
}

/// Parse a strict JSON request of at most `MAX_REQUEST` bytes.
pub fn parse_request<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, Refusal> {
    if bytes.len() > MAX_REQUEST {
        return Err(Refusal::new("invalid_request"));
    }
    serde_json::from_slice(bytes).map_err(|_| Refusal::new("invalid_request"))
}

struct Cli {
    store: PathBuf,
    store_given: bool,
    now: Option<u64>,
    version: bool,
    source_repository: Option<String>,
    source_commit: Option<String>,
    command: Vec<String>,
}

fn invalid_argument(argument: &str) -> Refusal {
    Refusal::new("invalid_argument").with("argument", json!(argument))
}

fn parse_args(args: &[String]) -> Result<Cli, Refusal> {
    let mut cli = Cli {
        store: PathBuf::from(DEFAULT_STORE),
        store_given: false,
        now: None,
        version: false,
        source_repository: None,
        source_commit: None,
        command: Vec::new(),
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--store" => {
                cli.store = PathBuf::from(it.next().ok_or_else(|| invalid_argument("--store"))?);
                cli.store_given = true;
            }
            "--now" => {
                let v = it.next().ok_or_else(|| invalid_argument("--now"))?;
                cli.now = Some(v.parse::<u64>().map_err(|_| invalid_argument("--now"))?);
            }
            "--source-repository" => {
                cli.source_repository = Some(
                    it.next()
                        .ok_or_else(|| invalid_argument("--source-repository"))?
                        .clone(),
                );
            }
            "--source-commit" => {
                cli.source_commit = Some(
                    it.next()
                        .ok_or_else(|| invalid_argument("--source-commit"))?
                        .clone(),
                );
            }
            "--version" => cli.version = true,
            _ if arg.starts_with("--") => return Err(invalid_argument(arg)),
            _ => cli.command.push(arg.clone()),
        }
    }
    Ok(cli)
}

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Read at most `MAX_REQUEST + 1` bytes; a longer request is refused.
fn read_request(stdin: impl Read) -> Result<Vec<u8>, Refusal> {
    let mut bytes = Vec::new();
    stdin
        .take(MAX_REQUEST as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Refusal::new("invalid_request"))?;
    if bytes.len() > MAX_REQUEST {
        return Err(Refusal::new("invalid_request"));
    }
    Ok(bytes)
}

fn dispatch(cli: &Cli, stdin: impl Read, ctx: &Context) -> Result<Answer, Failure> {
    let cmd: Vec<&str> = cli.command.iter().map(String::as_str).collect();
    let is_install = matches!(cmd.as_slice(), ["schema", "install", _]);
    if !is_install && (cli.source_repository.is_some() || cli.source_commit.is_some()) {
        return Err(invalid_argument("--source-*").into());
    }
    match cmd.as_slice() {
        ["schema", "check", file] => check(
            ctx,
            &CheckRequest {
                file: PathBuf::from(file),
            },
        ),
        ["schema", "install", file] => install(
            ctx,
            &InstallRequest {
                file: PathBuf::from(file),
                source_repository: cli.source_repository.clone(),
                source_commit: cli.source_commit.clone(),
            },
        ),
        ["describe"] => describe(ctx, &DescribeRequest::parse(&read_request(stdin)?)?),
        ["get"] => get(ctx, &GetRequest::parse(&read_request(stdin)?)?),
        ["backup"] => backup(ctx, &BackupRequest::default()),
        _ => Err(invalid_argument("command").into()),
    }
}

fn render(result: Result<Answer, Failure>, now: u64) -> Outcome {
    match result {
        Ok(Answer::Plain(value)) => Outcome::line(&value, 0),
        Ok(Answer::Enveloped {
            pins,
            revision,
            result,
        }) => {
            let schema: Map<String, Value> = pins
                .iter()
                .map(|(d, p)| (d.clone(), json!({"version":p.version,"sha256":p.sha256})))
                .collect();
            Outcome::line(
                &json!({
                    "status":"ok",
                    "authorization_revision":revision,
                    "now":now,
                    "schema":schema,
                    "trust_limit":TRUST_LIMIT,
                    "result":result
                }),
                0,
            )
        }
        Err(Failure::Refused(refusal)) => {
            let mut body = Map::new();
            body.insert("status".into(), json!("refused"));
            body.insert("code".into(), json!(refusal.code));
            body.extend(refusal.extra);
            body.entry("trust_limit").or_insert(json!(TRUST_LIMIT));
            Outcome::line(&Value::Object(body), 2)
        }
        Err(Failure::Internal(i)) => Outcome {
            stdout: String::new(),
            stderr: format!("{}@{}:{}\n", i.code, i.file, i.line),
            code: 1,
        },
    }
}

/// Run one command. Exit 0 ok, 2 refused (input, admission, busy store),
/// 1 internal (a damaged or foreign store, an I/O failure).
pub fn run(args: &[String], stdin: impl Read, real_uid: u32, effective_uid: u32) -> Outcome {
    let cli = match parse_args(args) {
        Ok(c) => c,
        Err(r) => return render(Err(r.into()), 0),
    };
    if cli.version && cli.command.is_empty() {
        return Outcome {
            stdout: format!(
                "ont {} {}\n",
                env!("CARGO_PKG_VERSION"),
                env!("ONT_BUILD_REVISION")
            ),
            stderr: String::new(),
            code: 0,
        };
    }
    if cli.now.is_some() && !cli.store_given {
        return render(Err(Refusal::new("now_requires_store").into()), 0);
    }
    if let Err(code) = check_uid(real_uid, effective_uid) {
        return render(Err(Refusal::new(code).into()), 0);
    }
    let now = cli.now.unwrap_or_else(now_seconds);
    let ctx = Context {
        store: cli.store.clone(),
        now,
        principal: PRINCIPAL.to_string(),
    };
    render(dispatch(&cli, stdin, &ctx), now)
}
