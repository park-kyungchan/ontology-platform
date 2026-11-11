//! `ont`: schema check/install, describe, get and backup over a format 3
//! store. One strict JSON request on stdin for describe/get, one JSON line
//! on stdout; exit 0 ok, 2 refused, 1 internal.
use ont_core::schema::{Schema, SchemaPin, SchemaSet};
use ont_core::{Authority, Grant, Id, Object, Policy, Query, Response, Scope, Snapshot};
use ont_store::Store;
use ont_store::schema_mode::{InstallArgs, StoreRefusal};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const TRUST_LIMIT: &str = "OS-account principal only; on this host every agent shares this UID, so the CLI does not authenticate a human.";
pub const MAX_REQUEST: usize = 65_536;
pub const DEFAULT_STORE: &str = "/var/lib/ontology-platform/ont/ontology.sqlite";
const PRINCIPAL: &str = "agent";

#[derive(Debug)]
pub struct Internal(pub &'static str);
#[derive(Debug)]
pub struct Outcome {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}
impl Outcome {
    fn line(value: Value, code: i32) -> Self {
        Self {
            stdout: format!("{value}\n"),
            stderr: String::new(),
            code,
        }
    }
    fn refused(code: &str, extra: Value) -> Self {
        let mut body = json!({"status":"refused","code":code});
        if let (Value::Object(dst), Value::Object(src)) = (&mut body, extra) {
            dst.extend(src);
        }
        Self::line(body, 2)
    }
}

pub fn check_uid(real: u32, effective: u32) -> Result<(), &'static str> {
    if real != 0 || effective != real {
        return Err("principal_not_trusted");
    }
    Ok(())
}
pub fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
pub fn sha256_of(bytes: &[u8]) -> String {
    sha256_hex(bytes)
}
fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}
fn pins_json(pins: &BTreeMap<String, SchemaPin>) -> Value {
    pins.iter()
        .map(|(d, p)| (d.clone(), json!({"version":p.version,"sha256":p.sha256})))
        .collect::<serde_json::Map<_, _>>()
        .into()
}
fn store_refusal(e: StoreRefusal) -> Outcome {
    let mut extra = json!({"trust_limit":TRUST_LIMIT});
    if let Some(v) = &e.violation {
        extra["violation"] = json!({"code":v.code,"pointer":v.pointer});
    }
    Outcome::refused(e.code, extra)
}
fn envelope(now: u64, pins: &BTreeMap<String, SchemaPin>, result: Value, revision: u64) -> Value {
    json!({
        "status":"ok",
        "authorization_revision":revision,
        "now":now,
        "schema":pins_json(pins),
        "trust_limit":TRUST_LIMIT,
        "result":result
    })
}

