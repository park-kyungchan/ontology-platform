//! The command line: argument parsing, stdin, and the only place where the
//! answer line, the exit code and the error text are built.
use crate::action::{
    ApplyActionRequest, ListActionsRequest, ValidateActionRequest, apply_action, list_actions,
    validate_action,
};
use crate::backup::{BackupRequest, backup};
use crate::describe::{DescribeRequest, describe};
use crate::get::{GetRequest, get};
use crate::index::{IndexRequest, index};
use crate::schema::{
    CheckRequest, CommitInstallRequest, InstallRequest, check, install, install_commit,
};
use crate::search::{SearchRequest, search};
use crate::{
    Answer, Context, DEFAULT_STORE, Failure, MAX_REQUEST, Refusal, TRUST_LIMIT, check_uid,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::io::Read;
use std::path::PathBuf;

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
    crate::input::parse(bytes)
}

struct Cli {
    store: PathBuf,
    store_given: bool,
    now: Option<u64>,
    version: bool,
    help: bool,
    given_options: Vec<String>,
    source_repository: Option<String>,
    source_commit: Option<String>,
    from_checkout: Option<String>,
    from_commit: Option<String>,
    from_path: Option<String>,
    sources: Vec<String>,
    client: Option<String>,
    command: Vec<String>,
    render_options: std::collections::BTreeMap<String, String>,
    render_args: std::collections::BTreeMap<String, String>,
}

fn invalid_argument(argument: &str) -> Refusal {
    let rule = match crate::usage::option(argument) {
        Some("") | None => "a command/option combination listed by ont help".to_string(),
        Some(form) => {
            format!("requires {form}; use the option with its command listed by ont help")
        }
    };
    crate::usage::invalid(argument, &rule)
}

fn parse_args(args: &[String]) -> Result<Cli, Refusal> {
    let mut cli = Cli {
        store: PathBuf::from(DEFAULT_STORE),
        store_given: false,
        now: None,
        version: false,
        help: false,
        given_options: Vec::new(),
        source_repository: None,
        source_commit: None,
        from_checkout: None,
        from_commit: None,
        from_path: None,
        sources: Vec::new(),
        client: None,
        command: Vec::new(),
        render_options: Default::default(),
        render_args: Default::default(),
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if arg.starts_with("--") || arg == "-h" {
            if crate::usage::option(arg).is_none() {
                // An unknown token is not copied: it may contain a secret value.
                return Err(crate::usage::invalid(
                    arg.split('=').next().unwrap_or("option"),
                    "an option listed by ont help",
                ));
            }
            cli.given_options.push(arg.clone());
        }
        match arg.as_str() {
            "--store" => {
                cli.store = PathBuf::from(it.next().ok_or_else(|| invalid_argument("--store"))?);
                cli.store_given = true;
            }
            "--now" => {
                let v = it.next().ok_or_else(|| invalid_argument("--now"))?;
                cli.now =
                    Some(v.parse::<u64>().map_err(|_| {
                        crate::usage::invalid("--now", "an unsigned 64-bit integer")
                    })?);
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
            "--from" => {
                cli.from_checkout =
                    Some(it.next().ok_or_else(|| invalid_argument("--from"))?.clone());
            }
            "--commit" => {
                cli.from_commit = Some(
                    it.next()
                        .ok_or_else(|| invalid_argument("--commit"))?
                        .clone(),
                );
            }
            "--path" => {
                cli.from_path = Some(it.next().ok_or_else(|| invalid_argument("--path"))?.clone());
            }
            "--source" => {
                cli.sources.push(
                    it.next()
                        .ok_or_else(|| invalid_argument("--source"))?
                        .clone(),
                );
            }
            "--client" => {
                cli.client = Some(
                    it.next()
                        .ok_or_else(|| invalid_argument("--client"))?
                        .clone(),
                );
            }
            "--write" | "--check" | "--section" | "--preimage-dir" | "--expect-blocks" => {
                let value = it.next().ok_or_else(|| invalid_argument(arg))?;
                if cli
                    .render_options
                    .insert(arg.clone(), value.clone())
                    .is_some()
                {
                    return Err(crate::usage::invalid(arg, "option must occur only once"));
                }
            }
            "--arg" => {
                let value = it.next().ok_or_else(|| invalid_argument(arg))?;
                let (name, value) = value.split_once('=').ok_or_else(|| invalid_argument(arg))?;
                if cli.render_args.insert(name.into(), value.into()).is_some() {
                    return Err(crate::usage::invalid(
                        "--arg",
                        "NAME=VALUE with each NAME supplied once",
                    ));
                }
            }
            "--version" => cli.version = true,
            "--help" | "-h" => cli.help = true,
            _ if arg.starts_with("--") => return Err(invalid_argument(arg)),
            _ => cli.command.push(arg.clone()),
        }
    }
    Ok(cli)
}

pub(crate) fn now_seconds() -> u64 {
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
        .map_err(|_| {
            Refusal::input(
                "invalid_request",
                "",
                "readable JSON request of at most 65536 bytes",
            )
        })?;
    if bytes.len() > MAX_REQUEST {
        return Err(Refusal::input(
            "invalid_request",
            "",
            "readable JSON request of at most 65536 bytes",
        ));
    }
    // Let typed parsing report the first required field for an empty request;
    // optional-only requests use their ordinary defaults.
    if bytes.iter().all(u8::is_ascii_whitespace) {
        bytes = b"{}".to_vec();
    }
    Ok(bytes)
}

/// The four operations that take a JSON request, by name: the one place that
/// maps a request to its typed `parse` and public operation function. The
/// subcommands and the MCP tools both come through here.
pub fn execute(op: Op, request: &[u8], ctx: &Context) -> Result<Answer, Failure> {
    match op {
        Op::Describe => describe(ctx, &DescribeRequest::parse(request)?),
        Op::Get => get(ctx, &GetRequest::parse(request)?),
        Op::Search => search(ctx, &SearchRequest::parse(request)?),
        Op::Around => crate::around::around(ctx, &crate::around::AroundRequest::parse(request)?),
        Op::SetRead => {
            let value: Value = parse_request(request)?;
            match value.get("op").and_then(Value::as_str) {
                Some("search_around") => execute(Op::Around, request, ctx),
                _ => execute(Op::Search, request, ctx),
            }
        }
        Op::Index => index(ctx, &IndexRequest::parse(request)?),
    }
}

/// An operation that takes a JSON request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Describe,
    Get,
    Search,
    Around,
    SetRead,
    Index,
}

