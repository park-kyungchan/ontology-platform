//! Store mode `Schema` (format 3): schema installs, typed ingest,
//! retraction-aware reads and the action log. Library only; packet O3 adds
//! the binary that calls this.
//!
//! The cached schema set is never trusted across a transaction boundary:
//! every transaction (read or write) first compares the latest
//! `schema_installs` rows with the cache and rebuilds the set inside that
//! transaction when they differ.
use super::{
    Append, Store, StoreMode, db, decode, open_file, open_file_no_create, open_file_read_only,
};
use ont_core::schema::{PropertyType, Schema, SchemaPin, SchemaSet, Violation, codes};
use ont_core::{Change, Error, Id, Knowledge, Observation, Scope};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

/// A schema-mode refusal: a stable code, and for `schema_violation` the
/// first violation (its code and JSON pointer) that caused it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreRefusal {
    pub code: &'static str,
    pub violation: Option<Violation>,
}
impl StoreRefusal {
    pub fn new(code: &'static str) -> Self {
        Self {
            code,
            violation: None,
        }
    }
    pub fn schema(violation: Violation) -> Self {
        Self {
            code: Error::SchemaViolation.code(),
            violation: Some(violation),
        }
    }
    pub fn busy() -> Self {
        Self::new("store_busy")
    }
    fn revision_conflict() -> Self {
        Self::new(Error::RevisionConflict.code())
    }
    fn store_failure() -> Self {
        Self::from(Error::StoreFailure)
    }
}
impl From<Error> for StoreRefusal {
    fn from(e: Error) -> Self {
        Self::new(e.code())
    }
}
impl std::fmt::Display for StoreRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.violation {
            Some(v) => write!(f, "{}: {}@{}", self.code, v.code, v.pointer),
            None => f.write_str(self.code),
        }
    }
}
impl std::error::Error for StoreRefusal {}

fn is_busy(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ffi::ErrorCode::DatabaseBusy)
}
fn map_db<T>(r: rusqlite::Result<T>) -> Result<T, StoreRefusal> {
    r.map_err(|e| {
        if is_busy(&e) {
            StoreRefusal::busy()
        } else {
            StoreRefusal::from(Error::StoreFailure)
        }
    })
}
fn violation(code: &'static str, pointer: impl Into<String>) -> Violation {
    Violation {
        code,
        pointer: pointer.into(),
        message: None,
    }
}
fn esc(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}
fn change_kind(c: &Change) -> &'static str {
    match c {
        Change::Initial => "initial",
        Change::Correction => "correction",
        Change::Backfill => "backfill",
        Change::Update => "update",
        Change::Refresh => "refresh",
        Change::Retraction => "retraction",
    }
}
fn to_i64(v: u64) -> Result<i64, StoreRefusal> {
    i64::try_from(v).map_err(|_| StoreRefusal::new("value_out_of_range"))
}
fn to_u64(v: i64) -> Result<u64, StoreRefusal> {
    u64::try_from(v).map_err(|_| StoreRefusal::store_failure())
}

pub(crate) fn load_schema_set(connection: &Connection) -> Result<SchemaSet, StoreRefusal> {
    let mut stmt = map_db(connection.prepare(
        "SELECT domain,version,sha256,bytes FROM schema_installs ORDER BY domain,version",
    ))?;
    let rows = map_db(stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Vec<u8>>(3)?,
        ))
    }))?;
    let mut latest: BTreeMap<String, (i64, String, Vec<u8>)> = BTreeMap::new();
    for row in rows {
        let (domain, version, sha, bytes) = map_db(row)?;
        latest.insert(domain, (version, sha, bytes));
    }
    let mut parsed: Vec<Schema> = Vec::new();
    for bytes in latest.values().map(|(_, _, b)| b) {
        parsed.push(
            Schema::parse(bytes).map_err(|_| {
                StoreRefusal::schema(violation(codes::INVALID_SCHEMA, String::new()))
            })?,
        );
    }
    let mut checked = Vec::new();
    for ((domain, (version, sha, bytes)), candidate) in latest.iter().zip(parsed.iter()) {
        let others: BTreeSet<String> = parsed
            .iter()
            .filter(|s| s.domain != candidate.domain)
            .flat_map(|s| s.object_types.keys().cloned())
            .collect();
        let c = Schema::check_bytes(bytes, &others).map_err(|v| {
            StoreRefusal::schema(
                v.into_iter()
                    .next()
                    .unwrap_or(violation(codes::INVALID_SCHEMA, String::new())),
            )
        })?;
        let pin = c.pin();
        if &pin.domain != domain || pin.version != to_u64(*version)? || &pin.sha256 != sha {
            return Err(StoreRefusal::new("schema_pin_mismatch"));
        }
        checked.push(c);
    }
    SchemaSet::new(checked).map_err(|v| {
        StoreRefusal::schema(
            v.into_iter()
                .next()
                .unwrap_or(violation(codes::INVALID_SCHEMA, String::new())),
        )
    })
}