/// Build the `agent` policy from the installed schemas' grants. Each
/// read/derive type gets its own grant so neither flag leaks to the other.
pub fn policy_for(set: &SchemaSet) -> Policy {
    let mut grants: Vec<Grant> = Vec::new();
    for domain in set.pins().keys() {
        let Some(schema) = set.schema(domain) else {
            continue;
        };
        for g in &schema.grants {
            if g.principal != PRINCIPAL {
                continue;
            }
            let Ok(scope) = Scope::domain(
                schema.namespace.clone(),
                schema.domain.clone(),
                g.scope.clone(),
            ) else {
                continue;
            };
            let types: BTreeSet<&String> = g.read.iter().chain(g.derive.iter()).collect();
            for t in types {
                grants.push(Grant {
                    scope: scope.clone(),
                    object_types: BTreeSet::from([t.clone()]),
                    read: g.read.contains(t),
                    derive: g.derive.contains(t),
                    actions: g.actions.iter().cloned().collect(),
                });
            }
        }
    }
    Policy {
        principals: BTreeMap::from([(PRINCIPAL.to_string(), grants)]),
    }
}
fn admitted_types(set: &SchemaSet, domain: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if let Some(schema) = set.schema(domain) {
        for g in &schema.grants {
            if g.principal == PRINCIPAL {
                out.extend(g.read.iter().cloned());
            }
        }
    }
    out
}
fn admitted_actions(set: &SchemaSet, domain: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if let Some(schema) = set.schema(domain) {
        for g in &schema.grants {
            if g.principal == PRINCIPAL {
                out.extend(g.actions.iter().cloned());
            }
        }
    }
    out
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
fn parse_args(args: &[String]) -> Result<Cli, Outcome> {
    let mut cli = Cli {
        store: PathBuf::from(DEFAULT_STORE),
        store_given: false,
        now: None,
        version: false,
        source_repository: None,
        source_commit: None,
        command: Vec::new(),
    };
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let take = |i: &mut usize| -> Option<String> {
            *i += 1;
            args.get(*i).cloned()
        };
        match arg.as_str() {
            "--store" => {
                let Some(v) = take(&mut i) else {
                    return Err(Outcome::refused(
                        "invalid_argument",
                        json!({"argument":"--store"}),
                    ));
                };
                cli.store = PathBuf::from(v);
                cli.store_given = true;
            }
            "--now" => {
                let Some(v) = take(&mut i) else {
                    return Err(Outcome::refused(
                        "invalid_argument",
                        json!({"argument":"--now"}),
                    ));
                };
                match v.parse::<u64>() {
                    Ok(n) => cli.now = Some(n),
                    Err(_) => {
                        return Err(Outcome::refused(
                            "invalid_argument",
                            json!({"argument":"--now"}),
                        ));
                    }
                }
            }
            "--source-repository" => {
                let Some(v) = take(&mut i) else {
                    return Err(Outcome::refused(
                        "invalid_argument",
                        json!({"argument":"--source-repository"}),
                    ));
                };
                cli.source_repository = Some(v);
            }
            "--source-commit" => {
                let Some(v) = take(&mut i) else {
                    return Err(Outcome::refused(
                        "invalid_argument",
                        json!({"argument":"--source-commit"}),
                    ));
                };
                cli.source_commit = Some(v);
            }
            "--version" => cli.version = true,
            _ if arg.starts_with("--") => {
                return Err(Outcome::refused(
                    "invalid_argument",
                    json!({"argument":arg}),
                ));
            }
            _ => cli.command.push(arg.clone()),
        }
        i += 1;
    }
    Ok(cli)
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
fn utc_stamp(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let tod = secs % 86_400;
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z",
        tod / 3600,
        (tod / 60) % 60,
        tod % 60
    )
}

pub fn run(
    args: &[String],
    stdin: &[u8],
    real_uid: u32,
    effective_uid: u32,
) -> Result<Outcome, Internal> {
    let cli = match parse_args(args) {
        Ok(c) => c,
        Err(o) => return Ok(o),
    };
    if cli.version && cli.command.is_empty() {
        return Ok(Outcome {
            stdout: format!(
                "ont {} {}\n",
                env!("CARGO_PKG_VERSION"),
                env!("ONT_BUILD_REVISION")
            ),
            stderr: String::new(),
            code: 0,
        });
    }
    if cli.now.is_some() && !cli.store_given {
        return Ok(Outcome::refused(
            "now_requires_store",
            json!({"trust_limit":TRUST_LIMIT}),
        ));
    }
    if let Err(code) = check_uid(real_uid, effective_uid) {
        return Ok(Outcome::refused(code, json!({"trust_limit":TRUST_LIMIT})));
    }
    let now = cli.now.unwrap_or_else(now_seconds);
    let cmd: Vec<&str> = cli.command.iter().map(String::as_str).collect();
    match cmd.as_slice() {
        ["schema", "check", file] => Ok(schema_check(&cli, file)),
        ["schema", "install", file] => Ok(schema_install(&cli, file, now)),
        ["describe"] | ["get"] | ["backup"] => {
            if cli.source_repository.is_some() || cli.source_commit.is_some() {
                return Ok(Outcome::refused(
                    "invalid_argument",
                    json!({"argument":"--source-*" ,"trust_limit":TRUST_LIMIT}),
                ));
            }
            match cmd[0] {
                "describe" => Ok(describe(&cli, stdin, now)),
                "get" => get(&cli, stdin, now),
                _ => Ok(backup_cmd(&cli, now)),
            }
        }
        _ => Ok(Outcome::refused(
            "invalid_argument",
            json!({"argument":"command","trust_limit":TRUST_LIMIT}),
        )),
    }
}

