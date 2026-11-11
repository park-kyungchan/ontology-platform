//! `schema check` and `schema install`.
use crate::principal::fresh_revision;
use crate::refusal::{open_store, st};
use crate::{Answer, Context, Failure, Refusal, TRUST_LIMIT, backup, pin_json, sha256_hex};
use ont_core::schema::Schema;
use ont_store::Store;
use ont_store::schema_mode::InstallArgs;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct CheckRequest {
    pub file: PathBuf,
}
#[derive(Debug, Clone)]
pub struct InstallRequest {
    pub file: PathBuf,
    pub source_repository: Option<String>,
    pub source_commit: Option<String>,
}
#[derive(Debug, Clone)]
pub struct CommitInstallRequest {
    pub checkout: PathBuf,
    pub commit: String,
    pub path: String,
}

/// True when `store` is the default store path: the file form is refused
/// there with `install_requires_commit`; only a merged commit may install.
pub fn install_requires_commit(store: &std::path::Path) -> bool {
    same_store_path(store, std::path::Path::new(crate::DEFAULT_STORE))
}

fn same_store_path(store: &std::path::Path, default: &std::path::Path) -> bool {
    fn resolved(path: &std::path::Path) -> Option<PathBuf> {
        let name = path.file_name()?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        Some(std::fs::canonicalize(parent).ok()?.join(name))
    }
    use std::os::unix::fs::MetadataExt;
    if matches!((std::fs::canonicalize(store), std::fs::canonicalize(default)), (Ok(a), Ok(b)) if a == b)
    {
        return true;
    }
    if let (Ok(a), Ok(b)) = (std::fs::metadata(store), std::fs::metadata(default))
        && (a.dev(), a.ino()) == (b.dev(), b.ino())
    {
        return true;
    }
    store == default || matches!((resolved(store), resolved(default)), (Some(a), Some(b)) if a == b)
}

fn valid_slug(s: &str) -> bool {
    let parts: Vec<&str> = s.split('/').collect();
    parts.len() == 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
}
fn valid_commit(s: &str) -> bool {
    s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}
fn read_schema_file(file: &std::path::Path) -> Result<Vec<u8>, Failure> {
    std::fs::read(file).map_err(|_| {
        Refusal::new("schema_file_unreadable")
            .with("path", json!(file.display().to_string()))
            .into()
    })
}
fn others_for(set: &ont_core::schema::SchemaSet, candidate: &str) -> BTreeSet<String> {
    set.pins()
        .keys()
        .filter(|d| d.as_str() != candidate)
        .filter_map(|d| set.schema(d))
        .flat_map(|s| s.object_types.keys().cloned())
        .collect()
}
fn candidate_domain(bytes: &[u8]) -> String {
    Schema::parse(bytes).map(|s| s.domain).unwrap_or_default()
}

fn check_outcome(
    bytes: &[u8],
    others: &BTreeSet<String>,
    checked_against: Vec<String>,
    store_exists: bool,
) -> Result<Answer, Failure> {
    match Schema::check_bytes(bytes, others) {
        Ok(checked) => {
            let pin = checked.pin();
            Ok(Answer::Plain(json!({
                "status":"ok","clean":true,
                "pin":pin_json(pin),
                "violations":[],
                "checked_against":checked_against,
                "store_exists":store_exists,
                "trust_limit":TRUST_LIMIT
            })))
        }
        Err(violations) => {
            let pin = Schema::parse(bytes)
                .ok()
                .map(|s| json!({"domain":s.domain,"version":s.version,"sha256":sha256_hex(bytes)}));
            Err(schema_input_refusal(bytes, &violations)
                .with("clean", json!(false))
                .with("pin", pin.unwrap_or(Value::Null))
                .with("violations", json!(violations))
                .with("checked_against", json!(checked_against))
                .with("store_exists", json!(store_exists))
                .into())
        }
    }
}

/// Check a schema file against the domains of the store (when it exists).
/// Never creates or writes a store.
pub fn check(ctx: &Context, req: &CheckRequest) -> Result<Answer, Failure> {
    let bytes = read_schema_file(&req.file)?;
    let store_exists = ctx.store.exists();
    let (others, against) = if store_exists {
        let mut store = open_store(&ctx.store, false)?;
        let set = st!(store.schema_set()).clone();
        (
            others_for(&set, &candidate_domain(&bytes)),
            set.pins().keys().cloned().collect(),
        )
    } else {
        (BTreeSet::new(), Vec::new())
    };
    check_outcome(&bytes, &others, against, store_exists)
}