/// The schema set as read inside `connection`'s transaction: the cached set
/// when the latest install rows still match it, a rebuilt set otherwise.
fn refreshed_set(cached: &SchemaSet, connection: &Connection) -> Result<SchemaSet, StoreRefusal> {
    let mut stmt = map_db(
        connection
            .prepare("SELECT domain,version,sha256 FROM schema_installs ORDER BY domain,version"),
    )?;
    let rows = map_db(stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, String>(2)?,
        ))
    }))?;
    let mut latest: BTreeMap<String, SchemaPin> = BTreeMap::new();
    for row in rows {
        let (domain, version, sha) = map_db(row)?;
        latest.insert(
            domain.clone(),
            SchemaPin {
                domain,
                version: to_u64(version)?,
                sha256: sha,
            },
        );
    }
    if &latest == cached.pins() {
        Ok(cached.clone())
    } else {
        load_schema_set(connection)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SchemaObservation {
    pub observation: Observation,
    pub ingested_at: u64,
    pub schema_pin: SchemaPin,
}
#[derive(Debug, Clone, PartialEq)]
pub struct SchemaHead {
    pub id: Id,
    pub scope: Scope,
    pub head: SchemaObservation,
    pub retracted: bool,
    pub schema_pin: SchemaPin,
}
#[derive(Debug, Clone, PartialEq)]
pub struct SchemaHistory {
    pub id: Id,
    pub scope: Scope,
    pub head: String,
    pub observations: Vec<SchemaObservation>,
    pub retracted: bool,
    pub schema_pin: SchemaPin,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ActionLogEntry {
    pub action_id: String,
    pub action_type: String,
    pub domain: String,
    pub schema_domain: String,
    pub schema_version: u64,
    pub actor: Value,
    pub submitted_at: u64,
    pub parameters: Value,
    pub basis: Value,
    pub edits: Value,
    pub outcome: String,
}
/// Where an install came from, recorded on the install row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallArgs {
    pub installed_at: u64,
    pub source_repository: Option<String>,
    pub source_commit: Option<String>,
}

/// The provenance recorded for the latest install of one domain. `path`
/// is `None` on a store written before that column existed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallProvenance {
    pub repository: Option<String>,
    pub commit: Option<String>,
    pub path: Option<String>,
}

fn has_column(connection: &Connection, table: &str, column: &str) -> Result<bool, StoreRefusal> {
    let mut stmt = map_db(connection.prepare(&format!("PRAGMA table_info({table})")))?;
    let rows = map_db(stmt.query_map([], |r| r.get::<_, String>(1)))?;
    for row in rows {
        if map_db(row)? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// One public write transaction in mode `Schema` (see `Store::write`).
pub struct SchemaWrite<'a> {
    tx: &'a Transaction<'a>,
    schemas: SchemaSet,
    now: u64,
}
impl SchemaWrite<'_> {
    pub fn append(&mut self, item: &Append) -> Result<(), StoreRefusal> {
        append_schema_in(self.tx, &self.schemas, item, self.now)
    }
    pub fn log_action(&mut self, entry: &ActionLogEntry) -> Result<i64, StoreRefusal> {
        append_action_log_in(self.tx, &self.schemas, entry)
    }
    pub fn head(&self, id: &Id) -> Result<Option<SchemaHead>, StoreRefusal> {
        head_in(self.tx, id)
    }
    /// Heads of one type inside this transaction (retracted hidden).
    pub fn scan_heads(
        &self,
        object_type: &str,
        limit: usize,
    ) -> Result<Vec<SchemaHead>, StoreRefusal> {
        scan_in(self.tx, &self.schemas, object_type, limit, None)
    }
    /// One page of heads of one type inside this transaction, continuing
    /// after `after` (exclusive) in primary-key order.
    pub fn scan_heads_after(
        &self,
        object_type: &str,
        limit: usize,
        after: Option<&Id>,
    ) -> Result<Vec<SchemaHead>, StoreRefusal> {
        scan_in(self.tx, &self.schemas, object_type, limit, after)
    }
    /// The whole action log inside this transaction, in `seq` order.
    pub fn action_log(&self) -> Result<Vec<(i64, ActionLogEntry)>, StoreRefusal> {
        action_log_in(self.tx)
    }
    /// One action-log row by its action id, inside this transaction.
    pub fn find_action(
        &self,
        action_id: &str,
    ) -> Result<Option<(i64, ActionLogEntry)>, StoreRefusal> {
        Ok(action_log_in(self.tx)?
            .into_iter()
            .find(|(_, e)| e.action_id == action_id))
    }
    pub fn schema_set(&self) -> &SchemaSet {
        &self.schemas
    }
}

