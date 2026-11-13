//! `backup`: one verified copy of the store, newest three kept.
use crate::principal::fresh_revision;
use crate::refusal::{io, open_store, st, store_err};
use crate::{Answer, Context, Failure, Refusal, sha256_hex};
use ont_store::Store;
use serde_json::json;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct BackupRequest {}

const KEEP: usize = 3;

/// A finished backup. `retention_failure` is `code@file:line` when the
/// copy is good but removing an older copy failed.
#[derive(Debug)]
pub(crate) struct Taken {
    pub path: PathBuf,
    pub sha256: String,
    pub retention_failure: Option<String>,
}

/// Back up an existing store (opened read-only, never created), comparing
/// and repairing the edits export beside it in full. No principal is bound:
/// a backup needs no grant.
pub fn backup(ctx: &Context, _req: &BackupRequest) -> Result<Answer, Failure> {
    let mut store = open_store(&ctx.store, false)?;
    let revision = fresh_revision()?;
    let pins = st!(store.schema_pins());
    let edits_log = crate::action::verify_edits_file(&ctx.store);
    let taken = take(&mut store, &ctx.store, ctx.now)?;
    Ok(Answer::Enveloped {
        pins,
        revision,
        result: json!({
            "path":taken.path.display().to_string(),
            "sha256":taken.sha256,
            "retention_failure":taken.retention_failure,
            "edits_log":edits_log.as_str()
        }),
    })
}

/// `(YYYYMMDDTHHMMSSZ, n)` of a name `ontology-<stamp>(-<n>)?.sqlite` as this
/// command writes it; the first backup of a second has n = 1.
fn parse_name(name: &str) -> Option<(String, u32)> {
    let mid = name.strip_prefix("ontology-")?.strip_suffix(".sqlite")?;
    let (stamp, suffix) = match mid.split_once('-') {
        Some((s, n)) => (s, Some(n)),
        None => (mid, None),
    };
    let b = stamp.as_bytes();
    let ok = b.len() == 16
        && b[..8].iter().all(u8::is_ascii_digit)
        && b[8] == b'T'
        && b[9..15].iter().all(u8::is_ascii_digit)
        && b[15] == b'Z';
    if !ok {
        return None;
    }
    let n = match suffix {
        None => 1,
        Some(s) if !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()) => s.parse().ok()?,
        Some(_) => return None,
    };
    Some((stamp.to_string(), n))
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

fn set_mode(path: &Path, mode: u32) -> Result<(), Failure> {
    io!(
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)),
        "mode_change_failed"
    );
    Ok(())
}

/// The backup directory: created with mode 0700 (`DirBuilder`, so there is
/// no wider moment); an existing directory must be a real directory and is
/// narrowed to 0700 when wider.
fn backup_dir(store_path: &Path) -> Result<PathBuf, Failure> {
    let dir = store_path
        .parent()
        .map(|p| p.join("backup"))
        .unwrap_or_else(|| PathBuf::from("backup"));
    match std::fs::symlink_metadata(&dir) {
        Ok(meta) => {
            if !meta.is_dir() {
                return Err(Failure::Internal(crate::internal!("backup_dir_invalid")));
            }
            if meta.permissions().mode() & 0o077 != 0 {
                set_mode(&dir, 0o700)?;
            }
        }
        Err(_) => {
            io!(
                std::fs::DirBuilder::new().mode(0o700).create(&dir),
                "backup_dir_create_failed"
            );
        }
    }
    Ok(dir)
}

/// One backup of `store` into `<store dir>/backup/`: `VACUUM INTO` inside the
/// 0700 directory, then mode 0600 before anything is answered, then a
/// readback in mode `Schema` that compares pins (a copy that fails is
/// removed). A failed `VACUUM INTO` removes its partial target. Keeps the
/// newest three of the names this command writes; a failure of that step is
/// reported in `retention_failure` and does not fail the backup.
pub(crate) fn take(store: &mut Store, store_path: &Path, now: u64) -> Result<Taken, Failure> {
    let dir = backup_dir(store_path)?;
    let stamp = utc_stamp(now);
    let mut target = dir.join(format!("ontology-{stamp}.sqlite"));
    let mut n = 2;
    while target.exists() {
        target = dir.join(format!("ontology-{stamp}-{n}.sqlite"));
        n += 1;
    }
    let before = st!(store.schema_pins());
    if let Err(e) = store.backup_to(&target) {
        // `backup_to` refuses an existing target before it writes, so any
        // other failure may have left a partial file of ours.
        if e.code != "backup_target_exists" {
            let _ = std::fs::remove_file(&target);
        }
        return Err(store_err(e));
    }
    let verified = (|| -> Result<(), Failure> {
        set_mode(&target, 0o600)?;
        let mut copy = st!(Store::open_schema_existing(&target));
        if st!(copy.schema_pins()) != before {
            return Err(Refusal::new("backup_pin_mismatch").into());
        }
        Ok(())
    })();
    if let Err(e) = verified {
        let _ = std::fs::remove_file(&target);
        return Err(e);
    }
    let bytes = io!(std::fs::read(&target), "backup_read_failed");
    let retention_failure = match retain(&dir, &target) {
        Ok(()) => None,
        Err(Failure::Internal(i)) => Some(format!("{}@{}:{}", i.code, i.file, i.line)),
        Err(Failure::Refused(r)) => Some(r.code.to_string()),
    };
    Ok(Taken {
        path: target,
        sha256: sha256_hex(&bytes),
        retention_failure,
    })
}

/// Keep the newest `KEEP` backups by (name timestamp, suffix); consider only
/// regular files (no symlink) named as this command names them; never remove
/// `just_written`.
fn retain(dir: &Path, just_written: &Path) -> Result<(), Failure> {
    let mut found: Vec<((String, u32), PathBuf)> = Vec::new();
    for entry in io!(std::fs::read_dir(dir), "backup_list_failed") {
        let entry = io!(entry, "backup_list_failed");
        let path = entry.path();
        let Some(key) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(parse_name)
        else {
            continue;
        };
        let meta = io!(std::fs::symlink_metadata(&path), "backup_list_failed");
        if meta.is_file() {
            found.push((key, path));
        }
    }
    found.sort();
    let mut excess = found.len().saturating_sub(KEEP);
    for (_, path) in &found {
        if excess == 0 {
            break;
        }
        if path == just_written {
            continue;
        }
        io!(std::fs::remove_file(path), "backup_remove_failed");
        excess -= 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_this_command_writes_are_parsed_and_others_are_not() {
        assert_eq!(
            parse_name("ontology-20261111T060400Z.sqlite"),
            Some(("20261111T060400Z".into(), 1))
        );
        assert_eq!(
            parse_name("ontology-20261111T060400Z-12.sqlite"),
            Some(("20261111T060400Z".into(), 12))
        );
        for bad in [
            "ontology-foo.sqlite",
            "notes.txt",
            "ontology-20261111T060400Z.sqlite.bak",
            "ontology-20261111T060400Z-.sqlite",
            "ontology-20261111T06040Z.sqlite",
            "ontology-2026111xT060400Z.sqlite",
        ] {
            assert_eq!(parse_name(bad), None, "{bad}");
        }
    }
}
