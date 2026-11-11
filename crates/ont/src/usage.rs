//! The command/option admission table is also the JSON usage source.
use crate::Refusal;
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    SchemaCheck,
    SchemaInstall,
    Describe,
    Get,
    Backup,
    Index,
    Search,
    Render,
    ActionList,
    ActionValidate,
    ActionApply,
    Mcp,
}
struct Spec {
    command: Command,
    words: &'static [&'static str],
    files: &'static [usize],
    form: &'static str,
    options: &'static [&'static str],
    stdin: bool,
    fields: &'static str,
    example: &'static str,
}
use Command::*;
const SPECS: &[Spec] = &[
    Spec {
        command: SchemaCheck,
        words: &["schema", "check"],
        files: &[1],
        form: "schema check FILE",
        options: &[],
        stdin: false,
        fields: "",
        example: "ont schema check /tmp/work.schema.json",
    },
    Spec {
        command: SchemaInstall,
        words: &["schema", "install"],
        files: &[0, 1],
        form: "schema install FILE | schema install --from CHECKOUT --commit SHA --path PATH",
        options: &[
            "--source-repository",
            "--source-commit",
            "--from",
            "--commit",
            "--path",
        ],
        stdin: false,
        fields: "",
        example: "ont schema install --from /repo --commit <40-lowercase-hex> --path ontology/work.schema.json",
    },
    Spec {
        command: Describe,
        words: &["describe"],
        files: &[0],
        form: "describe",
        options: &[],
        stdin: true,
        fields: "op?: describe|null; domain?: string|null; object_type?: string|null; lists views and args",
        example: "echo '{}' | ont describe",
    },
    Spec {
        command: Get,
        words: &["get"],
        files: &[0],
        form: "get",
        options: &[],
        stdin: true,
        fields: "op?: get|null; id: {object_type:string,namespace:{instance:string,account:string},native_key:string}",
        example: "echo '{\"id\":{\"object_type\":\"task\",\"namespace\":{\"instance\":\"local\",\"account\":\"root\"},\"native_key\":\"demo\"}}' | ont get",
    },
    Spec {
        command: Backup,
        words: &["backup"],
        files: &[0],
        form: "backup",
        options: &[],
        stdin: false,
        fields: "",
        example: "ont backup",
    },
    Spec {
        command: Index,
        words: &["index"],
        files: &[0],
        form: "index [--source NAME]...",
        options: &["--source"],
        stdin: false,
        fields: "",
        example: "ont index --source task_states",
    },
    Spec {
        command: Search,
        words: &["search"],
        files: &[0],
        form: "search",
        options: &[],
        stdin: true,
        fields: "op?: search|null; object_type: string; where?: filter|null; limit?: null or integer 1-1000; continuation?: null or string from previous result. Filter: eq|ne|lt|le|gt|ge|prefix [property,value], in [property,[values]], unresolved property, all|any|none [filters]",
        example: "echo '{\"object_type\":\"task\",\"limit\":10}' | ont search",
    },
    Spec {
        command: Render,
        words: &["render"],
        files: &[0],
        form: "render | render --check FILE [--expect-blocks N] | render --write FILE --section VIEW --preimage-dir DIR [--arg NAME=VALUE]...",
        options: &[
            "--check",
            "--expect-blocks",
            "--write",
            "--section",
            "--preimage-dir",
            "--arg",
        ],
        stdin: true,
        fields: "stdin only without options: op?: render|null; view: string from installed schema; args?: {name:string}",
        example: "ont render --check BOARD.md --expect-blocks 2",
    },
    Spec {
        command: ActionList,
        words: &["action", "list"],
        files: &[0],
        form: "action list",
        options: &["--client"],
        stdin: true,
        fields: "op?: list_actions|null; domain?: string|null",
        example: "echo '{}' | ont action list",
    },
    Spec {
        command: ActionValidate,
        words: &["action", "validate"],
        files: &[0],
        form: "action validate",
        options: &["--client"],
        stdin: true,
        fields: "op?: validate_action|null; action_type: string; parameters: object (names/types from action list); agent?,session?: null or 1-64 of ASCII letters,digits,._:-; request_id?: null or 1-64 of ASCII letters,digits,._-; expected?: null or [{id,revision:string}]",
        example: "echo '{\"action_type\":\"create_tag\",\"parameters\":{\"label\":\"demo\"},\"request_id\":\"demo-1\"}' | ont action validate",
    },
    Spec {
        command: ActionApply,
        words: &["action", "apply"],
        files: &[0],
        form: "action apply",
        options: &["--client"],
        stdin: true,
        fields: "op?: apply_action|null; action_type: string; parameters: object (action list); agent?,session?: null or 1-64 of ASCII letters,digits,._:-; request_id?: null or 1-64 of ASCII letters,digits,._- (reuse unchanged on retry); expected?: null or [{id,revision:string}] from validate basis; id form as get",
        example: "echo '{\"action_type\":\"create_tag\",\"parameters\":{\"label\":\"demo\"},\"request_id\":\"demo-1\"}' | ont action apply",
    },
    Spec {
        command: Mcp,
        words: &["mcp"],
        files: &[0],
        form: "mcp [--client NAME]",
        options: &["--client"],
        stdin: false,
        fields: "newline JSON-RPC 2.0: initialize, ping, tools/list, tools/call; discover tool request fields with tools/list",
        example: "ont mcp --client codex",
    },
];
pub(crate) const OPTIONS: &[(&str, &str)] = &[
    ("--store", "PATH"),
    ("--now", "U64 (requires --store)"),
    (
        "--client",
        "NAME: ASCII lowercase letter/digit then lowercase letters/digits/'-', 1-32 bytes",
    ),
    (
        "--source-repository",
        "OWNER/REPO: nonempty parts of ASCII letters/digits/._-",
    ),
    ("--source-commit", "40 ASCII hex (0-9, a-f, A-F)"),
    ("--from", "CHECKOUT"),
    ("--commit", "40 lowercase hex"),
    (
        "--path",
        "nonempty relative PATH without a parent (..) component",
    ),
    ("--source", "NAME (repeatable)"),
    ("--write", "FILE"),
    ("--check", "FILE"),
    ("--section", "VIEW"),
    ("--preimage-dir", "DIR"),
    ("--expect-blocks", "N: unsigned integer"),
    ("--arg", "NAME=VALUE (repeatable, unique names)"),
    ("--version", ""),
    ("--help", ""),
    ("-h", ""),
];
pub(crate) fn option(name: &str) -> Option<&'static str> {
    OPTIONS.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
}
pub(crate) fn resolve(words: &[String]) -> Result<Command, Refusal> {
    SPECS
        .iter()
        .find(|s| {
            words.starts_with(&s.words.iter().map(|w| w.to_string()).collect::<Vec<_>>())
                && s.files.contains(&words.len().saturating_sub(s.words.len()))
        })
        .map(|s| s.command)
        .ok_or_else(|| invalid("command", "a command and argument form listed by ont help"))
}
pub(crate) fn allows(command: Command, option: &str) -> bool {
    matches!(option, "--store" | "--now" | "--version" | "--help" | "-h")
        || SPECS
            .iter()
            .any(|s| s.command == command && s.options.contains(&option))
}
pub(crate) fn invalid(field: &str, rule: &str) -> Refusal {
    Refusal::input("invalid_argument", field, rule)
        .with("argument", json!(field))
        .with("hint", json!("ont help"))
}
pub(crate) fn help(words: &[String]) -> Result<Value, Refusal> {
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let specs: Vec<_> = SPECS
        .iter()
        .filter(|s| s.words.starts_with(&words))
        .collect();
    if specs.is_empty() {
        return Err(invalid("command", "a command listed by ont help"));
    }
    let commands: Vec<Value> = specs
        .iter()
        .map(|s| {
            json!({
                "command":s.words.join(" "),"form":s.form,"options":s.options,
                "stdin_json":s.stdin,"fields":s.fields,"example":s.example
            })
        })
        .collect();
    let options: serde_json::Map<_, _> = OPTIONS
        .iter()
        .filter(|(name, _)| words.is_empty() || specs.iter().any(|s| allows(s.command, name)))
        .map(|(n, v)| (n.to_string(), json!(v)))
        .collect();
    Ok(
        json!({"status":"ok","commands":commands,"options":options,"help":"ont help | ont --help | ont -h | ont COMMAND --help"}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn usage_and_dispatch_have_the_same_commands_and_options() {
        let help = help(&[]).unwrap_or(Value::Null);
        assert_eq!(help["commands"].as_array().map(Vec::len), Some(SPECS.len()));
        assert!(help.to_string().len() + 1 < 6000);
        for spec in SPECS {
            for files in spec.files {
                let mut words: Vec<String> = spec.words.iter().map(|s| s.to_string()).collect();
                words.extend((0..*files).map(|_| "FILE".to_string()));
                assert_eq!(resolve(&words).ok(), Some(spec.command));
            }
            for (option, _) in OPTIONS {
                assert_eq!(
                    allows(spec.command, option),
                    matches!(*option, "--store" | "--now" | "--version" | "--help" | "-h")
                        || spec.options.contains(option)
                );
                assert!(help["options"].get(option).is_some());
            }
        }
        assert!(resolve(&["unknown".into()]).is_err());
        assert!(option("--unknown").is_none());
        assert_eq!(crate::input::pointer("/args", "a/b~c"), "/args/a~1b~0c");
    }
}
