//! `ont`: schema check/install, describe, get, backup, index and search over
//! a format 3 store.
//!
//! Layout: [`cli`] parses arguments and builds every answer line, exit code
//! and the error text (the only place that does); [`schema`], [`describe`],
//! [`get`] and [`backup`] are one public operation each, from a typed request
//! and a [`Context`] to `Result<Answer, Failure>` (as are [`index`] and
//! [`search`]; [`sources`] holds the file readers of `index`); [`principal`] holds the one
//! grant view that both admission and `describe` use.
//!
//! Opening rule: only `schema install` may create or initialise a store
//! file (it creates it with mode 0600 before SQLite opens it). Every other
//! operation opens an existing format 3 file through
//! `Store::open_schema_existing` (read-only) or, for `index`,
//! `Store::open_schema_existing_writable`; neither can create or initialise.
//!
//! `authorization_revision` is the policy generation of the `Authority`
//! built for the request ([`principal::Principal::revision`], or
//! [`principal::fresh_revision`] where no principal is bound), read through
//! `Authority::generation`.
//!
//! Behaviours worth knowing: a backup taken before an install that then
//! refuses remains; `--now` also sets the install time and the backup name
//! stamp; `schema check` does not predict `schema_version_conflict`. No
//! function grant exists yet, so `describe` lists the functions of every
//! listed domain.
pub mod action;
pub mod backup;
pub mod cli;
pub mod describe;
pub mod get;
pub mod gitread;
pub mod index;
pub mod mcp;
pub mod principal;
pub mod refusal;
pub mod schema;
pub mod search;
pub(crate) mod sources;

pub use cli::{Outcome, run};
pub use refusal::{Failure, Internal, Refusal};

use ont_core::schema::SchemaPin;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const TRUST_LIMIT: &str = "OS-account principal only; on this host every agent shares this UID, so the CLI does not authenticate a human.";
pub const MAX_REQUEST: usize = 65_536;
/// The store `ont` uses without `--store`.
pub const DEFAULT_STORE: &str = "/var/lib/ontology-platform/ont/ontology.sqlite";

/// Real = effective = 0, else `principal_not_trusted`.
pub fn check_uid(real: u32, effective: u32) -> Result<(), &'static str> {
    if real != 0 || effective != real {
        return Err("principal_not_trusted");
    }
    Ok(())
}

/// What an operation needs besides its request: the store path, `now` and
/// the self-reported attribution of the caller. `client` is the command's
/// `--client` value (default `cli`); `agent` comes from the request field
/// or `ONT_AGENT`; `session` is given per request. All three are
/// self-reported, as DESIGN section 7 states.
#[derive(Debug, Clone)]
pub struct Context {
    pub store: PathBuf,
    pub now: u64,
    pub client: String,
    pub agent: Option<String>,
    pub session: Option<String>,
    /// Snapshot paging for action evaluation (test-only tuning via
    /// [`Context::with_snapshot_bounds`]); `Context::new` is the supported
    /// constructor.
    pub snapshot_page_size: usize,
    pub snapshot_max_heads: usize,
}
impl Context {
    pub fn new(store: PathBuf, now: u64) -> Self {
        Self {
            store,
            now,
            client: "cli".to_string(),
            agent: None,
            session: None,
            snapshot_page_size: 500,
            snapshot_max_heads: 100_000,
        }
    }
    /// Test-only constructor: a small snapshot page size (and hard head
    /// bound) without a global.
    pub fn with_snapshot_bounds(
        store: PathBuf,
        now: u64,
        page_size: usize,
        max_heads: usize,
    ) -> Self {
        let mut ctx = Self::new(store, now);
        ctx.snapshot_page_size = page_size.max(1);
        ctx.snapshot_max_heads = max_heads.max(1);
        ctx
    }
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
    /// An enveloped answer of an operation that ran but whose work failed in
    /// part (`index` with a failed source): same envelope, exit 2.
    Failed {
        pins: BTreeMap<String, SchemaPin>,
        revision: u64,
        result: Value,
    },
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}
pub(crate) fn pin_json(p: &SchemaPin) -> Value {
    json!({"domain":p.domain,"version":p.version,"sha256":p.sha256})
}
