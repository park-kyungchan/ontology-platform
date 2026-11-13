//! Deterministic Markdown views and atomic replacement of generated blocks.
use crate::refusal::{open_store, st};
use crate::search::{self, Filter, SearchRequest};
use crate::{Answer, Context, Failure, Refusal};
use ont_core::schema::{PropertyType, Schema, ViewDef, ViewDirection};
use serde::Deserialize;
use serde_json::{Value, json};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenderRequest {
    #[serde(default)]
    pub op: Option<String>,
    pub view: String,
    #[serde(default)]
    pub args: BTreeMap<String, String>,
}
impl RenderRequest {
    pub fn parse(bytes: &[u8]) -> Result<Self, Refusal> {
        let req: Self = crate::cli::parse_request(bytes)?;
        if req.op.as_deref().is_some_and(|op| op != "render") {
            return Err(Refusal::input(
                "invalid_request",
                "/op",
                "must be render when non-null",
            ));
        }
        Ok(req)
    }
}
fn invalid(name: &str, rule: &str) -> Refusal {
    Refusal::input(
        "invalid_argument",
        &crate::input::pointer("/args", name),
        rule,
    )
    .with("argument", json!(name))
}
fn safe_value(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:/-".contains(&b))
}
fn opening(req: &RenderRequest, schema: &str) -> String {
    let mut s = format!("<!-- ont:render {}", req.view);
    for (name, value) in &req.args {
        s.push_str(&format!(" {name}={value}"));
    }
    format!("{s} schema={schema} -->")
}
// A reference argument is a native key; only eq admits this shorthand.
// Materialize the same full Id that search already compares, without a copy
// of the key in the referenced object's properties.
fn reference_args(filter: &Value, schema: &Schema, view: &ViewDef, out: &mut Vec<String>) {
    if let Some(m) = filter.as_object() {
        for (op, value) in m {
            if op == "eq" {
                if let Some(pair) = value.as_array()
                    && let (Some(property), Some(arg)) = (
                        pair.first().and_then(Value::as_str),
                        pair.get(1)
                            .and_then(Value::as_str)
                            .and_then(|s| s.strip_prefix('$')),
                    )
                    && schema
                        .object_types
                        .get(&view.object_type)
                        .and_then(|t| t.properties.get(property))
                        .is_some_and(|p| matches!(p.ty, PropertyType::Ref(_)))
                {
                    out.push(arg.to_string());
                }
            } else if let Some(conditions) = value.as_array() {
                for condition in conditions {
                    reference_args(condition, schema, view, out);
                }
            }
        }
    }
}
fn substitute_filter(
    filter: &mut Value,
    args: &BTreeMap<String, String>,
    schema: &Schema,
    view: &ViewDef,
) {
    let Some(map) = filter.as_object_mut() else {
        return;
    };
    for (op, value) in map {
        if matches!(op.as_str(), "all" | "any" | "none") {
            if let Some(conditions) = value.as_array_mut() {
                for condition in conditions {
                    substitute_filter(condition, args, schema, view);
                }
            }
        } else if let Some(pair) = value.as_array_mut().filter(|a| a.len() == 2) {
            let reference = pair[0]
                .as_str()
                .and_then(|p| {
                    schema
                        .object_types
                        .get(&view.object_type)?
                        .properties
                        .get(p)
                })
                .and_then(|p| match &p.ty {
                    PropertyType::Ref(r) => Some(r),
                    _ => None,
                });
            fn substitute(
                value: &mut Value,
                args: &BTreeMap<String, String>,
                reference: Option<&ont_core::schema::RefType>,
                schema: &Schema,
            ) {
                if let Some(key) = value
                    .as_str()
                    .and_then(|s| s.strip_prefix('$'))
                    .and_then(|n| args.get(n))
                {
                    *value = match reference {
                        Some(r) => {
                            json!({"object_type":r.target,"namespace":schema.namespace,"native_key":key})
                        }
                        None => json!(key),
                    };
                } else if let Some(items) = value.as_array_mut() {
                    for item in items {
                        substitute(item, args, reference, schema);
                    }
                }
            }
            substitute(&mut pair[1], args, reference, schema);
        }
    }
}
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace("\r\n", " ")
        .replace(['\r', '\n'], " ")
        .replace("<!--", "<\\!--")
        .replace("-->", "--\\>")
}
fn value_text(v: &Value, ty: Option<&PropertyType>) -> String {
    match (v, ty) {
        (Value::Object(m), Some(PropertyType::Ref(_))) => m
            .get("native_key")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        (Value::Array(a), _) => {
            let inner = match ty {
                Some(PropertyType::Array(a)) => Some(a.array.as_ref()),
                _ => None,
            };
            a.iter()
                .map(|v| value_text(v, inner))
                .collect::<Vec<_>>()
                .join(", ")
        }
        (Value::String(s), _) => s.clone(),
        _ => v.to_string(),
    }
}
fn cell(object: &Value, property: &str, schema: &Schema, view: &ViewDef) -> String {
    if property == "$key" {
        return escape(object["id"]["native_key"].as_str().unwrap_or_default());
    }
    if property == "$observed_at" {
        // The head's observation time as `YYYY-MM-DDTHH:MM:SSZ`.
        return object["head"]["observed_at"]
            .as_u64()
            .and_then(|t| crate::action::application_time(t).ok())
            .unwrap_or_default();
    }
    let k = &object["head"]["fields"][property];
    let raw = if k["kind"] == "value" {
        let ty = schema
            .object_types
            .get(&view.object_type)
            .and_then(|t| t.properties.get(property))
            .map(|p| &p.ty);
        let mut text = value_text(&k["value"], ty);
        if object["head"]["stale"] == true {
            text.push_str(" (stale)");
        }
        text
    } else {
        format!("({})", k["kind"].as_str().unwrap_or("missing"))
    };
    escape(&raw)
}
fn order_value<'a>(o: &'a Value, p: &str) -> &'a Value {
    if p == "$key" {
        &o["id"]["native_key"]
    } else {
        &o["head"]["fields"][p]["value"]
    }
}
fn compare(a: &Value, b: &Value) -> Ordering {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => {
            let wide = |n: &serde_json::Number| {
                n.as_i64()
                    .map(i128::from)
                    .or_else(|| n.as_u64().map(i128::from))
            };
            match (wide(a), wide(b)) {
                (Some(a), Some(b)) => a.cmp(&b),
                _ => a
                    .as_f64()
                    .partial_cmp(&b.as_f64())
                    .unwrap_or(Ordering::Equal),
            }
        }
        (Value::String(a), Value::String(b)) => a.cmp(b),
        (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
        _ => a.to_string().cmp(&b.to_string()),
    }
}
fn table_row(cells: impl IntoIterator<Item = String>) -> String {
    format!(
        "| {} |\n",
        cells.into_iter().collect::<Vec<_>>().join(" | ")
    )
}