impl Store {
    pub fn open_schema(path: &Path) -> Result<Self, StoreRefusal> {
        Self::open_schema_with_busy_timeout(path, Duration::from_secs(10))
    }
    pub fn open_schema_with_busy_timeout(
        path: &Path,
        busy: Duration,
    ) -> Result<Self, StoreRefusal> {
        let mut store = Self::configure_with_busy(
            open_file(path).map_err(StoreRefusal::from)?,
            StoreMode::Schema,
            busy,
            true,
        )
        .map_err(StoreRefusal::from)?;
        store.schemas = load_schema_set(&store.connection)?;
        Ok(store)
    }
    /// Open a store that must already exist and already be a format 3
    /// store. The connection is read-only, there is no create flag, and an
    /// empty or foreign file refuses with `store_failure` and keeps its
    /// bytes. A missing file is `store_not_found`.
    pub fn open_schema_existing(path: &Path) -> Result<Self, StoreRefusal> {
        Self::open_schema_existing_with(path, false)
    }
    /// Like [`Store::open_schema_existing`] but the connection may write
    /// (`Store::write`). It has no create flag and never initialises: a
    /// missing file is `store_not_found`, an empty or foreign file is
    /// `store_failure` with its bytes unchanged. `open_schema` stays the
    /// only open that creates or initialises.
    pub fn open_schema_existing_writable(path: &Path) -> Result<Self, StoreRefusal> {
        Self::open_schema_existing_with(path, true)
    }
    fn open_schema_existing_with(path: &Path, writable: bool) -> Result<Self, StoreRefusal> {
        if !path.exists() {
            return Err(StoreRefusal::new("store_not_found"));
        }
        let connection = if writable {
            open_file_no_create(path)
        } else {
            open_file_read_only(path)
        }
        .map_err(StoreRefusal::from)?;
        let mut store = Self::configure_with_busy(
            connection,
            StoreMode::Schema,
            Duration::from_secs(10),
            false,
        )
        .map_err(StoreRefusal::from)?;
        store.schemas = load_schema_set(&store.connection)?;
        Ok(store)
    }
    pub fn memory_schema() -> Result<Self, StoreRefusal> {
        let mut store = Self::configure(
            db(Connection::open_in_memory()).map_err(StoreRefusal::from)?,
            StoreMode::Schema,
        )
        .map_err(StoreRefusal::from)?;
        store.schemas = load_schema_set(&store.connection)?;
        Ok(store)
    }
    fn require_schema(&self) -> Result<(), StoreRefusal> {
        if self.mode != StoreMode::Schema {
            return Err(StoreRefusal::from(Error::StoreFailure));
        }
        Ok(())
    }
    fn read_tx(&mut self) -> Result<Transaction<'_>, StoreRefusal> {
        map_db(
            self.connection
                .transaction_with_behavior(TransactionBehavior::Deferred),
        )
    }
    fn write_tx(&mut self) -> Result<Transaction<'_>, StoreRefusal> {
        map_db(
            self.connection
                .transaction_with_behavior(TransactionBehavior::Immediate),
        )
    }
    /// The current schema set, refreshed inside a read transaction first.
    pub fn schema_set(&mut self) -> Result<&SchemaSet, StoreRefusal> {
        self.require_schema()?;
        let cached = self.schemas.clone();
        let tx = self.read_tx()?;
        let set = refreshed_set(&cached, &tx)?;
        drop(tx);
        self.schemas = set;
        Ok(&self.schemas)
    }
    pub fn schema_pins(&mut self) -> Result<BTreeMap<String, SchemaPin>, StoreRefusal> {
        Ok(self.schema_set()?.pins().clone())
    }

    /// The provenance of the latest install of each domain. A store
    /// written before the `source_path` column existed still reads, with
    /// `path: None`.
    pub fn schema_provenance(
        &mut self,
    ) -> Result<BTreeMap<String, InstallProvenance>, StoreRefusal> {
        self.require_schema()?;
        let tx = self.read_tx()?;
        let has_path = has_column(&tx, "schema_installs", "source_path")?;
        let sql = if has_path {
            "SELECT domain,source_repository,source_commit,source_path FROM schema_installs ORDER BY domain,version"
        } else {
            "SELECT domain,source_repository,source_commit,NULL FROM schema_installs ORDER BY domain,version"
        };
        let mut stmt = map_db(tx.prepare(sql))?;
        let rows = map_db(stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        }))?;
        let mut out: BTreeMap<String, InstallProvenance> = BTreeMap::new();
        for row in rows {
            let (domain, repository, commit, path) = map_db(row)?;
            out.insert(
                domain,
                InstallProvenance {
                    repository,
                    commit,
                    path,
                },
            );
        }
        Ok(out)
    }

    /// One IMMEDIATE write transaction: the schema set is refreshed inside
    /// it, the closure runs, and the transaction commits when the closure
    /// returns `Ok` (and rolls back otherwise). The cache is replaced only
    /// after a successful commit. `now` is the caller's trusted clock value,
    /// recorded as `ingested_at` on appended observations.
    pub fn write<T>(
        &mut self,
        now: u64,
        f: impl FnOnce(&mut SchemaWrite<'_>) -> Result<T, StoreRefusal>,
    ) -> Result<T, StoreRefusal> {
        self.require_schema()?;
        let cached = self.schemas.clone();
        let tx = self.write_tx()?;
        let schemas = refreshed_set(&cached, &tx)?;
        let mut handle = SchemaWrite {
            tx: &tx,
            schemas: schemas.clone(),
            now,
        };
        let result = f(&mut handle)?;
        drop(handle);
        map_db(tx.commit())?;
        self.schemas = schemas;
        Ok(result)
    }

    /// Install one checked schema file inside one IMMEDIATE transaction.
    /// A domain's version only grows; different bytes of an installed
    /// version refuse; installing the exact bytes of the latest installed
    /// version again is a no-op.
    pub fn install_schema(
        &mut self,
        bytes: &[u8],
        args: InstallArgs,
    ) -> Result<SchemaPin, StoreRefusal> {
        self.install_schema_inner(bytes, args, None)
    }

    /// Like [`Store::install_schema`], also recording the repository path
    /// of the schema file. Adds the `source_path` column first when a
    /// store written before it existed does not have it.
    pub fn install_schema_with_provenance(
        &mut self,
        bytes: &[u8],
        args: InstallArgs,
        source_path: Option<String>,
    ) -> Result<SchemaPin, StoreRefusal> {
        self.install_schema_inner(bytes, args, source_path)
    }

    fn install_schema_inner(
        &mut self,
        bytes: &[u8],
        args: InstallArgs,
        source_path: Option<String>,
    ) -> Result<SchemaPin, StoreRefusal> {
        self.require_schema()?;
        let cached = self.schemas.clone();
        let tx = self.write_tx()?;
        let set = refreshed_set(&cached, &tx)?;
        let candidate_domain = Schema::parse(bytes).map(|s| s.domain).unwrap_or_default();
        let others: BTreeSet<String> = set
            .pins()
            .keys()
            .filter(|d| **d != candidate_domain)
            .filter_map(|d| set.schema(d))
            .flat_map(|s| s.object_types.keys().cloned())
            .collect();
        let checked = Schema::check_bytes(bytes, &others).map_err(|v| {
            StoreRefusal::schema(
                v.into_iter()
                    .next()
                    .unwrap_or(violation(codes::INVALID_SCHEMA, String::new())),
            )
        })?;
        let pin = checked.pin().clone();
        let mut stmt = map_db(tx.prepare(
            "SELECT version,sha256 FROM schema_installs WHERE domain=?1 ORDER BY version",
        ))?;
        let rows = map_db(stmt.query_map(params![pin.domain], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        }))?;
        let mut existing: Vec<(i64, String)> = Vec::new();
        for row in rows {
            existing.push(map_db(row)?);
        }
        drop(stmt);
        let mut has_path = has_column(&tx, "schema_installs", "source_path")?;
        if !has_path && source_path.is_some() {
            map_db(tx.execute_batch("ALTER TABLE schema_installs ADD COLUMN source_path TEXT"))?;
            has_path = true;
        }
        if let Some((latest_version, latest_sha)) = existing.last() {
            if *latest_version == to_i64(pin.version)? && *latest_sha == pin.sha256 {
                if args.source_repository.is_some()
                    || args.source_commit.is_some()
                    || source_path.is_some()
                {
                    if has_path {
                        map_db(tx.execute(
                            "UPDATE schema_installs SET source_repository=COALESCE(?2,source_repository),source_commit=COALESCE(?3,source_commit),source_path=COALESCE(?4,source_path) WHERE domain=?1 AND version=?5",
                            params![
                                pin.domain,
                                args.source_repository,
                                args.source_commit,
                                source_path,
                                to_i64(pin.version)?
                            ],
                        ))?;
                    } else {
                        map_db(tx.execute(
                            "UPDATE schema_installs SET source_repository=COALESCE(?2,source_repository),source_commit=COALESCE(?3,source_commit) WHERE domain=?1 AND version=?4",
                            params![
                                pin.domain,
                                args.source_repository,
                                args.source_commit,
                                to_i64(pin.version)?
                            ],
                        ))?;
                    }
                }
                map_db(tx.commit())?;
                self.schemas = set;
                return Ok(pin);
            }
            if to_i64(pin.version)? <= *latest_version {
                return Err(StoreRefusal::new("schema_version_conflict"));
            }
        }
        if has_path {
            map_db(tx.execute(
                "INSERT INTO schema_installs(domain,version,sha256,bytes,installed_at,source_repository,source_commit,source_path) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    pin.domain,
                    to_i64(pin.version)?,
                    pin.sha256,
                    bytes,
                    to_i64(args.installed_at)?,
                    args.source_repository,
                    args.source_commit,
                    source_path
                ],
            ))?;
        } else {
            map_db(tx.execute(
                "INSERT INTO schema_installs(domain,version,sha256,bytes,installed_at,source_repository,source_commit) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![
                    pin.domain,
                    to_i64(pin.version)?,
                    pin.sha256,
                    bytes,
                    to_i64(args.installed_at)?,
                    args.source_repository,
                    args.source_commit
                ],
            ))?;
        }
        map_db(tx.commit())?;
        // No reload outside a transaction: reuse the set read in this
        // transaction plus the new install. If that cannot be built, the
        // next transaction refreshes from the table anyway.
        if let Ok(next) = set.with_domain(checked) {
            self.schemas = next;
        } else {
            self.schemas = set;
        }
        Ok(pin)
    }

    /// Typed ingest: `write` with appends only.
    pub fn append_schema(
        &mut self,
        items: &[Append],
        ingested_at: u64,
    ) -> Result<(), StoreRefusal> {
        self.write(ingested_at, |tx| {
            for item in items {
                tx.append(item)?;
            }
            Ok(())
        })
    }

    /// Append one action-log row; returns the store-assigned `seq`.
    pub fn append_action_log(&mut self, entry: &ActionLogEntry) -> Result<i64, StoreRefusal> {
        self.write(entry.submitted_at, |tx| tx.log_action(entry))
    }

    /// The head of one object. A retracted head is returned with
    /// `retracted: true`, not as absent.
    pub fn head(&mut self, id: &Id) -> Result<Option<SchemaHead>, StoreRefusal> {
        self.require_schema()?;
        let cached = self.schemas.clone();
        let tx = self.read_tx()?;
        let set = refreshed_set(&cached, &tx)?;
        let result = head_in(&tx, id);
        drop(tx);
        if result.is_ok() {
            self.schemas = set;
        }
        result
    }

    /// The full history of one object, in append order.
    pub fn history(&mut self, id: &Id) -> Result<Option<SchemaHistory>, StoreRefusal> {
        self.require_schema()?;
        let cached = self.schemas.clone();
        let tx = self.read_tx()?;
        let set = refreshed_set(&cached, &tx)?;
        let result = history_in(&tx, id);
        drop(tx);
        if result.is_ok() {
            self.schemas = set;
        }
        result
    }

    /// Heads of one object type (retracted heads hidden), in primary-key
    /// order `(instance, account, native_key)`; `after` continues from the
    /// last returned `Id` (exclusive).
    pub fn scan_heads(
        &mut self,
        object_type: &str,
        limit: usize,
        after: Option<&Id>,
    ) -> Result<Vec<SchemaHead>, StoreRefusal> {
        self.require_schema()?;
        if limit > 10_000 {
            return Err(StoreRefusal::new("limit_too_large"));
        }
        let cached = self.schemas.clone();
        let tx = self.read_tx()?;
        let set = refreshed_set(&cached, &tx)?;
        let result = scan_in(&tx, &set, object_type, limit, after);
        drop(tx);
        if result.is_ok() {
            self.schemas = set;
        }
        result
    }

    /// Copy the whole store to `target` with `VACUUM INTO` (bound
    /// parameter). The target must not exist.
    pub fn backup_to(&mut self, target: &Path) -> Result<(), StoreRefusal> {
        self.require_schema()?;
        if target.exists() {
            return Err(StoreRefusal::new("backup_target_exists"));
        }
        let target = target
            .to_str()
            .ok_or_else(|| StoreRefusal::from(Error::StoreFailure))?;
        map_db(self.connection.execute("VACUUM INTO ?1", params![target]))?;
        Ok(())
    }

    pub fn action_log(&mut self) -> Result<Vec<(i64, ActionLogEntry)>, StoreRefusal> {
        self.require_schema()?;
        let cached = self.schemas.clone();
        let tx = self.read_tx()?;
        let set = refreshed_set(&cached, &tx)?;
        let result = action_log_in(&tx);
        drop(tx);
        if result.is_ok() {
            self.schemas = set;
        }
        result
    }
}