fn dispatch(cli: &Cli, stdin: impl Read, ctx: &Context) -> Result<Answer, Failure> {
    let command = crate::usage::resolve(&cli.command)?;
    for option in &cli.given_options {
        if !crate::usage::allows(command, option) {
            return Err(crate::usage::invalid(
                option,
                "option must be used with its command listed by ont help",
            )
            .into());
        }
    }
    let is_file_install = command == crate::usage::Command::SchemaInstall && cli.command.len() == 3;
    let is_commit_install =
        command == crate::usage::Command::SchemaInstall && cli.command.len() == 2;
    let from_option = if cli.from_checkout.is_some() {
        Some("--from")
    } else if cli.from_commit.is_some() {
        Some("--commit")
    } else if cli.from_path.is_some() {
        Some("--path")
    } else {
        None
    };
    if is_file_install && let Some(option) = from_option {
        return Err(crate::usage::invalid(
            option,
            "commit install options require schema install without FILE",
        )
        .into());
    }
    if is_commit_install && (cli.source_repository.is_some() || cli.source_commit.is_some()) {
        return Err(crate::usage::invalid(
            if cli.source_repository.is_some() {
                "--source-repository"
            } else {
                "--source-commit"
            },
            "provenance option requires schema install FILE, without --from",
        )
        .into());
    }
    use crate::usage::Command::*;
    match command {
        SchemaCheck => {
            let file = &cli.command[2];
            let answer = check(
                ctx,
                &CheckRequest {
                    file: PathBuf::from(file),
                },
            )?;
            crate::render::check_view_names(ctx, std::path::Path::new(file))?;
            Ok(answer)
        }
        SchemaInstall if cli.command.len() == 3 => install(
            ctx,
            &InstallRequest {
                file: PathBuf::from(&cli.command[2]),
                source_repository: cli.source_repository.clone(),
                source_commit: cli.source_commit.clone(),
            },
        ),
        SchemaInstall => {
            let (Some(checkout), Some(commit), Some(path)) = (
                cli.from_checkout.clone(),
                cli.from_commit.clone(),
                cli.from_path.clone(),
            ) else {
                let missing = if cli.from_checkout.is_none() {
                    "--from"
                } else if cli.from_commit.is_none() {
                    "--commit"
                } else {
                    "--path"
                };
                return Err(crate::usage::invalid(missing, "schema install from a commit requires --from CHECKOUT, --commit SHA and --path PATH").into());
            };
            install_commit(
                ctx,
                &CommitInstallRequest {
                    checkout: PathBuf::from(checkout),
                    commit,
                    path,
                },
            )
        }
        Describe => execute(Op::Describe, &read_request(stdin)?, ctx),
        Get => execute(Op::Get, &read_request(stdin)?, ctx),
        Backup => backup(ctx, &BackupRequest::default()),
        Index => {
            let request = if cli.sources.is_empty() {
                json!({"op":"index"})
            } else {
                json!({"op":"index","sources":cli.sources})
            };
            execute(Op::Index, request.to_string().as_bytes(), ctx)
        }
        Search => execute(Op::Search, &read_request(stdin)?, ctx),
        Around => execute(Op::Around, &read_request(stdin)?, ctx),
        Render => {
            use crate::render::{RenderRequest, write_file};
            let opts = &cli.render_options;
            if opts.is_empty() && cli.render_args.is_empty() {
                return crate::render::render(ctx, &RenderRequest::parse(&read_request(stdin)?)?);
            }
            if (opts.len() == 1 || (opts.len() == 2 && opts.contains_key("--expect-blocks")))
                && cli.render_args.is_empty()
                && let Some(file) = opts.get("--check")
            {
                let expected = opts
                    .get("--expect-blocks")
                    .map(|n| {
                        n.parse::<usize>().map_err(|_| {
                            crate::usage::invalid(
                                "--expect-blocks",
                                "an unsigned integer fitting usize",
                            )
                        })
                    })
                    .transpose()?;
                return crate::render::check_file_count(ctx, std::path::Path::new(file), expected);
            }
            if opts.len() == 3
                && let (Some(file), Some(view), Some(dir)) = (
                    opts.get("--write"),
                    opts.get("--section"),
                    opts.get("--preimage-dir"),
                )
            {
                return write_file(
                    ctx,
                    std::path::Path::new(file),
                    &RenderRequest {
                        op: None,
                        view: view.clone(),
                        args: cli.render_args.clone(),
                    },
                    std::path::Path::new(dir),
                );
            }
            let missing = if opts.contains_key("--check") {
                "--check"
            } else if !opts.contains_key("--write") {
                "--write"
            } else if !opts.contains_key("--section") {
                "--section"
            } else {
                "--preimage-dir"
            };
            Err(crate::usage::invalid(missing, "use --check FILE [--expect-blocks N] alone, or --write FILE --section VIEW --preimage-dir DIR [--arg NAME=VALUE]...").into())
        }
        ActionList => list_actions(ctx, &ListActionsRequest::parse(&read_request(stdin)?)?),
        ActionValidate => {
            validate_action(ctx, &ValidateActionRequest::parse(&read_request(stdin)?)?)
        }
        ActionApply => apply_action(ctx, &ApplyActionRequest::parse(&read_request(stdin)?)?),
        Mcp => Err(invalid_argument("command").into()),
    }
}