thread_local! {
    static BETWEEN_PAGES: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}
/// Test seam: run once between search pages on this thread.
#[doc(hidden)]
pub fn __set_between_pages_hook(hook: impl FnOnce() + 'static) {
    BETWEEN_PAGES.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

pub fn render(ctx: &Context, req: &RenderRequest) -> Result<Answer, Failure> {
    let mut store = open_store(&ctx.store, false)?;
    st!(store.read_schema(|tx| read_render(ctx, req, tx)))
}
fn read_render(
    ctx: &Context,
    req: &RenderRequest,
    tx: &ont_store::schema_mode::SchemaRead<'_>,
) -> Result<Answer, Failure> {
    let set = tx.schema_set();
    let (schema, view) = set
        .pins()
        .keys()
        .filter_map(|d| set.schema(d))
        .find_map(|s| s.views.get(&req.view).map(|v| (s, v)))
        .ok_or_else(|| Refusal::new("unavailable"))?;
    // Type admission precedes argument checks without scanning extra heads.
    let principal = crate::principal::Principal::bind(set)?;
    if !principal
        .view()
        .read_types(&schema.domain)
        .contains(&view.object_type)
    {
        return Err(Refusal::new("unavailable").into());
    }
    for name in &view.args {
        if !req.args.contains_key(name) {
            return Err(invalid(name, "required argument declared by the view").into());
        }
    }
    let mut ref_args = Vec::new();
    if let Some(filter) = &view.filter {
        reference_args(filter, schema, view, &mut ref_args);
    }
    for (name, value) in &req.args {
        if ref_args.contains(name) && !safe_value(value) {
            return Err(Refusal::input(
                "reference_argument_unrepresentable",
                &crate::input::pointer("/args", name),
                "nonempty ASCII letters, digits, or . _ : / -",
            )
            .with("argument", json!(name))
            .into());
        }
        if !view.args.contains(name) || !safe_value(value) {
            return Err(invalid(
                name,
                "declared view argument; nonempty ASCII letters, digits, or . _ : / -",
            )
            .into());
        }
    }
    let filter = view
        .filter
        .clone()
        .map(|mut f| {
            substitute_filter(&mut f, &req.args, schema, view);
            serde_json::from_value::<Filter>(f).map_err(|_| {
                Refusal::input(
                    "invalid_request",
                    "/args",
                    "substituted view filter must have the search filter form",
                )
            })
        })
        .transpose()?;
    let mut request = SearchRequest {
        op: None,
        object_type: view.object_type.clone(),
        filter,
        limit: Some(1000),
        continuation: None,
    };
    let mut objects = Vec::new();
    let mut scanned = 0;
    loop {
        let Answer::Enveloped { result, .. } = search::read_search(
            ctx,
            &request,
            1000,
            crate::search::SCAN_LIMIT - scanned as usize,
            tx,
        )?
        else {
            return Err(crate::internal!("render_search_failed").into());
        };
        if result["basis"]["oversized_excluded"]
            .as_u64()
            .is_some_and(|n| n > 0)
        {
            return Err(Refusal::new(ont_core::Error::ObservationTooLarge.code()).into());
        }
        scanned += result["basis"]["scanned"].as_u64().unwrap_or(0);
        if scanned > crate::search::SCAN_LIMIT as u64 {
            return Err(Refusal::new("result_too_large").into());
        }
        if let Some(rows) = result["objects"].as_array() {
            objects.extend(rows.iter().cloned());
        }
        request.continuation = result["continuation"].as_str().map(str::to_string);
        if request.continuation.is_none() {
            break;
        }
        let hook = BETWEEN_PAGES.with(|slot| slot.borrow_mut().take());
        if let Some(hook) = hook {
            hook();
        }
    }
    objects.sort_by(|a, b| {
        for order in &view.order {
            let c = compare(
                order_value(a, &order.property),
                order_value(b, &order.property),
            );
            let c = if order.direction == ViewDirection::Desc {
                c.reverse()
            } else {
                c
            };
            if c != Ordering::Equal {
                return c;
            }
        }
        compare(&a["id"]["native_key"], &b["id"]["native_key"])
    });
    let schema_id = format!("{}@{}", schema.domain, schema.version);
    let mut markdown = opening(req, &schema_id) + "\n";
    markdown.push_str(&table_row(view.columns.iter().map(|c| escape(&c.header))));
    markdown.push_str(&table_row(view.columns.iter().map(|_| "---".into())));
    let rows = objects.len().min(view.limit as usize);
    for object in objects.iter().take(rows) {
        markdown.push_str(&table_row(
            view.columns
                .iter()
                .map(|c| cell(object, &c.property, schema, view)),
        ));
    }
    let truncated = objects.len() > rows;
    if truncated {
        markdown.push_str(&table_row((0..view.columns.len()).map(|i| {
            if i == 0 {
                format!("(+{} more; limit {})", objects.len() - rows, view.limit)
            } else {
                String::new()
            }
        })));
    }
    markdown.push_str("<!-- /ont:render -->");
    Ok(Answer::Plain(
        json!({"status":"ok","view":req.view,"schema":schema_id,"rows":rows,"truncated":truncated,"markdown":markdown}),
    ))
}

struct Block {
    start: usize,
    end: Option<usize>,
    line: usize,
    request: Option<RenderRequest>,
}
fn parse_open(line: &str) -> Option<RenderRequest> {
    let body = line
        .strip_prefix("<!-- ont:render ")?
        .strip_suffix(" -->")?;
    let mut parts = body.split(' ');
    let view = parts.next()?.to_string();
    if !ont_core::schema::valid_name(&view) {
        return None;
    }
    let mut args = BTreeMap::new();
    let tokens: Vec<_> = parts.collect();
    if tokens
        .last()?
        .strip_prefix("schema=")
        .is_none_or(str::is_empty)
    {
        return None;
    }
    for token in &tokens[..tokens.len() - 1] {
        let (name, value) = token.split_once('=')?;
        if !ont_core::schema::valid_name(name)
            || !safe_value(value)
            || args.insert(name.into(), value.into()).is_some()
        {
            return None;
        }
    }
    Some(RenderRequest {
        op: None,
        view,
        args,
    })
}
/// A fence run after zero to three spaces, plus the remaining bytes.
fn fence_run(line: &[u8]) -> Option<(u8, usize, &[u8])> {
    let indent = line.iter().take_while(|b| **b == b' ').count();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let marker = *rest.first()?;
    if !matches!(marker, b'`' | b'~') {
        return None;
    }
    let length = rest.iter().take_while(|b| **b == marker).count();
    (length >= 3).then_some((marker, length, &rest[length..]))
}

fn blocks(text: &[u8]) -> Result<Vec<Block>, usize> {
    let mut out = Vec::new();
    let mut offset = 0;
    let mut active: Option<usize> = None;
    // Character, length and opening line; a shorter or different run is content.
    let mut fence: Option<(u8, usize, usize)> = None;
    for (i, raw) in text.split_inclusive(|b| *b == b'\n').enumerate() {
        let line_bytes = raw.strip_suffix(b"\n").unwrap_or(raw);
        let line_bytes = line_bytes.strip_suffix(b"\r").unwrap_or(line_bytes);
        let line = std::str::from_utf8(line_bytes).unwrap_or("");
        let run = fence_run(line_bytes);
        if let Some((marker, length, _)) = fence {
            if run.is_some_and(|(c, n, tail)| {
                c == marker && n >= length && tail.iter().all(|b| *b == b' ')
            }) {
                fence = None;
            }
        } else if let Some((marker, length, tail)) = run
            // CommonMark forbids backticks in a backtick fence's info string.
            && (marker != b'`' || !tail.contains(&b'`'))
        {
            fence = Some((marker, length, i + 1));
        } else if line.starts_with("<!-- ont:render") {
            out.push(Block {
                start: offset,
                end: None,
                line: i + 1,
                request: parse_open(line),
            });
            active = Some(out.len() - 1);
        } else if line == "<!-- /ont:render -->"
            && let Some(at) = active.take()
        {
            out[at].end = Some(offset + line.len());
        }
        offset += raw.len();
    }
    match fence {
        Some((_, _, line)) => Err(line),
        None => Ok(out),
    }
}
fn target(path: &Path) -> Result<(Vec<u8>, fs::Metadata), Failure> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(path)
        .map_err(|_| Refusal::new("render_target_invalid"))?;
    let meta = file
        .metadata()
        .map_err(|_| Refusal::new("render_target_invalid"))?;
    if !meta.is_file() || meta.nlink() != 1 {
        return Err(Refusal::new("render_target_invalid").into());
    }
    let mut text = Vec::new();
    file.read_to_end(&mut text)
        .map_err(|_| Refusal::new("render_target_invalid"))?;
    Ok((text, meta))
}
fn plain(answer: Answer) -> Result<Value, Failure> {
    match answer {
        Answer::Plain(v) => Ok(v),
        _ => Err(crate::internal!("render_answer_invalid").into()),
    }
}
pub fn check_file(ctx: &Context, path: &Path) -> Result<Answer, Failure> {
    check_file_count(ctx, path, None)
}
/// Check the caller's recorded count before checking the generated contents.
pub fn check_file_count(
    ctx: &Context,
    path: &Path,
    expected: Option<usize>,
) -> Result<Answer, Failure> {
    let (text, _) = target(path)?;
    let found = blocks(&text).map_err(|line| {
        Refusal::new("render_drift").with(
            "drift",
            json!([{
                "view":"", "args":{}, "line":line, "refusal_code":"render_fence_unclosed"
            }]),
        )
    })?;
    if let Some(expected) = expected
        && found.len() != expected
    {
        return Err(Refusal::new("render_block_count")
            .with("expected", json!(expected))
            .with("found", json!(found.len()))
            .into());
    }
    let mut drift = Vec::new();
    for block in &found {
        let mut refusal_code = None;
        let equal = if let (Some(end), Some(req)) = (block.end, &block.request) {
            match render(ctx, req) {
                Ok(answer) => {
                    plain(answer)?["markdown"].as_str().map(str::as_bytes)
                        == Some(&text[block.start..end])
                }
                Err(Failure::Refused(r)) => {
                    refusal_code = Some(r.code);
                    false
                }
                Err(e) => return Err(e),
            }
        } else {
            false
        };
        if !equal {
            let view = block
                .request
                .as_ref()
                .map(|r| r.view.clone())
                .unwrap_or_default();
            let args = block
                .request
                .as_ref()
                .map(|r| r.args.clone())
                .unwrap_or_default();
            let mut entry = json!({"view":view,"args":args,"line":block.line});
            if let Some(code) = refusal_code {
                entry["refusal_code"] = json!(code);
            }
            drift.push(entry);
        }
    }
    if !drift.is_empty() {
        return Err(Refusal::new("render_drift")
            .with("drift", json!(drift))
            .into());
    }
    Ok(Answer::Plain(
        json!({"status":"ok","blocks":found.len(),"drift":[]}),
    ))
}

