//! Explicit page compaction and candidate projection rebuild, with a verified backup.
use crate::principal::fresh_revision;
use crate::refusal::{io, open_store, st};
use crate::{Answer, Context, Failure};
use ont_store::Store;
use ont_store::schema_mode::StoreRefusal;
use serde_json::json;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Default)]
pub struct CompactRequest {}

/// Only derived candidate tables may change during compaction.
pub fn checksum(store: &mut Store) -> Result<String, StoreRefusal> {
    let mut hash = Sha256::new();
    store.for_each_row_canonical(&["head_candidates", "candidate_generation"], &mut |bytes| {
        hash.update(bytes)
    })?;
    Ok(format!("{:x}", hash.finalize()))
}

/// Compact an existing store without binding a principal, as for backup.
pub fn compact(ctx: &Context, _req: &CompactRequest) -> Result<Answer, Failure> {
    let mut store = open_store(&ctx.store, true)?;
    let page_size_before = st!(store.page_size());
    let bytes_before = io!(std::fs::metadata(&ctx.store), "store_stat_failed").len();
    let pins = st!(store.schema_pins());
    let revision = fresh_revision()?;
    if page_size_before == 16384 {
        return Ok(Answer::Enveloped {
            pins,
            revision,
            result: json!({"compacted":false,"page_size":16384,"bytes":bytes_before}),
        });
    }
    let taken = crate::backup::take(&mut store, &ctx.store, ctx.now)?;
    let before = st!(checksum(&mut store));
    st!(store.compact());
    if st!(checksum(&mut store)) != before {
        return Err(Failure::Internal(crate::internal!("compact_rows_changed")));
    }
    let page_size = st!(store.page_size());
    let bytes = io!(std::fs::metadata(&ctx.store), "store_stat_failed").len();
    Ok(Answer::Enveloped {
        pins,
        revision,
        result: json!({
            "compacted":true,"page_size_before":page_size_before,"page_size":page_size,
            "bytes_before":bytes_before,"bytes":bytes,
            "backup":taken.path.display().to_string(),"backup_sha256":taken.sha256,
            "retention_failure":taken.retention_failure
        }),
    })
}