/// Install a schema file. The only operation that may create a store: the
/// file is created with mode 0600 (`create_new`) before SQLite opens it, and
/// only when its parent directory exists.
pub fn install(ctx: &Context, req: &InstallRequest) -> Result<Answer, Failure> {
    install_at_default(ctx, req, std::path::Path::new(crate::DEFAULT_STORE))
}

fn install_at_default(
    ctx: &Context,
    req: &InstallRequest,
    default: &std::path::Path,
) -> Result<Answer, Failure> {
    if same_store_path(&ctx.store, default) {
        return Err(Refusal::input(
            "install_requires_commit",
            "--store",
            "default store schema install requires a merged commit",
        )
        .into());
    }
    if std::fs::symlink_metadata(&ctx.store).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(Refusal::input(
            "store_path_is_symlink",
            "--store",
            "file install requires a store path that is not a symbolic link",
        )
        .into());
    }
    if let Some(repo) = &req.source_repository
        && !valid_slug(repo)
    {
        return Err(Refusal::input("invalid_argument", "--source-repository", "exactly two nonempty slash-separated parts; each contains ASCII letters, digits, or . _ -")
            .with("argument", json!("--source-repository"))
            .into());
    }
    if let Some(commit) = &req.source_commit
        && !valid_commit(commit)
    {
        return Err(Refusal::input(
            "invalid_argument",
            "--source-commit",
            "40 ASCII hexadecimal characters (0-9, a-f, A-F)",
        )
        .with("argument", json!("--source-commit"))
        .into());
    }
    let bytes = read_schema_file(&req.file)?;
    install_bytes(
        ctx,
        &bytes,
        req.source_repository.clone(),
        req.source_commit.clone(),
        None,
    )
}

/// Install the schema at a merged commit of a repository checkout.
pub fn install_commit(ctx: &Context, req: &CommitInstallRequest) -> Result<Answer, Failure> {
    let got = crate::gitread::read_schema(&req.checkout, &req.commit, &req.path)?;
    install_bytes(
        ctx,
        &got.bytes,
        Some(got.repository),
        Some(req.commit.clone()),
        Some(req.path.clone()),
    )
}

fn install_bytes(
    ctx: &Context,
    bytes: &[u8],
    source_repository: Option<String>,
    source_commit: Option<String>,
    source_path: Option<String>,
) -> Result<Answer, Failure> {
    let store_exists = ctx.store.exists();
    let mut store = if store_exists {
        st!(Store::open_schema(&ctx.store))
    } else {
        // Check first; a refused check must not create a store.
        check_outcome(bytes, &BTreeSet::new(), Vec::new(), false)?;
        let parent = ctx.store.parent();
        if parent.is_some_and(|p| !p.as_os_str().is_empty() && !p.exists()) {
            return Err(Refusal::new("store_parent_missing")
                .with("directory", json!(parent.map(|p| p.display().to_string())))
                .into());
        }
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&ctx.store)
        {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => {
                return Err(Failure::Internal(crate::internal!("store_create_failed")));
            }
        }
        st!(Store::open_schema(&ctx.store))
    };
    let set = st!(store.schema_set()).clone();
    let checked = match Schema::check_bytes(bytes, &others_for(&set, &candidate_domain(bytes))) {
        Ok(c) => c,
        Err(_) => {
            return check_outcome(
                bytes,
                &others_for(&set, &candidate_domain(bytes)),
                set.pins().keys().cloned().collect(),
                true,
            );
        }
    };
    let pin = checked.pin().clone();
    let previous = st!(store.schema_pins()).get(&pin.domain).cloned();
    let changed = previous.as_ref() != Some(&pin);
    // Refuse before making a backup; the store repeats this rule in its
    // install transaction as protection against concurrent installs.
    if changed && previous.as_ref().is_some_and(|p| pin.version <= p.version) {
        return Err(Refusal::new("schema_version_conflict").into());
    }
    // Taken before the commit: nothing after `install_schema` can fail the
    // answer of a committed install.
    let revision = fresh_revision()?;
    let mut backup_path: Option<String> = None;
    let mut backup_retention_failure: Option<String> = None;
    if store_exists && changed && !st!(store.schema_pins()).is_empty() {
        let taken = backup::take(&mut store, &ctx.store, ctx.now)?;
        backup_path = Some(taken.path.display().to_string());
        backup_retention_failure = taken.retention_failure;
    }
    st!(store.install_schema_with_provenance(
        bytes,
        InstallArgs {
            installed_at: ctx.now,
            source_repository: source_repository.clone(),
            source_commit: source_commit.clone(),
        },
        source_path.clone(),
    ));
    Ok(Answer::Enveloped {
        pins: st!(store.schema_pins()),
        revision,
        result: json!({
            "domain":pin.domain,
            "previous_version":previous.map(|p| p.version),
            "version":pin.version,
            "pin":pin_json(&pin),
            "changed":changed,
            "backup":backup_path,
            "backup_retention_failure":backup_retention_failure,
            "source_repository":source_repository,
            "source_commit":source_commit,
            "source_path":source_path
        }),
    })
}

