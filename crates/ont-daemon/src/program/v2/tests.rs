#![allow(clippy::unwrap_used)]
use super::*;
pub(crate) const NOW: u64 = 1793843040;
pub(crate) struct Fixture {
    pub row: Value,
    pub manifest: Vec<u8>,
    pub evidence: Evidence,
    pub contract: IntakeContract,
}
fn bytes(v: &Value) -> Vec<u8> {
    serde_json::to_vec(v).unwrap()
}
fn add(out: &mut BTreeMap<String, Vec<u8>>, name: &str, v: Value) -> Value {
    let data = bytes(&v);
    let a = ArtifactRef::new(name, &data);
    out.insert(name.into(), data);
    value(&a).unwrap()
}
fn unknown() -> Value {
    json!({"kind":"unknown"})
}
fn known(v: Value) -> Value {
    json!({"kind":"value","value":v})
}
fn source(status: &str, at: Option<u64>, artifact: Value, start: u64, end: u64) -> Value {
    json!({"status":status,"reason":if status=="captured"{"none"}else{"not_performed"},"capture":{"start":start,"end":end},"observed_at":at.map_or_else(unknown,|at|known(json!(at))),"artifact":artifact,"exit_code":if status=="not_performed"{json!({"kind":"not_performed"})}else{unknown()},"signal":if status=="not_performed"{json!({"kind":"not_performed"})}else{unknown()}})
}
pub(crate) fn fixture() -> Fixture {
    let ns = Namespace {
        instance: "vps://ws-replan".into(),
        account: "program-owner".into(),
    };
    let gn = Namespace {
        instance: "vps://ws-replan/guardrails".into(),
        account: "program-owner".into(),
    };
    let mut manifest_repos = serde_json::Map::new();
    let paths = BTreeSet::from(["old.ts".to_string(), "한글\t\n\"\\.ts".to_string()]);
    for key in ["a", "b"] {
        manifest_repos.insert(key.into(),json!({"tracked_file_count":2,"groups":[{"disposition":"DELETE_NOW","count":2,"files":paths}]}));
    }
    let manifest = bytes(
        &json!({"schema_version":1,"repos":manifest_repos,"verification":{"exact_non_keep_removal_paths":4}}),
    );
    let identities: BTreeMap<String, Identity> =
        [("a", 41, "owner-a/same"), ("b", 42, "owner-b/same")]
            .into_iter()
            .map(|(key, id, name)| {
                (
                    key.into(),
                    Identity {
                        github_id: id,
                        full_name: name.into(),
                    },
                )
            })
            .collect();
    let cs:BTreeMap<String,wire::Contract>=[json!({"kind":"billing","account":"synthetic-account","month":{"year":2026,"month":11},"linux_skus":["synthetic-linux"],"limit":3000}),json!({"kind":"disk","minimum":{"kind":"exact","quantity":15,"unit":"gib"}}),json!({"kind":"recall","baseline":{"supported":15,"total":33,"evaluation_set_sha256":"a".repeat(64),"result_revision":"synthetic-baseline","artifact_sha256":"b".repeat(64)}}),json!({"kind":"defects","week_start":1793577600_u64,"limit":0})].into_iter().map(|v|{let c:wire::Contract=serde_json::from_value(v).unwrap();(c.kind().into(),c)}).collect();
    let mut artifacts = BTreeMap::new();
    let build = add(
        &mut artifacts,
        "build.json",
        json!({"source_commit":"1".repeat(40),"source_tree":"2".repeat(40),"binary_sha256":"3".repeat(64),"clean":true}),
    );
    let definition = add(
        &mut artifacts,
        "definition.json",
        json!({"revision":DEFINITION,"algorithm_revision":ALGORITHM,"manifest_sha256":digest(&manifest),"repositories":identities.values().collect::<Vec<_>>(),"parity_tool_ids":["synthetic-tool"],"billing":cs["billing"],"disk":cs["disk"],"recall":cs["recall"],"defects":cs["defects"]}),
    );
    let requests:Vec<_>=identities.iter().map(|(key,r)|json!({"manifest_key":key,"github_id":r.github_id,"full_name":r.full_name,"local_root":unknown(),"backend":"api_only"})).collect();
    let request = add(
        &mut artifacts,
        "request.json",
        json!({"schema_version":"ws-replan-kpi-capture-request/v2","measurement_id":"synthetic-test","manifest_path":super::super::MANIFEST_PATH,"definition_contract_path":"/synthetic/definition.json","build_evidence_path":"/synthetic/build.json","journal_path":SNAPSHOTS_PATH,"evidence_root":EVIDENCE_ROOT,"repositories":requests,"billing_recording_path":{"kind":"not_performed"},"parity_recording_path":{"kind":"not_performed"},"recall_recording_path":{"kind":"not_performed"},"defects_recording_path":{"kind":"not_performed"},"disk_target":"/","limits":{"process_timeout_seconds":5,"capture_timeout_seconds":60,"process_output_bytes":1048576,"evidence_bytes":33554432}}),
    );
    let mut repos = Vec::new();
    for (index, (key, r)) in identities.iter().enumerate() {
        let commit = if index == 0 { "4" } else { "5" }.repeat(40);
        let tree = if index == 0 { "6" } else { "7" }.repeat(40);
        let interval = json!({"start":NOW-4,"end":NOW-3});
        let tree_interval = json!({"start":NOW-3,"end":NOW-2});
        let ref_ = add(
            &mut artifacts,
            &format!("repo-{index}-ref.json"),
            json!({"identity":r,"ref":"refs/heads/main","commit_sha":commit,"identity_response_sha256":"8".repeat(64),"ref_response_sha256":"9".repeat(64),"capture":interval}),
        );
        let leaf = BTreeSet::from(["old.ts".to_string()]);
        let absent = paths.difference(&leaf).cloned().collect::<BTreeSet<_>>();
        let tr = add(
            &mut artifacts,
            &format!("repo-{index}-tree.json"),
            json!({"identity":r,"commit_sha":commit,"tree_sha":tree,"backend":"github_api","capture":tree_interval,"commit_response_sha256":"a".repeat(64),"tree_response_sha256":"b".repeat(64),"leaf_path_set_sha256":path_hash(&leaf),"absent_path_set_sha256":path_hash(&absent),"complete":true}),
        );
        let mut members = paths
            .iter()
            .map(|p| json!({"path_sha256":digest(p.as_bytes()),"present":leaf.contains(p)}))
            .collect::<Vec<_>>();
        members.sort_by_key(|m| m["path_sha256"].as_str().unwrap().to_string());
        let membership = add(
            &mut artifacts,
            &format!("repo-{index}-membership.json"),
            json!({"identity":r,"commit_sha":commit,"tree_sha":tree,"members":members}),
        );
        repos.push(json!({"manifest_key":key,"identity":r,"ref_name":"refs/heads/main","ref_read":source("captured",Some(NOW-3),known(ref_),NOW-4,NOW-3),"commit_sha":known(json!(commit)),"tree_sha":known(json!(tree)),"tree_read":source("captured",Some(NOW-3),known(tr),NOW-3,NOW-2),"backend":"github_api","population":{"total":2,"path_set_sha256":path_hash(&paths)},"removed":known(json!(1)),"leaf_path_set_sha256":known(json!(path_hash(&leaf))),"absent_path_set_sha256":known(json!(path_hash(&absent))),"membership":known(membership),"source_qualified":true,"qualification":"qualified"}));
    }
    let mut guard = serde_json::Map::new();
    for (field, kind, unit) in [
        ("guard_ci_minutes_month", "billing", "linux_minutes"),
        ("guard_disk_free_gib", "disk", "bytes"),
        ("guard_recall_regression", "recall", "fraction"),
        ("guard_escaped_defects_week", "defects", "count"),
    ] {
        let contract = add(
            &mut artifacts,
            &format!("{kind}-contract.json"),
            value(&cs[kind]).unwrap(),
        );
        let performed = kind == "disk";
        let recording = add(
            &mut artifacts,
            &format!("{kind}-recording.json"),
            json!({"source_revision":if performed{"statvfs-root-f-bfree/v2"}else{"explicit-not-performed/v2"},"capture_start":NOW-1,"capture_end":NOW-1,"outcome":if performed{json!({"kind":"disk","observed_at":NOW-1,"numerator":20_u64<<30,"denominator":1,"unit":"bytes"})}else{json!({"kind":"not_performed"})}}),
        );
        guard.insert(field.into(),json!({"kind":kind,"value":if performed{known(json!(20_u64<<30))}else{json!({"kind":"not_performed"})},"unit":unit,"total":if kind=="recall"{json!({"kind":"not_performed"})}else{unknown()},"free_gib":if performed{known(json!({"numerator":20_u64<<30,"denominator":GIB,"unit":"GiB"}))}else{unknown()},"source":source(if performed{"captured"}else{"not_performed"},if performed{Some(NOW-1)}else{None},known(recording.clone()),NOW-1,NOW),"source_qualified":performed,"qualification":if performed{"qualified"}else{"not_performed"},"contract":contract,"recording":known(recording)}));
    }
    let producer = ProducerPin {
        source_commit: "1".repeat(40),
        source_tree: "2".repeat(40),
        binary_sha256: "3".repeat(64),
        build_evidence_sha256: build["sha256"].as_str().unwrap().into(),
    };
    let contract = IntakeContract {
        namespace: ns.clone(),
        guardrail_namespace: gn,
        program_key: "ws-replan".into(),
        source_owner_uid: 0,
        definition: MetricDefinition {
            reference: ont_core::program::DefinitionRef {
                id: Id {
                    object_type: "metric".into(),
                    namespace: ns,
                    native_key: "exact".into(),
                },
                revision: DEFINITION.into(),
            },
            formula: "exact absent paths / non-KEEP paths".into(),
            population: "synthetic manifest".into(),
            unit: Unit::Fraction,
            direction: ont_core::program::Direction::Increase,
            role: MetricRole::Indicator,
        },
        producer: producer.clone(),
        definition_sha256: definition["sha256"].as_str().unwrap().into(),
        manifest_sha256: digest(&manifest),
        repositories: identities,
        guardrails: cs,
    };
    for repo in &mut repos {
        repo["ref_read"]["exit_code"] = known(json!(0));
        repo["tree_read"]["exit_code"] = known(json!(0));
    }
    let refs = artifacts
        .iter()
        .map(|(name, b)| ArtifactRef::new(name, b))
        .collect::<Vec<_>>();
    let mut row = json!({"schema_version":SCHEMA,"measurement_id":"synthetic-test","capture_status":"partial","capture":{"start":NOW-5,"end":NOW},"producer":{"revision":PRODUCER,"source_commit":producer.source_commit,"source_tree":producer.source_tree,"binary_sha256":producer.binary_sha256,"build_evidence":build,"git_version":"git version synthetic","gh_version":"gh version synthetic"},"definition":{"revision":DEFINITION,"algorithm_revision":ALGORITHM,"contract":definition,"manifest":{"source_id":"ws-replan-plan-a-v1-manifest","sha256":digest(&manifest),"bytes":manifest.len()},"request":request},"kpi_legacy_removal":{"value":known(json!({"removed":2,"total":4,"unit":"fraction"})),"source":source("captured",Some(NOW-3),unknown(),NOW-5,NOW),"source_qualified":true,"qualification":"qualified"},"kpi_mcp_parity":{"value":{"kind":"not_performed"},"source":source("not_performed",None,unknown(),NOW-1,NOW),"source_qualified":false,"qualification":"not_performed"},"repositories":repos,"artifacts":refs});
    row.as_object_mut().unwrap().extend(guard);
    Fixture {
        row,
        manifest,
        evidence: BTreeMap::from([("synthetic-test".into(), artifacts)]),
        contract,
    }
}
impl Fixture {
    pub fn journal(&self) -> Vec<u8> {
        let mut b = bytes(&self.row);
        b.push(b'\n');
        b
    }
    pub fn capture(&self) -> Result<Capture> {
        Capture::recorded(
            &self.journal(),
            &self.manifest,
            self.evidence.clone(),
            NOW + 1,
            NOW + 2,
            self.contract.clone(),
        )
    }
    pub(crate) fn replace_artifact(&mut self, name: &str, v: Value) {
        let b = bytes(&v);
        let new = value(&ArtifactRef::new(name, &b)).unwrap();
        fn replace(v: &mut Value, name: &str, new: &Value) {
            match v {
                Value::Object(m) if m.get("path").and_then(Value::as_str) == Some(name) => {
                    *v = new.clone()
                }
                Value::Object(m) => {
                    for v in m.values_mut() {
                        replace(v, name, new);
                    }
                }
                Value::Array(a) => {
                    for v in a {
                        replace(v, name, new);
                    }
                }
                _ => {}
            }
        }
        replace(&mut self.row, name, &new);
        self.evidence
            .get_mut("synthetic-test")
            .unwrap()
            .insert(name.into(), b);
    }
}
#[test]
fn partial_capture_exact_rows_pins_and_pr5_fields() {
    let f = fixture();
    let c = f.capture().unwrap();
    assert_eq!(c.population.repositories.len(), 2);
    assert_ne!(
        c.population.repositories.keys().next().unwrap(),
        c.population.repositories.keys().nth(1).unwrap()
    );
    let e: Value = serde_json::from_slice(c.envelope_bytes()).unwrap();
    assert_eq!(
        e["row_sources"][0]["raw_utf8"],
        String::from_utf8(f.journal()).unwrap()
    );
    assert_eq!(e["row_sources"][0]["line"], 1);
    assert_eq!(e["row_sources"][0]["sha256"], digest(&f.journal()));
    assert_eq!(e["source"]["snapshots_sha256"], digest(&f.journal()));
    let g = c
        .selected_graph(
            ReportWindow {
                start: 0,
                end: u64::MAX,
            },
            NOW,
        )
        .unwrap();
    let removal = g
        .objects
        .iter()
        .find(|o| o.id.object_type == "measurement" && o.id.native_key.ends_with(SELECTORS[0]))
        .unwrap();
    assert_eq!(removal.observations[0].coverage, Coverage::Complete);
    assert!(
        g.artifacts
            .contains_key(&removal.observations[0].lineage.artifact_sha256)
    );
    let raw = g
        .objects
        .iter()
        .find(|o| o.id.object_type == "program_source")
        .unwrap();
    assert_eq!(raw.observations[0].lineage.artifact_sha256, c.hash);
    assert_eq!(
        removal.observations[0].lineage.pointer,
        "/rows/0/kpi_legacy_removal/value"
    );
    let disk = g
        .objects
        .iter()
        .find(|o| o.id.object_type == "guardrail" && o.id.native_key.ends_with(":disk"))
        .unwrap();
    let fields = &disk.observations[0].fields;
    assert_eq!(fields["value"], Knowledge::Value((20_u64 << 30).into()));
    assert_eq!(
        fields["bound_value"],
        Knowledge::Value(16106127360_u64.into())
    );
    assert_eq!(fields["source_unit"], Knowledge::Value("bytes".into()));
    assert_eq!(fields["bound_satisfied"], Knowledge::Value(true.into()));
    for kind in ["billing", "recall", "defects"] {
        let o = g
            .objects
            .iter()
            .find(|o| {
                o.id.object_type == "guardrail" && o.id.native_key.ends_with(&format!(":{kind}"))
            })
            .unwrap();
        assert_eq!(o.observations[0].fields["value"], Knowledge::NotPerformed);
        assert_eq!(o.observations[0].coverage, Coverage::Partial);
    }
}
#[test]
fn strict_closed_duplicate_fractional_truncation_and_bounds() {
    let f = fixture();
    let journal = f.journal();
    let admit = |b: &[u8]| {
        Capture::recorded(
            b,
            &f.manifest,
            f.evidence.clone(),
            NOW + 1,
            NOW + 2,
            f.contract.clone(),
        )
    };
    assert!(admit(&journal[..journal.len() - 1]).is_err());
    assert!(admit(b"{}\n").is_err());
    assert!(admit(b"{\n").is_err());
    let duplicate = String::from_utf8(journal.clone()).unwrap().replacen(
        "\"capture_status\":\"partial\"",
        "\"capture_status\":\"partial\",\"capture_status\":\"partial\"",
        1,
    );
    assert!(admit(duplicate.as_bytes()).is_err());
    for pointer in [
        "/future",
        "/producer/extra",
        "/capture/future",
        "/repositories/0/identity/future",
        "/guard_disk_free_gib/value/future",
    ] {
        let mut v = f.row.clone();
        if pointer == "/future" {
            v["future"] = json!(1);
        } else {
            let (parent, key) = pointer.rsplit_once('/').unwrap();
            v.pointer_mut(parent)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert(key.into(), json!(1));
        }
        let mut b = bytes(&v);
        b.push(b'\n');
        assert!(admit(&b).is_err(), "{pointer}");
    }
    for token in ["1.0", "1e0", "-1", "18446744073709551616", "true"] {
        let row = String::from_utf8(journal.clone()).unwrap().replacen(
            "\"total\":4",
            &format!("\"total\":{token}"),
            1,
        );
        assert!(admit(row.as_bytes()).is_err(), "{token}");
    }
    let mut oversized = journal.clone();
    oversized.splice(0..0, std::iter::repeat_n(b' ', LINE_LIMIT));
    assert!(admit(&oversized).is_err());
    assert!(admit(&vec![b'\n'; JOURNAL_LIMIT + 1]).is_err());
}
#[test]
fn artifact_trust_endpoint_and_membership_contradictions_refuse() {
    for mutate in 0..10 {
        let mut f = fixture();
        match mutate {
            0 => f
                .evidence
                .get_mut("synthetic-test")
                .unwrap()
                .get_mut("build.json")
                .unwrap()
                .push(b' '),
            1 => f.contract.producer.source_commit = "f".repeat(40),
            2 => f.row["repositories"][0]["identity"]["github_id"] = json!(999),
            3 => f.row["repositories"][0]["tree_sha"] = known(json!("f".repeat(40))),
            4 => {
                let mut v: Value =
                    serde_json::from_slice(&f.evidence["synthetic-test"]["repo-0-membership.json"])
                        .unwrap();
                v["members"][0]["present"] = json!(!v["members"][0]["present"].as_bool().unwrap());
                f.replace_artifact("repo-0-membership.json", v);
            }
            5 => {
                let mut v: Value =
                    serde_json::from_slice(&f.evidence["synthetic-test"]["repo-0-ref.json"])
                        .unwrap();
                v["capture"]["end"] = json!(NOW - 2);
                f.replace_artifact("repo-0-ref.json", v);
            }
            6 => f.row["repositories"]
                .as_array_mut()
                .unwrap()
                .pop()
                .map(|_| ())
                .unwrap(),
            7 => {
                // Structurally consistent row values still contradict the retained recording.
                f.row["guard_disk_free_gib"]["value"] = known(json!(19_u64 << 30));
                f.row["guard_disk_free_gib"]["free_gib"]["value"]["numerator"] =
                    json!(19_u64 << 30);
            }
            8 => f.row["guard_ci_minutes_month"]["source_qualified"] = json!(true),
            _ => f.row["guard_disk_free_gib"]["qualification"] = json!("unresolved_disk_unit"),
        }
        assert!(f.capture().is_err(), "mutation {mutate}");
    }
}
#[test]
fn replay_same_id_conflict_is_atomic_and_keeps_first_lineage() {
    let f = fixture();
    let c = f.capture().unwrap();
    let mut p = Projection::new(&c, NOW).unwrap();
    assert_eq!(p.ingest(&c).unwrap(), Replay::Unchanged);
    let mut raw = f.journal();
    raw.insert(0, b' ');
    let conflict = Capture::recorded(
        &raw,
        &f.manifest,
        f.evidence.clone(),
        NOW + 1,
        NOW + 2,
        f.contract.clone(),
    )
    .unwrap();
    assert_eq!(p.ingest(&conflict), Err(IntakeError::InvalidData));
    assert_eq!(p.ingest(&c).unwrap(), Replay::Unchanged);
    let mut second = f.journal();
    second.extend(f.journal());
    let repeated = Capture::recorded(
        &second,
        &f.manifest,
        f.evidence.clone(),
        NOW + 1,
        NOW + 2,
        f.contract,
    )
    .unwrap();
    assert_eq!(p.ingest(&repeated).unwrap(), Replay::Unchanged);
}
#[test]
fn missing_repository_keeps_qualified_component_and_expiry_is_recomputed() {
    let mut f = fixture();
    f.row["repositories"][1]["source_qualified"] = json!(false);
    f.row["repositories"][1]["qualification"] = json!("failed_source");
    f.row["repositories"][1]["removed"] = unknown();
    f.row["repositories"][1]["tree_read"]["status"] = json!("failed");
    f.row["repositories"][1]["tree_read"]["reason"] = json!("git_exit_nonzero");
    f.row["kpi_legacy_removal"]["value"] = unknown();
    f.row["kpi_legacy_removal"]["source_qualified"] = json!(false);
    f.row["kpi_legacy_removal"]["qualification"] = json!("incomplete_source");
    f.row["kpi_legacy_removal"]["source"]["status"] = json!("incomplete");
    f.row["kpi_legacy_removal"]["source"]["reason"] = json!("incomplete_repository_population");
    f.row["kpi_legacy_removal"]["source"]["observed_at"] = unknown();
    let c = f.capture().unwrap();
    let g = c
        .selected_graph(
            ReportWindow {
                start: 0,
                end: u64::MAX,
            },
            NOW,
        )
        .unwrap();
    assert_eq!(
        g.objects
            .iter()
            .filter(|o| o.id.object_type == "measurement" && o.id.namespace != c.contract.namespace)
            .count(),
        1
    );
    let aggregate = g
        .objects
        .iter()
        .find(|o| o.id.object_type == "measurement" && o.id.native_key.ends_with(SELECTORS[0]))
        .unwrap();
    assert_eq!(aggregate.observations[0].coverage, Coverage::Partial);
    assert_eq!(
        aggregate.observations[0].fields["value"],
        Knowledge::Unknown
    );
    let f = fixture();
    let c = f.capture().unwrap();
    for (now, qualified) in [(NOW + 899, true), (NOW + 900, false)] {
        let g = c
            .selected_graph(
                ReportWindow {
                    start: 0,
                    end: u64::MAX,
                },
                now,
            )
            .unwrap();
        let d = g
            .objects
            .iter()
            .find(|o| o.id.native_key.ends_with(":disk"))
            .unwrap();
        assert_eq!(
            d.observations[0].fields["source_qualified"],
            Knowledge::Value(qualified.into())
        );
    }
}