/// The persistent sidecar survives target rename, so waiting writers lock
/// the same inode even after another writer publishes its replacement.
fn writer_lock(path: &Path) -> Result<std::fs::File, Failure> {
    let name = path
        .file_name()
        .ok_or_else(|| Refusal::new("render_target_invalid"))?;
    let mut lock_name = name.to_os_string();
    lock_name.push(".ont-render.lock");
    let lock_path = path.with_file_name(lock_name);
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(lock_path)
        .map_err(|_| Refusal::new("render_lock_unavailable"))?;
    let meta = lock
        .metadata()
        .map_err(|_| Refusal::new("render_lock_unavailable"))?;
    if !meta.is_file() || meta.nlink() != 1 {
        return Err(Refusal::new("render_lock_unavailable").into());
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(250);
    loop {
        match rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => return Ok(lock),
            Err(rustix::io::Errno::WOULDBLOCK) => {
                if std::time::Instant::now() >= deadline {
                    return Err(Refusal::new("render_busy").into());
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(_) => return Err(Refusal::new("render_lock_unavailable").into()),
        }
    }
}

pub fn write_file(
    ctx: &Context,
    path: &Path,
    req: &RenderRequest,
    preimage_dir: &Path,
) -> Result<Answer, Failure> {
    let _lock = writer_lock(path)?;
    let (text, meta) = target(path)?;
    let found = blocks(&text)
        .map_err(|line| Refusal::new("render_fence_unclosed").with("line", json!(line)))?;
    let matches: Vec<_> = found
        .iter()
        .filter(|b| {
            b.request
                .as_ref()
                .is_some_and(|r| r.view == req.view && r.args == req.args)
        })
        .collect();
    let block = match matches.as_slice() {
        [] => return Err(Refusal::new("render_block_missing").into()),
        [one] => *one,
        _ => return Err(Refusal::new("render_block_ambiguous").into()),
    };
    let end = block
        .end
        .ok_or_else(|| Refusal::new("render_block_missing"))?;
    // An overlapping/unclosed block may never absorb bytes outside its area.
    if found.iter().any(|b| b.start > block.start && b.start < end) {
        return Err(Refusal::new("render_block_ambiguous").into());
    }
    let answer = plain(render(ctx, req)?)?;
    let markdown = answer["markdown"]
        .as_str()
        .ok_or(crate::internal!("render_answer_invalid"))?;
    let changed = &text[block.start..end] != markdown.as_bytes();
    let mut preimage = None;
    if changed {
        if !preimage_dir.is_dir() {
            return Err(Refusal::new("render_preimage_failed").into());
        }
        let name = path
            .file_name()
            .ok_or_else(|| Refusal::new("render_target_invalid"))?;
        let mut backup_name = name.to_os_string();
        backup_name.push(format!(".{}", &crate::sha256_hex(&text)[..12]));
        let backup = preimage_dir.join(backup_name);
        let saved = (|| -> std::io::Result<()> {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&backup)
            {
                Ok(mut file) => {
                    let result = file
                        .set_permissions(fs::Permissions::from_mode(0o600))
                        .and_then(|()| file.write_all(&text))
                        .and_then(|()| file.sync_all());
                    if result.is_err() {
                        let _ = fs::remove_file(&backup);
                    }
                    result
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let (previous, metadata) =
                        target(&backup).map_err(|_| std::io::Error::other("invalid preimage"))?;
                    if previous != text || metadata.permissions().mode() & 0o777 != 0o600 {
                        return Err(std::io::Error::other("invalid preimage"));
                    }
                    Ok(())
                }
                Err(e) => Err(e),
            }
        })();
        saved.map_err(|_| Refusal::new("render_preimage_failed"))?;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let temp = parent.join(format!(
            ".ont-render-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        let mut created_temp = false;
        let replacement = (|| -> std::io::Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temp)?;
            created_temp = true;
            file.write_all(&text[..block.start])?;
            file.write_all(markdown.as_bytes())?;
            file.write_all(&text[end..])?;
            file.set_permissions(meta.permissions())?;
            file.sync_all()?;
            // Refuse concurrent replacement/edit instead of silently losing it.
            let (current, _) = target(path).map_err(|_| std::io::Error::other("target changed"))?;
            if current != text {
                return Err(std::io::Error::other("target changed"));
            }
            fs::rename(&temp, path)
        })();
        if replacement.is_err() && created_temp {
            let _ = fs::remove_file(&temp);
        }
        replacement.map_err(|_| Refusal::new("render_write_failed"))?;
        preimage = Some(backup.display().to_string());
    }
    Ok(Answer::Plain(
        json!({"status":"ok","file":path,"view":req.view,"changed":changed,"rows":answer["rows"],"preimage":preimage}),
    ))
}

/// The schema command's ordinary checks cover one domain; check view names
/// against installed domains too, without changing the schema operation API.
pub(crate) fn check_view_names(ctx: &Context, file: &Path) -> Result<(), Failure> {
    if !ctx.store.exists() {
        return Ok(());
    }
    let bytes = fs::read(file).map_err(|_| Refusal::new("schema_file_unreadable"))?;
    let candidate =
        Schema::parse(&bytes).map_err(|_| crate::schema::schema_input_refusal(&bytes, &[]))?;
    let mut store = open_store(&ctx.store, false)?;
    let set = st!(store.schema_set());
    for domain in set.pins().keys().filter(|d| *d != &candidate.domain) {
        if let Some(schema) = set.schema(domain) {
            for name in candidate.views.keys() {
                if schema.views.contains_key(name) {
                    return Err(Refusal::input(
                        "schema_violation",
                        &crate::input::pointer("/views", name),
                        "view name must be unique across installed object types and views",
                    )
                    .with(
                        "violations",
                        json!([{"code":"duplicate_type_name","pointer":format!("/views/{name}")}]),
                    )
                    .into());
                }
            }
        }
    }
    Ok(())
}
