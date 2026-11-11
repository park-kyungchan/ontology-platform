//! Store mode `Schema` (format 3): schema installs, typed ingest,
//! retraction-aware reads and the action log. Library only; packet O3 adds
//! the binary that calls this.
use super::{Append, Store, StoreMode, db, decode, open_file};
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
        if &pin.domain != domain || pin.version != *version as u64 || &pin.sha256 != sha {
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

#[derive(Debug, Clone, PartialEq)]
pub struct SchemaObservation {
    pub observation: Observation,
    pub ingested_at: u64,
    pub schema_sha256: String,
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
    pub schema_sha256: String,
    pub actor: Value,
    pub submitted_at: u64,
    pub parameters: Value,
    pub basis: Value,
    pub edits: Value,
    pub outcome: String,
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
    pub fn schema_pins(&self) -> BTreeMap<String, SchemaPin> {
        self.schemas.pins().clone()
    }
    pub fn schema_set(&self) -> &SchemaSet {
        &self.schemas
    }
    fn write_tx(&mut self) -> Result<Transaction<'_>, StoreRefusal> {
        map_db(
            self.connection
                .transaction_with_behavior(TransactionBehavior::Immediate),
        )
    }

    /// Install one checked schema file. A domain's version only grows; the
    /// same (domain, version) with different bytes refuses; installing the
    /// same bytes again is a no-op.
    pub fn install_schema(
        &mut self,
        bytes: &[u8],
        installed_at: u64,
    ) -> Result<SchemaPin, StoreRefusal> {
        self.require_schema()?;
        // Type names of the candidate's own domain do not count as others.
        let candidate_domain = Schema::parse(bytes).map(|s| s.domain).unwrap_or_default();
        let others: BTreeSet<String> = self
            .schemas
            .pins()
            .keys()
            .filter(|d| **d != candidate_domain)
            .filter_map(|d| self.schemas.schema(d))
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
        let tx = self.write_tx()?;
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
        if existing
            .iter()
            .any(|(v, s)| *v == pin.version as i64 && *s == pin.sha256)
        {
            map_db(tx.commit())?;
            return Ok(pin);
        }
        if let Some((latest_version, _)) = existing.last()
            && pin.version as i64 <= *latest_version
        {
            return Err(StoreRefusal::new("schema_version_conflict"));
        }
        map_db(tx.execute(
            "INSERT INTO schema_installs VALUES (?1,?2,?3,?4,?5)",
            params![
                pin.domain,
                pin.version as i64,
                pin.sha256,
                bytes,
                installed_at as i64
            ],
        ))?;
        map_db(tx.commit())?;
        self.schemas = load_schema_set(&self.connection)?;
        Ok(pin)
    }

    /// Typed ingest: the whole batch in one IMMEDIATE transaction, or
    /// nothing. `ingested_at` is the caller's trusted clock value.
    pub fn append_schema(
        &mut self,
        items: &[Append],
        ingested_at: u64,
    ) -> Result<(), StoreRefusal> {
        self.require_schema()?;
        let schemas = self.schemas.clone();
        let tx = self.write_tx()?;
        for item in items {
            append_schema_in(&tx, &schemas, item, ingested_at)?;
        }
        map_db(tx.commit())
    }

    fn pin_of(&self, object_type: &str, stored_sha: &str) -> SchemaPin {
        match self.schemas.schema_of(object_type) {
            Some(s) => match self.schemas.pin(&s.domain) {
                Some(p) if p.sha256 == stored_sha => p.clone(),
                Some(p) => SchemaPin {
                    domain: s.domain.clone(),
                    version: p.version,
                    sha256: stored_sha.to_string(),
                },
                None => SchemaPin {
                    domain: s.domain.clone(),
                    version: 0,
                    sha256: stored_sha.to_string(),
                },
            },
            None => SchemaPin {
                domain: String::new(),
                version: 0,
                sha256: stored_sha.to_string(),
            },
        }
    }

    /// The head of one object. A retracted head is returned with
    /// `retracted: true`, not as absent.
    pub fn head(&mut self, id: &Id) -> Result<Option<SchemaHead>, StoreRefusal> {
        self.require_schema()?;
        let tx = map_db(
            self.connection
                .transaction_with_behavior(TransactionBehavior::Deferred),
        )?;
        let row = map_db(tx.query_row(
            "SELECT o.scope, ob.payload, ob.ingested_at, ob.schema_sha256, ob.change_kind FROM objects o JOIN observations ob ON ob.object_type=o.object_type AND ob.instance=o.instance AND ob.account=o.account AND ob.native_key=o.native_key AND ob.revision=o.head WHERE o.object_type=?1 AND o.instance=?2 AND o.account=?3 AND o.native_key=?4",
            params![id.object_type, id.namespace.instance, id.namespace.account, id.native_key],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?)),
        ).optional())?;
        let Some((scope_raw, payload, ingested_at, sha, kind)) = row else {
            return Ok(None);
        };
        let scope: Scope = decode(&scope_raw).map_err(StoreRefusal::from)?;
        let observation: Observation = decode(&payload).map_err(StoreRefusal::from)?;
        drop(tx);
        Ok(Some(SchemaHead {
            retracted: kind == "retraction",
            schema_pin: self.pin_of(&id.object_type, &sha),
            id: id.clone(),
            scope,
            head: SchemaObservation {
                observation,
                ingested_at: ingested_at as u64,
                schema_sha256: sha,
            },
        }))
    }

    /// The full history of one object, in append order.
    pub fn history(&mut self, id: &Id) -> Result<Option<SchemaHistory>, StoreRefusal> {
        self.require_schema()?;
        let tx = map_db(
            self.connection
                .transaction_with_behavior(TransactionBehavior::Deferred),
        )?;
        let row = map_db(tx.query_row(
            "SELECT scope, head FROM objects WHERE object_type=?1 AND instance=?2 AND account=?3 AND native_key=?4",
            params![id.object_type, id.namespace.instance, id.namespace.account, id.native_key],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        ).optional())?;
        let Some((scope_raw, head)) = row else {
            return Ok(None);
        };
        let scope: Scope = decode(&scope_raw).map_err(StoreRefusal::from)?;
        let mut stmt = map_db(tx.prepare(
            "SELECT payload, ingested_at, schema_sha256, change_kind FROM observations WHERE object_type=?1 AND instance=?2 AND account=?3 AND native_key=?4 ORDER BY ordinal",
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
                ))
            },
        ))?;
        let mut observations = Vec::new();
        let mut retracted = false;
        let mut last_sha = String::new();
        for row in rows {
            let (payload, ingested_at, sha, kind) = map_db(row)?;
            retracted = kind == "retraction";
            last_sha = sha.clone();
            observations.push(SchemaObservation {
                observation: decode(&payload).map_err(StoreRefusal::from)?,
                ingested_at: ingested_at as u64,
                schema_sha256: sha,
            });
        }
        drop(stmt);
        drop(tx);
        Ok(Some(SchemaHistory {
            retracted,
            schema_pin: self.pin_of(&id.object_type, &last_sha),
            id: id.clone(),
            scope,
            head,
            observations,
        }))
    }

    /// Heads of one object type (retracted heads hidden), ordered by
    /// `native_key`; `after` is the continuation key (exclusive).
    pub fn scan_heads(
        &mut self,
        object_type: &str,
        limit: usize,
        after: Option<&str>,
    ) -> Result<Vec<SchemaHead>, StoreRefusal> {
        self.require_schema()?;
        let Some(decl) = self.schemas.schema_of(object_type) else {
            return Err(StoreRefusal::schema(violation(
                codes::UNKNOWN_TYPE,
                format!("/object_types/{}", esc(object_type)),
            )));
        };
        let pin = self
            .schemas
            .pin(&decl.domain)
            .cloned()
            .ok_or_else(|| StoreRefusal::from(Error::StoreFailure))?;
        if limit == 0 {
            return Ok(Vec::new());
        }
        let tx = map_db(
            self.connection
                .transaction_with_behavior(TransactionBehavior::Deferred),
        )?;
        let mut stmt = map_db(tx.prepare(
            "SELECT o.object_type, o.instance, o.account, o.native_key, o.scope, ob.payload, ob.ingested_at, ob.schema_sha256 FROM objects o JOIN observations ob ON ob.object_type=o.object_type AND ob.instance=o.instance AND ob.account=o.account AND ob.native_key=o.native_key AND ob.revision=o.head WHERE o.object_type=?1 AND o.native_key>?2 AND ob.change_kind!='retraction' ORDER BY o.native_key LIMIT ?3",
        ))?;
        let rows = map_db(stmt.query_map(
            params![object_type, after.unwrap_or(""), limit as i64],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, String>(7)?,
                ))
            },
        ))?;
        let mut out = Vec::new();
        for row in rows {
            let (ot, inst, acc, key, scope_raw, payload, ingested_at, sha) = map_db(row)?;
            out.push(SchemaHead {
                id: Id {
                    object_type: ot,
                    namespace: ont_core::Namespace {
                        instance: inst,
                        account: acc,
                    },
                    native_key: key,
                },
                scope: decode(&scope_raw).map_err(StoreRefusal::from)?,
                head: SchemaObservation {
                    observation: decode(&payload).map_err(StoreRefusal::from)?,
                    ingested_at: ingested_at as u64,
                    schema_sha256: sha,
                },
                retracted: false,
                schema_pin: pin.clone(),
            });
        }
        Ok(out)
    }

    /// Append one action-log row; returns the store-assigned `seq`.
    pub fn append_action_log(&mut self, entry: &ActionLogEntry) -> Result<i64, StoreRefusal> {
        self.require_schema()?;
        let tx = self.write_tx()?;
        let seq = append_action_log_in(&tx, entry)?;
        map_db(tx.commit())?;
        Ok(seq)
    }
    pub fn action_log(&mut self) -> Result<Vec<(i64, ActionLogEntry)>, StoreRefusal> {
        self.require_schema()?;
        let mut stmt = map_db(self.connection.prepare(
            "SELECT seq,action_id,action_type,domain,schema_sha256,actor,submitted_at,parameters,basis,edits,outcome FROM action_log ORDER BY seq",
        ))?;
        let rows = map_db(stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, String>(8)?,
                r.get::<_, String>(9)?,
                r.get::<_, String>(10)?,
            ))
        }))?;
        let mut out = Vec::new();
        for row in rows {
            let (
                seq,
                action_id,
                action_type,
                domain,
                sha,
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
                    schema_sha256: sha,
                    actor: decode(&actor).map_err(StoreRefusal::from)?,
                    submitted_at: at as u64,
                    parameters: decode(&params).map_err(StoreRefusal::from)?,
                    basis: decode(&basis).map_err(StoreRefusal::from)?,
                    edits: decode(&edits).map_err(StoreRefusal::from)?,
                    outcome,
                },
            ));
        }
        Ok(out)
    }
}