fn decode_scope(raw: &str) -> Result<Scope, StoreRefusal> {
    let scope: Scope = decode(raw).map_err(StoreRefusal::from)?;
    scope
        .validate()
        .map_err(|_| StoreRefusal::store_failure())?;
    Ok(scope)
}
fn decode_observation(payload: &str) -> Result<Observation, StoreRefusal> {
    let observation: Observation = decode(payload).map_err(StoreRefusal::from)?;
    observation
        .validate()
        .map_err(|_| StoreRefusal::store_failure())?;
    Ok(observation)
}

fn head_in(connection: &Connection, id: &Id) -> Result<Option<SchemaHead>, StoreRefusal> {
    let row = map_db(connection.query_row(
        "SELECT scope, head FROM objects WHERE object_type=?1 AND instance=?2 AND account=?3 AND native_key=?4",
        params![id.object_type, id.namespace.instance, id.namespace.account, id.native_key],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    ).optional())?;
    let Some((scope_raw, head)) = row else {
        return Ok(None);
    };
    let scope = decode_scope(&scope_raw)?;
    let row = map_db(connection.query_row(
        "SELECT ob.payload, ob.ingested_at, ob.change_kind, si.domain, si.version, si.sha256 FROM observations ob JOIN schema_installs si ON si.domain=ob.schema_domain AND si.version=ob.schema_version WHERE ob.object_type=?1 AND ob.instance=?2 AND ob.account=?3 AND ob.native_key=?4 AND ob.revision=?5",
        params![id.object_type, id.namespace.instance, id.namespace.account, id.native_key, head],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
            ))
        },
    ).optional())?;
    let Some((payload, ingested_at, kind, domain, version, sha)) = row else {
        // An objects row whose head observation is missing is corruption,
        // not absence.
        return Err(StoreRefusal::store_failure());
    };
    let pin = SchemaPin {
        domain: domain.clone(),
        version: to_u64(version)?,
        sha256: sha,
    };
    Ok(Some(SchemaHead {
        retracted: kind == "retraction",
        schema_pin: pin.clone(),
        id: id.clone(),
        scope,
        head: SchemaObservation {
            observation: decode_observation(&payload)?,
            ingested_at: to_u64(ingested_at)?,
            schema_pin: pin,
        },
    }))
}