fn envelope(
    pins: &std::collections::BTreeMap<String, ont_core::schema::SchemaPin>,
    revision: u64,
    result: Value,
    now: u64,
    status: &str,
    code: i32,
) -> Outcome {
    let schema: Map<String, Value> = pins
        .iter()
        .map(|(d, p)| (d.clone(), json!({"version":p.version,"sha256":p.sha256})))
        .collect();
    Outcome::line(
        &json!({
            "status":status,
            "authorization_revision":revision,
            "now":now,
            "schema":schema,
            "trust_limit":TRUST_LIMIT,
            "result":result
        }),
        code,
    )
}

pub(crate) fn render(result: Result<Answer, Failure>, now: u64) -> Outcome {
    match result {
        Ok(Answer::Plain(value)) => Outcome::line(&value, 0),
        Ok(Answer::Enveloped {
            pins,
            revision,
            result,
        }) => envelope(&pins, revision, result, now, "ok", 0),
        Ok(Answer::Failed {
            pins,
            revision,
            result,
        }) => envelope(&pins, revision, result, now, "source_failed", 2),
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
    if cli.help || cli.command.first().is_some_and(|c| c == "help") {
        let words = if cli.command.first().is_some_and(|c| c == "help") {
            &cli.command[1..]
        } else {
            &cli.command[..]
        };
        return render(
            crate::usage::help(words)
                .map(Answer::Plain)
                .map_err(Failure::from),
            0,
        );
    }
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
        return render(
            Err(Refusal::input(
                "now_requires_store",
                "--store",
                "must be explicitly supplied when --now is used",
            )
            .into()),
            0,
        );
    }
    if let Err(code) = check_uid(real_uid, effective_uid) {
        return render(Err(Refusal::new(code).into()), 0);
    }
    if crate::usage::resolve(&cli.command).is_ok_and(|c| c == crate::usage::Command::Mcp) {
        return run_mcp(&cli, stdin);
    }
    let now = cli.now.unwrap_or_else(now_seconds);
    let is_action_cmd = crate::usage::resolve(&cli.command).is_ok_and(|c| {
        matches!(
            c,
            crate::usage::Command::ActionList
                | crate::usage::Command::ActionValidate
                | crate::usage::Command::ActionApply
        )
    });
    let client = if is_action_cmd {
        match cli.client.as_deref() {
            None => "cli".to_string(),
            Some(c) if crate::mcp::valid_client(c) => c.to_string(),
            Some(_) => return render(Err(Refusal::input("invalid_request", "--client", "1 to 32 ASCII characters: lowercase letter or digit first; then lowercase letters, digits, or hyphen").into()), now),
        }
    } else {
        "cli".to_string()
    };
    let mut ctx = Context::new(cli.store.clone(), now);
    ctx.client = client;
    ctx.agent = std::env::var("ONT_AGENT").ok().filter(|s| !s.is_empty());
    render(dispatch(&cli, stdin, &ctx), now)
}

