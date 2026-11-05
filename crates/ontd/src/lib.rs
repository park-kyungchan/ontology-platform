//! Composition and synthetic demonstration only. No listener or production authentication.
use ont_core::{Authority, Error, Query, Session};
use ont_store::Store;

pub mod local_fake;
pub mod program_git;
pub mod synthetic;

/// Public restricted read entrypoint. The embedding owner authenticates and binds
/// sessions; neither Store nor Authority is provided to an untrusted protocol client.
pub struct Service {
    store: Store,
    authority: Authority,
}
impl Service {
    pub fn new(store: Store, authority: Authority) -> Self {
        Self { store, authority }
    }
    /// Authorization guard remains held through snapshot, derivation and serialization.
    /// `now` is supplied by the trusted embedding clock, never by stdin.
    pub fn render(&mut self, session: &Session, query: Query, now: u64) -> Result<String, Error> {
        let admission = self.authority.admit(session)?;
        self.store.snapshot(|snapshot| {
            let response = admission.query(snapshot, query, now)?;
            serde_json::to_string(&response).map_err(|_| Error::StoreFailure)
        })
    }
}