pub(crate) fn append_action_log_in(
    tx: &Transaction<'_>,
    entry: &ActionLogEntry,
) -> Result<i64, StoreRefusal> {
    let text =
        |v: &Value| serde_json::to_string(v).map_err(|_| StoreRefusal::from(Error::StoreFailure));
    map_db(tx.execute(
        "INSERT INTO action_log(action_id,action_type,domain,schema_sha256,actor,submitted_at,parameters,basis,edits,outcome) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            entry.action_id,
            entry.action_type,
            entry.domain,
            entry.schema_sha256,
            text(&entry.actor)?,
            entry.submitted_at as i64,
            text(&entry.parameters)?,
            text(&entry.basis)?,
            text(&entry.edits)?,
            entry.outcome
        ],
    ))?;
    Ok(tx.last_insert_rowid())
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
    let duplicate = map_db(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM observations WHERE object_type=?1 AND instance=?2 AND account=?3 AND native_key=?4 AND revision=?5)",
        params![id.object_type, id.namespace.instance, id.namespace.account, id.native_key, observation.revision],
        |r| r.get::<_, bool>(0),
    ))?;
    if duplicate {
        return Err(StoreRefusal::revision_conflict());
    }
    match (&existing, &observation.change) {
        (None, Change::Initial) => {
            let scope_json = serde_json::to_string(scope)
                .map_err(|_| StoreRefusal::from(Error::StoreFailure))?;
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
                // A retracted object has no head for the next initial; the
                // full history stays.
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
        "INSERT INTO observations(object_type,instance,account,native_key,revision,ordinal,payload,ingested_at,schema_sha256,change_kind) SELECT ?1,?2,?3,?4,?5,COALESCE(MAX(ordinal),-1)+1,?6,?7,?8,?9 FROM observations WHERE object_type=?1 AND instance=?2 AND account=?3 AND native_key=?4",
        params![
            id.object_type,
            id.namespace.instance,
            id.namespace.account,
            id.native_key,
            observation.revision,
            payload,
            ingested_at as i64,
            pin.sha256,
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