#[test]
fn complete_capture_cross_checks_all_pr5_units_cutoffs_baselines_and_fixed_incidents() {
    let mut f = fixture();
    let mut request: Value =
        serde_json::from_slice(&f.evidence["synthetic-test"]["request.json"]).unwrap();
    for kind in ["billing", "parity", "recall", "defects"] {
        request[format!("{kind}_recording_path")] = json!(format!("/synthetic/{kind}.json"));
    }
    f.replace_artifact("request.json", request);
    for (field, kind, outcome, n, total) in [
        (
            "guard_ci_minutes_month",
            "billing",
            json!({"kind":"billing","account":"synthetic-account","month":{"year":2026,"month":11},"period_start":1793491200_u64,"period_end":1796083200_u64,"cutoff":NOW-1,"settlement_lag_seconds":0,"population_complete":true,"pages":[{"number":1,"has_more":false,"items":[{"id":"usage-1","sku":"synthetic-linux","unit":"linux_minutes","quantity":12}]}]}),
            12,
            None,
        ),
        (
            "guard_recall_regression",
            "recall",
            json!({"kind":"recall","observed_at":NOW-1,"evaluation_set_sha256":"a".repeat(64),"result_revision":"synthetic-executed-result","supported":16,"total":33}),
            16,
            Some(33),
        ),
        (
            "guard_escaped_defects_week",
            "defects",
            json!({"kind":"defects","week_start":1793577600_u64,"week_end":1794182400_u64,"cutoff":NOW-1,"population_complete":true,"incidents":[{"id":"escaped-1","escaped_at":NOW-10,"fixed_at":NOW-2}]}),
            1,
            None,
        ),
    ] {
        let name = format!("{kind}-recording.json");
        f.replace_artifact(&name,json!({"source_revision":"synthetic-executed/v2","capture_start":NOW-1,"capture_end":NOW-1,"outcome":outcome}));
        let a = f.row[field]["recording"].clone();
        let g = &mut f.row[field];
        g["value"] = known(json!(n));
        g["total"] = total.map_or_else(unknown, |n| known(json!(n)));
        g["source_qualified"] = json!(true);
        g["qualification"] = json!("qualified");
        g["source"] = source("captured", Some(NOW - 1), a, NOW - 1, NOW);
    }
    let parity = json!({"source_revision":"synthetic-executed-tool-status/v2","observed_at":NOW-1,"population_complete":true,"tools":[{"id":"synthetic-tool","bounded_implemented":true,"production_qualified":false,"parity":unknown(),"synthetic_tests":unknown(),"independent_review":unknown(),"ci":unknown(),"rollback":unknown()}]});
    let b = bytes(&parity);
    let a = value(&ArtifactRef::new("parity-recording.json", &b)).unwrap();
    f.evidence
        .get_mut("synthetic-test")
        .unwrap()
        .insert("parity-recording.json".into(), b);
    f.row["artifacts"].as_array_mut().unwrap().push(a.clone());
    f.row["artifacts"]
        .as_array_mut()
        .unwrap()
        .sort_by_key(|a| a["path"].as_str().unwrap().to_string());
    f.row["kpi_mcp_parity"] = json!({"value":known(json!({"qualified":0,"total":1,"bounded_implemented":known(json!(1)),"unit":"fraction","tool_set_sha256":path_hash(&BTreeSet::from(["synthetic-tool".into()])),"result_revision":"synthetic-executed-tool-status/v2"})),"source":source("captured",Some(NOW-1),known(a),NOW-1,NOW),"source_qualified":true,"qualification":"qualified"});
    f.row["capture_status"] = json!("complete");
    let c = f.capture().unwrap();
    let g = c
        .selected_graph(
            ReportWindow {
                start: 0,
                end: u64::MAX,
            },
            NOW,
        )
        .unwrap();
    let fields = |kind: &str| {
        &g.objects
            .iter()
            .find(|o| {
                o.id.object_type == "guardrail" && o.id.native_key.ends_with(&format!(":{kind}"))
            })
            .unwrap()
            .observations[0]
            .fields
    };
    assert_eq!(fields("billing")["value"], Knowledge::Value(12.into()));
    assert_eq!(
        fields("billing")["recorded_value"],
        Knowledge::Value(12.into())
    );
    assert_eq!(
        fields("billing")["settlement_lag_seconds"],
        Knowledge::Value(0.into())
    );
    assert_eq!(fields("recall")["value"], Knowledge::Value(16.into()));
    assert_eq!(fields("recall")["total"], Knowledge::Value(33.into()));
    assert_eq!(
        fields("recall")["bound_satisfied"],
        Knowledge::Value(true.into())
    );
    assert_eq!(fields("defects")["value"], Knowledge::Value(1.into()));
    assert_eq!(fields("defects")["fixed_count"], Knowledge::Value(1.into()));
    assert_eq!(
        fields("defects")["bound_satisfied"],
        Knowledge::Value(false.into())
    );
}

