//! Small trusted SQLite boundary. No source adapters, principal inference or effects.
use ont_core::{Change, Error, Id, Link, Namespace, Object, Observation, Scope, Snapshot};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use std::path::Path;
use std::time::Duration;

pub mod actions;
pub mod fake;
pub mod schema_mode;

/// Largest supported candidate clock; action-created heads never become stale.
pub const NEVER_STALE: u64 = 253_402_300_799;

pub const SCHEMA: &str = include_str!("../schema.sql");
pub(crate) const SCHEMA_V3: &str = include_str!("../schema-v3.sql");
const APPLICATION_ID: i64 = 1_330_533_430;
fn db<T>(result: rusqlite::Result<T>) -> Result<T, Error> {
    result.map_err(|_| Error::StoreFailure)
}
fn db_mode<T>(result: rusqlite::Result<T>, mode: StoreMode) -> Result<T, Error> {
    result.map_err(|e| {
        if mode == StoreMode::Schema
            && matches!(e, rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ffi::ErrorCode::DatabaseBusy)
        {
            Error::StoreBusy
        } else {
            Error::StoreFailure
        }
    })
}
fn decode<T: serde::de::DeserializeOwned>(s: &str) -> Result<T, Error> {
    serde_json::from_str(s).map_err(|_| Error::StoreFailure)
}
pub(crate) fn open_file(path: &Path) -> Result<Connection, Error> {
    db(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    ))
}
/// Open an existing file read-write without the create flag: a missing path
/// fails and an empty file is never initialised by this open.
pub(crate) fn open_file_no_create(path: &Path) -> Result<Connection, Error> {
    db(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    ))
}
/// Open an existing file read-only: no create flag, no write.
pub(crate) fn open_file_read_only(path: &Path) -> Result<Connection, Error> {
    db(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    ))
}
/// A store's domain and version are selected explicitly, never inferred from rows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StoreMode {
    Education,
    Program,
    Schema,
}
impl StoreMode {
    fn version(self) -> i64 {
        match self {
            Self::Education => 1,
            Self::Program => 2,
            Self::Schema => 4,
        }
    }
    fn accepts(self, scope: &Scope, namespace: &Namespace) -> bool {
        matches!(
            (self, scope),
            (Self::Education, Scope::Education { .. })
                | (Self::Program, Scope::Program { .. })
                | (Self::Schema, Scope::Domain { .. })
        ) && scope.validate().is_ok()
            && scope.namespace() == namespace
    }
    fn decode_scope(self, raw: &str, namespace: &Namespace) -> Result<Scope, Error> {
        let scope = decode(raw)?;
        if !self.accepts(&scope, namespace) {
            return Err(Error::StoreFailure);
        }
        Ok(scope)
    }
}
/// Every persisted v1 scope must remain readable by the Education-only reader.
fn v1_scope(scope: &Scope, namespace: &Namespace) -> Result<(), Error> {
    if !StoreMode::Education.accepts(scope, namespace) {
        return Err(Error::StoreFailure);
    }
    Ok(())
}
fn decode_v1_scope(s: &str, namespace: &Namespace) -> Result<Scope, Error> {
    StoreMode::Education.decode_scope(s, namespace)
}
pub struct Store {
    connection: Connection,
    mode: StoreMode,
    schemas: ont_core::schema::SchemaSet,
    candidate_triggers: std::cell::Cell<Option<(i64, bool)>>,
}
pub struct ReadSnapshot<'a> {
    connection: &'a Connection,
    mode: StoreMode,
}
/// Trusted ingestion input.
#[derive(Clone)]
pub struct Append {
    pub id: Id,
    pub scope: Scope,
    pub observation: Observation,
}
impl Store {
    pub fn memory() -> Result<Self, Error> {
        Self::configure(db(Connection::open_in_memory())?, StoreMode::Education)
    }
    /// Only task-owned synthetic files in S06. No live DB opening or migration.
    pub fn open(path: &Path) -> Result<Self, Error> {
        Self::open_mode(path, StoreMode::Education)
    }
    /// Separate task-owned Program projection; schema v2, without action tables.
    /// Existing v1 or unknown databases refuse without conversion or row reads.
    pub fn open_program(path: &Path) -> Result<Self, Error> {
        Self::open_mode(path, StoreMode::Program)
    }
    /// Disposable Program projection with the same v2 contract as open_program.
    pub fn memory_program() -> Result<Self, Error> {
        Self::configure(db(Connection::open_in_memory())?, StoreMode::Program)
    }
    fn open_mode(path: &Path, mode: StoreMode) -> Result<Self, Error> {
        Self::configure(open_file(path)?, mode)
    }
    fn configure(connection: Connection, mode: StoreMode) -> Result<Self, Error> {
        let busy = if mode == StoreMode::Schema {
            Duration::from_secs(10)
        } else {
            Duration::from_millis(250)
        };
        Self::configure_with_busy(connection, mode, busy, true)
    }
    fn configure_with_busy(
        mut connection: Connection,
        mode: StoreMode,
        busy: Duration,
        init: bool,
    ) -> Result<Self, Error> {
        db_mode(connection.busy_timeout(busy), mode)?;
        let app: i64 = db_mode(
            connection.query_row("PRAGMA application_id", [], |r| r.get(0)),
            mode,
        )?;
        let version: i64 = db_mode(
            connection.query_row("PRAGMA user_version", [], |r| r.get(0)),
            mode,
        )?;
        let count: i64 = db_mode(
            connection.query_row(
                "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
                [],
                |r| r.get(0),
            ),
            mode,
        )?;
        if app == 0 && version == 0 && count == 0 {
            if !init {
                // An empty file is not a store: never initialise it here.
                return Err(Error::StoreFailure);
            }
            db_mode(connection.pragma_update(None, "page_size", 16384), mode)?;
            let transaction = db_mode(
                connection.transaction_with_behavior(TransactionBehavior::Immediate),
                mode,
            )?;
            db_mode(
                transaction.execute_batch(if mode == StoreMode::Schema {
                    SCHEMA_V3
                } else {
                    SCHEMA
                }),
                mode,
            )?;
            db_mode(
                transaction.pragma_update(None, "user_version", mode.version()),
                mode,
            )?;
            db_mode(transaction.commit(), mode)?;
        } else if app != APPLICATION_ID
            || (version != mode.version() && !(mode == StoreMode::Schema && version == 3))
        {
            return Err(Error::StoreFailure);
        }
        db_mode(connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; PRAGMA trusted_schema=OFF;"), mode)?;
        Ok(Self {
            connection,
            mode,
            schemas: ont_core::schema::SchemaSet::default(),
            candidate_triggers: std::cell::Cell::new(None),
        })
    }
    /// Atomic graph initialization, preserving explicit observation append order.
    pub fn seed(&mut self, objects: &[Object], links: &[Link]) -> Result<(), Error> {
        if self.mode == StoreMode::Schema {
            return Err(Error::SchemaViolation);
        }
        let transaction = db(self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate))?;
        for object in objects {
            if object.observations.last().map(|o| &o.revision) != Some(&object.head) {
                return Err(Error::InvalidEvidence);
            }
            for observation in &object.observations {
                append_in(
                    &transaction,
                    self.mode,
                    &Append {
                        id: object.id.clone(),
                        scope: object.scope.clone(),
                        observation: observation.clone(),
                    },
                )?;
            }
        }
        for link in links {
            link.source.validate()?;
            link.target.validate()?;
            if link.relation.is_empty() || link.relation.len() > 64 {
                return Err(Error::InvalidEvidence);
            }
            let snapshot = ReadSnapshot {
                connection: &transaction,
                mode: self.mode,
            };
            snapshot.get(&link.source)?.ok_or(Error::StoreFailure)?;
            snapshot.get(&link.target)?.ok_or(Error::StoreFailure)?;
            db(transaction.execute(
                "INSERT INTO links VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    link.source.object_type,
                    link.source.namespace.instance,
                    link.source.namespace.account,
                    link.source.native_key,
                    link.relation,
                    link.target.object_type,
                    link.target.namespace.instance,
                    link.target.namespace.account,
                    link.target.native_key
                ],
            ))?;
        }
        db(transaction.commit())
    }
    pub fn append_batch(&mut self, items: &[Append]) -> Result<(), Error> {
        if self.mode == StoreMode::Schema {
            return Err(Error::SchemaViolation);
        }
        let transaction = db(self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate))?;
        for item in items {
            append_in(&transaction, self.mode, item)?;
        }
        db(transaction.commit())
    }
    /// One SQLite transaction covers object heads, histories and all link reads.
    pub fn snapshot<T>(
        &mut self,
        read: impl FnOnce(&ReadSnapshot<'_>) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let transaction = db(self
            .connection
            .transaction_with_behavior(TransactionBehavior::Deferred))?;
        let result = read(&ReadSnapshot {
            connection: &transaction,
            mode: self.mode,
        })?;
        db(transaction.commit())?;
        Ok(result)
    }
}
fn append_in(transaction: &Transaction<'_>, mode: StoreMode, item: &Append) -> Result<(), Error> {
    let Append {
        id,
        scope,
        observation,
    } = item;
    let payload = ont_core::observation_payload(observation)?;
    id.validate()?;
    observation.validate()?;
    // Schema-only change kinds are refused by the v1/v2 store paths.
    if matches!(
        observation.change,
        Change::Update | Change::Refresh | Change::Retraction
    ) {
        return Err(Error::InvalidEvidence);
    }
    if !mode.accepts(scope, &id.namespace) {
        return Err(Error::InvalidEvidence);
    }
    let existing: Option<(String, String)> = db(transaction.query_row("SELECT scope,head FROM objects WHERE object_type=?1 AND instance=?2 AND account=?3 AND native_key=?4", params![id.object_type,id.namespace.instance,id.namespace.account,id.native_key], |r| Ok((r.get(0)?, r.get(1)?))).optional())?;
    if let Some((old_scope, head)) = existing {
        if mode.decode_scope(&old_scope, &id.namespace)? != *scope {
            return Err(Error::ScopeConflict);
        }
        if observation.change == Change::Initial || observation.supersedes.as_ref() != Some(&head) {
            return Err(Error::RevisionConflict);
        }
        let duplicate: bool = db(transaction.query_row("SELECT EXISTS(SELECT 1 FROM observations WHERE object_type=?1 AND instance=?2 AND account=?3 AND native_key=?4 AND revision=?5)", params![id.object_type,id.namespace.instance,id.namespace.account,id.native_key,observation.revision], |r| r.get(0)))?;
        if duplicate {
            return Err(Error::RevisionConflict);
        }
    } else {
        if observation.change != Change::Initial || observation.supersedes.is_some() {
            return Err(Error::RevisionConflict);
        }
        let scope_json = serde_json::to_string(scope).map_err(|_| Error::InvalidEvidence)?;
        db(transaction.execute(
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
    db(transaction.execute("INSERT INTO observations SELECT ?1,?2,?3,?4,?5,COALESCE(MAX(ordinal),-1)+1,?6 FROM observations WHERE object_type=?1 AND instance=?2 AND account=?3 AND native_key=?4", params![id.object_type,id.namespace.instance,id.namespace.account,id.native_key,observation.revision,payload]))?;
    db(transaction.execute("UPDATE objects SET head=?5 WHERE object_type=?1 AND instance=?2 AND account=?3 AND native_key=?4", params![id.object_type,id.namespace.instance,id.namespace.account,id.native_key,observation.revision]))?;
    Ok(())
}
impl Snapshot for ReadSnapshot<'_> {
    fn get(&self, id: &Id) -> Result<Option<Object>, Error> {
        let row: Option<(String, String)> = db(self.connection.query_row("SELECT scope,head FROM objects WHERE object_type=?1 AND instance=?2 AND account=?3 AND native_key=?4", params![id.object_type,id.namespace.instance,id.namespace.account,id.native_key], |r| Ok((r.get(0)?,r.get(1)?))).optional())?;
        let Some((scope, head)) = row else {
            return Ok(None);
        };
        let scope = self.mode.decode_scope(&scope, &id.namespace)?;
        let mut statement = db(self.connection.prepare("SELECT payload FROM observations WHERE object_type=?1 AND instance=?2 AND account=?3 AND native_key=?4 ORDER BY ordinal"))?;
        let rows = db(statement.query_map(
            params![
                id.object_type,
                id.namespace.instance,
                id.namespace.account,
                id.native_key
            ],
            |r| r.get::<_, String>(0),
        ))?;
        let mut observations = Vec::new();
        for row in rows {
            observations.push(decode::<Observation>(&db(row)?)?);
        }
        if observations.last().map(|o| &o.revision) != Some(&head) {
            return Err(Error::StoreFailure);
        }
        Ok(Some(Object {
            id: id.clone(),
            scope,
            head,
            observations,
        }))
    }
    fn targets(&self, root: &Id, relation: &str) -> Result<Vec<Id>, Error> {
        // Links resolve scopes through their objects, including the source.
        self.get(root)?;
        let mut statement = db(self.connection.prepare("SELECT target_type,target_instance,target_account,target_key FROM links WHERE source_type=?1 AND source_instance=?2 AND source_account=?3 AND source_key=?4 AND relation=?5 ORDER BY target_type,target_instance,target_account,target_key"))?;
        let rows = db(statement.query_map(
            params![
                root.object_type,
                root.namespace.instance,
                root.namespace.account,
                root.native_key,
                relation
            ],
            |r| {
                Ok(Id {
                    object_type: r.get(0)?,
                    namespace: Namespace {
                        instance: r.get(1)?,
                        account: r.get(2)?,
                    },
                    native_key: r.get(3)?,
                })
            },
        ))?;
        let mut targets = Vec::new();
        for row in rows {
            let id = db(row)?;
            self.get(&id)?.ok_or(Error::StoreFailure)?;
            targets.push(id);
        }
        Ok(targets)
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_v3;
