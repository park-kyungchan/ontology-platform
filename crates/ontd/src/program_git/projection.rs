use super::*;
use ont_core::{Change, Knowledge, Lineage, Link, Object, Observation, Scope};
use ont_store::Store;

/// Trusted disclosure choice: an aggregate measurement requires its own grant.
/// This builder supplies no grants, sessions or action permissions. program and
/// measurement must share the owner's program namespace; source objects retain
/// their separate repository namespaces. The program root exposes only configured
/// identity metadata; a program-type grant does not grant aggregate disclosure.
pub struct ProjectionSpec {
    pub program: Id,
    pub measurement: Id,
    pub program_key: String,
    pub now: u64,
}
pub struct Projection {
    pub attribution: program::Attribution,
    pub objects: Vec<Object>,
    pub links: Vec<Link>,
    /// Exact sanitized envelope backing every observation's lineage pointer.
    pub artifact: Vec<u8>,
    /// Per-object artifacts keep public source objects independent of aggregates.
    pub artifacts: BTreeMap<String, Vec<u8>>,
}
impl Projection {
    /// Rebuild a disposable, complete Program graph. Source refresh is owner-run;
    /// no graph append/refresh API is exposed to a consumer query.
    pub fn memory_store(&self) -> Result<Store, IntakeError> {
        let mut store = Store::memory_program().map_err(|_| IntakeError::Store)?;
        store
            .seed(&self.objects, &self.links)
            .map_err(|_| IntakeError::Store)?;
        Ok(store)
    }
}