#[test]
fn knowledge_unit_variants_reject_even_null_content_fields() {
    for pointer in [
        "/kpi_mcp_parity/value",
        "/repositories/0/ref_read/signal",
        "/guard_recall_regression/total",
    ] {
        let mut f = fixture();
        f.row
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("value".into(), Value::Null);
        assert!(
            f.capture().is_err(),
            "unit wrapper must contain only kind: {pointer}"
        );
    }
}

pub(crate) fn failed_repository_fixture() -> Fixture {
    let mut f = fixture();
    let r = &mut f.row["repositories"][1];
    r["source_qualified"] = json!(false);
    r["qualification"] = json!("failed_source");
    r["removed"] = unknown();
    r["tree_read"]["status"] = json!("failed");
    r["tree_read"]["reason"] = json!("github_exit_nonzero");
    r["tree_read"]["exit_code"] = known(json!(1));
    r["tree_read"]["observed_at"] = unknown();
    r["tree_read"]["artifact"] = unknown();
    r["membership"] = unknown();
    r["leaf_path_set_sha256"] = unknown();
    r["absent_path_set_sha256"] = unknown();
    for name in ["repo-1-tree.json", "repo-1-membership.json"] {
        f.evidence.get_mut("synthetic-test").unwrap().remove(name);
        f.row["artifacts"]
            .as_array_mut()
            .unwrap()
            .retain(|a| a["path"] != name);
    }
    let aggregate = &mut f.row["kpi_legacy_removal"];
    aggregate["value"] = unknown();
    aggregate["source_qualified"] = json!(false);
    aggregate["qualification"] = json!("incomplete_source");
    aggregate["source"]["status"] = json!("incomplete");
    aggregate["source"]["reason"] = json!("incomplete_repository_population");
    aggregate["source"]["observed_at"] = unknown();
    f
}
fn retain(f: &mut Fixture, name: &str, v: Value) -> Value {
    let b = bytes(&v);
    let a = value(&ArtifactRef::new(name, &b)).unwrap();
    f.evidence
        .get_mut("synthetic-test")
        .unwrap()
        .insert(name.into(), b);
    f.row["artifacts"].as_array_mut().unwrap().push(a.clone());
    f.row["artifacts"]
        .as_array_mut()
        .unwrap()
        .sort_by_key(|a| a["path"].as_str().unwrap().to_string());
    a
}
fn parity_with_gates(production: bool, complete: bool) -> Fixture {
    let mut f = fixture();
    let mut request: Value =
        serde_json::from_slice(&f.evidence["synthetic-test"]["request.json"]).unwrap();
    request["parity_recording_path"] = json!("/synthetic/parity.json");
    f.replace_artifact("request.json", request);
    let mut tool = json!({"id":"synthetic-tool","bounded_implemented":true,"production_qualified":production,"parity":unknown(),"synthetic_tests":unknown(),"independent_review":unknown(),"ci":unknown(),"rollback":unknown()});
    for kind in if complete {
        vec![
            "parity",
            "synthetic_tests",
            "independent_review",
            "ci",
            "rollback",
        ]
    } else {
        vec!["parity"]
    } {
        let a = retain(
            &mut f,
            &format!("gate-{kind}.json"),
            json!({"gate":kind,"candidate_commit":"a".repeat(40),"passed":production,"owner_revision":"synthetic-gate-owner"}),
        );
        tool[kind] = known(a);
    }
    let a = retain(
        &mut f,
        "parity-recording.json",
        json!({"source_revision":"synthetic-parity-read","observed_at":NOW-1,"population_complete":true,"tools":[tool]}),
    );
    f.row["kpi_mcp_parity"] = json!({"value":known(json!({"qualified":u64::from(production),"total":1,"bounded_implemented":known(json!(1)),"unit":"fraction","tool_set_sha256":path_hash(&BTreeSet::from(["synthetic-tool".into()])),"result_revision":"synthetic-parity-read"})),"source":source("captured",Some(NOW-1),known(a),NOW-1,NOW),"source_qualified":true,"qualification":"qualified"});
    f
}
#[test]
fn review_partial_failed_gates_preserve_unqualified_tool_progress() {
    let f = parity_with_gates(false, false);
    assert!(
        f.capture().is_ok(),
        "an unqualified tool may retain a failed or partial gate as diagnostic evidence"
    );
    assert!(parity_with_gates(true, true).capture().is_ok());
    assert!(
        parity_with_gates(true, false).capture().is_err(),
        "qualification still requires all five passing gates"
    );
}
#[test]
fn review_absent_parity_rejects_cross_state_contradictions() {
    let mut f = fixture();
    f.row["kpi_mcp_parity"]["source"]["status"] = json!("failed");
    f.row["kpi_mcp_parity"]["source"]["reason"] = json!("failed_source");
    assert!(
        f.capture().is_err(),
        "failed source cannot become NotPerformed"
    );
    let mut f = fixture();
    let mut request: Value =
        serde_json::from_slice(&f.evidence["synthetic-test"]["request.json"]).unwrap();
    request["parity_recording_path"] = json!("/synthetic/parity.json");
    f.replace_artifact("request.json", request);
    f.row["kpi_mcp_parity"]["source"]["status"] = json!("captured");
    f.row["kpi_mcp_parity"]["source"]["reason"] = json!("none");
    f.row["kpi_mcp_parity"]["qualification"] = json!("qualified");
    f.row["kpi_mcp_parity"]["value"] = unknown();
    assert!(
        f.capture().is_err(),
        "captured/qualified parity requires evidence"
    );
}
