//! Small trusted SQLite boundary. No source adapters, principal inference or effects.
use ont_core::{Change, Error, Id, Link, Namespace, Object, Observation, Scope, Snapshot};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use std::path::Path;
use std::time::Duration;

pub mod actions;
pub mod fake;

pub const SCHEMA: &str = include_str!("../schema.sql");
const APPLICATION_ID: i64 = 1_330_533_430;
fn db<T>(result: rusqlite::Result<T>) -> Result<T, Error> {
    result.map_err(|_| Error::StoreFailure)
}
fn decode<T: serde::de::DeserializeOwned>(s: &str) -> Result<T, Error> {
    serde_json::from_str(s).map_err(|_| Error::StoreFailure)
}
pub struct Store {
    connection: Connection,
}
pub struct ReadSnapshot<'a> {
    connection: &'a Connection,
}
/// Trusted ingestion input: not exposed by ontd stdin.
#[derive(Clone)]
pub struct Append {
    pub id: Id,
    pub scope: Scope,
    pub observation: Observation,
}
impl Store {
    pub fn memory() -> Result<Self, Error> {
        Self::configure(db(Connection::open_in_memory())?)
    }
    /// Only task-owned synthetic files in S06. No live DB opening or migration.
    pub fn open(path: &Path) -> Result<Self, Error> {
        Self::configure(db(Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ))?)
    }
    fn configure(mut connection: Connection) -> Result<Self, Error> {
        db(connection.busy_timeout(Duration::from_millis(250)))?;
        let app: i64 = db(connection.query_row("PRAGMA application_id", [], |r| r.get(0)))?;
        let version: i64 = db(connection.query_row("PRAGMA user_version", [], |r| r.get(0)))?;
        let count: i64 = db(connection.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
            [],
            |r| r.get(0),
        ))?;
        if app == 0 && version == 0 && count == 0 {
            let transaction =
                db(connection.transaction_with_behavior(TransactionBehavior::Immediate))?;
            db(transaction.execute_batch(SCHEMA))?;
            db(transaction.commit())?;
        } else if app != APPLICATION_ID || version != 1 {
            return Err(Error::StoreFailure);
        }
        db(connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; PRAGMA trusted_schema=OFF;"))?;
        Ok(Self { connection })
    }
    /// Atomic graph initialization, preserving explicit observation append order.
    pub fn seed(&mut self, objects: &[Object], links: &[Link]) -> Result<(), Error> {
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
        let transaction = db(self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate))?;
        for item in items {
            append_in(&transaction, item)?;
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
        })?;
        db(transaction.commit())?;
        Ok(result)
    }
}
fn append_in(transaction: &Transaction<'_>, item: &Append) -> Result<(), Error> {
    let Append {
        id,
        scope,
        observation,
    } = item;
    id.validate()?;
    observation.validate()?;
    if !matches!(scope, Scope::Education { .. })
        || scope.validate().is_err()
        || scope.namespace() != &id.namespace
    {
        return Err(Error::InvalidEvidence);
    }
    let existing: Option<(String, String)> = db(transaction.query_row("SELECT scope,head FROM objects WHERE object_type=?1 AND instance=?2 AND account=?3 AND native_key=?4", params![id.object_type,id.namespace.instance,id.namespace.account,id.native_key], |r| Ok((r.get(0)?, r.get(1)?))).optional())?;
    if let Some((old_scope, head)) = existing {
        if decode::<Scope>(&old_scope)? != *scope {
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
    let payload = serde_json::to_string(observation).map_err(|_| Error::InvalidEvidence)?;
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
            scope: decode(&scope)?,
            head,
            observations,
        }))
    }
    fn targets(&self, root: &Id, relation: &str) -> Result<Vec<Id>, Error> {
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
        rows.map(db).collect()
    }
}
