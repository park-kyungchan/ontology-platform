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
            Err(Refusal::new("schema_violation")
                .with("clean", json!(false))
                .with("pin", pin.unwrap_or(Value::Null))
                .with(
                    "violations",
                    json!(
                        violations
                            .iter()
                            .map(|v| json!({"code":v.code,"pointer":v.pointer}))
                            .collect::<Vec<_>>()
                    ),
                )
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
        return Err(Refusal::new("install_requires_commit").into());
    }
    if let Some(repo) = &req.source_repository
        && !valid_slug(repo)
    {
        return Err(Refusal::new("invalid_argument")
            .with("argument", json!("--source-repository"))
            .into());
    }
    if let Some(commit) = &req.source_commit
        && !valid_commit(commit)
    {
        return Err(Refusal::new("invalid_argument")
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
                matches!(install_at_default(&ctx, &req, &default), Err(Failure::Refused(r)) if r.code == "install_requires_commit")
            );
            assert_eq!(std::fs::read(&default).unwrap(), before);
            assert!(!root.join("real/backup").exists());
        }
        std::fs::remove_file(&default).unwrap();
        assert!(same_store_path(&root.join("alias/store.sqlite"), &default));
        std::fs::remove_dir_all(root).unwrap();
    }
}