fn read_schema_file(file: &str) -> Result<Vec<u8>, Outcome> {
    std::fs::read(file).map_err(|_| {
        Outcome::refused(
            "schema_file_unreadable",
            json!({"path":file,"trust_limit":TRUST_LIMIT}),
        )
    })
}
fn others_for(set: &SchemaSet, candidate_domain: &str) -> BTreeSet<String> {
    set.pins()
        .keys()
        .filter(|d| **d != candidate_domain)
        .filter_map(|d| set.schema(d))
        .flat_map(|s| s.object_types.keys().cloned())
        .collect()
}
fn check_outcome(
    bytes: &[u8],
    others: &BTreeSet<String>,
    checked_against: Vec<String>,
    store_exists: bool,
) -> Outcome {
    match Schema::check_bytes(bytes, others) {
        Ok(checked) => {
            let pin = checked.pin();
            Outcome::line(
                json!({
                    "status":"ok","clean":true,
                    "pin":{"domain":pin.domain,"version":pin.version,"sha256":pin.sha256},
                    "violations":[],
                    "checked_against":checked_against,
                    "store_exists":store_exists,
                    "trust_limit":TRUST_LIMIT
                }),
                0,
            )
        }
        Err(violations) => {
            let pin = Schema::parse(bytes)
                .ok()
                .map(|s| json!({"domain":s.domain,"version":s.version,"sha256":sha256_hex(bytes)}));
            Outcome::line(
                json!({
                    "status":"refused","code":"schema_violation","clean":false,
                    "pin":pin,
                    "violations":violations.iter().map(|v| json!({"code":v.code,"pointer":v.pointer})).collect::<Vec<_>>(),
                    "checked_against":checked_against,
                    "store_exists":store_exists,
                    "trust_limit":TRUST_LIMIT
                }),
                2,
            )
        }
    }
}
fn schema_check(cli: &Cli, file: &str) -> Outcome {
    let bytes = match read_schema_file(file) {
        Ok(b) => b,
        Err(o) => return o,
    };
    let store_exists = cli.store.exists();
    let (others, against) = if store_exists {
        match Store::open_schema(&cli.store) {
            Ok(mut store) => match store.schema_set() {
                Ok(set) => {
                    let set = set.clone();
                    let candidate = Schema::parse(&bytes).map(|s| s.domain).unwrap_or_default();
                    (
                        others_for(&set, &candidate),
                        set.pins().keys().cloned().collect(),
                    )
                }
                Err(e) => return store_refusal(e),
            },
            Err(e) => return store_refusal(e),
        }
    } else {
        (BTreeSet::new(), Vec::new())
    };
    check_outcome(&bytes, &others, against, store_exists)
}

fn set_file_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
}

/// One backup of `store` into `<store dir>/backup/`, verified by opening the
/// copy and comparing pins; keeps the newest three. Returns (path, sha256).
fn do_backup(store: &mut Store, store_path: &Path, now: u64) -> Result<(PathBuf, String), Outcome> {
    let dir = store_path
        .parent()
        .map(|p| p.join("backup"))
        .unwrap_or_else(|| PathBuf::from("backup"));
    std::fs::create_dir_all(&dir)
        .map_err(|_| Outcome::refused("store_failure", json!({"trust_limit":TRUST_LIMIT})))?;
    set_file_mode(&dir, 0o700);
    let stamp = utc_stamp(now);
    let mut target = dir.join(format!("ontology-{stamp}.sqlite"));
    let mut n = 2;
    while target.exists() {
        target = dir.join(format!("ontology-{stamp}-{n}.sqlite"));
        n += 1;
    }
    let before = store.schema_pins().map_err(store_refusal)?;
    store.backup_to(&target).map_err(store_refusal)?;
    set_file_mode(&target, 0o600);
    let mut copy = Store::open_schema(&target).map_err(store_refusal)?;
    let copy_pins = copy.schema_pins().map_err(store_refusal)?;
    if copy_pins != before {
        return Err(Outcome::refused(
            "backup_pin_mismatch",
            json!({"trust_limit":TRUST_LIMIT}),
        ));
    }
    let bytes = std::fs::read(&target)
        .map_err(|_| Outcome::refused("store_failure", json!({"trust_limit":TRUST_LIMIT})))?;
    // Keep the newest three backups.
    let mut names: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("ontology-") && n.ends_with(".sqlite"))
                })
                .collect()
        })
        .unwrap_or_default();
    // Newest by modification time (names alone do not order same-second
    // backups); the file just written is therefore always kept.
    names.sort_by_key(|p| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });
    while names.len() > 3 {
        let old = names.remove(0);
        let _ = std::fs::remove_file(old);
    }
    Ok((target, sha256_hex(&bytes)))
}

