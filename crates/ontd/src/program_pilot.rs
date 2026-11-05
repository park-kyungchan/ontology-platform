//! One-shot, OS-account-bound ProgramReport composition. No listener or human login.
//! Only execute is public; graph/config/policy administration stays crate-internal.
use crate::{Service, program, program_git, program_guardrails as guards};
use ont_core::program::{DefinitionRef, Direction, MetricDefinition, MetricRole, Unit};
use ont_core::report::ReportWindow;
use ont_core::{
    Authority, Change, Coverage, Grant, Id, Knowledge, Lineage, Link, Namespace, Object,
    Observation, Policy, Query, Response, Scope,
};
use ont_store::Store;
use program::v2;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

pub const TRUST_LIMIT: &str = "OS-account principal only; on this host every agent shares this UID, so the CLI does not authenticate a human.";
const PRINCIPAL: &str = "program-owner";
const PROGRAM: &str = "ws-replan";
const ADOPTED_UID: u32 = 0;
const EVIDENCE: &str = "/home/palantirkc/work/ontology-coengineering/pr7-pilot/evidence";
const CONFIG: &str =
    "/home/palantirkc/work/ontology-coengineering/pr7-pilot/evidence/owner-config.json";
const MAX_INPUT: usize = 65_536;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PilotError(&'static str);
impl PilotError {
    pub fn code(self) -> &'static str {
        self.0
    }
}
type Result<T> = std::result::Result<T, PilotError>;
fn err(code: &'static str) -> PilotError {
    PilotError(code)
}
fn core(e: ont_core::Error) -> PilotError {
    err(e.code())
}
fn intake(e: program::IntakeError) -> PilotError {
    err(e.code())
}
fn git_error(e: program_git::IntakeError) -> PilotError {
    err(e.code())
}
fn guard_error(e: guards::IntakeError) -> PilotError {
    err(e.code())
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn pin(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn namespace(guardrails: bool) -> Namespace {
    Namespace {
        instance: if guardrails {
            "vps://ws-replan/guardrails"
        } else {
            "vps://ws-replan"
        }
        .into(),
        account: PRINCIPAL.into(),
    }
}
fn id(kind: &str, key: &str, guardrails: bool) -> Id {
    Id {
        object_type: kind.into(),
        namespace: namespace(guardrails),
        native_key: key.into(),
    }
}
fn scope(ns: Namespace) -> Result<Scope> {
    Scope::program(ns, PROGRAM).map_err(core)
}
fn check_uid(real: u32, effective: u32) -> Result<()> {
    if real != ADOPTED_UID || effective != real {
        return Err(err("pilot_uid_refused"));
    }
    Ok(())
}
fn clock() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| err("pilot_clock_failure"))
}
fn request(bytes: &[u8]) -> Result<(Query, ReportWindow)> {
    if bytes.len() > MAX_INPUT {
        return Err(err("invalid_request"));
    }
    let value = program::strict::parse(bytes).map_err(|_| err("invalid_request"))?;
    let query: Query = serde_json::from_value(value).map_err(|_| err("invalid_request"))?;
    match query {
        Query::ProgramReport { root, window }
            if root == id("program", PROGRAM, false) && window.start <= window.end =>
        {
            Ok((Query::ProgramReport { root, window }, window))
        }
        _ => Err(err("invalid_request")),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Definition {
    id: Id,
    revision: String,
    formula: String,
    population: String,
    adoption: Option<Adoption>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Adoption {
    strategy_id: Id,
    strategy_revision: String,
    evidence: Lineage,
}
impl Definition {
    fn metric(&self) -> Result<MetricDefinition> {
        let d = MetricDefinition {
            reference: DefinitionRef {
                id: self.id.clone(),
                revision: self.revision.clone(),
            },
            formula: self.formula.clone(),
            population: self.population.clone(),
            unit: Unit::Fraction,
            direction: Direction::Increase,
            role: self
                .adoption
                .as_ref()
                .map_or(MetricRole::Indicator, |a| MetricRole::Kpi {
                    strategy: DefinitionRef {
                        id: a.strategy_id.clone(),
                        revision: a.strategy_revision.clone(),
                    },
                    adoption: a.evidence.clone(),
                }),
        };
        d.validate().map_err(|_| err("pilot_invalid_config"))?;
        Ok(d)
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepositoryConfig {
    manifest_key: String,
    owner: String,
    name: String,
    github_id: u64,
    before_commit: String,
    after_commit: String,
    before_observed_at: u64,
    after_observed_at: u64,
}
#[derive(Deserialize)]
#[serde(tag = "version", rename_all = "snake_case", deny_unknown_fields)]
enum SnapshotInput {
    V1 {},
    V2 {
        producer: v2::ProducerPin,
        definition_sha256: String,
        repositories: BTreeMap<String, v2::model::Identity>,
        guardrails: BTreeMap<String, v2::wire::Contract>,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerConfig {
    schema: String,
    adopted_uid: u32,
    snapshot_input: Option<SnapshotInput>,
    producer_sha256: Option<String>,
    manifest_sha256: Option<String>,
    legacy_definition: Option<Definition>,
    exact_definition: Option<Definition>,
    repositories: Vec<RepositoryConfig>,
}
impl OwnerConfig {
    fn parse(bytes: &[u8]) -> Result<Self> {
        let v = program::strict::parse(bytes).map_err(|_| err("pilot_invalid_config"))?;
        let c: Self = serde_json::from_value(v).map_err(|_| err("pilot_invalid_config"))?;
        if c.schema != "ont-program-pilot/v1"
            || c.adopted_uid != ADOPTED_UID
            || c.repositories.len() > 16
        {
            return Err(err("pilot_invalid_config"));
        }
        if matches!(c.snapshot_input, Some(SnapshotInput::V2 { .. }))
            && (c.producer_sha256.is_some() || c.legacy_definition.is_some())
        {
            return Err(err("pilot_ambiguous_snapshot_input"));
        }
        Ok(c)
    }
    fn v2_contract(&self) -> Result<v2::IntakeContract> {
        let Some(SnapshotInput::V2 {
            producer,
            definition_sha256,
            repositories,
            guardrails,
        }) = &self.snapshot_input
        else {
            return Err(err("pilot_invalid_config"));
        };
        Ok(v2::IntakeContract {
            namespace: namespace(false),
            guardrail_namespace: namespace(true),
            program_key: PROGRAM.into(),
            definition: self
                .exact_definition
                .as_ref()
                .ok_or(err("pilot_config_not_ready"))?
                .metric()?,
            producer: producer.clone(),
            definition_sha256: definition_sha256.clone(),
            manifest_sha256: self
                .manifest_sha256
                .clone()
                .ok_or(err("pilot_config_not_ready"))?,
            repositories: repositories.clone(),
            guardrails: guardrails.clone(),
        })
    }
    fn contract(&self) -> Result<program::IntakeContract> {
        let producer = self
            .producer_sha256
            .as_ref()
            .filter(|p| pin(p))
            .ok_or(err("pilot_config_not_ready"))?;
        self.manifest_sha256
            .as_ref()
            .filter(|p| pin(p))
            .ok_or(err("pilot_config_not_ready"))?;
        let definition = self
            .legacy_definition
            .as_ref()
            .ok_or(err("pilot_config_not_ready"))?
            .metric()?;
        if definition.reference.id.namespace != namespace(false) {
            return Err(err("pilot_invalid_config"));
        }
        Ok(program::IntakeContract {
            namespace: namespace(false),
            program_key: PROGRAM.into(),
            legacy_definition: definition,
            producer_sha256: producer.clone(),
        })
    }
    fn policy(&self) -> Result<Policy> {
        let mut grants = vec![
            grant(
                namespace(false),
                &["program", "metric", "measurement", "target"],
            )?,
            grant(namespace(true), &["guardrail"])?,
        ];
        let mut repositories = BTreeSet::new();
        let mut ids = BTreeSet::new();
        for r in &self.repositories {
            let repo = program_git::Repository::github(&r.owner, &r.name, r.github_id)
                .map_err(git_error)?;
            if !repositories.insert(repo.id().clone()) || !ids.insert(r.github_id) {
                return Err(err("pilot_invalid_config"));
            }
            grants.push(grant(
                repo.id().namespace.clone(),
                &["repository", "change"],
            )?);
        }
        Ok(Policy {
            principals: BTreeMap::from([(PRINCIPAL.into(), grants)]),
        })
    }
}
fn grant(ns: Namespace, types: &[&str]) -> Result<Grant> {
    Ok(Grant {
        scope: scope(ns)?,
        object_types: types.iter().map(|s| (*s).into()).collect(),
        read: true,
        derive: true,
        actions: BTreeSet::new(),
    })
}

#[derive(Default, Clone)]
struct Graph {
    objects: Vec<Object>,
    links: Vec<Link>,
    artifacts: BTreeMap<String, Vec<u8>>,
}
impl Graph {
    fn artifact(&mut self, bytes: Vec<u8>) -> String {
        let hash = digest(&bytes);
        self.artifacts.insert(hash.clone(), bytes);
        hash
    }
    fn merge(&mut self, other: Self) -> Result<()> {
        for (hash, bytes) in other.artifacts {
            if digest(&bytes) != hash {
                return Err(err("pilot_invalid_artifact"));
            }
            self.artifacts.insert(hash, bytes);
        }
        self.objects.extend(other.objects);
        self.links.extend(other.links);
        Ok(())
    }
    fn seed(self) -> Result<(Store, BTreeMap<String, Vec<u8>>)> {
        let mut objects = BTreeMap::new();
        for o in self.objects {
            if let Some(old) = objects.insert(o.id.clone(), o.clone())
                && old != o
            {
                return Err(err("pilot_composition_collision"));
            }
        }
        let mut links = BTreeSet::new();
        for l in self.links {
            if !objects.contains_key(&l.source) || !objects.contains_key(&l.target) {
                return Err(err("pilot_missing_endpoint"));
            }
            links.insert((l.source, l.relation, l.target));
        }
        for o in objects.values() {
            for observation in &o.observations {
                let bytes = self
                    .artifacts
                    .get(&observation.lineage.artifact_sha256)
                    .ok_or(err("pilot_invalid_artifact"))?;
                if digest(bytes) != observation.lineage.artifact_sha256 {
                    return Err(err("pilot_invalid_artifact"));
                }
                let value: Value =
                    serde_json::from_slice(bytes).map_err(|_| err("pilot_invalid_artifact"))?;
                if value.pointer(&observation.lineage.pointer).is_none() {
                    return Err(err("pilot_invalid_artifact"));
                }
            }
        }
        let mut store = Store::memory_program().map_err(core)?;
        store
            .seed(
                &objects.into_values().collect::<Vec<_>>(),
                &links
                    .into_iter()
                    .map(|(source, relation, target)| Link {
                        source,
                        relation,
                        target,
                    })
                    .collect::<Vec<_>>(),
            )
            .map_err(core)?;
        Ok((store, self.artifacts))
    }
}
fn edge(source: Id, relation: &str, target: Id) -> Link {
    Link {
        source,
        relation: relation.into(),
        target,
    }
}
fn canonical_root(graph: &mut Graph) -> Result<()> {
    let root = id("program", PROGRAM, false);
    for object in graph
        .objects
        .iter()
        .filter(|o| o.id.object_type == "program")
    {
        if object.id != root
            || object.scope != scope(namespace(false))?
            || object.observations.len() != 1
        {
            return Err(err("pilot_composition_collision"));
        }
        let observation = &object.observations[0];
        let pr3_fields = BTreeMap::from([(
            "source_contract".into(),
            Knowledge::Value(program::ADAPTER_REVISION.into()),
        )]);
        let identity_fields =
            BTreeMap::from([("program_key".into(), Knowledge::Value(PROGRAM.into()))]);
        let admitted_pr3 =
            observation.fields == pr3_fields && observation.revision == program::ADAPTER_REVISION;
        let admitted_identity = observation.fields == identity_fields
            && [
                program_git::ADAPTER_REVISION,
                v2::ADAPTER_REVISION,
                "program-pilot-identity/v1",
            ]
            .contains(&observation.source_revision.as_str());
        if !(admitted_pr3 || admitted_identity) {
            return Err(err("pilot_composition_collision"));
        }
    }
    graph.objects.retain(|o| o.id.object_type != "program");
    let identity = json!({"program_key":PROGRAM});
    let hash = graph
        .artifact(serde_json::to_vec(&identity).map_err(|_| err("pilot_serialization_failure"))?);
    graph.objects.push(Object {
        id: root,
        scope: scope(namespace(false))?,
        head: hash.clone(),
        observations: vec![Observation {
            revision: hash.clone(),
            source_revision: "program-pilot-identity/v1".into(),
            observed_at: 0,
            fresh_until: u64::MAX,
            change: Change::Initial,
            supersedes: None,
            coverage: Coverage::Complete,
            fields: BTreeMap::from([("program_key".into(), Knowledge::Value(PROGRAM.into()))]),
            lineage: Lineage {
                artifact_sha256: hash,
                pointer: "".into(),
            },
        }],
    });
    Ok(())
}
fn snapshots_v2(capture: &v2::Capture, window: ReportWindow, now: u64) -> Result<Graph> {
    let g = capture.selected_graph(window, now).map_err(intake)?;
    Ok(Graph {
        objects: g.objects,
        links: g.links,
        artifacts: g.artifacts,
    })
}
fn snapshots(capture: &program::Capture, window: ReportWindow) -> Result<Graph> {
    let (objects, links) = program::selected_graph(capture, window).map_err(intake)?;
    Ok(Graph {
        objects,
        links,
        artifacts: BTreeMap::from([(
            capture.envelope_sha256().into(),
            capture.envelope_bytes().to_vec(),
        )]),
    })
}
fn git_sources<T: program_git::Transport, C: FnMut() -> u64>(
    config: &OwnerConfig,
    capture: &program::Capture,
    transport: T,
    mut clock: C,
) -> Result<Graph> {
    git_population(config, capture.population(), transport, &mut clock)
}
fn git_population<T: program_git::Transport, C: FnMut() -> u64>(
    config: &OwnerConfig,
    source_population: &ont_core::program::PathPopulation,
    transport: T,
    mut clock: C,
) -> Result<Graph> {
    if config.repositories.is_empty() {
        return Ok(Graph::default());
    }
    let is_v2 = matches!(config.snapshot_input, Some(SnapshotInput::V2 { .. }));
    let exact = config
        .exact_definition
        .as_ref()
        .ok_or(err("pilot_config_not_ready"))?
        .metric()?;
    if exact.reference.id.namespace != namespace(false) {
        return Err(err("pilot_invalid_config"));
    }
    if !is_v2 && exact.reference == source_population.basis.definition {
        return Err(err("pilot_incompatible_definition"));
    }
    let mut population = source_population.clone();
    population.basis.definition = exact.reference;
    if !is_v2 {
        population.basis.adapter_revision = "program-pilot-exact-paths/v1".into();
    }
    let old = std::mem::take(&mut population.repositories);
    let mut configured = BTreeSet::new();
    let mut sources = Vec::new();
    let mut collector =
        program_git::Collector::new(transport, &mut clock, program_git::Limits::default())
            .map_err(git_error)?;
    for r in &config.repositories {
        if !configured.insert(r.manifest_key.clone()) {
            return Err(err("pilot_invalid_config"));
        }
        let repository =
            program_git::Repository::github(&r.owner, &r.name, r.github_id).map_err(git_error)?;
        if let Some(SnapshotInput::V2 { repositories, .. }) = &config.snapshot_input {
            let binding = repositories
                .get(&r.manifest_key)
                .ok_or(err("pilot_invalid_config"))?;
            if binding.github_id != r.github_id
                || binding.full_name != format!("{}/{}", r.owner, r.name)
            {
                return Err(err("pilot_invalid_config"));
            }
        }
        let paths = old
            .iter()
            .find(|(id, _)| {
                if is_v2 {
                    *id == repository.id()
                } else {
                    id.native_key == r.manifest_key
                }
            })
            .map(|(_, p)| p.clone())
            .ok_or(err("pilot_invalid_config"))?;
        let repository =
            program_git::Repository::github(&r.owner, &r.name, r.github_id).map_err(git_error)?;
        if population
            .repositories
            .insert(repository.id().clone(), paths)
            .is_some()
        {
            return Err(err("pilot_invalid_config"));
        }
        sources.push(
            collector
                .collect(
                    &repository,
                    &population.basis,
                    program_git::EndpointBinding {
                        commit: r.before_commit.clone(),
                        source_measurement: None,
                        observed_at: r.before_observed_at,
                    },
                    program_git::EndpointBinding {
                        commit: r.after_commit.clone(),
                        source_measurement: None,
                        observed_at: r.after_observed_at,
                    },
                )
                .map_err(git_error)?,
        );
    }
    if old.len() != configured.len() {
        return Err(err("pilot_incomplete_population"));
    }
    population
        .validate()
        .map_err(|_| err("pilot_invalid_config"))?;
    drop(collector);
    let now = clock();
    let p = program_git::build_projection(
        &population,
        &sources,
        program_git::ProjectionSpec {
            program: id("program", PROGRAM, false),
            measurement: id("measurement", "git-exact-paths", false),
            program_key: PROGRAM.into(),
            now,
        },
    )
    .map_err(git_error)?;
    Ok(Graph {
        objects: p.objects,
        links: p.links,
        artifacts: p.artifacts,
    })
}
fn not_performed(kind: &str, unit: &str, now: u64) -> Result<Graph> {
    // Lead explicitly did not perform these sources. No fabricated PR5 Contract,
    // billing identity/month or recall baseline is introduced to describe absence.
    let mut g = Graph::default();
    let fields = BTreeMap::from([
        ("guardrail".into(), Knowledge::Value(kind.into())),
        ("unit".into(), Knowledge::Value(unit.into())),
        ("value".into(), Knowledge::NotPerformed),
        ("bound_satisfied".into(), Knowledge::NotPerformed),
        ("source_qualified".into(), Knowledge::Value(false.into())),
        (
            "qualification".into(),
            Knowledge::Value("not_performed".into()),
        ),
    ]);
    let bytes=serde_json::to_vec(&json!({"adapter":"program-pilot-unperformed/v1","directive":"Lead DECISION-1: not performed until sanitized recordings supplied","fields":fields,"observed_at":now})).map_err(|_|err("pilot_serialization_failure"))?;
    let hash = g.artifact(bytes);
    let object = Object {
        id: id("guardrail", kind, true),
        scope: scope(namespace(true))?,
        head: hash.clone(),
        observations: vec![Observation {
            revision: hash.clone(),
            source_revision: "program-pilot-unperformed/v1".into(),
            observed_at: now,
            fresh_until: now,
            change: Change::Initial,
            supersedes: None,
            coverage: Coverage::Partial,
            fields,
            lineage: Lineage {
                artifact_sha256: hash,
                pointer: "/fields".into(),
            },
        }],
    };
    g.links.push(edge(
        id("program", PROGRAM, false),
        "guardrail",
        object.id.clone(),
    ));
    g.objects.push(object);
    Ok(g)
}
fn disk(recording: Option<&[u8]>, now: u64) -> Result<Graph> {
    let qualified = guards::qualify_recording(
        recording,
        &guards::Contract::Disk {
            minimum: guards::DiskMinimum::Exact {
                quantity: 15,
                unit: guards::DiskUnit::Gib,
            },
        },
        now,
    )
    .map_err(guard_error)?;
    let mut g = Graph::default();
    g.artifact(qualified.envelope_bytes().to_vec());
    let o = qualified
        .object(id("guardrail", "disk", true), scope(namespace(true))?)
        .map_err(guard_error)?;
    g.links.push(edge(
        id("program", PROGRAM, false),
        "guardrail",
        o.id.clone(),
    ));
    g.objects.push(o);
    Ok(g)
}
#[derive(Serialize)]
struct Answer {
    trust_limit: &'static str,
    real_user_session_binding: &'static str,
    attribution: &'static str,
    response: Response,
}
fn render(
    mut graph: Graph,
    policy: Policy,
    query: Query,
    now: u64,
) -> Result<(String, BTreeMap<String, Vec<u8>>)> {
    canonical_root(&mut graph)?;
    let (store, artifacts) = graph.seed()?;
    let authority = Authority::new(policy);
    let session = authority.bind_trusted(PRINCIPAL).map_err(core)?;
    let bytes = Service::new(store, authority)
        .render(&session, query, now)
        .map_err(core)?;
    let response = serde_json::from_str(&bytes).map_err(|_| err("pilot_serialization_failure"))?;
    let answer = Answer {
        trust_limit: TRUST_LIMIT,
        real_user_session_binding: "SCOPED-DEVIATION",
        attribution: "qualified merge contributions + unresolved reconciliation",
        response,
    };
    Ok((
        serde_json::to_string(&answer).map_err(|_| err("pilot_serialization_failure"))?,
        artifacts,
    ))
}

#[cfg(target_os = "linux")]
fn owner_file() -> Result<Vec<u8>> {
    use std::io::Read;
    use std::os::unix::fs::MetadataExt;
    let parent = std::fs::symlink_metadata(EVIDENCE).map_err(|_| err("pilot_config_not_ready"))?;
    if !parent.is_dir() || parent.uid() != ADOPTED_UID || parent.mode() & 0o077 != 0 {
        return Err(err("pilot_config_custody"));
    }
    let file = program::source::open_file(CONFIG).map_err(intake)?;
    let m = file.metadata().map_err(|_| err("pilot_config_custody"))?;
    if m.uid() != ADOPTED_UID || m.mode() & 0o077 != 0 {
        return Err(err("pilot_config_custody"));
    }
    let mut bytes = Vec::new();
    file.take((MAX_INPUT + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| err("pilot_config_custody"))?;
    if bytes.len() > MAX_INPUT {
        return Err(err("pilot_invalid_config"));
    }
    Ok(bytes)
}
#[cfg(target_os = "linux")]
fn disk_recording(earliest: u64) -> Result<(Vec<u8>, u64)> {
    let start = clock()?;
    if start < earliest {
        return Err(err("pilot_clock_failure"));
    }
    let outcome = match rustix::fs::statvfs("/") {
        Ok(v) => match v.f_bavail.checked_mul(v.f_frsize) {
            Some(bytes) => {
                json!({"kind":"disk","observed_at":start,"numerator":bytes,"denominator":1,"unit":"bytes"})
            }
            None => json!({"kind":"failed"}),
        },
        Err(_) => json!({"kind":"failed"}),
    };
    let end = clock()?;
    if end < start {
        return Err(err("pilot_clock_failure"));
    }
    serde_json::to_vec(&json!({"source_revision":"linux-statvfs/v1","capture_start":start,"capture_end":end,"outcome":outcome})).map(|bytes| (bytes,end)).map_err(|_|err("pilot_serialization_failure"))
}
#[cfg(target_os = "linux")]
fn retain(
    artifacts: BTreeMap<String, Vec<u8>>,
    answer: &str,
    request: &[u8],
    config: &[u8],
    now: u64,
) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| err("pilot_clock_failure"))?
        .as_nanos();
    let root = std::path::Path::new(EVIDENCE).join(format!("run-{}-{unique}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .map_err(|_| err("pilot_evidence_failure"))?;
    let write = |name: &str, bytes: &[u8]| -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(root.join(name))
            .map_err(|_| err("pilot_evidence_failure"))?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| err("pilot_evidence_failure"))
    };
    for (hash, bytes) in artifacts {
        write(&format!("{hash}.json"), &bytes)?;
    }
    write("answer.json", answer.as_bytes())?;
    write("request.json", request)?;
    let manifest=serde_json::to_vec(&json!({"render_now":now,"request_sha256":digest(request),"owner_config_sha256":digest(config),"answer_sha256":digest(answer.as_bytes()),"trust_limit":TRUST_LIMIT,"real_user_session_binding":"SCOPED-DEVIATION"})).map_err(|_|err("pilot_serialization_failure"))?;
    write("run.json", &manifest)?;
    std::fs::File::open(&root)
        .and_then(|f| f.sync_all())
        .map_err(|_| err("pilot_evidence_failure"))
}

#[cfg(target_os = "linux")]
fn execute_v2(
    input: &[u8],
    config_bytes: &[u8],
    config: &OwnerConfig,
    policy: Policy,
    query: Query,
    window: ReportWindow,
) -> Result<String> {
    let mut previous = clock()?;
    let mut failed = false;
    let capture = v2::capture_fixed(config.v2_contract()?, || match clock() {
        Ok(t) if t >= previous => {
            previous = t;
            t
        }
        _ => {
            failed = true;
            previous
        }
    })
    .map_err(intake)?;
    if failed {
        return Err(err("pilot_clock_failure"));
    }
    let git = git_population(
        config,
        capture.population(),
        program_git::GhTransport,
        || match clock() {
            Ok(t) if t >= previous => {
                previous = t;
                t
            }
            _ => {
                failed = true;
                previous
            }
        },
    )?;
    if failed {
        return Err(err("pilot_clock_failure"));
    }
    let now = clock()?;
    if now < previous {
        return Err(err("pilot_clock_failure"));
    }
    let mut graph = snapshots_v2(&capture, window, now)?;
    graph.merge(git)?;
    let (answer, artifacts) = render(graph, policy, query, now)?;
    retain(artifacts, &answer, input, config_bytes, now)?;
    Ok(answer)
}

/// Fixed host UID/config/source boundary. Call only in a separately released live
/// run. Unit tests use internal synthetic seams and never invoke this acquisition.
#[cfg(target_os = "linux")]
pub fn execute(input: &[u8]) -> Result<String> {
    check_uid(
        rustix::process::getuid().as_raw(),
        rustix::process::geteuid().as_raw(),
    )?;
    let (query, window) = request(input)?;
    let config_bytes = owner_file()?;
    let config = OwnerConfig::parse(&config_bytes)?;
    let policy = config.policy()?;
    if matches!(config.snapshot_input, Some(SnapshotInput::V2 { .. })) {
        return execute_v2(input, &config_bytes, &config, policy, query, window);
    }
    let contract = config.contract()?;
    // Checked fallible clock around fixed source capture; callback is a trusted
    // captured value, never stdin, with each boundary sampled explicitly.
    let start = clock()?;
    let mut previous = start;
    let mut clock_failed = false;
    let mut trusted_clock = || match clock() {
        Ok(t) if t >= previous => {
            previous = t;
            t
        }
        _ => {
            clock_failed = true;
            previous
        }
    };
    let capture_result = program::capture_fixed(contract, &mut trusted_clock);
    if clock_failed {
        return Err(err("pilot_clock_failure"));
    }
    let capture = capture_result.map_err(intake)?;
    let end = clock()?;
    if end < previous {
        return Err(err("pilot_clock_failure"));
    }
    if config.manifest_sha256.as_deref() != Some(&capture.population().basis.manifest_sha256) {
        return Err(err("pilot_manifest_mismatch"));
    }
    let mut g = snapshots(&capture, window)?;
    let mut previous = end;
    let mut clock_failed = false;
    let git_result = git_sources(
        &config,
        &capture,
        program_git::GhTransport,
        || match clock() {
            Ok(t) if t >= previous => {
                previous = t;
                t
            }
            _ => {
                clock_failed = true;
                previous
            }
        },
    );
    if clock_failed {
        return Err(err("pilot_clock_failure"));
    }
    g.merge(git_result?)?;
    let (recording, disk_end) = disk_recording(previous)?;
    let now = clock()?;
    if now < disk_end {
        return Err(err("pilot_clock_failure"));
    }
    g.merge(disk(Some(&recording), now)?)?;
    for (kind, unit) in [
        ("billing", "linux_minutes"),
        ("recall", "fraction"),
        ("defects", "count"),
    ] {
        g.merge(not_performed(kind, unit, now)?)?;
    }
    let (answer, artifacts) = render(g, policy, query, now)?;
    retain(artifacts, &answer, input, &config_bytes, now)?;
    Ok(answer)
}
#[cfg(not(target_os = "linux"))]
pub fn execute(_: &[u8]) -> Result<String> {
    Err(err("pilot_platform_unavailable"))
}

#[cfg(test)]
mod tests;