/// Preserve schema violation codes/pointers; add the same outer input feedback
/// as command requests. Structural errors reuse serde's expected form.
pub(crate) fn schema_input_refusal(
    bytes: &[u8],
    violations: &[ont_core::schema::Violation],
) -> Refusal {
    if let Err(e) = ont_core::input::parse_json::<Schema>(bytes) {
        if e.rule != "required field"
            && matches!(
                e.field.split('/').collect::<Vec<_>>().as_slice(),
                ["", "object_types", _, "properties", _, "type"]
                    | ["", "action_types", _, "parameters", _, "type"]
            )
        {
            return Refusal::input(
                "schema_violation",
                &e.field,
                "type must be string, integer, boolean, decimal, timestamp, date, {ref:type_name}, {array:type}, {struct:{field:type}} or {value_type:name}",
            );
        }
        return Refusal::input("schema_violation", &e.field, &e.rule);
    }
    let Some(v) = violations.first() else {
        return Refusal::input("schema_violation", "", "a valid ontology-schema/v1 schema");
    };
    let parts: Vec<_> = v.pointer.split('/').collect();
    let parameter = matches!(
        parts.as_slice(),
        ["", "action_types", _, "parameters", _, ..]
    );
    let primary_key = matches!(parts.as_slice(), ["", "object_types", _, "primary_key", _]);
    let last = parts.last().copied().unwrap_or("");
    let rule = match v.code {
        "invalid_format" => "must be ontology-schema/v1",
        "invalid_version" => "positive unsigned version integer",
        "invalid_namespace" => {
            "namespace component must be a nonempty string of at most 512 UTF-8 bytes"
        }
        "duplicate_link_name" => "forward and reverse link names must be unique across this schema",
        "invalid_value_type_definition" => match last {
            "enum" => "enum must be nonempty and every value must match the declared scalar base",
            "pattern" => {
                "pattern requires string base and a compilable whole-value regular expression"
            }
            "min" => {
                "bound must match the non-boolean scalar base (decimal/date use their valid string forms)"
            }
            _ => {
                "bound must match the non-boolean scalar base; when min and max are valid, max must be at least min"
            }
        },
        "invalid_from_pointer" if parameter => "from is forbidden on action parameters",
        "invalid_from_pointer" => "property from must start with /",
        "invalid_template" if parameter => "template is forbidden on action parameters",
        "invalid_template" if primary_key => {
            "primary-key template requires source ownership and no from; at most 256 UTF-8 bytes and 8 placeholders; placeholders contain nonempty ASCII letters, digits or underscore with balanced braces"
        }
        "invalid_template" => {
            "property template requires source ownership and no from; at most 256 UTF-8 bytes and 8 placeholders; placeholders contain nonempty ASCII letters, digits or underscore with balanced braces"
        }
        "struct_in_struct" => "a struct cannot contain another struct, including through an array",
        "reference_array_from_source_unsupported" => {
            "source-owned arrays of references cannot use from or template"
        }
        "invalid_link_backing" if last == "cardinality" => {
            "ref backing requires many_to_one; array-of-ref backing requires many_to_many"
        }
        "invalid_link_backing" => {
            "link backing must be a ref or array of refs whose target equals link to"
        }
        "invalid_reference_property" => "reference rule property must be a ref or array of refs",
        "action_edits_source_owned" => "action rules may edit only ontology-owned object types",
        "reference_read_forbidden" => {
            "every grant allowing this action must also read the target type of a dereferenced parameter"
        }
        "wrong_value_type" if v.pointer.is_empty() => "value must satisfy the supplied schema type",
        "wrong_value_type" => "value must satisfy the declared property type",
        "value_constraint_violation" if v.pointer.is_empty() => {
            "value must satisfy the declared value-type enum/pattern/min/max constraints"
        }
        "value_constraint_violation" => {
            "value must satisfy the declared value-type enum/pattern/min/max and property max_bytes constraints"
        }
        "invalid_rule" if matches!(parts.as_slice(), ["", "action_types", _, "rules", _]) => {
            "rule must select exactly one of create_object, modify_object, create_or_modify_object, delete_object, set_reference or clear_reference"
        }
        "invalid_rule"
            if matches!(
                parts.as_slice(),
                [
                    "",
                    "action_types",
                    _,
                    "rules",
                    _,
                    "clear_reference",
                    "value"
                ]
            ) =>
        {
            "clear_reference must not supply value"
        }
        "invalid_rule" => {
            "rule value using $now requires string base and an unrestricted or canonical UTC pattern; a referenced parameter property must have a compatible declared type/base; required properties on create/create_or_modify must not depend on optional parameters"
        }
        "invalid_criterion"
            if last == "where"
                || matches!(
                    parts.as_slice(),
                    ["", "action_types", _, "submission_criteria", _]
                ) =>
        {
            "criterion group must contain exactly one of all, any or none"
        }
        "invalid_criterion" => {
            "non-in comparison using builtin $now requires string-typed operands and $-prefixed declared parameter references; a bare property comparison without a linked context requires exactly one reference parameter"
        }
        "invalid_schema" if last == "description" => "description must be at most 300 UTF-8 bytes",
        "invalid_schema" if parts.get(1) == Some(&"views") => match last {
            "args" => {
                "view args must be distinct names of 1 to 64 ASCII characters, lowercase letter first then lowercase letters, digits or underscore"
            }
            "limit" => "view limit must be an integer from 1 to 1000",
            "order" => "view order must contain at least one entry",
            "columns" => "view columns must be nonempty and each header must exclude |, CR and LF",
            _ => {
                "view filter must use declared properties/args and supported operators/types, depth at most 8 (root 0), at most 64 leaves and at most 100 in values"
            }
        },
        "invalid_name"
            if matches!(
                v.pointer.split('/').collect::<Vec<_>>().as_slice(),
                ["", "action_types", _, "parameters", "now"]
            ) =>
        {
            "1 to 64 ASCII characters: lowercase letter first; then lowercase letters, digits or underscore; now is reserved for action parameters"
        }
        "invalid_name" => {
            "1 to 64 ASCII characters: lowercase letter first; then lowercase letters, digits or underscore"
        }
        "reserved_domain" => "program is a reserved domain name",
        "unknown_source_kind" => {
            "source kind must be markdown_fields, markdown_table, file_digest, git_local, github_api, json_records or command"
        }
        "invalid_source_option" => {
            "markdown_fields allows only glob, exclude, head_lines, complete_listing, fresh_for_s; file_digest allows only paths, exclude, complete_listing, fresh_for_s. glob: required string or 1 to 16 strings; paths: required 1 to 256 strings; exclude: optional 0 to 256 strings; head_lines: optional integer 1 to 10000; complete_listing: optional boolean; fresh_for_s: required integer 1 to 315360000"
        }
        "invalid_transform" if parameter => "transform is forbidden on action parameters",
        "invalid_transform" if primary_key => {
            "primary-key transform must be a supported string-producing form: first_word, {segment_after:string} or {replace_prefix:[from,to]}; parse_integer is forbidden. Requires source ownership, from or template, and from must not be $action_id; segment_after at most 1024 UTF-8 bytes starting and ending with /; replace_prefix nonempty from, both strings at most 1024 UTF-8 bytes"
        }
        "invalid_transform" => {
            "transform must be first_word, parse_integer, {segment_after:string} or {replace_prefix:[from,to]}; segment_after: at most 1024 UTF-8 bytes, starting and ending with '/'; replace_prefix: nonempty from, each string at most 1024 UTF-8 bytes. Requires a source-owned property with from or template; parse_integer requires integer base, other transforms require string base or ref"
        }
        "invalid_glob" => {
            r"absolute glob at most 4096 UTF-8 bytes; no NUL, empty, '.' or '..' segment, or { } [ ] \; '*' and '?' wildcards allowed; '**' must occupy a whole segment, occur at most 8 times and not be the last segment"
        }
        "unknown_source" => "source name must be declared in schema sources",
        "unknown_type" => "object type name must be declared in this schema",
        "unknown_value_type" => "value type name must be declared in schema value_types",
        "unknown_action" => "action name must be declared in schema action_types",
        "unknown_link" => "link must name a declared forward or reverse link name",
        "unknown_parameter" => "parameter reference must name a declared action parameter",
        "unknown_property" => {
            "property must be declared on the referenced object type; view columns and order also allow $key"
        }
        "unknown_scope_key" => "scope key must be declared in schema scope_keys",
        "unknown_source_field" | "unknown_template_field" => {
            "field must be produced by the declared source kind"
        }
        "template_field_not_string" => {
            "template placeholder must name a string field produced by the declared source kind"
        }
        "unknown_field" => "field must be declared by ontology-schema/v1",
        "missing_field" => "required schema field",
        "duplicate_field" => "field must occur only once",
        "duplicate_type_name" => "name must be unique across installed object types and views",
        // Semantic schema refusals already include their full constraint code
        // and schema pointer in violations, rather than a command request field.
        _ => "schema field must satisfy the semantic constraint named in violations[].code",
    };
    Refusal::input("schema_violation", &v.pointer, rule)
}

