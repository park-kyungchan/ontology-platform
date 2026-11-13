//! Rebuildable current-head property index; payloads are read only at migration/write.
use super::*;

/// One necessary equality condition, plus properties whose unresolved states
/// must remain visible in the basis. `v<value>` posts a complete current-pin
/// value, `?` a stored value that cannot be posted, and `-` no value.
/// Search readers set `absent` to retain no-value basis effects; reverse linked
/// readers leave it false. Construct only after filter validation.
pub struct CandidateCondition {
    pub property: String,
    pub values: Vec<Value>,
    pub used: BTreeSet<String>,
    pub absent: bool,
}
pub(super) fn exists(c: &Connection) -> Result<bool, StoreRefusal> {
    map_db(c.query_row(
        "SELECT (SELECT user_version FROM pragma_user_version)=4 AND EXISTS(SELECT 1 FROM sqlite_master WHERE name='head_candidates') AND EXISTS(SELECT 1 FROM sqlite_master WHERE name='candidate_generation')",
        [],
        |r| r.get(0),
    ))
}
// Bump whenever posting meaning changes; generation equality alone cannot
// certify a projection built by an older engine. Zero/missing means unversioned.
const SEMANTICS_VERSION: i64 = 1;
fn has_generation_column(c: &Connection, name: &str) -> Result<bool, StoreRefusal> {
    map_db(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('candidate_generation') WHERE name=?1)",
        [name],
        |r| r.get(0),
    ))
}
pub(super) fn semantics_current(c: &Connection) -> Result<bool, StoreRefusal> {
    if !exists(c)?
        || !has_generation_column(c, "semantics_version")?
        || !has_generation_column(c, "semantics_projected")?
    {
        return Ok(false);
    }
    let recorded = map_db(
        c.query_row(
            "SELECT semantics_version,semantics_projected=projected FROM candidate_generation WHERE singleton=1",
            [],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, bool>(1)?)),
        )
        .optional(),
    )?;
    Ok(recorded == Some((SEMANTICS_VERSION, true)))
}
pub(super) type TriggerCache = std::cell::Cell<Option<(i64, bool)>>;
fn triggers() -> impl Iterator<Item = (String, String)> {
    ["objects", "observations", "schema_installs"]
        .into_iter()
        .flat_map(|table| {
            ["INSERT", "UPDATE", "DELETE"]
                .into_iter()
                .map(move |event| {
                    let name = format!("candidate_{table}_{event}");
                    let sql = format!(
                        "CREATE TRIGGER {name}
                    AFTER {event} ON {table} BEGIN
                    UPDATE candidate_generation SET mutation=mutation+1 WHERE singleton=1; END"
                    );
                    (name, sql)
                })
        })
}
pub(super) fn protected(c: &Connection, cache: &TriggerCache) -> Result<bool, StoreRefusal> {
    let cookie = map_db(c.pragma_query_value(None, "schema_version", |r| r.get(0)))?;
    if let Some((previous, valid)) = cache.get()
        && previous == cookie
    {
        return Ok(valid);
    }
    let mut valid = true;
    for (name, expected) in triggers() {
        let actual: Option<String> = map_db(
            c.query_row(
                "SELECT sql FROM sqlite_master WHERE type='trigger' AND name=?1",
                [name],
                |r| r.get(0),
            )
            .optional(),
        )?;
        valid &= actual.as_ref() == Some(&expected);
    }
    cache.set(Some((cookie, valid)));
    Ok(valid)
}
pub(super) fn repair_triggers(c: &Connection) -> Result<(), StoreRefusal> {
    for (name, sql) in triggers() {
        map_db(c.execute_batch(&format!("DROP TRIGGER IF EXISTS {name}; {sql}")))?;
    }
    Ok(())
}
const GENERATION_CHECK: &str = "SELECT mutation=projected AND semantics_projected=projected,semantics_version FROM candidate_generation WHERE singleton=1";
pub(super) fn current(c: &Connection, cache: &TriggerCache) -> Result<bool, StoreRefusal> {
    if !protected(c, cache)? {
        return Ok(false);
    }
    let (projected, semantics) = map_db(c.query_row(GENERATION_CHECK, [], |r| {
        Ok((r.get::<_, bool>(0)?, r.get::<_, i64>(1)?))
    }))?;
    Ok(projected && semantics == SEMANTICS_VERSION)
}
pub(super) fn require_current(c: &Connection, cache: &TriggerCache) -> Result<(), StoreRefusal> {
    if current(c, cache)? {
        Ok(())
    } else {
        Err(StoreRefusal::new("projection_stale"))
    }
}
pub(super) fn checkpoint(c: &Connection) -> Result<(), StoreRefusal> {
    map_db(c.execute(
        "UPDATE candidate_generation SET projected=mutation,semantics_projected=mutation,semantics_version=?1 WHERE singleton=1",
        [SEMANTICS_VERSION],
    ))?;
    Ok(())
}
#[cfg(test)]
thread_local! { static INTERRUPT_REBUILD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
pub(super) fn project(
    c: &Connection,
    set: &SchemaSet,
    head: &SchemaHead,
) -> Result<(), StoreRefusal> {
    project_access(
        c,
        set,
        &HeadAccess {
            id: head.id.clone(),
            scope: head.scope.clone(),
            retracted: head.retracted,
        },
        Some((&head.head.observation, &head.schema_pin)),
    )
}
fn project_access(
    c: &Connection,
    set: &SchemaSet,
    access: &HeadAccess,
    observation: Option<(&Observation, &SchemaPin)>,
) -> Result<(), StoreRefusal> {
    let id = &access.id;
    map_db(c.execute("DELETE FROM head_candidates WHERE object_type=?1 AND instance=?2 AND account=?3 AND native_key=?4", params![id.object_type,id.namespace.instance,id.namespace.account,id.native_key]))?;
    if access.retracted {
        return Ok(());
    }
    let schema = set
        .schema_of(&id.object_type)
        .ok_or_else(StoreRefusal::store_failure)?;
    // Materialize each matching read-scope predicate as an indexed partition.
    // This includes namespace/domain and every required scope key; omitted
    // grant keys keep native wildcard semantics. Schema installs rebuild it.
    let mut visibility = BTreeSet::new();
    for grant in &schema.grants {
        let scope = Scope::domain(
            schema.namespace.clone(),
            schema.domain.clone(),
            grant.scope.clone(),
        )
        .map_err(StoreRefusal::from)?;
        if grant.read.contains(&id.object_type) && scope.matches(&access.scope) {
            visibility
                .insert(serde_json::to_string(&scope).map_err(|_| StoreRefusal::store_failure())?);
        }
    }
    let scope_keys = match &access.scope {
        Scope::Domain { keys, .. } => json_text(keys)?,
        _ => return Err(StoreRefusal::store_failure()),
    };
    let mut postings = vec![(String::new(), String::new())];
    if let Some(props) = set.properties(&id.object_type) {
        for p in props.keys() {
            let value = match observation
                .and_then(|(obs, pin)| obs.fields.get(p).map(|field| (obs, pin, field)))
            {
                Some((obs, pin, Knowledge::Value(v))) => {
                    if obs.coverage == ont_core::Coverage::Complete
                        && set.pin(&pin.domain) == Some(pin)
                    {
                        format!("v{v}")
                    } else {
                        "?".into()
                    }
                }
                _ => "-".into(),
            };
            postings.push((p.clone(), value));
        }
    }
    // Sixteen radix prefixes per expiry permit an ordered range to be read
    // as <=240 disjoint equality streams, each already in object-key order.
    if let Some((obs, _)) = observation.filter(|(obs, _)| obs.fresh_until < crate::NEVER_STALE) {
        let expiry = format!("{:016x}", obs.fresh_until);
        for n in 1..=16 {
            postings.push(("$stale".into(), expiry[..n].into()));
        }
    }
    let mut insert =
        map_db(c.prepare_cached("INSERT INTO head_candidates VALUES (?1,?2,?3,?4,?5,?6,?7,?8)"))?;
    for scope in visibility {
        for (p, v) in &postings {
            map_db(insert.execute(params![
                id.object_type,
                scope,
                p,
                v,
                id.namespace.instance,
                id.namespace.account,
                id.native_key,
                scope_keys
            ]))?;
        }
    }
    Ok(())
}
pub(super) fn rebuild(c: &Connection, set: &SchemaSet) -> Result<(), StoreRefusal> {
    map_db(c.execute("DELETE FROM head_candidates", []))?;
    #[cfg(test)]
    if INTERRUPT_REBUILD.replace(false) {
        return Err(StoreRefusal::new("test_interrupted_rebuild"));
    }
    for schema in set.pins().keys().filter_map(|d| set.schema(d)) {
        for ty in schema.object_types.keys() {
            let mut after = None;
            loop {
                let heads = access_in(c, None, ty, 256, after.as_ref())?;
                if heads.is_empty() {
                    break;
                }
                after = heads.last().map(|h| h.id.clone());
                for access in heads {
                    if access.retracted {
                        continue;
                    }
                    match head_after_access_in(c, &access) {
                        Ok(Some(head)) => project(c, set, &head)?,
                        // Preserve a visible uncertain posting for every property:
                        // set reads must see and count an excluded legacy head,
                        // including under an equality filter, without loading it.
                        Err(e) if e.code == Error::ObservationTooLarge.code() => {
                            project_access(c, set, &access, None)?
                        }
                        Err(e) => return Err(e),
                        Ok(None) => return Err(StoreRefusal::store_failure()),
                    }
                }
            }
        }
    }
    checkpoint(c)
}
pub(super) fn ensure(c: &mut Connection, set: &SchemaSet) -> Result<(), StoreRefusal> {
    if semantics_current(c)? {
        return Ok(());
    }
    let tx = map_db(c.transaction_with_behavior(TransactionBehavior::Immediate))?;
    // Another opener may already have completed the upgrade while we waited.
    if semantics_current(&tx)? {
        return map_db(tx.commit());
    }
    if !exists(&tx)? {
        map_db(tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS candidate_generation (
            singleton INTEGER PRIMARY KEY CHECK(singleton=1),
            mutation INTEGER NOT NULL CHECK(typeof(mutation)='integer' AND mutation>=0),
            projected INTEGER NOT NULL CHECK(projected>=0 AND projected<=mutation));",
        ))?;
        // All candidate inputs: identity/scope/head, observation contents/pins,
        // and schema bytes (including grants). Links/action_log are not inputs.
        repair_triggers(&tx)?;
        map_db(tx.execute_batch("DROP TABLE IF EXISTS head_candidates;
            CREATE TABLE head_candidates (
            object_type TEXT NOT NULL, visibility TEXT NOT NULL,
            property TEXT NOT NULL, value TEXT NOT NULL, instance TEXT NOT NULL,
            account TEXT NOT NULL, native_key TEXT NOT NULL, scope_keys TEXT NOT NULL,
            PRIMARY KEY(object_type,visibility,property,value,instance,account,native_key)) WITHOUT ROWID;
            CREATE INDEX candidates_head ON head_candidates(object_type,instance,account,native_key);"))?;
    }
    map_db(tx.execute(
        "INSERT OR IGNORE INTO candidate_generation(singleton,mutation,projected) VALUES(1,0,0)",
        [],
    ))?;
    if !has_generation_column(&tx, "semantics_version")? {
        map_db(tx.execute_batch("ALTER TABLE candidate_generation ADD COLUMN semantics_version INTEGER NOT NULL DEFAULT 0 CHECK(semantics_version>=0)"))?;
    }
    if !has_generation_column(&tx, "semantics_projected")? {
        map_db(tx.execute_batch("ALTER TABLE candidate_generation ADD COLUMN semantics_projected INTEGER NOT NULL DEFAULT 0 CHECK(semantics_projected>=0)"))?;
    }
    // Fresh and old-semantic projections use the same projector. DDL, postings,
    // generation and semantics marker commit or roll back in this transaction.
    rebuild(&tx, &refreshed_set(set, &tx)?)?;
    map_db(tx.pragma_update(None, "user_version", 4))?;
    map_db(tx.commit())
}
fn json_text(value: &impl serde::Serialize) -> Result<String, StoreRefusal> {
    serde_json::to_string(value).map_err(|_| StoreRefusal::store_failure())
}
const POSTING_SQL: &str = "SELECT instance,account,native_key,scope_keys FROM head_candidates
    WHERE object_type=?1 AND visibility=?2 AND property=?3 AND value=?4
    AND (instance,account,native_key)>(?5,?6,?7)
    ORDER BY instance,account,native_key LIMIT ?8";
#[cfg(test)]
thread_local! { static STEPS: std::cell::Cell<i32> = const { std::cell::Cell::new(0) }; }
impl SchemaRead<'_> {
    /// Read bounded, visible posting streams and merge their first `limit`
    /// keys. The merge retains <=limit metadata rows; no payloads are read.
    /// Current native read admission is fully represented by these scopes;
    /// the caller still rechecks Authority/session admission before payloads.
    pub fn candidate_access(
        &self,
        ty: &str,
        condition: Option<&CandidateCondition>,
        now: u64,
        limit: usize,
        after: Option<&Id>,
        scopes: &[Scope],
    ) -> Result<Vec<HeadAccess>, StoreRefusal> {
        require_current(self.tx, self.trigger_cache)?;
        candidate_access_in(
            self.tx,
            &self.schemas,
            ty,
            condition,
            now,
            limit,
            after,
            scopes,
        )
    }
}
impl SchemaWrite<'_> {
    /// The write transaction already required a current projection at entry.
    pub fn candidate_access(
        &self,
        ty: &str,
        condition: Option<&CandidateCondition>,
        now: u64,
        limit: usize,
        after: Option<&Id>,
        scopes: &[Scope],
    ) -> Result<Vec<HeadAccess>, StoreRefusal> {
        candidate_access_in(
            self.tx,
            &self.schemas,
            ty,
            condition,
            now,
            limit,
            after,
            scopes,
        )
    }
}
#[allow(clippy::too_many_arguments)]
fn candidate_access_in(
    c: &Connection,
    schemas: &SchemaSet,
    ty: &str,
    condition: Option<&CandidateCondition>,
    now: u64,
    limit: usize,
    after: Option<&Id>,
    scopes: &[Scope],
) -> Result<Vec<HeadAccess>, StoreRefusal> {
    if now > crate::NEVER_STALE {
        return Err(StoreRefusal::new("clock_out_of_range"));
    }
    let schema = schemas
        .schema_of(ty)
        .ok_or_else(StoreRefusal::store_failure)?;
    let mut streams = BTreeSet::new();
    if let Some(condition) = condition {
        for value in &condition.values {
            streams.insert((condition.property.clone(), format!("v{value}")));
        }
        for p in condition
            .used
            .iter()
            .chain(std::iter::once(&condition.property))
        {
            streams.insert((p.clone(), "?".into()));
            if condition.absent {
                streams.insert((p.clone(), "-".into()));
            }
        }
        let clock = format!("{now:016x}");
        for (i, digit) in clock.bytes().enumerate() {
            let digit = (digit as char)
                .to_digit(16)
                .ok_or_else(StoreRefusal::store_failure)?;
            for n in 0..digit {
                streams.insert(("$stale".into(), format!("{}{:x}", &clock[..i], n)));
            }
        }
    } else {
        streams.insert((String::new(), String::new()));
    }
    let visibility: BTreeSet<_> = scopes.iter().map(json_text).collect::<Result<_, _>>()?;
    let cursor = after
        .map(|id| {
            (
                id.namespace.instance.as_str(),
                id.namespace.account.as_str(),
                id.native_key.as_str(),
            )
        })
        .unwrap_or(("", "", ""));
    let mut out = BTreeMap::new();
    for scope in visibility {
        for (p, v) in &streams {
            let mut stmt = map_db(c.prepare_cached(POSTING_SQL))?;
            let rows = map_db(stmt.query_map(
                params![
                    ty,
                    scope,
                    p,
                    v,
                    cursor.0,
                    cursor.1,
                    cursor.2,
                    to_i64(limit as u64)?
                ],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                },
            ))?;
            for row in rows {
                let (instance, account, native_key, keys) = map_db(row)?;
                let id = Id {
                    object_type: ty.into(),
                    namespace: ont_core::Namespace { instance, account },
                    native_key,
                };
                let scope = Scope::domain(
                    schema.namespace.clone(),
                    schema.domain.clone(),
                    decode(&keys).map_err(StoreRefusal::from)?,
                )
                .map_err(StoreRefusal::from)?;
                out.entry(id.clone()).or_insert(HeadAccess {
                    id,
                    scope,
                    retracted: false,
                });
                if out.len() > limit {
                    out.pop_last();
                }
            }
            #[cfg(test)]
            {
                assert_eq!(stmt.get_status(rusqlite::StatementStatus::Sort), 0);
                STEPS.set(STEPS.get() + stmt.reset_status(rusqlite::StatementStatus::VmStep));
            }
        }
    }
    Ok(out.into_values().collect())
}

#[cfg(test)]
#[path = "candidates_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "generation_tests.rs"]
mod generation_tests;