/// `ont mcp [--client <name>]`: the server loop of [`crate::mcp`] over `stdin`
/// and the process's standard output (the one subcommand that writes while it
/// runs). An explicit client name is checked before anything is read; absent
/// --client leaves attribution to initialize clientInfo.name.
fn run_mcp(cli: &Cli, stdin: impl Read) -> Outcome {
    if let Some(option) = cli
        .given_options
        .iter()
        .find(|option| !crate::usage::allows(crate::usage::Command::Mcp, option))
    {
        return render(
            Err(crate::usage::invalid(
                option,
                "option is not allowed with mcp; use ont mcp --help",
            )
            .into()),
            0,
        );
    }
    let client = match cli.client.as_deref() {
        None => "",
        Some(client) if crate::mcp::valid_client(client) => client,
        Some(_) => return render(Err(Refusal::input("invalid_request", "--client", "1 to 32 ASCII characters: lowercase letter or digit first; then lowercase letters, digits, or hyphen").into()), 0),
    };
    crate::mcp::install_panic_recorder();
    let config = crate::mcp::Config {
        store: cli.store.clone(),
        now: cli.now,
        client: client.to_string(),
    };
    let mut out = std::io::stdout().lock();
    match crate::mcp::serve(&config, std::io::BufReader::new(stdin), &mut out) {
        Ok(_) => Outcome {
            stdout: String::new(),
            stderr: String::new(),
            code: 0,
        },
        Err(_) => render(Err(Failure::Internal(crate::internal!("mcp_io_failed"))), 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_documented_option_is_parsed_and_unknown_options_refuse() {
        for (name, form) in crate::usage::OPTIONS {
            let mut args = vec![name.to_string()];
            if !form.is_empty() {
                args.push(
                    match *name {
                        "--now" | "--expect-blocks" => "0",
                        "--arg" => "name=value",
                        _ => "value",
                    }
                    .into(),
                );
            }
            assert!(parse_args(&args).is_ok(), "{name}");
        }
        assert!(parse_args(&["--unknown".into()]).is_err());
    }
    #[test]
    fn schema_check_keeps_single_hyphen_positional_file() {
        let args = ["schema", "check", "-schema.json"].map(str::to_string);
        assert_eq!(
            parse_args(&args)
                .unwrap_or_else(|e| panic!("{e:?}"))
                .command,
            args
        );
    }
    #[test]
    fn schema_install_keeps_single_hyphen_positional_file() {
        let args = ["schema", "install", "-schema.json"].map(str::to_string);
        assert_eq!(
            parse_args(&args)
                .unwrap_or_else(|e| panic!("{e:?}"))
                .command,
            args
        );
    }
}
