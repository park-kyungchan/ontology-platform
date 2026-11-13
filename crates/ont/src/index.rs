//! `index`: read the sources of the installed schemas and append what
//! changed, one write transaction per source.
//!
//! Per source: read the files (`sources`), map each record to an object of
//! every type that names the source as `owner`, then in one
//! `Store::write` decide per object from the head read inside that
//! transaction: no head or a retracted head is `initial`; different values
//! are `update`; equal values with changed bytes, coverage or schema pin are `refresh`;
//! unchanged stale heads are renewed in place; equal values
//! with a fresh head append nothing. A `records` source carries its own time
//! per record (`T`): see `README.md`, "records", for the rule that replaces
//! `now` as the observation time. With `complete_listing` a non-retracted
//! head whose key is absent from the listing gets a `retraction`.
//!
//! Revision form: `r<N>@<written_at>`, where `N` is 0 for the first
//! observation of an object and the head's `N` plus one afterwards, so a
//! revision is unique per object even when `--now` repeats.
//!
//! A source that fails appends nothing and is reported `source_failed` with
//! a located cause (`code@place`); the other sources still run. A busy or
//! damaged store is not a source failure: it fails the whole command.
use crate::principal::fresh_revision;
use crate::refusal::{open_store, st, store_err};
use crate::sources::{self, Listing, Record};
use crate::{Answer, Context, Failure, Refusal};
use ont_core::schema::{FileSourceKind, ObjectType, Owner, PropertyType, Schema, Transform};
use ont_core::{Change, Coverage, Id, Knowledge, Lineage, Observation, Scope};
use ont_store::Append;
use ont_store::schema_mode::{SchemaHead, StoreRefusal};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// Most existing heads of one source the retraction scan will list.
const MAX_EXISTING_HEADS: usize = 100_000;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexRequest {
    #[serde(default)]
    pub op: Option<String>,
    /// Source names, run in this order; default every source of every
    /// installed schema.
    #[serde(default)]
    pub sources: Option<Vec<String>>,
}
impl IndexRequest {
    /// Parse the strict request (an `op`, when present, must be `index`).
    pub fn parse(bytes: &[u8]) -> Result<Self, Refusal> {
        let req: Self = crate::cli::parse_request(bytes)?;
        if req.op.as_deref().is_some_and(|op| op != "index") {
            return Err(Refusal::input(
                "invalid_request",
                "/op",
                "must be index when non-null",
            ));
        }
        Ok(req)
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct Counts {
    initial: u64,
    update: u64,
    refresh: u64,
    renewed: u64,
    retraction: u64,
    unchanged: u64,
}

/// Why one source failed: `code@at`.
struct Failed {
    code: String,
    at: String,
    records: usize,
    skipped_symlinks: usize,
}
impl Failed {
    fn new(code: impl Into<String>, at: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            at: at.into(),
            records: 0,
            skipped_symlinks: 0,
        }
    }
}

/// One object ready to append.
struct Mapped {
    id: Id,
    fields: BTreeMap<String, Knowledge>,
    coverage: Coverage,
    sha256: String,
    /// The record's own time (`records` sources only).
    time: Option<u64>,
    /// `path` or `path:line`, for a failure cause.
    place: String,
}

pub fn index(ctx: &Context, req: &IndexRequest) -> Result<Answer, Failure> {
    if let Some(names) = &req.sources {
        let distinct: BTreeSet<&String> = names.iter().collect();
        if names.is_empty() || distinct.len() != names.len() {
            return Err(Refusal::input(
                "invalid_request",
                "/sources",
                "a nonempty array of distinct source names",
            )
            .into());
        }
    }
    let mut store = open_store(&ctx.store, true)?;
    let _run_lock = take_run_lock(&ctx.store)?;
    let set = st!(store.schema_set()).clone();
    let revision = fresh_revision()?;
    // (domain, source) in the order to run.
    let mut plan: Vec<(String, String)> = Vec::new();
    match &req.sources {
        None => {
            for domain in set.pins().keys() {
                if let Some(schema) = set.schema(domain) {
                    for name in schema.sources.keys() {
                        plan.push((domain.clone(), name.clone()));
                    }
                }
            }
        }
        Some(names) => {
            for name in names {
                let before = plan.len();
                for domain in set.pins().keys() {
                    if set
                        .schema(domain)
                        .is_some_and(|s| s.sources.contains_key(name))
                    {
                        plan.push((domain.clone(), name.clone()));
                    }
                }
                if plan.len() == before {
                    return Err(Refusal::input(
                        "unknown_source",
                        "/sources",
                        "source names declared in installed schemas; use describe",
                    )
                    .into());
                }
            }
        }
    }
    let mut entries = Vec::new();
    let mut all_ok = true;
    for (domain, name) in &plan {
        let schema = set
            .schema(domain)
            .ok_or(Failure::Internal(crate::internal!("schema_missing")))?;
        let kind = schema.sources[name].kind.clone();
        let mut entry = json!({"domain":domain,"source":name,"kind":kind});
        match run_source(&mut store, schema, name, ctx.now)? {
            Ok((listing_records, skipped, counts)) => {
                entry["records"] = json!(listing_records);
                entry["skipped_symlinks"] = json!(skipped);
                entry["appended"] = json!({
                    "initial":counts.initial,
                    "update":counts.update,
                    "refresh":counts.refresh,
                    "retraction":counts.retraction
                });
                entry["renewed"] = json!(counts.renewed);
                entry["unchanged"] = json!(counts.unchanged);
                entry["status"] = json!("ok");
            }
            Err(f) => {
                all_ok = false;
                entry["records"] = json!(f.records);
                entry["skipped_symlinks"] = json!(f.skipped_symlinks);
                entry["appended"] = json!({"initial":0,"update":0,"refresh":0,"retraction":0});
                entry["renewed"] = json!(0);
                entry["unchanged"] = json!(0);
                entry["status"] = json!("source_failed");
                entry["cause"] = json!(format!("{}@{}", f.code, f.at));
                entry["code"] = json!(f.code);
            }
        }
        entries.push(entry);
    }
    let pins: BTreeMap<_, _> = plan
        .iter()
        .filter_map(|(d, _)| set.pin(d).map(|p| (d.clone(), p.clone())))
        .collect();
    let result = json!({"sources":entries});
    Ok(if all_ok {
        Answer::Enveloped {
            pins,
            revision,
            result,
        }
    } else {
        Answer::Failed {
            pins,
            revision,
            result,
        }
    })
}

/// One `index` run at a time per store: an exclusive non-blocking advisory
/// lock on `<store>.index.lock` (mode 0600, opened `O_NOFOLLOW`, so a symlink
/// at that name is not followed), held until the returned file is dropped.
/// A held lock is the refusal `index_busy`; a lock file that cannot be opened
/// or created (a symlink, a directory without write permission) is the
/// refusal `index_lock_unavailable`. The lock file is never removed.
fn take_run_lock(store: &std::path::Path) -> Result<std::fs::File, Failure> {
    use rustix::fs::OFlags;
    use std::os::unix::fs::OpenOptionsExt;
    let mut name = store.as_os_str().to_os_string();
    name.push(".index.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags((OFlags::NOFOLLOW | OFlags::CLOEXEC).bits() as i32)
        .open(&name)
        .map_err(|_| Refusal::new("index_lock_unavailable"))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(Refusal::new("index_busy").into()),
        Err(_) => Err(Failure::Internal(crate::internal!("index_lock_failed"))),
    }
}

/// Run one source. The outer `Err` is a failure of the whole command (a busy
/// or damaged store); the inner `Err` is a failed source.
#[allow(clippy::type_complexity)]
fn run_source(
    store: &mut ont_store::Store,
    schema: &Schema,
    name: &str,
    now: u64,
) -> Result<Result<(usize, usize, Counts), Failed>, Failure> {
    let def = &schema.sources[name];
    let spec = match def.file_spec() {
        Ok(Some(spec)) => spec,
        Ok(None) => {
            return Ok(Err(Failed::new(
                "source_kind_unsupported",
                format!("/sources/{name}/kind"),
            )));
        }
        Err(e) => {
            return Ok(Err(Failed::new(
                e.code,
                format!("/sources/{name}{}", e.pointer),
            )));
        }
    };
    let owned: Vec<(&String, &ObjectType)> = schema
        .object_types
        .iter()
        .filter(|(_, t)| t.owner == Owner::Source(name.to_string()))
        .collect();
    let read = match (spec.kind, def.records_options()) {
        (FileSourceKind::Records, Ok(Some(opts))) => match owned.as_slice() {
            [(owner_type, _)] => {
                sources::read_records(&spec, &opts, owner_type, &sources::Bounds::default())
            }
            _ => {
                return Ok(Err(Failed::new(
                    "records_source_owner",
                    format!("/sources/{name}"),
                )));
            }
        },
        _ => sources::read_files(&spec),
    };
    let Listing {
        records,
        skipped_symlinks,
    } = match read {
        Ok(l) => l,
        Err(e) => return Ok(Err(Failed::new(e.code, e.at))),
    };
    let record_count = records.len();
    let fail = |code: &str, at: String| -> Result<Result<_, Failed>, Failure> {
        Ok(Err(Failed {
            code: code.to_string(),
            at,
            records: record_count,
            skipped_symlinks,
        }))
    };
    if !owned.is_empty() && !schema.scope_keys.is_empty() {
        return fail("scope_keys_unmapped", "/scope_keys".to_string());
    }
    let Some(fresh_until) = now.checked_add(spec.fresh_for_s) else {
        return fail(
            "fresh_until_overflow",
            format!("/sources/{name}/fresh_for_s"),
        );
    };
    let scope = Scope::domain(
        schema.namespace.clone(),
        schema.domain.clone(),
        BTreeMap::new(),
    )
    .map_err(|_| Failure::Internal(crate::internal!("scope_invalid")))?;
    let mut mapped: Vec<Mapped> = Vec::new();
    let mut seen: BTreeSet<Id> = BTreeSet::new();
    for (type_name, ty) in &owned {
        for record in &records {
            match map_record(schema, type_name, ty, record) {
                Ok(m) => {
                    if !seen.insert(m.id.clone()) {
                        return fail("duplicate_key", record.place());
                    }
                    if m.time.is_some_and(|t| t > now) {
                        return fail("record_time_in_future", record.place());
                    }
                    mapped.push(m);
                }
                Err(code) => return fail(code, record.place()),
            }
        }
    }
    // Retraction candidates: the non-retracted heads of the owned types.
    let mut existing: Vec<Id> = Vec::new();
    if spec.complete_listing {
        for (type_name, _) in &owned {
            let mut after: Option<Id> = None;
            loop {
                let page = store
                    .scan_heads(type_name, 1_000, after.as_ref())
                    .map_err(store_err)?;
                let done = page.len() < 1_000;
                if let Some(last) = page.last() {
                    after = Some(last.id.clone());
                }
                existing.extend(page.into_iter().map(|h| h.id));
                if existing.len() > MAX_EXISTING_HEADS {
                    return fail("existing_heads_too_many", (*type_name).clone());
                }
                if done {
                    break;
                }
            }
        }
    }
    let listing_sha = {
        let mut h = Sha256::new();
        for r in &records {
            h.update(r.path.as_bytes());
            h.update(b"\n");
        }
        format!("{:x}", h.finalize())
    };
    let mut aborted: Option<(String, String)> = None;
    let result = store.write(now, |tx| {
        let store_pin = tx.schema_set().pin(&schema.domain).cloned();
        let mut counts = Counts::default();
        let mut keys: BTreeSet<&Id> = BTreeSet::new();
        for m in &mapped {
            keys.insert(&m.id);
            let head = tx.head(&m.id)?;
            // The time of the observation this record makes: the run's time,
            // or for a `records` source the record's own time `T`.
            let at = m.time.unwrap_or(now);
            let (change, supersedes, n, observed_at) = match &head {
                None => (Change::Initial, None, 0, at),
                Some(h) => {
                    let obs = &h.head.observation;
                    let Some(n) = revision_number(&obs.revision).and_then(|n| n.checked_add(1))
                    else {
                        aborted = Some(("revision_unparseable".into(), m.id.native_key.clone()));
                        return Err(StoreRefusal::new("index_aborted"));
                    };
                    if h.retracted {
                        (Change::Initial, None, n, at)
                    } else if obs.fields != m.fields {
                        if m.time.is_some() && at < obs.observed_at {
                            // Older content over newer: the source fails.
                            aborted = Some(("record_older_than_head".into(), m.place.clone()));
                            return Err(StoreRefusal::new("index_aborted"));
                        }
                        (Change::Update, Some(obs.revision.clone()), n, at)
                    } else if obs.lineage.artifact_sha256 != m.sha256
                        || obs.coverage != m.coverage
                        || Some(&h.schema_pin) != store_pin.as_ref()
                    {
                        let later = if m.time.is_some() {
                            at.max(obs.observed_at)
                        } else {
                            at
                        };
                        (Change::Refresh, Some(obs.revision.clone()), n, later)
                    } else if now > obs.fresh_until || (m.time.is_some() && at > obs.observed_at) {
                        let later = if m.time.is_some() {
                            at.max(obs.observed_at)
                        } else {
                            at
                        };
                        tx.renew(&m.id, later, fresh_until)?;
                        counts.renewed += 1;
                        continue;
                    } else {
                        counts.unchanged += 1;
                        continue;
                    }
                }
            };
            match change {
                Change::Initial => counts.initial += 1,
                Change::Update => counts.update += 1,
                _ => counts.refresh += 1,
            }
            tx.append(&Append {
                id: m.id.clone(),
                scope: scope.clone(),
                observation: Observation {
                    revision: format!("r{n}@{now}"),
                    source_revision: m.sha256.clone(),
                    observed_at,
                    fresh_until,
                    change,
                    supersedes,
                    coverage: m.coverage.clone(),
                    fields: m.fields.clone(),
                    lineage: Lineage {
                        artifact_sha256: m.sha256.clone(),
                        pointer: String::new(),
                    },
                },
            })?;
        }
        if spec.complete_listing {
            for id in &existing {
                if keys.contains(id) {
                    continue;
                }
                let Some(head): Option<SchemaHead> = tx.head(id)? else {
                    continue;
                };
                if head.retracted {
                    continue;
                }
                let obs = &head.head.observation;
                let Some(n) = revision_number(&obs.revision).and_then(|n| n.checked_add(1)) else {
                    aborted = Some(("revision_unparseable".into(), id.native_key.clone()));
                    return Err(StoreRefusal::new("index_aborted"));
                };
                counts.retraction += 1;
                tx.append(&Append {
                    id: id.clone(),
                    scope: scope.clone(),
                    observation: Observation {
                        revision: format!("r{n}@{now}"),
                        source_revision: listing_sha.clone(),
                        observed_at: now,
                        fresh_until,
                        change: Change::Retraction,
                        supersedes: Some(obs.revision.clone()),
                        coverage: Coverage::Complete,
                        fields: BTreeMap::new(),
                        lineage: Lineage {
                            artifact_sha256: listing_sha.clone(),
                            pointer: String::new(),
                        },
                    },
                })?;
            }
        }
        Ok(counts)
    });
    match result {
        Ok(counts) => Ok(Ok((record_count, skipped_symlinks, counts))),
        Err(e) => {
            if let Some((code, at)) = aborted {
                return fail(&code, at);
            }
            if e.code == "store_busy" || e.code == "store_failure" {
                return Err(store_err(e));
            }
            let at = e
                .violation
                .as_ref()
                .map(|v| v.pointer.clone())
                .unwrap_or_default();
            let code = match &e.violation {
                Some(v) => format!("{}:{}", e.code, v.code),
                None => e.code.to_string(),
            };
            fail(&code, at)
        }
    }
}

/// `N` of a revision `r<N>@<written_at>`.
fn revision_number(revision: &str) -> Option<u64> {
    revision
        .strip_prefix('r')?
        .split_once('@')?
        .0
        .parse::<u64>()
        .ok()
}

/// Map one record to one object of `ty`.
fn map_record(
    schema: &Schema,
    type_name: &str,
    ty: &ObjectType,
    record: &Record,
) -> Result<Mapped, &'static str> {
    let key = resolve_mapped(
        &record.value,
        ty.primary_key.from.as_deref(),
        ty.primary_key.template.as_deref(),
        ty.primary_key.transform.as_ref(),
    );
    let Some(Value::String(native_key)) = key else {
        return Err("key_unresolved");
    };
    let id = Id {
        object_type: type_name.to_string(),
        namespace: schema.namespace.clone(),
        native_key,
    };
    if id.validate().is_err() {
        return Err("key_invalid");
    }
    let mut fields = BTreeMap::new();
    let mut coverage = Coverage::Complete;
    for (pname, prop) in &ty.properties {
        let raw = resolve_mapped(
            &record.value,
            prop.from.as_deref(),
            prop.template.as_deref(),
            prop.transform.as_ref(),
        );
        // A reference property is built from the resolved string: the
        // value the store and `get` already use for a reference is the
        // full `Id` object (see `validate_value` in ont-core schema).
        let value = match &prop.ty {
            PropertyType::Ref(r) => raw.and_then(|v| reference_value(schema, &r.target, &v)),
            _ => raw,
        };
        match value {
            Some(v) => {
                fields.insert(pname.clone(), Knowledge::Value(v));
            }
            None => {
                // An unresolved value is `missing` (the design's state for a
                // `Key: value` line that is not there); a required one makes
                // the observation `partial`.
                fields.insert(pname.clone(), Knowledge::Missing);
                if prop.required {
                    coverage = Coverage::Partial;
                }
            }
        }
    }
    Ok(Mapped {
        id,
        fields,
        coverage,
        sha256: record.sha256.clone(),
        time: record.time,
        place: record.place(),
    })
}

