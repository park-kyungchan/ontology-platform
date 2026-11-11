//! Composition and synthetic demonstration only. No listener or production authentication.
use ont_core::{Authority, Error, Query, Session};
use ont_store::Store;

pub mod local_fake;
pub mod program;
pub mod program_git;
pub mod program_guardrails;
pub mod program_pilot;
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
    /// Explicit ontology-answer/v1 mode. Admission and the snapshot remain held
    /// through record construction and serialization, just as in native mode.
    /// Refused/error outcomes carry only an enumerated code, never the query.
    pub fn render_answer_record(
        &mut self,
        session: &Session,
        query: Query,
        now: u64,
    ) -> Result<String, Error> {
        self.render_answer_record_with_status(session, query, now)
            .map(|(bytes, _)| bytes)
    }
    /// The boolean is the native refusal class, not serialization failure.
    /// CLI owners use it to retain nonzero refused-outcome exit status.
    pub fn render_answer_record_with_status(
        &mut self,
        session: &Session,
        query: Query,
        now: u64,
    ) -> Result<(String, bool), Error> {
        self.render_answer_record_version_with_status(
            session,
            query,
            now,
            ont_core::answer::Version::V1,
        )
    }
    /// Select a publication schema while retaining the same admission/snapshot.
    pub fn render_answer_record_version_with_status(
        &mut self,
        session: &Session,
        query: Query,
        now: u64,
        version: ont_core::answer::Version,
    ) -> Result<(String, bool), Error> {
        use ont_core::answer::AnswerRecord;
        let serialize = |record: AnswerRecord| {
            let refused = record.is_refused();
            record.render().map(|bytes| (bytes, refused))
        };
        let admission = match self.authority.admit(session) {
            Ok(admission) => admission,
            Err(error) => return serialize(AnswerRecord::from_error_version(error, version)),
        };
        let query = match (version, query) {
            (ont_core::answer::Version::V3, Query::ProgramReport { root, window }) => {
                Query::ProgramReportV2 { root, window }
            }
            (_, query) => query,
        };
        let rendered = self.store.snapshot(|snapshot| {
            let record = match admission.query(snapshot, query, now) {
                Ok(response) => AnswerRecord::from_response_version(&response, version)?,
                Err(error) => AnswerRecord::from_error_version(error, version),
            };
            serialize(record)
        });
        match rendered {
            Ok(record) => Ok(record),
            Err(error) => serialize(AnswerRecord::from_error_version(error, version)),
        }
    }
}
