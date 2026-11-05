//! Trusted, bounded read-only GitHub intake. No protocol route, source writes,
//! repository checkout, authentication administration or background refresh.
//!
//! The embedding owner supplies allowlisted public/source-authorized repositories,
//! immutable endpoint SHAs, source-owned bindings and a trusted clock. `gh api`
//! uses its native environment/credential provider; no token is read into this model.
//! Tests inject responses. All attribution arithmetic is delegated to ont-core.
use ont_core::program::{
    self, CaptureInterval, Endpoint, MainlineChange, PathBasis, PrCandidate, PrId, PrSearch,
    RepositoryInterval, Tree,
};
use ont_core::{Coverage, Id, Namespace};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

mod live;
mod projection;
pub use live::GhTransport;
pub use projection::{Projection, ProjectionSpec, build_projection};

pub const ADAPTER_REVISION: &str = "github-git-evidence-v1";

/// Stable owner diagnostics only; raw process/API payloads never enter errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntakeError {
    InvalidConfiguration,
    Unavailable,
    RateLimited,
    Timeout,
    LimitExceeded,
    InvalidResponse,
    FirstParentGap,
    Rules(program::RuleError),
    Store,
}
impl IntakeError {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidConfiguration => "invalid_git_configuration",
            Self::Unavailable => "git_source_unavailable",
            Self::RateLimited => "git_source_rate_limited",
            Self::Timeout => "git_source_timeout",
            Self::LimitExceeded => "git_capture_limit",
            Self::InvalidResponse => "invalid_git_response",
            Self::FirstParentGap => "git_first_parent_gap",
            Self::Rules(e) => e.code(),
            Self::Store => "git_projection_store_failure",
        }
    }
}
impl std::fmt::Display for IntakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for IntakeError {}
impl From<program::RuleError> for IntakeError {
    fn from(e: program::RuleError) -> Self {
        Self::Rules(e)
    }
}
fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn oid(s: &str) -> bool {
    s.len() == 40
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn component(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

/// Exact configured repository identity, independent of local filesystem paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    id: Id,
    slug: String,
    github_id: u64,
}
impl Repository {
    pub fn github(owner: &str, name: &str, github_id: u64) -> Result<Self, IntakeError> {
        if github_id == 0 || !component(owner) || !component(name) {
            return Err(IntakeError::InvalidConfiguration);
        }
        let slug = format!("{owner}/{name}");
        Ok(Self {
            id: Id {
                object_type: "repository".into(),
                namespace: Namespace {
                    instance: format!("https://github.com/{slug}"),
                    account: owner.into(),
                },
                native_key: slug.clone(),
            },
            slug,
            github_id,
        })
    }
    pub fn id(&self) -> &Id {
        &self.id
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resource {
    Repository,
    Commit(String),
    Tree(String),
    Pulls { commit: String, page: usize },
    Checks { head: String, page: usize },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    repository: Repository,
    resource: Resource,
}
impl Request {
    pub fn repository(&self) -> &Repository {
        &self.repository
    }
    pub fn resource(&self) -> &Resource {
        &self.resource
    }
    pub fn route(&self) -> String {
        let prefix = format!("repos/{}", self.repository.slug);
        match &self.resource {
            Resource::Repository => prefix,
            Resource::Commit(sha) => format!("{prefix}/git/commits/{sha}"),
            Resource::Tree(sha) => format!("{prefix}/git/trees/{sha}?recursive=1"),
            Resource::Pulls { commit, page } => {
                format!("{prefix}/commits/{commit}/pulls?per_page=100&page={page}")
            }
            Resource::Checks { head, page } => {
                format!("{prefix}/commits/{head}/check-runs?filter=all&per_page=100&page={page}")
            }
        }
    }
}
/// Trusted transport boundary. Implementations must enforce max_bytes while
/// receiving, never log credentials/response bodies and perform only this GET.
pub trait Transport {
    fn get(&mut self, request: &Request, max_bytes: usize) -> Result<Vec<u8>, IntakeError>;
}
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_bytes: usize,
    pub max_pages: usize,
    pub max_commits: usize,
    pub max_tree_entries: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes: 8 * 1024 * 1024,
            max_pages: 10,
            max_commits: 256,
            max_tree_entries: 100_000,
        }
    }
}
#[derive(Debug, Clone)]
pub struct EndpointBinding {
    pub commit: String,
    /// Must come from the source measurement's original capture. None stays None.
    pub source_measurement: Option<Id>,
    pub observed_at: u64,
}

