//! Rebuildable current-head property index; payloads are read only at migration/write.
use super::*;

/// One necessary equality condition, plus properties whose unresolved states
/// must remain visible in the basis. Construct only after filter validation.
pub struct CandidateCondition {
    pub property: String,
    pub values: Vec<Value>,
    pub used: BTreeSet<String>,
}
pub(super) fn exists(c: &Connection) -> Result<bool, StoreRefusal> {
    map_db(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='head_candidates')",
        [],
        |r| r.get(0),
    ))
}
pub(super) fn project(
    c: &Connection,
    set: &SchemaSet,
    head: &SchemaHead,
) -> Result<(), StoreRefusal> {
    let id = &head.id;
    map_db(c.execute("DELETE FROM head_candidates WHERE object_type=?1 AND instance=?2 AND account=?3 AND native_key=?4", params![id.object_type,id.namespace.instance,id.namespace.account,id.native_key]))?;
    if head.retracted {
        return Ok(());
    }
    let obs = &head.head.observation;
    if let Some(props) = set.properties(&id.object_type) {
        for p in props.keys() {
            let value = match obs.fields.get(p) {
                Some(Knowledge::Value(v))
                    if obs.coverage == ont_core::Coverage::Complete
                        && set.pin(&head.schema_pin.domain) == Some(&head.schema_pin) =>
                {
                    Some(v.to_string())
                }
                _ => None,
            };
            map_db(c.execute(
                "INSERT INTO head_candidates VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![
                    id.object_type,
                    id.namespace.instance,
                    id.namespace.account,
                    id.native_key,
                    p,
                    value,
                    format!("{:020}", obs.fresh_until)
                ],
            ))?;
        }
    }
    Ok(())
}
pub(super) fn rebuild(c: &Connection, set: &SchemaSet) -> Result<(), StoreRefusal> {
    map_db(c.execute("DELETE FROM head_candidates", []))?;
    for schema in set.pins().keys().filter_map(|d| set.schema(d)) {
        for ty in schema.object_types.keys() {
            let mut after = None;
            loop {
                let heads = scan_in(c, set, ty, 256, after.as_ref())?;
                if heads.is_empty() {
                    break;
                }
                after = heads.last().map(|h| h.id.clone());
                for head in heads {
                    project(c, set, &head)?;
                }
            }
        }
    }
    Ok(())
}
pub(super) fn ensure(c: &mut Connection, set: &SchemaSet) -> Result<(), StoreRefusal> {
    if exists(c)? {
        return Ok(());
    }
    let tx = map_db(c.transaction_with_behavior(TransactionBehavior::Immediate))?;
    if !exists(&tx)? {
        map_db(tx.execute_batch("CREATE TABLE head_candidates (
            object_type TEXT NOT NULL, instance TEXT NOT NULL, account TEXT NOT NULL,
            native_key TEXT NOT NULL, property TEXT NOT NULL, value TEXT, fresh_until TEXT NOT NULL,
            PRIMARY KEY(object_type,instance,account,native_key,property)) WITHOUT ROWID;
            CREATE INDEX candidates_value ON head_candidates(object_type,property,value,instance,account,native_key);
            CREATE INDEX candidates_fresh ON head_candidates(object_type,property,fresh_until,instance,account,native_key);"))?;
        rebuild(&tx, &refreshed_set(set, &tx)?)?;
    }
    map_db(tx.commit())
}
impl SchemaRead<'_> {
    /// Page candidate metadata in identity order. No observation payload is
    /// touched; the caller must admit each result before head_after_access.
    pub fn candidate_access(
        &self,
        ty: &str,
        condition: Option<&CandidateCondition>,
        now: u64,
        limit: usize,
        after: Option<&Id>,
    ) -> Result<Vec<HeadAccess>, StoreRefusal> {
        let Some(condition) = condition else {
            return self.scan_head_access(ty, limit, after);
        };
        let mut args: Vec<rusqlite::types::Value> = vec![ty.to_string().into()];
        let mut branches = Vec::new();
        let mut add = |p: &str, predicate: &str, value: Option<String>| {
            args.push(p.to_string().into());
            let prop = args.len();
            let expr = if let Some(v) = value {
                args.push(v.into());
                format!("{predicate} ?{}", args.len())
            } else {
                predicate.to_string()
            };
            branches.push(format!("SELECT instance,account,native_key FROM head_candidates WHERE object_type=?1 AND property=?{prop} AND {expr}"));
        };
        for v in &condition.values {
            add(&condition.property, "value =", Some(v.to_string()));
        }
        for p in condition
            .used
            .iter()
            .chain(std::iter::once(&condition.property))
        {
            add(p, "value IS NULL", None);
        }
        add(
            &condition.property,
            "fresh_until <",
            Some(format!("{now:020}")),
        );
        let cursor = after
            .map(|id| {
                (
                    id.namespace.instance.as_str(),
                    id.namespace.account.as_str(),
                    id.native_key.as_str(),
                )
            })
            .unwrap_or(("", "", ""));
        let n = args.len();
        args.extend([
            cursor.0.to_string().into(),
            cursor.1.to_string().into(),
            cursor.2.to_string().into(),
            to_i64(limit as u64)?.into(),
        ]);
        let sql = format!(
            "SELECT o.instance,o.account,o.native_key,o.scope FROM ({}) c JOIN objects o ON o.object_type=?1 AND (o.instance,o.account,o.native_key)=(c.instance,c.account,c.native_key) WHERE (c.instance,c.account,c.native_key)>(?{},?{},?{}) ORDER BY c.instance,c.account,c.native_key LIMIT ?{}",
            branches.join(" UNION "),
            n + 1,
            n + 2,
            n + 3,
            n + 4
        );
        let mut stmt = map_db(self.tx.prepare(&sql))?;
        let rows = map_db(stmt.query_map(rusqlite::params_from_iter(args), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        }))?;
        let mut out = Vec::new();
        for row in rows {
            let (instance, account, native_key, scope) = map_db(row)?;
            out.push(HeadAccess {
                id: Id {
                    object_type: ty.into(),
                    namespace: ont_core::Namespace { instance, account },
                    native_key,
                },
                scope: decode_scope(&scope)?,
                retracted: false,
            });
        }
        Ok(out)
    }
}