#[cfg(test)]
#[path = "schema_diagnostics_tests.rs"]
mod diagnostics_tests;

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    #[test]
    fn existing_default_aliases_refuse_without_rows_or_backups() {
        let root = std::env::temp_dir().join(format!("ont-default-aliases-{}", std::process::id()));
        std::fs::create_dir_all(root.join("real")).unwrap();
        let default = root.join("real/store.sqlite");
        let mut store = Store::open_schema(&default).unwrap();
        let bytes = include_bytes!("../../ont-core/tests/schema-fixtures/example.json");
        store
            .install_schema(
                bytes,
                InstallArgs {
                    installed_at: 1,
                    source_repository: None,
                    source_commit: None,
                },
            )
            .unwrap();
        drop(store);
        std::os::unix::fs::symlink(root.join("real"), root.join("alias")).unwrap();
        std::os::unix::fs::symlink(&default, root.join("file-symlink.sqlite")).unwrap();
        std::fs::hard_link(&default, root.join("file-hardlink.sqlite")).unwrap();
        let before = std::fs::read(&default).unwrap();
        let cwd = std::env::current_dir().unwrap();
        // No process-wide chdir: spell an absolute path relative to cwd.
        let relative = format!(
            "{}/{}",
            "../".repeat(cwd.components().count() - 1),
            default.strip_prefix("/").unwrap().display()
        );
        for alias in [
            default.display().to_string(),
            root.join("file-symlink.sqlite").display().to_string(),
            root.join("file-hardlink.sqlite").display().to_string(),
            format!("{}/alias/store.sqlite", root.display()),
            format!("{}/real/../real/store.sqlite", root.display()),
            format!("{}/real//store.sqlite", root.display()),
            format!("{}/real/store.sqlite/", root.display()),
            relative,
        ] {
            let ctx = Context::new(PathBuf::from(&alias), 1);
            let req = InstallRequest {
                file: root.join("missing.json"),
                source_repository: None,
                source_commit: None,
            };
            assert!(same_store_path(&ctx.store, &default), "{alias}");
            assert!(
                matches!(install_at_default(&ctx, &req, &default), Err(Failure::Refused(r)) if r.code == "install_requires_commit" && r.extra["field"] == "--store")
            );
            assert_eq!(std::fs::read(&default).unwrap(), before);
            assert!(!root.join("real/backup").exists());
        }
        std::fs::remove_file(&default).unwrap();
        assert!(same_store_path(&root.join("alias/store.sqlite"), &default));
        std::fs::remove_dir_all(root).unwrap();
    }
}
