//! `ont`: schema check/install, describe, get and backup over a format 3
//! store.
//!
//! Layout: [`cli`] parses arguments and builds every answer line, exit code
//! and the error text (the only place that does); [`schema`], [`describe`],
//! [`get`] and [`backup`] are one public operation each, from a typed request
//! and a [`Context`] to `Result<Answer, Failure>`; [`principal`] holds the one
//! grant view that both admission and `describe` use.
//!
//! Opening rule: only `schema install` may create or initialise a store
//! file (it creates it with mode 0600 before SQLite opens it). Every other
//! operation opens an existing format 3 file through
//! `Store::open_schema_existing`, which cannot create or initialise.
//!
//! `authorization_revision` is the policy generation of the `Authority`
//! built for the request ([`principal::Principal::revision`]).
//!
//! Behaviours worth knowing: a backup taken before an install that then
//! refuses remains; `--now` also sets the install time and the backup name
//! stamp; `schema check` does not predict `schema_version_conflict`. No
//! function grant exists yet, so `describe` lists the functions of every
//! listed domain.
pub mod backup;
pub mod cli;
pub mod describe;
pub mod get;
pub mod principal;
pub mod refusal;
pub mod schema;

pub use cli::{Outcome, run};
pub use refusal::{Failure, Internal, Refusal};

use ont_core::schema::SchemaPin;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const TRUST_LIMIT: &str = "OS-account principal only; on this host every agent shares this UID, so the CLI does not authenticate a human.";
pub const MAX_REQUEST: usize = 65_536;

/// Real = effective = 0, else `principal_not_trusted`.
pub fn check_uid(real: u32, effective: u32) -> Result<(), &'static str> {
    if real != 0 || effective != real {
        return Err("principal_not_trusted");
    }
    Ok(())
}

/// What an operation needs besides its request: the store path, `now` and
/// the principal name.
#[derive(Debug, Clone)]
pub struct Context {
    pub store: PathBuf,
    pub now: u64,
    pub principal: String,
}

/// An operation's answer; `run` wraps `Enveloped` into the common envelope.
#[derive(Debug)]
pub enum Answer {
    Enveloped {
        pins: BTreeMap<String, SchemaPin>,
        revision: u64,
        result: Value,
    },
    /// A complete answer object (`schema check`).
    Plain(Value),
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}
pub(crate) fn pin_json(p: &SchemaPin) -> Value {
    json!({"domain":p.domain,"version":p.version,"sha256":p.sha256})
}