fn history_in(connection: &Connection, id: &Id) -> Result<Option<SchemaHistory>, StoreRefusal> {
    let row = map_db(connection.query_row(
        "SELECT scope, head FROM objects WHERE object_type=?1 AND instance=?2 AND account=?3 AND native_key=?4",
        params![id.object_type, id.namespace.instance, id.namespace.account, id.native_key],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    ).optional())?;
    let Some((scope_raw, head)) = row else {
        return Ok(None);
    };
    let scope = decode_scope(&scope_raw)?;
    let mut stmt = map_db(connection.prepare(
        "SELECT ob.payload, ob.ingested_at, ob.change_kind, si.domain, si.version, si.sha256 FROM observations ob JOIN schema_installs si ON si.domain=ob.schema_domain AND si.version=ob.schema_version WHERE ob.object_type=?1 AND ob.instance=?2 AND ob.account=?3 AND ob.native_key=?4 ORDER BY ob.ordinal",
    ))?;
    let rows = map_db(stmt.query_map(
        params![
            id.object_type,
            id.namespace.instance,
            id.namespace.account,
            id.native_key
        ],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
            ))
        },
    ))?;
    let mut observations = Vec::new();
    let mut retracted = false;
    let mut pin: Option<SchemaPin> = None;
    for row in rows {
        let (payload, ingested_at, kind, domain, version, sha) = map_db(row)?;
        retracted = kind == "retraction";
        let p = SchemaPin {
            domain,
            version: to_u64(version)?,
            sha256: sha,
        };
        pin = Some(p.clone());
        observations.push(SchemaObservation {
            observation: decode_observation(&payload)?,
            ingested_at: to_u64(ingested_at)?,
            schema_pin: p,
        });
    }
    if observations.last().map(|o| &o.observation.revision) != Some(&head) {
        return Err(StoreRefusal::store_failure());
    }
    Ok(Some(SchemaHistory {
        retracted,
        schema_pin: pin.unwrap_or(SchemaPin {
            domain: String::new(),
            version: 0,
            sha256: String::new(),
        }),
        id: id.clone(),
        scope,
        head,
        observations,
    }))
}