/// The value a mapping resolves to in `record`, after the transform.
/// `None` when the pointer does not resolve, resolves to null, a template
/// field is absent or not a string, or the transform does not apply.
fn resolve_mapped(
    record: &Value,
    from: Option<&str>,
    template: Option<&str>,
    transform: Option<&Transform>,
) -> Option<Value> {
    let value = match (from, template) {
        (Some(pointer), None) => {
            let value = record.pointer(pointer)?;
            if value.is_null() {
                return None;
            }
            value.clone()
        }
        (None, Some(tpl)) => Value::String(resolve_template(record, tpl)?),
        _ => return None,
    };
    apply_transform(&value, transform)
}

/// A template with each `{field}` replaced by the record's top-level
/// string field of that name; `None` when a named field is absent or is
/// not a string, or the template is malformed.
fn resolve_template(record: &Value, template: &str) -> Option<String> {
    let names = ont_core::schema::template_placeholders(template).ok()?;
    let mut out = String::new();
    let mut rest = template;
    let mut used = 0usize;
    while !rest.is_empty() {
        if let Some(stripped) = rest.strip_prefix('{') {
            let name = names.get(used)?;
            used += 1;
            let after = &stripped[name.len() + 1..];
            let field = record.get(name)?.as_str()?;
            out.push_str(field);
            rest = after;
        } else {
            let c = rest.chars().next()?;
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    Some(out)
}

fn apply_transform(value: &Value, transform: Option<&Transform>) -> Option<Value> {
    match transform {
        None => Some(value.clone()),
        Some(Transform::Named(n)) if n == "first_word" => value
            .as_str()
            .and_then(|s| s.split_whitespace().next())
            .map(|w| json!(w)),
        Some(Transform::Named(n)) if n == "parse_integer" => match value {
            Value::Number(num) if num.is_i64() || num.is_u64() => Some(value.clone()),
            Value::String(s) => s.trim().parse::<i64>().ok().map(|n| json!(n)),
            _ => None,
        },
        Some(Transform::SegmentAfter(prefix)) => {
            let s = value.as_str()?;
            let rest = s.strip_prefix(prefix.as_str())?;
            let segment = rest.split('/').next()?;
            if segment.is_empty() || segment == "." || segment == ".." {
                None
            } else {
                Some(json!(segment))
            }
        }
        Some(Transform::ReplacePrefix(pair)) => {
            let s = value.as_str()?;
            let rest = s.strip_prefix(pair[0].as_str())?;
            Some(json!(format!("{}{rest}", pair[1])))
        }
        Some(_) => None,
    }
}

/// Build a reference value from a resolved string: the full `Id` object
/// (`{"object_type":T,"namespace":{...},"native_key":K}`) that
/// `validate_value` accepts for a `ref` property and that `get` returns.
/// An empty string is unresolved, never a reference to an empty key; a
/// value that is already such an `Id` object for the target is kept.
fn reference_value(schema: &Schema, target: &str, value: &Value) -> Option<Value> {
    match value {
        Value::String(s) => {
            if s.is_empty() {
                return None;
            }
            let id = Id {
                object_type: target.to_string(),
                namespace: schema.namespace.clone(),
                native_key: s.clone(),
            };
            if id.validate().is_err() {
                return None;
            }
            serde_json::to_value(id).ok()
        }
        Value::Object(_) => {
            let id: Id = serde_json::from_value(value.clone()).ok()?;
            if id.object_type != target || id.validate().is_err() {
                return None;
            }
            Some(value.clone())
        }
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn transforms_and_pointers() {
        let named = |n: &str| Some(Transform::Named(n.to_string()));
        let rec = json!({"a":"  hello world ","n":"42","m":" -7 ","x":"4x","k":null,"i":5});
        assert_eq!(
            resolve_mapped(&rec, Some("/a"), None, named("first_word").as_ref()),
            Some(json!("hello"))
        );
        assert_eq!(
            resolve_mapped(&rec, Some("/n"), None, named("parse_integer").as_ref()),
            Some(json!(42))
        );
        assert_eq!(
            resolve_mapped(&rec, Some("/m"), None, named("parse_integer").as_ref()),
            Some(json!(-7))
        );
        assert_eq!(
            resolve_mapped(&rec, Some("/x"), None, named("parse_integer").as_ref()),
            None
        );
        assert_eq!(
            resolve_mapped(&rec, Some("/i"), None, named("parse_integer").as_ref()),
            Some(json!(5))
        );
        assert_eq!(resolve_mapped(&rec, Some("/k"), None, None), None);
        assert_eq!(resolve_mapped(&rec, Some("/zz"), None, None), None);
        assert_eq!(
            resolve_mapped(&rec, Some("/a"), None, named("other").as_ref()),
            None
        );
        // Templates and the two object transforms.
        let rec = json!({"dir":"my-task","stem_id":"O5a","path":"/w/my-task/TASK-O5a.md"});
        assert_eq!(
            resolve_mapped(&rec, None, Some("{dir}/{stem_id}"), None),
            Some(json!("my-task/O5a"))
        );
        assert_eq!(resolve_mapped(&rec, None, Some("{dir}/{nope}"), None), None);
        let seg = Transform::SegmentAfter("/w/".to_string());
        assert_eq!(
            resolve_mapped(&rec, Some("/path"), None, Some(&seg)),
            Some(json!("my-task"))
        );
        assert_eq!(resolve_mapped(&rec, Some("/dir"), None, Some(&seg)), None);
        // `.` and `..` are not segments; `//` is empty; a lone file name is.
        for (value, want) in [
            ("/w/../x", None),
            ("/w/./x", None),
            ("/w//x", None),
            ("/w/x.md", Some(json!("x.md"))),
        ] {
            let rec = json!({"path": value});
            assert_eq!(
                resolve_mapped(&rec, Some("/path"), None, Some(&seg)),
                want,
                "{value}"
            );
        }
        let rep = Transform::ReplacePrefix(["/w/".to_string(), String::new()]);
        assert_eq!(
            resolve_mapped(&rec, Some("/path"), None, Some(&rep)),
            Some(json!("my-task/TASK-O5a.md"))
        );
        assert_eq!(resolve_mapped(&rec, Some("/dir"), None, Some(&rep)), None);
    }

    #[test]
    fn revision_numbers() {
        assert_eq!(revision_number("r0@100"), Some(0));
        assert_eq!(revision_number("r12@5"), Some(12));
        assert_eq!(revision_number("act-1#0"), None);
        assert_eq!(revision_number("r@5"), None);
    }

    /// The run-time stop `scope_keys_unmapped` is a defence behind the check
    /// that `schema install` applies; a hand-built schema reaches it.
    #[test]
    fn scope_keys_unmapped_stops_a_source_at_run_time() {
        let dir = std::env::temp_dir().join(format!("ont-index-unit-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("tasks")).unwrap();
        let store = dir.join("store.sqlite");
        let installed = serde_json::json!({
            "format":"ontology-schema/v1","domain":"work","version":1,
            "namespace":{"instance":"vps://x","account":"root"},
            "scope_keys":[],"value_types":{},"sources":{},
            "object_types":{},"link_types":{},"action_types":{},"functions":{},
            "grants":[]
        });
        let file = dir.join("schema.json");
        std::fs::write(&file, installed.to_string()).unwrap();
        let args: Vec<String> = ["--store", store.to_str().unwrap(), "schema", "install"]
            .into_iter()
            .map(String::from)
            .chain([file.display().to_string()])
            .collect();
        assert_eq!(crate::run(&args, &b""[..], 0, 0).code, 0);
        let mut schema: Schema = serde_json::from_value(serde_json::json!({
            "format":"ontology-schema/v1","domain":"work","version":1,
            "namespace":{"instance":"vps://x","account":"root"},
            "scope_keys":["project"],"value_types":{},
            "sources":{"s":{"kind":"markdown_fields",
                "glob":format!("{}/tasks/*/STATE.md", dir.display()),"fresh_for_s":60}},
            "object_types":{"task":{"owner":{"source":"s"},"primary_key":{"from":"/dir"},
                "properties":{}}},
            "link_types":{},"action_types":{},"functions":{},"grants":[]
        }))
        .unwrap();
        let mut handle = open_store(&store, true).unwrap();
        let failed = match run_source(&mut handle, &schema, "s", 10).unwrap() {
            Err(f) => f,
            Ok(_) => panic!("the source ran"),
        };
        assert_eq!(
            (failed.code.as_str(), failed.at.as_str()),
            ("scope_keys_unmapped", "/scope_keys")
        );
        // Without scope keys the same schema runs (and indexes nothing).
        schema.scope_keys.clear();
        assert!(run_source(&mut handle, &schema, "s", 10).unwrap().is_ok());
        drop(handle);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