/// Finite vocabulary: program -> repository / has_measurement,
/// measurement -> supported_by -> change, change -> in_repository.
/// Every endpoint exists, every change has exactly one repository. Typed
/// attribution references and links are generated together, never inferred from
/// navigational edges. Exact path sets stay internal; evidence exports hashes.
pub fn build_projection(
    population: &program::PathPopulation,
    captures: &[CollectedInterval],
    spec: ProjectionSpec,
) -> Result<Projection, IntakeError> {
    if spec.program.object_type != "program"
        || spec.measurement.object_type != "measurement"
        || spec.program.namespace != spec.measurement.namespace
        || spec.program_key.is_empty()
        || spec.program_key.len() > 512
        || captures.is_empty()
    {
        return Err(IntakeError::InvalidConfiguration);
    }
    spec.program
        .validate()
        .map_err(|_| IntakeError::InvalidConfiguration)?;
    spec.measurement
        .validate()
        .map_err(|_| IntakeError::InvalidConfiguration)?;
    let intervals: Vec<_> = captures.iter().map(|c| c.interval.clone()).collect();
    let attribution = program::attribute_v1(population, &intervals, spec.now)?;
    let start = captures
        .iter()
        .map(|c| c.captured.start)
        .min()
        .ok_or(IntakeError::InvalidConfiguration)?;
    // Oldest status capture bounds freshness, never the time of projection build.
    let fresh_until = start
        .checked_add(900)
        .ok_or(IntakeError::InvalidConfiguration)?;
    let changes: Vec<_> = attribution.contributions.iter().map(|c| {
        let (association, prs): (&str, Vec<program::PrId>) = match &c.association {
            program::PrAssociation::Unique(id) => ("unique", vec![id.clone()]),
            program::PrAssociation::Ambiguous(ids) => ("ambiguous", ids.iter().cloned().collect()),
            program::PrAssociation::Missing => ("missing", vec![]),
            program::PrAssociation::Unknown { eligible } => ("unknown", eligible.iter().cloned().collect()),
        };
        json!({"repository":c.repository,"commit":c.commit,"first_parent":c.first_parent,"newly_absent_count":c.newly_absent.len(),"restored_count":c.restored.len(),"path_set_sha256":sha256(json!({"absent":c.newly_absent,"restored":c.restored}).to_string().as_bytes()),"paths":c.paths.to_string(),"association":association,"changes":prs.iter().map(change_id).collect::<Vec<_>>()})
    }).collect();
    let mut records: BTreeMap<Id, Value> = BTreeMap::new();
    records.insert(
        spec.program.clone(),
        json!({"program_key":spec.program_key}),
    );
    let details = json!({
        "rule":"attribution-v1", "adapter":ADAPTER_REVISION,
        "definition":{"id":population.basis.definition.id,"revision":population.basis.definition.revision},
        "manifest_sha256":population.basis.manifest_sha256,
        "population_adapter_revision":population.basis.adapter_revision,
        "algorithm":"exact_utf8_paths_v1", "denominator":population.basis.denominator,
        "delta_bound":attribution.delta_bound.map(|n|n.to_string()),
        "residual":attribution.residual.map(|n|n.to_string()),
        "qualification":attribution.qualification(),
        "gaps":attribution.gaps.iter().map(|g| match g {
            program::ReconciliationGap::EndpointBinding=>"endpoint_binding",
            program::ReconciliationGap::DefinitionMismatch=>"definition_mismatch",
            program::ReconciliationGap::FirstParentCoverage=>"first_parent_coverage",
            program::ReconciliationGap::PrAssociation=>"pr_association",
            program::ReconciliationGap::FutureEndpoint=>"future_endpoint",
        }).collect::<Vec<_>>(),
        "contributions":changes,
    });
    records.insert(spec.measurement.clone(), json!({"attribution":details}));
    let mut links = vec![Link {
        source: spec.program.clone(),
        relation: "has_measurement".into(),
        target: spec.measurement.clone(),
    }];
    let mut prs = BTreeMap::new();
    for capture in captures {
        let repository = &capture.repository;
        if records
            .insert(
                repository.id.clone(),
                json!({"repository":repository.slug,"github_id":repository.github_id}),
            )
            .is_some()
        {
            return Err(IntakeError::InvalidConfiguration);
        }
        links.push(Link {
            source: spec.program.clone(),
            relation: "repository".into(),
            target: repository.id.clone(),
        });
        for change in &capture.interval.changes {
            for pr in &change.prs.candidates {
                if pr.id.repository != repository.id {
                    return Err(IntakeError::InvalidResponse);
                }
                let id = change_id(&pr.id);
                let pull = capture.artifact["responses"]
                    .as_array()
                    .and_then(|responses| {
                        responses
                            .iter()
                            .filter_map(|r| r["pulls"].as_array())
                            .flatten()
                            .find(|p| p["number"].as_u64() == Some(pr.id.number))
                    })
                    .ok_or(IntakeError::InvalidResponse)?;
                let metadata = json!({"repository":repository.id,"number":pr.id.number,"merged":pr.merged,"merge_commit":pr.merge_commit,"metadata":pull});
                if let Some(old) = prs.insert(id.clone(), metadata.clone())
                    && old != metadata
                {
                    return Err(IntakeError::InvalidResponse);
                }
            }
        }
    }
    for (id, metadata) in prs {
        let repository = captures
            .iter()
            .find(|c| c.repository.id.namespace == id.namespace)
            .ok_or(IntakeError::InvalidResponse)?
            .repository
            .id
            .clone();
        if records.insert(id.clone(), metadata).is_some() {
            return Err(IntakeError::InvalidConfiguration);
        }
        links.push(Link {
            source: id,
            relation: "in_repository".into(),
            target: repository,
        });
    }
    let mut supported = BTreeSet::new();
    for contribution in &attribution.contributions {
        let ids: Vec<_> = match &contribution.association {
            program::PrAssociation::Unique(id) => vec![id.clone()],
            program::PrAssociation::Ambiguous(ids)
            | program::PrAssociation::Unknown { eligible: ids } => ids.iter().cloned().collect(),
            program::PrAssociation::Missing => vec![],
        };
        supported.extend(ids.iter().map(change_id));
    }
    for id in supported {
        if !records.contains_key(&id) {
            return Err(IntakeError::InvalidResponse);
        }
        links.push(Link {
            source: spec.measurement.clone(),
            relation: "supported_by".into(),
            target: id,
        });
    }
    let partial = !attribution.gaps.is_empty()
        || captures.iter().any(|c| {
            c.artifact["responses"].as_array().is_some_and(|responses| {
                responses.iter().any(|r| {
                    r.get("failure").is_some()
                        || r["pulls"].as_array().is_some_and(|pulls| {
                            pulls.iter().any(|p| p["checks"]["coverage"] == "partial")
                        })
                })
            })
        });
    let records: Vec<_> = records.into_iter().map(|(id, fields)| {
        // Configured program/repository identity and public source observations
        // never inherit aggregate endpoint/reconciliation state or another
        // repository's sources, timing, coverage or artifact hash.
        let (observed_at, fresh_until, partial) = match id.object_type.as_str() {
            "program" | "repository" => (0, u64::MAX, false), // complete configured identity only
            "change" => {
                let source = captures.iter().find(|c| c.repository.id.namespace == id.namespace).ok_or(IntakeError::InvalidResponse)?;
                let since = source.captured.start;
                (since, since.checked_add(900).ok_or(IntakeError::InvalidConfiguration)?, spec.now < source.captured.end || fields["metadata"]["checks"]["coverage"] != "complete")
            }
            _ => (start, fresh_until, partial),
        };
        Ok(json!({"id":id,"scope":Scope::Program { namespace:id.namespace.clone(), program_key:spec.program_key.clone() },"fields":fields,"observed_at":observed_at,"fresh_until":fresh_until,"coverage":if partial {Coverage::Partial} else {Coverage::Complete}}))
    }).collect::<Result<Vec<_>, IntakeError>>()?;
    let envelope = json!({"adapter":ADAPTER_REVISION,"sources":captures.iter().map(|c| &c.artifact).collect::<Vec<_>>(),"records":records,"links":links});
    let artifact = serde_json::to_vec(&envelope).map_err(|_| IntakeError::InvalidResponse)?;
    let digest = sha256(&artifact);
    let mut artifacts = BTreeMap::from([(digest.clone(), artifact.clone())]);
    let mut objects = vec![];
    for (i, record) in records.iter().enumerate() {
        let id: Id = serde_json::from_value(record["id"].clone())
            .map_err(|_| IntakeError::InvalidResponse)?;
        let scope = serde_json::from_value(record["scope"].clone())
            .map_err(|_| IntakeError::InvalidResponse)?;
        let fields = record["fields"]
            .as_object()
            .ok_or(IntakeError::InvalidResponse)?
            .iter()
            .map(|(k, v)| (k.clone(), Knowledge::Value(v.clone())))
            .collect();
        let (object_digest, pointer) =
            if matches!(id.object_type.as_str(), "program" | "repository" | "change") {
                let source_artifact =
                    serde_json::to_vec(&json!({"adapter":ADAPTER_REVISION,"record":record}))
                        .map_err(|_| IntakeError::InvalidResponse)?;
                let own_digest = sha256(&source_artifact);
                artifacts.insert(own_digest.clone(), source_artifact);
                (own_digest, "/record".to_string())
            } else {
                (digest.clone(), format!("/records/{i}"))
            };
        let observation = Observation {
            revision: object_digest.clone(),
            source_revision: ADAPTER_REVISION.into(),
            observed_at: record["observed_at"]
                .as_u64()
                .ok_or(IntakeError::InvalidResponse)?,
            fresh_until: record["fresh_until"]
                .as_u64()
                .ok_or(IntakeError::InvalidResponse)?,
            change: Change::Initial,
            supersedes: None,
            coverage: serde_json::from_value(record["coverage"].clone())
                .map_err(|_| IntakeError::InvalidResponse)?,
            fields,
            lineage: Lineage {
                artifact_sha256: object_digest.clone(),
                pointer,
            },
        };
        observation
            .validate()
            .map_err(|_| IntakeError::InvalidResponse)?;
        objects.push(Object {
            id,
            scope,
            head: object_digest,
            observations: vec![observation],
        });
    }
    Ok(Projection {
        attribution,
        objects,
        links,
        artifact,
        artifacts,
    })
}
fn change_id(pr: &PrId) -> Id {
    Id {
        object_type: "change".into(),
        namespace: pr.repository.namespace.clone(),
        native_key: pr.number.to_string(),
    }
}