fn scan_in(
    connection: &Connection,
    schemas: &SchemaSet,
    object_type: &str,
    limit: usize,
    after: Option<&Id>,
) -> Result<Vec<SchemaHead>, StoreRefusal> {
    let Some(decl) = schemas.schema_of(object_type) else {
        return Err(StoreRefusal::schema(violation(
            codes::UNKNOWN_TYPE,
            format!("/object_types/{}", esc(object_type)),
        )));
    };
    let _ = decl;
    if limit == 0 {
        return Ok(Vec::new());
    }
    let (inst, acc, key) = match after {
        Some(id) => (
            id.namespace.instance.clone(),
            id.namespace.account.clone(),
            id.native_key.clone(),
        ),
        None => (String::new(), String::new(), String::new()),
    };
    let limit_i64 = to_i64(limit as u64)?;
    let mut stmt = map_db(connection.prepare(
        "SELECT o.object_type, o.instance, o.account, o.native_key, o.scope, ob.payload, ob.ingested_at, ob.change_kind, si.domain, si.version, si.sha256 FROM objects o JOIN observations ob ON ob.object_type=o.object_type AND ob.instance=o.instance AND ob.account=o.account AND ob.native_key=o.native_key AND ob.revision=o.head JOIN schema_installs si ON si.domain=ob.schema_domain AND si.version=ob.schema_version WHERE o.object_type=?1 AND (o.instance,o.account,o.native_key)>(?2,?3,?4) AND ob.change_kind!='retraction' ORDER BY o.instance,o.account,o.native_key LIMIT ?5",
    ))?;
    let rows = map_db(
        stmt.query_map(params![object_type, inst, acc, key, limit_i64], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, String>(8)?,
                r.get::<_, i64>(9)?,
                r.get::<_, String>(10)?,
            ))
        }),
    )?;
    let mut out = Vec::new();
    for row in rows {
        let (ot, inst, acc, key, scope_raw, payload, ingested_at, _kind, domain, version, sha) =
            map_db(row)?;
        let pin = SchemaPin {
            domain,
            version: to_u64(version)?,
            sha256: sha,
        };
        out.push(SchemaHead {
            id: Id {
                object_type: ot,
                namespace: ont_core::Namespace {
                    instance: inst,
                    account: acc,
                },
                native_key: key,
            },
            scope: decode_scope(&scope_raw)?,
            head: SchemaObservation {
                observation: decode_observation(&payload)?,
                ingested_at: to_u64(ingested_at)?,
                schema_pin: pin.clone(),
            },
            retracted: false,
            schema_pin: pin,
        });
    }
    Ok(out)
}

fn action_log_in(connection: &Connection) -> Result<Vec<(i64, ActionLogEntry)>, StoreRefusal> {
    let mut stmt = map_db(connection.prepare(
        "SELECT seq,action_id,action_type,domain,schema_domain,schema_version,actor,submitted_at,parameters,basis,edits,outcome FROM action_log ORDER BY seq",
    ))?;
    let rows = map_db(stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, i64>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, i64>(7)?,
            r.get::<_, String>(8)?,
            r.get::<_, String>(9)?,
            r.get::<_, String>(10)?,
            r.get::<_, String>(11)?,
        ))
    }))?;
    let mut out = Vec::new();
    for row in rows {
        let (
            seq,
            action_id,
            action_type,
            domain,
            schema_domain,
            schema_version,
            actor,
            at,
            params,
            basis,
            edits,
            outcome,
        ) = map_db(row)?;
        out.push((
            seq,
            ActionLogEntry {
                action_id,
                action_type,
                domain,
                schema_domain,
                schema_version: to_u64(schema_version)?,
                actor: decode(&actor).map_err(StoreRefusal::from)?,
                submitted_at: to_u64(at)?,
                parameters: decode(&params).map_err(StoreRefusal::from)?,
                basis: decode(&basis).map_err(StoreRefusal::from)?,
                edits: decode(&edits).map_err(StoreRefusal::from)?,
                outcome,
            },
        ));
    }
    Ok(out)
}

pub(crate) fn append_action_log_in(
    tx: &Transaction<'_>,
    schemas: &SchemaSet,
    entry: &ActionLogEntry,
) -> Result<i64, StoreRefusal> {
    if entry.domain != entry.schema_domain {
        return Err(StoreRefusal::new("action_domain_mismatch"));
    }
    let current = schemas.pin(&entry.schema_domain);
    if current.is_none_or(|p| p.version != entry.schema_version) {
        return Err(StoreRefusal::new("action_schema_mismatch"));
    }
    let exists = map_db(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM action_log WHERE action_id=?1)",
        params![entry.action_id],
        |r| r.get::<_, bool>(0),
    ))?;
    if exists {
        return Err(StoreRefusal::new("duplicate_action_id"));
    }
    let text =
        |v: &Value| serde_json::to_string(v).map_err(|_| StoreRefusal::from(Error::StoreFailure));
    let result = tx.execute(
        "INSERT INTO action_log(action_id,action_type,domain,schema_domain,schema_version,actor,submitted_at,parameters,basis,edits,outcome) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![
            entry.action_id,
            entry.action_type,
            entry.domain,
            entry.schema_domain,
            to_i64(entry.schema_version)?,
            text(&entry.actor)?,
            to_i64(entry.submitted_at)?,
            text(&entry.parameters)?,
            text(&entry.basis)?,
            text(&entry.edits)?,
            entry.outcome
        ],
    );
    match result {
        Ok(_) => Ok(tx.last_insert_rowid()),
        Err(rusqlite::Error::SqliteFailure(f, _))
            if f.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE =>
        {
            Err(StoreRefusal::new("duplicate_action_id"))
        }
        Err(e) => map_db(Err(e)),
    }
}