fn schema_install(cli: &Cli, file: &str, now: u64) -> Outcome {
    if let Some(repo) = &cli.source_repository
        && !valid_slug(repo)
    {
        return Outcome::refused(
            "invalid_argument",
            json!({"argument":"--source-repository","trust_limit":TRUST_LIMIT}),
        );
    }
    if let Some(commit) = &cli.source_commit
        && !valid_commit(commit)
    {
        return Outcome::refused(
            "invalid_argument",
            json!({"argument":"--source-commit","trust_limit":TRUST_LIMIT}),
        );
    }
    let bytes = match read_schema_file(file) {
        Ok(b) => b,
        Err(o) => return o,
    };
    let store_exists = cli.store.exists();
    let mut store = if store_exists {
        match Store::open_schema(&cli.store) {
            Ok(s) => s,
            Err(e) => return store_refusal(e),
        }
    } else {
        // Check first; a refused check must not create a store.
        let outcome = check_outcome(&bytes, &BTreeSet::new(), Vec::new(), false);
        if outcome.code != 0 {
            return outcome;
        }
        let parent = cli.store.parent();
        if parent.is_some_and(|p| !p.as_os_str().is_empty() && !p.exists()) {
            return Outcome::refused(
                "store_parent_missing",
                json!({"directory":parent.map(|p| p.display().to_string()),"trust_limit":TRUST_LIMIT}),
            );
        }
        match Store::open_schema(&cli.store) {
            Ok(s) => s,
            Err(e) => return store_refusal(e),
        }
    };
    let checked = {
        let set = match store.schema_set() {
            Ok(s) => s.clone(),
            Err(e) => return store_refusal(e),
        };
        let candidate = Schema::parse(&bytes).map(|s| s.domain).unwrap_or_default();
        match Schema::check_bytes(&bytes, &others_for(&set, &candidate)) {
            Ok(c) => c,
            Err(_violations) => {
                return check_outcome(
                    &bytes,
                    &others_for(&set, &candidate),
                    set.pins().keys().cloned().collect(),
                    true,
                );
            }
        }
    };
    let pin = checked.pin().clone();
    let previous = match store.schema_pins() {
        Ok(pins) => pins.get(&pin.domain).cloned(),
        Err(e) => return store_refusal(e),
    };
    let changed = previous.as_ref() != Some(&pin);
    let mut backup_path: Option<String> = None;
    if store_exists && changed {
        let holds_schema = match store.schema_pins() {
            Ok(pins) => !pins.is_empty(),
            Err(e) => return store_refusal(e),
        };
        if holds_schema {
            match do_backup(&mut store, &cli.store, now) {
                Ok((path, _)) => backup_path = Some(path.display().to_string()),
                Err(o) => return o,
            }
        }
    }
    if let Err(e) = store.install_schema(
        &bytes,
        InstallArgs {
            installed_at: now,
            source_repository: cli.source_repository.clone(),
            source_commit: cli.source_commit.clone(),
        },
    ) {
        return store_refusal(e);
    }
    if !store_exists {
        set_file_mode(&cli.store, 0o600);
    }
    let pins = match store.schema_pins() {
        Ok(p) => p,
        Err(e) => return store_refusal(e),
    };
    Outcome::line(
        envelope(
            now,
            &pins,
            json!({
                "domain":pin.domain,
                "previous_version":previous.map(|p| p.version),
                "version":pin.version,
                "pin":{"domain":pin.domain,"version":pin.version,"sha256":pin.sha256},
                "changed":changed,
                "backup":backup_path
            }),
            1,
        ),
        0,
    )
}