/// Fields are sealed so only validated source collection creates an interval.
#[derive(Debug, Clone)]
pub struct CollectedInterval {
    interval: RepositoryInterval,
    repository: Repository,
    artifact: Value,
    captured: CaptureInterval,
}
impl CollectedInterval {
    pub fn interval(&self) -> &RepositoryInterval {
        &self.interval
    }
    /// Sanitized, allowlisted evidence only; tree paths are exact-byte hashes.
    pub fn artifact(&self) -> &Value {
        &self.artifact
    }
}

pub struct Collector<T, C> {
    transport: T,
    clock: C,
    limits: Limits,
    captures: Vec<Value>,
    received_bytes: usize,
}
impl<T: Transport, C: FnMut() -> u64> Collector<T, C> {
    pub fn new(transport: T, clock: C, limits: Limits) -> Result<Self, IntakeError> {
        if limits.max_bytes == 0
            || limits.max_bytes > 32 * 1024 * 1024
            || limits.max_pages == 0
            || limits.max_pages > 100
            || limits.max_commits == 0
            || limits.max_commits > 4096
            || limits.max_tree_entries == 0
            || limits.max_tree_entries > 1_000_000
        {
            return Err(IntakeError::InvalidConfiguration);
        }
        Ok(Self {
            transport,
            clock,
            limits,
            captures: vec![],
            received_bytes: 0,
        })
    }
    fn read<D: serde::de::DeserializeOwned>(
        &mut self,
        repository: &Repository,
        resource: Resource,
    ) -> Result<(D, usize), IntakeError> {
        let request = Request {
            repository: repository.clone(),
            resource,
        };
        let start = (self.clock)();
        let response = self.transport.get(&request, self.limits.max_bytes);
        let end = (self.clock)();
        if start > end
            || self
                .captures
                .last()
                .and_then(|v| v["capture"]["end"].as_u64())
                .is_some_and(|last| start < last)
        {
            return Err(IntakeError::InvalidResponse);
        }
        let index = self.captures.len();
        match response {
            Ok(bytes) => {
                self.received_bytes = self
                    .received_bytes
                    .checked_add(bytes.len())
                    .ok_or(IntakeError::LimitExceeded)?;
                if bytes.len() > self.limits.max_bytes || self.received_bytes > 64 * 1024 * 1024 {
                    return Err(IntakeError::LimitExceeded);
                }
                let parsed =
                    serde_json::from_slice(&bytes).map_err(|_| IntakeError::InvalidResponse)?;
                self.captures.push(json!({"route": request.route(), "capture": {"start":start,"end":end}, "response_sha256":sha256(&bytes)}));
                Ok((parsed, index))
            }
            Err(e) => {
                self.captures.push(json!({"route":request.route(),"capture":{"start":start,"end":end},"failure":e.code()}));
                Err(e)
            }
        }
    }
    fn tree(
        &mut self,
        repository: &Repository,
        sha: &str,
    ) -> Result<(Tree, Vec<String>), IntakeError> {
        let (commit, i): (GitCommit, _) = self.read(repository, Resource::Commit(sha.into()))?;
        if !valid_time(&commit.committer.date)
            || commit.sha != sha
            || !oid(&commit.tree.sha)
            || commit.parents.iter().any(|p| !oid(&p.sha))
        {
            return Err(IntakeError::InvalidResponse);
        }
        let parents: Vec<_> = commit.parents.into_iter().map(|p| p.sha).collect();
        self.captures[i]["commit"] = json!({"sha":commit.sha,"tree":commit.tree.sha,"parents":parents,"committed_at":commit.committer.date});
        let (tree, i): (GitTree, _) =
            self.read(repository, Resource::Tree(commit.tree.sha.clone()))?;
        if tree.sha != commit.tree.sha
            || tree.truncated
            || tree.tree.len() > self.limits.max_tree_entries
        {
            return Err(IntakeError::InvalidResponse);
        }
        let mut paths = BTreeSet::new();
        let mut entries = BTreeSet::new();
        for entry in tree.tree {
            if !oid(&entry.sha) || !valid_path(&entry.path) || !entries.insert(entry.path.clone()) {
                return Err(IntakeError::InvalidResponse);
            }
            match entry.kind.as_str() {
                "blob" | "commit" => {
                    if !paths.insert(entry.path) {
                        return Err(IntakeError::InvalidResponse);
                    }
                }
                "tree" => {}
                _ => return Err(IntakeError::InvalidResponse),
            }
        }
        self.captures[i]["tree"] = json!({"sha":tree.sha,"truncated":false,"path_sha256":paths.iter().map(|p| sha256(p.as_bytes())).collect::<Vec<_>>()});
        Ok((
            Tree {
                commit: sha.into(),
                tree: tree.sha,
                paths,
                coverage: Coverage::Complete,
            },
            parents,
        ))
    }
    fn pulls(&mut self, repository: &Repository, commit: &str) -> Result<PrSearch, IntakeError> {
        let mut candidates = BTreeMap::new();
        let mut coverage = Coverage::Partial;
        for page in 1..=self.limits.max_pages {
            let result: Result<(Vec<Pull>, usize), _> = self.read(
                repository,
                Resource::Pulls {
                    commit: commit.into(),
                    page,
                },
            );
            let (pulls, i) = match result {
                Ok(p) => p,
                Err(IntakeError::Unavailable | IntakeError::RateLimited | IntakeError::Timeout) => {
                    break;
                }
                Err(e) => return Err(e),
            };
            if pulls.len() > 100 {
                return Err(IntakeError::InvalidResponse);
            }
            let count = pulls.len();
            let mut sanitized = vec![];
            for pull in pulls {
                if pull.number == 0
                    || (pull.base.repo.full_name != repository.slug
                        || pull.base.repo.id != repository.github_id)
                    || !oid(&pull.head.sha)
                    || !matches!(pull.state.as_str(), "open" | "closed")
                {
                    return Err(IntakeError::InvalidResponse);
                }
                // merged_at is a required nullable field, not a defaulted Option.
                let merged = match &pull.merged_at {
                    Value::Null => false,
                    Value::String(s) if valid_time(s) && pull.state == "closed" => true,
                    _ => return Err(IntakeError::InvalidResponse),
                };
                if pull.merge_commit_sha.as_ref().is_some_and(|s| !oid(s))
                    || (merged && pull.merge_commit_sha.is_none())
                {
                    return Err(IntakeError::InvalidResponse);
                }
                let candidate = PrCandidate {
                    id: PrId {
                        repository: repository.id.clone(),
                        number: pull.number,
                    },
                    merged,
                    merge_commit: pull.merge_commit_sha.clone(),
                };
                // Pagination overlap/duplicates cannot prove a complete search.
                if candidates.insert(pull.number, candidate).is_some() {
                    return Err(IntakeError::InvalidResponse);
                }
                let checks = self.checks(repository, &pull.head.sha)?;
                sanitized.push(json!({"number":pull.number,"repository":repository.slug,"state":pull.state,"merged_at":pull.merged_at,"merge_commit":pull.merge_commit_sha,"head":pull.head.sha,"checks":checks}));
            }
            self.captures[i]["pulls"] = json!(sanitized);
            if count < 100 {
                coverage = Coverage::Complete;
                break;
            }
        }
        Ok(PrSearch {
            coverage,
            candidates: candidates.into_values().collect(),
        })
    }
    fn checks(&mut self, repository: &Repository, head: &str) -> Result<Value, IntakeError> {
        let mut expected = None;
        let mut checks = BTreeMap::new();
        for page in 1..=self.limits.max_pages {
            let (response, i): (Checks, _) = match self.read(
                repository,
                Resource::Checks {
                    head: head.into(),
                    page,
                },
            ) {
                Ok(p) => p,
                Err(IntakeError::Unavailable | IntakeError::RateLimited | IntakeError::Timeout) => {
                    return Ok(
                        json!({"coverage":"partial","runs":checks.into_values().collect::<Vec<_>>()}),
                    );
                }
                Err(e) => return Err(e),
            };
            // A valid population may exceed our page budget. Keep bounded runs
            // and let the loop's Partial result preserve immutable merge facts.
            if expected.is_some_and(|n| n != response.total_count)
                || response.check_runs.len() > 100
            {
                return Err(IntakeError::InvalidResponse);
            }
            expected = Some(response.total_count);
            let n = response.check_runs.len();
            for run in response.check_runs {
                if (run.status == "completed") != run.conclusion.is_some()
                    || run.id == 0
                    || run.head_sha != head
                    || run.name.is_empty()
                    || run.name.len() > 256
                    || !matches!(
                        run.status.as_str(),
                        "queued"
                            | "in_progress"
                            | "completed"
                            | "waiting"
                            | "requested"
                            | "pending"
                    )
                    || run.conclusion.as_ref().is_some_and(|s| {
                        !matches!(
                            s.as_str(),
                            "success"
                                | "failure"
                                | "neutral"
                                | "cancelled"
                                | "skipped"
                                | "timed_out"
                                | "action_required"
                                | "stale"
                                | "startup_failure"
                        )
                    })
                {
                    return Err(IntakeError::InvalidResponse);
                }
                let value = json!({"id":run.id,"name":run.name,"head_sha":run.head_sha,"status":run.status,"conclusion":run.conclusion});
                if checks.insert(run.id, value).is_some() {
                    return Err(IntakeError::InvalidResponse);
                }
            }
            self.captures[i]["checks"] = json!({"total_count":response.total_count,"head":head});
            if checks.len() > response.total_count {
                return Err(IntakeError::InvalidResponse);
            }
            if n < 100 {
                if checks.len() != response.total_count {
                    return Err(IntakeError::InvalidResponse);
                }
                return Ok(
                    json!({"coverage":"complete","runs":checks.into_values().collect::<Vec<_>>()}),
                );
            }
        }
        Ok(json!({"coverage":"partial","runs":checks.into_values().collect::<Vec<_>>()}))
    }
    /// Walk actual first parents backwards, then present forward order. A failed
    /// tree/metadata capture refuses the interval rather than creating empty paths.
    pub fn collect(
        &mut self,
        repository: &Repository,
        basis: &PathBasis,
        before: EndpointBinding,
        after: EndpointBinding,
    ) -> Result<CollectedInterval, IntakeError> {
        if !oid(&before.commit) || !oid(&after.commit) {
            return Err(IntakeError::InvalidConfiguration);
        }
        for binding in [&before, &after] {
            if binding
                .source_measurement
                .as_ref()
                .is_some_and(|id| id.validate().is_err() || id.object_type != "measurement")
            {
                return Err(IntakeError::InvalidConfiguration);
            }
        }
        self.captures.clear();
        self.received_bytes = 0;
        let start = (self.clock)();
        let (identity, index): (RepositoryIdentity, _) =
            self.read(repository, Resource::Repository)?;
        if identity.id != repository.github_id
            || identity.full_name != repository.slug
            || identity.owner.login != repository.id.namespace.account
        {
            return Err(IntakeError::InvalidResponse);
        }
        self.captures[index]["repository"] =
            json!({"id":identity.id,"full_name":identity.full_name,"owner":identity.owner.login});
        let mut reverse = vec![];
        let mut seen = BTreeSet::new();
        let mut next = after.commit.clone();
        loop {
            if !seen.insert(next.clone()) {
                return Err(IntakeError::FirstParentGap);
            }
            let (tree, parents) = self.tree(repository, &next)?;
            if next == before.commit {
                reverse.push((tree, None));
                break;
            }
            if reverse.len() >= self.limits.max_commits {
                return Err(IntakeError::LimitExceeded);
            }
            let first = parents.first().ok_or(IntakeError::FirstParentGap)?.clone();
            let prs = self.pulls(repository, &next)?;
            reverse.push((tree, Some(prs)));
            next = first;
        }
        let end = (self.clock)();
        if end < start
            || self
                .captures
                .last()
                .and_then(|r| r["capture"]["end"].as_u64())
                .is_some_and(|last| end < last)
        {
            return Err(IntakeError::InvalidResponse);
        }
        reverse.reverse();
        let mut iter = reverse.into_iter();
        let (first, _) = iter.next().ok_or(IntakeError::FirstParentGap)?;
        let mut previous = first.clone();
        let mut changes = vec![];
        for (accepted, prs) in iter {
            changes.push(MainlineChange {
                first_parent: previous,
                accepted: accepted.clone(),
                prs: prs.ok_or(IntakeError::InvalidResponse)?,
            });
            previous = accepted;
        }
        let captured = CaptureInterval { start, end };
        let interval = RepositoryInterval {
            repository: repository.id.clone(),
            before: Endpoint {
                basis: basis.clone(),
                tree: first,
                source_measurement: before.source_measurement,
                observed_at: before.observed_at,
                capture: captured.clone(),
            },
            after: Endpoint {
                basis: basis.clone(),
                tree: previous,
                source_measurement: after.source_measurement,
                observed_at: after.observed_at,
                capture: captured.clone(),
            },
            changes,
            coverage: Coverage::Complete,
        };
        let artifact = json!({"adapter":ADAPTER_REVISION,"repository":repository.id,"capture":{"start":start,"end":end},"before":{"commit":before.commit,"measurement":interval.before.source_measurement,"observed_at":before.observed_at},"after":{"commit":after.commit,"measurement":interval.after.source_measurement,"observed_at":after.observed_at},"responses":self.captures});
        Ok(CollectedInterval {
            interval,
            repository: repository.clone(),
            artifact,
            captured,
        })
    }
}
fn valid_path(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('/')
        && !s.contains('\0')
        && s.split('/').all(|p| !p.is_empty() && p != "." && p != "..")
}
fn valid_time(s: &str) -> bool {
    // Preserve the source clock literally; only accept GitHub UTC second format.
    let b = s.as_bytes();
    b.len() == 20
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            10 => *c == b'T',
            13 | 16 => *c == b':',
            19 => *c == b'Z',
            _ => c.is_ascii_digit(),
        })
}
#[derive(Deserialize)]
struct Sha {
    sha: String,
}
#[derive(Deserialize)]
struct GitCommit {
    committer: Committer,
    sha: String,
    tree: Sha,
    parents: Vec<Sha>,
}
#[derive(Deserialize)]
struct Entry {
    path: String,
    sha: String,
    #[serde(rename = "type")]
    kind: String,
}
#[derive(Deserialize)]
struct GitTree {
    sha: String,
    truncated: bool,
    tree: Vec<Entry>,
}
#[derive(Deserialize)]
struct RepoName {
    id: u64,
    full_name: String,
}
#[derive(Deserialize)]
struct Base {
    repo: RepoName,
}
#[derive(Deserialize)]
struct Pull {
    number: u64,
    state: String,
    merged_at: Value,
    merge_commit_sha: Option<String>,
    head: Sha,
    base: Base,
}
#[derive(Deserialize)]
struct Checks {
    total_count: usize,
    check_runs: Vec<Check>,
}
#[derive(Deserialize)]
struct Check {
    id: u64,
    name: String,
    head_sha: String,
    status: String,
    conclusion: Option<String>,
}

#[derive(Deserialize)]
struct RepositoryIdentity {
    id: u64,
    full_name: String,
    owner: Owner,
}
#[derive(Deserialize)]
struct Owner {
    login: String,
}

#[derive(Deserialize)]
struct Committer {
    date: String,
}