fn check_ref_namespaces(
    schemas: &SchemaSet,
    ty: &PropertyType,
    value: &Value,
    pointer: &str,
) -> Result<(), StoreRefusal> {
    match ty {
        PropertyType::Ref(r) => {
            let id: Id = serde_json::from_value(value.clone()).map_err(|_| {
                StoreRefusal::schema(violation(codes::WRONG_VALUE_TYPE, pointer.to_string()))
            })?;
            let Some(target) = schemas.schema_of(&r.target) else {
                return Ok(());
            };
            if id.namespace != target.namespace {
                return Err(StoreRefusal::schema(violation(
                    codes::REF_NAMESPACE_MISMATCH,
                    pointer.to_string(),
                )));
            }
            Ok(())
        }
        PropertyType::Array(a) => {
            if let Value::Array(items) = value {
                for item in items {
                    check_ref_namespaces(schemas, &a.array, item, pointer)?;
                }
            }
            Ok(())
        }
        PropertyType::Struct(s) => {
            if let Value::Object(map) = value {
                for (fname, fty) in &s.struct_ {
                    if let Some(fv) = map.get(fname) {
                        check_ref_namespaces(schemas, fty, fv, pointer)?;
                    }
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

pub(crate) fn append_schema_in(
    tx: &Transaction<'_>,
    schemas: &SchemaSet,
    item: &Append,
    ingested_at: u64,
) -> Result<(), StoreRefusal> {
    let Append {
        id,
        scope,
        observation,
    } = item;
    id.validate().map_err(StoreRefusal::from)?;
    if observation.change == Change::Initial && observation.supersedes.is_some() {
        return Err(StoreRefusal::schema(violation(
            codes::INVALID_CHANGE_KIND,
            "/supersedes",
        )));
    }
    observation.validate().map_err(StoreRefusal::from)?;
    let Some(decl) = schemas.schema_of(&id.object_type) else {
        return Err(StoreRefusal::schema(violation(
            codes::UNKNOWN_TYPE,
            format!("/object_types/{}", esc(&id.object_type)),
        )));
    };
    let Scope::Domain {
        namespace: scope_namespace,
        domain: scope_domain,
        keys: scope_keys,
    } = scope
    else {
        return Err(StoreRefusal::schema(violation(
            codes::SCOPE_DOMAIN_MISMATCH,
            "/scope",
        )));
    };
    if scope_domain != &decl.domain {
        return Err(StoreRefusal::schema(violation(
            codes::SCOPE_DOMAIN_MISMATCH,
            "/scope/domain",
        )));
    }
    if scope_namespace != &decl.namespace {
        return Err(StoreRefusal::schema(violation(
            codes::SCOPE_NAMESPACE_MISMATCH,
            "/scope/namespace",
        )));
    }
    if scope_keys.iter().any(|(k, v)| k.is_empty() || v.is_empty()) {
        return Err(StoreRefusal::schema(violation(
            codes::SCOPE_KEYS_MISMATCH,
            "/scope/keys",
        )));
    }
    let declared_keys: BTreeSet<&String> = decl.scope_keys.iter().collect();
    if scope_keys.len() != declared_keys.len()
        || scope_keys.keys().any(|k| !declared_keys.contains(k))
    {
        return Err(StoreRefusal::schema(violation(
            codes::SCOPE_KEYS_MISMATCH,
            "/scope/keys",
        )));
    }
    if id.namespace != decl.namespace {
        return Err(StoreRefusal::schema(violation(
            codes::ID_NAMESPACE_MISMATCH,
            "/id/namespace",
        )));
    }
    if observation.change == Change::Backfill {
        return Err(StoreRefusal::schema(violation(
            codes::INVALID_CHANGE_KIND,
            "/change",
        )));
    }
    if observation.change != Change::Retraction {
        schemas
            .validate_fields(&id.object_type, &observation.fields)
            .map_err(StoreRefusal::schema)?;
        if let Some(props) = schemas.properties(&id.object_type) {
            for (name, prop) in props {
                if let Some(Knowledge::Value(v)) = observation.fields.get(name) {
                    check_ref_namespaces(schemas, &prop.ty, v, &format!("/fields/{}", esc(name)))?;
                }
            }
        }
    }
    let pin = schemas
        .pin(&decl.domain)
        .ok_or_else(|| StoreRefusal::from(Error::StoreFailure))?;
    let existing = map_db(tx.query_row(
        "SELECT o.scope, o.head, ob.change_kind, ob.payload FROM objects o JOIN observations ob ON ob.object_type=o.object_type AND ob.instance=o.instance AND ob.account=o.account AND ob.native_key=o.native_key AND ob.revision=o.head WHERE o.object_type=?1 AND o.instance=?2 AND o.account=?3 AND o.native_key=?4",
        params![id.object_type, id.namespace.instance, id.namespace.account, id.native_key],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        },
    ).optional())?;
    // An objects row whose head observation is missing is corruption.
    let object_row: Option<String> = map_db(tx.query_row(
        "SELECT head FROM objects WHERE object_type=?1 AND instance=?2 AND account=?3 AND native_key=?4",
        params![id.object_type, id.namespace.instance, id.namespace.account, id.native_key],
        |r| r.get::<_, String>(0),
    ).optional())?;
    if object_row.is_some() && existing.is_none() {
        return Err(StoreRefusal::store_failure());
    }
    let duplicate = map_db(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM observations WHERE object_type=?1 AND instance=?2 AND account=?3 AND native_key=?4 AND revision=?5)",
        params![id.object_type, id.namespace.instance, id.namespace.account, id.native_key, observation.revision],
        |r| r.get::<_, bool>(0),
    ))?;
    if duplicate {
        return Err(StoreRefusal::new("duplicate_revision"));
    }
    match (&existing, &observation.change) {
        (None, Change::Initial) => {
            let scope_json = serde_json::to_string(scope).map_err(|_| {
                StoreRefusal::schema(violation(codes::SCOPE_KEYS_MISMATCH, "/scope/keys"))
            })?;
            map_db(tx.execute(
                "INSERT INTO objects VALUES (?1,?2,?3,?4,?5,?6)",
                params![
                    id.object_type,
                    id.namespace.instance,
                    id.namespace.account,
                    id.native_key,
                    scope_json,
                    observation.revision
                ],
            ))?;
        }
        (None, _) => return Err(StoreRefusal::revision_conflict()),
        (Some((old_scope, head, head_kind, head_payload)), _) => {
            let old: Scope = decode(old_scope).map_err(StoreRefusal::from)?;
            if old != *scope {
                return Err(StoreRefusal::from(Error::ScopeConflict));
            }
            if head_kind == "retraction" {
                if observation.change != Change::Initial {
                    return Err(StoreRefusal::revision_conflict());
                }
            } else {
                match observation.change {
                    Change::Initial => return Err(StoreRefusal::revision_conflict()),
                    Change::Update | Change::Refresh | Change::Correction | Change::Retraction => {
                        if observation.supersedes.as_ref() != Some(head) {
                            return Err(StoreRefusal::revision_conflict());
                        }
                        if observation.change == Change::Refresh {
                            let head_obs: Observation =
                                decode(head_payload).map_err(StoreRefusal::from)?;
                            if head_obs.fields != observation.fields {
                                return Err(StoreRefusal::schema(violation(
                                    codes::REFRESH_FIELDS_DIFFER,
                                    "/fields",
                                )));
                            }
                        }
                    }
                    Change::Backfill => unreachable!(),
                }
            }
        }
    }
    let payload =
        serde_json::to_string(observation).map_err(|_| StoreRefusal::from(Error::StoreFailure))?;
    map_db(tx.execute(
        "INSERT INTO observations(object_type,instance,account,native_key,revision,ordinal,payload,ingested_at,schema_domain,schema_version,change_kind) SELECT ?1,?2,?3,?4,?5,COALESCE(MAX(ordinal),-1)+1,?6,?7,?8,?9,?10 FROM observations WHERE object_type=?1 AND instance=?2 AND account=?3 AND native_key=?4",
        params![
            id.object_type,
            id.namespace.instance,
            id.namespace.account,
            id.native_key,
            observation.revision,
            payload,
            to_i64(ingested_at)?,
            decl.domain,
            to_i64(pin.version)?,
            change_kind(&observation.change)
        ],
    ))?;
    map_db(tx.execute(
        "UPDATE objects SET head=?5 WHERE object_type=?1 AND instance=?2 AND account=?3 AND native_key=?4",
        params![
            id.object_type,
            id.namespace.instance,
            id.namespace.account,
            id.native_key,
            observation.revision
        ],
    ))?;
    Ok(())
}

#[cfg(test)]
mod provenance_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn bytes(version: u64) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "format":"ontology-schema/v1","domain":"work","version":version,
            "namespace":{"instance":"vps://synthetic","account":"root"},
            "object_types":{
              "thing":{"owner":"ontology","primary_key":{"from":"$action_id"},
                "properties":{"name":{"type":"string","required":true}}}
            },
            "grants":[{"principal":"agent","scope":{},"read":["thing"],"derive":[],"actions":[]}]
        }))
        .unwrap()
    }

    #[test]
    fn store_without_source_path_column_still_opens_and_gains_it_on_install() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ont-store-prov-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("old.sqlite");
        {
            let mut store = Store::open_schema(&path).unwrap();
            store
                .install_schema(
                    &bytes(1),
                    InstallArgs {
                        installed_at: 1,
                        source_repository: Some("example/repo".into()),
                        source_commit: Some("a".repeat(40)),
                    },
                )
                .unwrap();
        }
        // Remove the column a store written by the base does not have.
        {
            let raw = Connection::open(&path).unwrap();
            raw.execute_batch("ALTER TABLE schema_installs DROP COLUMN source_path")
                .unwrap();
        }
        let mut store = Store::open_schema(&path).unwrap();
        assert_eq!(store.schema_pins().unwrap()["work"].version, 1);
        let before = store.schema_provenance().unwrap();
        assert_eq!(before["work"].repository.as_deref(), Some("example/repo"));
        assert_eq!(before["work"].path, None);
        store
            .install_schema_with_provenance(
                &bytes(2),
                InstallArgs {
                    installed_at: 2,
                    source_repository: Some("example/repo".into()),
                    source_commit: Some("b".repeat(40)),
                },
                Some("ontology/work.schema.json".into()),
            )
            .unwrap();
        let after = store.schema_provenance().unwrap();
        assert_eq!(
            after["work"].path.as_deref(),
            Some("ontology/work.schema.json")
        );
        assert_eq!(
            after["work"].commit.as_deref(),
            Some("b".repeat(40).as_str())
        );
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