fn open_existing(cli: &Cli) -> Result<Store, Outcome> {
    if !cli.store.exists() {
        return Err(Outcome::refused(
            "store_not_found",
            json!({"path":cli.store.display().to_string(),"trust_limit":TRUST_LIMIT}),
        ));
    }
    Store::open_schema(&cli.store).map_err(store_refusal)
}
fn parse_request<T: for<'de> Deserialize<'de>>(stdin: &[u8]) -> Result<T, Outcome> {
    if stdin.len() > MAX_REQUEST {
        return Err(Outcome::refused(
            "invalid_request",
            json!({"trust_limit":TRUST_LIMIT}),
        ));
    }
    serde_json::from_slice(stdin)
        .map_err(|_| Outcome::refused("invalid_request", json!({"trust_limit":TRUST_LIMIT})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DescribeRequest {
    #[serde(default)]
    op: Option<String>,
    #[serde(default)]
    domain: Option<String>,
    #[serde(default)]
    object_type: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GetRequest {
    #[serde(default)]
    op: Option<String>,
    id: Id,
}

fn describe(cli: &Cli, stdin: &[u8], now: u64) -> Outcome {
    let req: DescribeRequest = match parse_request(stdin) {
        Ok(r) => r,
        Err(o) => return o,
    };
    if req.op.as_deref().is_some_and(|op| op != "describe") {
        return Outcome::refused("invalid_request", json!({"trust_limit":TRUST_LIMIT}));
    }
    let mut store = match open_existing(cli) {
        Ok(s) => s,
        Err(o) => return o,
    };
    let set = match store.schema_set() {
        Ok(s) => s.clone(),
        Err(e) => return store_refusal(e),
    };
    if let Some(domain) = &req.domain
        && set.schema(domain).is_none()
    {
        return Outcome::refused(
            "unknown_domain",
            json!({"domain":domain,"trust_limit":TRUST_LIMIT}),
        );
    }
    let domains: Vec<String> = match &req.domain {
        Some(d) => vec![d.clone()],
        None => set.pins().keys().cloned().collect(),
    };
    let mut object_types = Vec::new();
    let mut link_types = Vec::new();
    let mut action_types = Vec::new();
    let mut functions = Vec::new();
    let mut sources = Vec::new();
    for domain in &domains {
        let Some(schema) = set.schema(domain) else {
            continue;
        };
        let admitted = admitted_types(&set, domain);
        let actions = admitted_actions(&set, domain);
        for (name, ty) in &schema.object_types {
            if req.object_type.as_ref().is_some_and(|w| w != name) {
                continue;
            }
            if !admitted.contains(name) {
                continue;
            }
            object_types.push(json!({
                "domain":domain,
                "name":name,
                "description":ty.description,
                "title":ty.title,
                "primary_key":serde_json::to_value(&ty.primary_key).unwrap_or(Value::Null),
                "owner":serde_json::to_value(&ty.owner).unwrap_or(Value::Null),
                "properties":ty.properties.iter().map(|(pname,p)| {
                    json!({
                        "name":pname,
                        "type":serde_json::to_value(&p.ty).unwrap_or(Value::Null),
                        "required":p.required,
                        "from":p.from,
                        "max_bytes":p.max_bytes
                    })
                }).collect::<Vec<_>>()
            }));
        }
        for (name, lt) in &schema.link_types {
            if !admitted.contains(&lt.from) {
                continue;
            }
            if req
                .object_type
                .as_ref()
                .is_some_and(|w| w != &lt.from && w != &lt.to)
            {
                continue;
            }
            link_types.push(json!({
                "domain":domain,
                "name":name,
                "from":lt.from,
                "via":lt.via,
                "to":lt.to,
                "cardinality":serde_json::to_value(lt.cardinality).unwrap_or(Value::Null),
                "names":{"forward":lt.names.forward,"reverse":lt.names.reverse}
            }));
        }
        for (name, at) in &schema.action_types {
            if !actions.contains(name) {
                continue;
            }
            action_types.push(json!({
                "domain":domain,
                "name":name,
                "description":at.description,
                "parameters":at.parameters.iter().map(|(pname,p)| {
                    json!({
                        "name":pname,
                        "type":serde_json::to_value(&p.ty).unwrap_or(Value::Null),
                        "required":p.required,
                        "max_bytes":p.max_bytes
                    })
                }).collect::<Vec<_>>(),
                "criterion_codes":at.submission_criteria.iter().map(|c| c.code.clone()).collect::<Vec<_>>()
            }));
        }
        for name in schema.functions.keys() {
            functions.push(json!({"domain":domain,"name":name}));
        }
        for (name, src) in &schema.sources {
            sources.push(json!({"domain":domain,"name":name,"kind":src.kind}));
        }
    }
    let pins: BTreeMap<_, _> = domains
        .iter()
        .filter_map(|d| set.pin(d).map(|p| (d.clone(), p.clone())))
        .collect();
    Outcome::line(
        envelope(
            now,
            &pins,
            json!({
                "domains":domains.iter().map(|d| {
                    let p = set.pin(d);
                    json!({"domain":d,"version":p.map(|p| p.version),"sha256":p.map(|p| p.sha256.clone())})
                }).collect::<Vec<_>>(),
                "object_types":object_types,
                "link_types":link_types,
                "action_types":action_types,
                "functions":functions,
                "sources":sources
            }),
            1,
        ),
        0,
    )
}

struct OneObject(Option<Object>);
impl Snapshot for OneObject {
    fn get(&self, id: &Id) -> Result<Option<Object>, ont_core::Error> {
        Ok(self.0.clone().filter(|o| &o.id == id))
    }
    fn targets(&self, _root: &Id, _relation: &str) -> Result<Vec<Id>, ont_core::Error> {
        Ok(Vec::new())
    }
}

fn get(cli: &Cli, stdin: &[u8], now: u64) -> Result<Outcome, Internal> {
    let req: GetRequest = match parse_request(stdin) {
        Ok(r) => r,
        Err(o) => return Ok(o),
    };
    if req.op.as_deref().is_some_and(|op| op != "get") {
        return Ok(Outcome::refused(
            "invalid_request",
            json!({"trust_limit":TRUST_LIMIT}),
        ));
    }
    let mut store = match open_existing(cli) {
        Ok(s) => s,
        Err(o) => return Ok(o),
    };
    let not_found = || {
        Outcome::refused(
            ont_core::Error::Unavailable.code(),
            json!({"trust_limit":TRUST_LIMIT}),
        )
    };
    let history = match store.history(&req.id) {
        Ok(Some(h)) => h,
        Ok(None) => return Ok(not_found()),
        Err(e) => return Ok(store_refusal(e)),
    };
    let set = match store.schema_set() {
        Ok(s) => s.clone(),
        Err(e) => return Ok(store_refusal(e)),
    };
    let object = Object {
        id: history.id.clone(),
        scope: history.scope.clone(),
        head: history.head.clone(),
        observations: history
            .observations
            .iter()
            .map(|o| o.observation.clone())
            .collect(),
    };
    let authority = Authority::new(policy_for(&set));
    let session = match authority.bind_trusted(PRINCIPAL) {
        Ok(s) => s,
        Err(_) => return Err(Internal("authority_failure")),
    };
    let admission = match authority.admit(&session) {
        Ok(a) => a,
        Err(_) => return Err(Internal("authority_failure")),
    };
    let response = admission.query(
        &OneObject(Some(object)),
        Query::Get { id: req.id.clone() },
        now,
    );
    let (revision, mut result) = match response {
        Ok(Response::Ok {
            authorization_revision,
            result,
        }) => (
            authorization_revision,
            serde_json::to_value(&result).unwrap_or(Value::Null),
        ),
        Ok(Response::Refused { .. }) => return Ok(not_found()),
        Err(ont_core::Error::Unavailable) => return Ok(not_found()),
        Err(ont_core::Error::InvalidRequest) => {
            return Ok(Outcome::refused(
                "invalid_request",
                json!({"trust_limit":TRUST_LIMIT}),
            ));
        }
        Err(_) => return Err(Internal("authority_failure")),
    };
    // Attach each observation's stored pin.
    let pin_by_revision: BTreeMap<&str, &SchemaPin> = history
        .observations
        .iter()
        .map(|o| (o.observation.revision.as_str(), &o.schema_pin))
        .collect();
    if let Some(objects) = result.get_mut("objects").and_then(|o| o.as_array_mut()) {
        for view in objects.iter_mut() {
            if let Some(observations) = view.get_mut("observations").and_then(|o| o.as_array_mut())
            {
                for obs in observations.iter_mut() {
                    let revision = obs
                        .get("revision")
                        .and_then(|r| r.as_str())
                        .unwrap_or("")
                        .to_string();
                    if let Some(pin) = pin_by_revision.get(revision.as_str())
                        && let Value::Object(map) = obs
                    {
                        map.insert(
                            "schema".into(),
                            json!({"domain":pin.domain,"version":pin.version,"sha256":pin.sha256}),
                        );
                    }
                }
            }
        }
    }
    let domain = set.schema_of(&req.id.object_type).map(|s| s.domain.clone());
    let pins: BTreeMap<_, _> = domain
        .iter()
        .filter_map(|d| set.pin(d).map(|p| (d.clone(), p.clone())))
        .collect();
    Ok(Outcome::line(envelope(now, &pins, result, revision), 0))
}

fn backup_cmd(cli: &Cli, now: u64) -> Outcome {
    let mut store = match open_existing(cli) {
        Ok(s) => s,
        Err(o) => return o,
    };
    let pins = match store.schema_pins() {
        Ok(p) => p,
        Err(e) => return store_refusal(e),
    };
    match do_backup(&mut store, &cli.store, now) {
        Ok((path, sha)) => Outcome::line(
            envelope(
                now,
                &pins,
                json!({"path":path.display().to_string(),"sha256":sha}),
                1,
            ),
            0,
        ),
        Err(o) => o,
    }
}
