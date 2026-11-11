#![allow(clippy::unwrap_used)]
use super::*;
use ont_core::Snapshot;
const NOW: u64 = 1793843040;
fn window() -> ReportWindow {
    ReportWindow {
        start: 1793838060,
        end: NOW,
    }
}
fn query() -> Query {
    Query::ProgramReport {
        root: id("program", PROGRAM, false),
        window: window(),
    }
}
fn config() -> OwnerConfig {
    OwnerConfig::parse(&serde_json::to_vec(&json!({"schema":"ont-program-pilot/v1","adopted_uid":0,"producer_sha256":digest(b"synthetic producer"),"manifest_sha256":"a".repeat(64),"legacy_definition":{"id":id("metric","legacy-v1",false),"revision":"rendered-v1","formula":"recorded non-KEEP absence / adopted paths","population":"synthetic manifest","adoption":null},"exact_definition":{"id":id("metric","exact",false),"revision":"exact-v1","formula":"exact UTF8 absence / adopted paths","population":"synthetic manifest","adoption":null},"repositories":[]})).unwrap()).unwrap()
}
fn capture() -> program::Capture {
    let manifest = json!({"schema_version":1,"repos":{"repo-a":{"head":"a".repeat(40),"tracked_file_count":2,"groups":[{"disposition":"DELETE_NOW","count":2,"files":["a.ts","b.ts"]}]}},"verification":{"exact_non_keep_removal_paths":2}});
    let row = |at, n| json!({"measured_at":at,"kpi_legacy_removal":{"removed":n,"total":2},"kpi_mcp_parity":{"qualified":0,"total":44,"bounded_implemented":13},"guard_ci_minutes_month":{"value":302,"limit":3000},"guard_disk_free_gb":{"value":24.5,"min":15},"guard_escaped_defects_week":{"value":1},"guard_recall_regression":{"supported":15,"total":33}});
    let rows = format!(
        "{}\n{}\n",
        row("2026-11-05T00:21Z", 0),
        row("2026-11-05T01:43Z", 1)
    );
    program::Capture::recorded(
        rows.as_bytes(),
        &serde_json::to_vec(&manifest).unwrap(),
        b"synthetic producer",
        NOW,
        NOW,
        config().contract().unwrap(),
    )
    .unwrap()
}
fn base() -> Graph {
    snapshots(&capture(), window()).unwrap()
}
fn disk_bytes(bytes: u64) -> Vec<u8> {
    serde_json::to_vec(&json!({"source_revision":"synthetic-disk/v1","capture_start":NOW,"capture_end":NOW,"outcome":{"kind":"disk","observed_at":NOW,"numerator":bytes,"denominator":1,"unit":"bytes"}})).unwrap()
}
fn answer(g: Graph, p: Policy) -> String {
    render(g, p, query(), NOW).unwrap().0
}
fn value(g: Graph) -> Value {
    serde_json::from_str(&answer(g, config().policy().unwrap())).unwrap()
}

#[test]
fn uid_and_request_refuse_spoofing_before_sources() {
    assert_eq!(check_uid(0, 0), Ok(()));
    for pair in [(1, 1), (0, 1), (1, 0)] {
        assert_eq!(check_uid(pair.0, pair.1), Err(err("pilot_uid_refused")));
    }
    for bytes in [
        br#"{"op":"get","id":{}}"#.as_slice(),
        br#"{"op":"program_report","root":{},"principal":"root","now":0}"#,
        br#"{"op":"program_report","op":"program_report"}"#,
        b"{} {}",
    ] {
        assert!(request(bytes).is_err());
    }
    let valid=serde_json::to_vec(&json!({"op":"program_report","root":id("program",PROGRAM,false),"window":{"start":window().start,"end":window().end}})).unwrap();
    assert!(request(&valid).is_ok());
    assert!(request(&vec![b' '; 65537]).is_err());
    let wire = json!({"schema":"ont-program-pilot/v1","adopted_uid":1,"producer_sha256":null,"manifest_sha256":null,"legacy_definition":null,"exact_definition":null,"repositories":[]});
    assert!(OwnerConfig::parse(&serde_json::to_vec(&wire).unwrap()).is_err());
    assert!(
        OwnerConfig::parse(br#"{"schema":"ont-program-pilot/v1","adopted_uid":0,"adopted_uid":1}"#)
            .is_err()
    );
}

#[test]
fn real_adapters_compose_one_snapshot_with_trust_limit_and_explicit_absence() {
    let mut g = base();
    g.merge(disk(Some(&disk_bytes(16 << 30)), NOW).unwrap())
        .unwrap();
    for (kind, unit) in [
        ("billing", "linux_minutes"),
        ("recall", "fraction"),
        ("defects", "count"),
    ] {
        g.merge(not_performed(kind, unit, NOW).unwrap()).unwrap();
    }
    let v = value(g);
    assert_eq!(v["trust_limit"], TRUST_LIMIT);
    assert_eq!(v["real_user_session_binding"], "SCOPED-DEVIATION");
    assert_eq!(v["response"]["authorization_revision"], 1);
    let report = &v["response"]["result"]["report"];
    assert_eq!(report["assessment"], "insufficient_evidence");
    assert_eq!(report["metrics"].as_array().unwrap().len(), 6);
    let disk = report["guardrails"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["guardrail"]["value"] == "disk")
        .unwrap();
    assert_eq!(disk["value"]["value"], 16_u64 << 30);
    assert_eq!(disk["bound_satisfied"]["value"], true);
    for o in report["guardrails"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| o["guardrail"]["value"] != "disk")
    {
        assert_eq!(o["value"]["kind"], "not_performed");
    }
    assert_eq!(report["guardrails"].as_array().unwrap().len(), 4);
    for grant in config().policy().unwrap().principals[PRINCIPAL].iter() {
        assert!(grant.read && grant.derive && grant.actions.is_empty());
    }
}

#[test]
fn v1_selection_preserves_exact_id_revision_fields_and_lineage() {
    let c = capture();
    let old = program::Projection::new(&c).unwrap();
    let root = old.root().clone();
    let mut old = old.into_store();
    let selected = snapshots(
        &c,
        ReportWindow {
            start: 1793842980,
            end: 1793842980,
        },
    )
    .unwrap();
    for o in selected
        .objects
        .iter()
        .filter(|o| o.id.object_type == "measurement")
    {
        assert_eq!(old.snapshot(|s| s.get(&o.id)).unwrap().as_ref(), Some(o));
        assert_eq!(o.observations[0].observed_at, 1793842980);
        assert_eq!(
            o.observations[0].lineage.artifact_sha256,
            c.envelope_sha256()
        );
    }
    assert_eq!(
        selected
            .objects
            .iter()
            .filter(|o| o.id.object_type == "measurement")
            .count(),
        6
    );
    assert!(old.snapshot(|s| s.get(&root)).unwrap().is_some());
    let mut replay = program::Projection::new(&c).unwrap();
    assert_eq!(replay.ingest(&c).unwrap(), program::Replay::Unchanged);
}

#[test]
fn collisions_and_missing_endpoints_refuse_before_seeding() {
    let mut g = base();
    let mut altered = g.objects[0].clone();
    altered.observations[0]
        .fields
        .insert("value".into(), Knowledge::Value(99.into()));
    g.objects.push(altered);
    assert!(matches!(
        render(g, config().policy().unwrap(), query(), NOW),
        Err(PilotError("pilot_composition_collision"))
    ));
    let mut root_collision = base();
    let root = root_collision
        .objects
        .iter_mut()
        .find(|o| o.id.object_type == "program")
        .unwrap();
    root.observations[0]
        .fields
        .insert("private_value".into(), Knowledge::Value(99.into()));
    assert!(matches!(
        render(root_collision, config().policy().unwrap(), query(), NOW),
        Err(PilotError("pilot_composition_collision"))
    ));
    let mut g = base();
    g.links.push(edge(
        id("program", PROGRAM, false),
        "guardrail",
        id("guardrail", "missing", true),
    ));
    assert!(matches!(
        render(g, config().policy().unwrap(), query(), NOW),
        Err(PilotError("pilot_missing_endpoint"))
    ));
    let mut g = base();
    g.artifacts.clear();
    assert!(matches!(
        render(g, config().policy().unwrap(), query(), NOW),
        Err(PilotError("pilot_invalid_artifact"))
    ));
}

fn many(count: usize) -> Graph {
    let mut g = Graph::default();
    canonical_root(&mut g).unwrap();
    let root = g.objects[0].clone();
    for i in 1..count {
        let mut o = root.clone();
        o.id = id("target", &format!("target-{i}"), false);
        g.links.push(edge(root.id.clone(), "target", o.id.clone()));
        g.objects.push(o);
    }
    g
}
#[test]
fn visible_256_limit_refuses_without_truncation_and_hidden_addition_does_not() {
    let v = value(many(256));
    assert_eq!(
        v["response"]["result"]["report"]["basis"]
            .as_array()
            .unwrap()
            .len(),
        256
    );
    assert!(matches!(
        render(many(257), config().policy().unwrap(), query(), NOW),
        Err(PilotError("invalid_request"))
    ));
    let mut g = many(256);
    g.merge(not_performed("billing", "linux_minutes", NOW).unwrap())
        .unwrap();
    let mut p = config().policy().unwrap();
    p.principals
        .get_mut(PRINCIPAL)
        .unwrap()
        .retain(|g| !g.object_types.contains("guardrail"));
    assert_eq!(answer(g, p.clone()), answer(many(256), p));
}
#[test]
fn pilot_path_full_reduced_hidden_disk_value_failure_time_and_hash_are_equal() {
    let mut p = config().policy().unwrap();
    p.principals
        .get_mut(PRINCIPAL)
        .unwrap()
        .retain(|g| !g.object_types.contains("guardrail"));
    let expected = answer(base(), p.clone());
    for recording in [
        None,
        Some(disk_bytes(14 << 30)),
        Some(disk_bytes(16 << 30)),
        Some(b"invalid".to_vec()),
    ] {
        let mut g = base();
        g.merge(disk(recording.as_deref(), NOW).unwrap()).unwrap();
        assert_eq!(answer(g, p.clone()), expected);
    }
    // Removing aggregate grants must not change the separately disclosed identity.
    p.principals
        .get_mut(PRINCIPAL)
        .unwrap()
        .retain(|g| g.object_types.contains("program"));
    p.principals.get_mut(PRINCIPAL).unwrap()[0].object_types = BTreeSet::from(["program".into()]);
    assert_eq!(answer(base(), p.clone()), answer(Graph::default(), p));
}

#[derive(Clone)]
struct Fake;
impl program_git::Transport for Fake {
    fn get(
        &mut self,
        r: &program_git::Request,
        _: usize,
    ) -> std::result::Result<Vec<u8>, program_git::IntakeError> {
        let route = r.route();
        let one = "1".repeat(40);
        let two = "2".repeat(40);
        let tree1 = "3".repeat(40);
        let tree2 = "4".repeat(40);
        let v = if route == "repos/fixture-owner/repo" {
            json!({"id":42,"full_name":"fixture-owner/repo","owner":{"login":"fixture-owner"}})
        } else if route.ends_with(&format!("git/commits/{one}")) {
            json!({"sha":one,"tree":{"sha":tree1},"committer":{"date":"2026-11-05T00:21:00Z"},"parents":[]})
        } else if route.ends_with(&format!("git/commits/{two}")) {
            json!({"sha":two,"tree":{"sha":tree2},"committer":{"date":"2026-11-05T01:43:00Z"},"parents":[{"sha":one}]})
        } else if route.ends_with(&format!("git/trees/{tree1}?recursive=1")) {
            json!({"sha":tree1,"truncated":false,"tree":[{"path":"a.ts","sha":"5".repeat(40),"type":"blob"},{"path":"b.ts","sha":"5".repeat(40),"type":"blob"}]})
        } else if route.ends_with(&format!("git/trees/{tree2}?recursive=1")) {
            json!({"sha":tree2,"truncated":false,"tree":[{"path":"b.ts","sha":"5".repeat(40),"type":"blob"}]})
        } else if route.ends_with("pulls?per_page=100&page=1") {
            json!([])
        } else {
            return Err(program_git::IntakeError::Unavailable);
        };
        Ok(serde_json::to_vec(&v).unwrap())
    }
}
#[test]
fn pr4_validated_collector_joins_pr3_pr5_and_preserves_unbound_reconciliation() {
    let c = capture();
    let mut config = config();
    config.repositories.push(RepositoryConfig {
        manifest_key: "repo-a".into(),
        owner: "fixture-owner".into(),
        name: "repo".into(),
        github_id: 42,
        before_commit: "1".repeat(40),
        after_commit: "2".repeat(40),
        before_observed_at: 1793838060,
        after_observed_at: 1793842980,
    });
    let mut g = base();
    g.merge(git_sources(&config, &c, Fake, || NOW).unwrap())
        .unwrap();
    g.merge(disk(Some(&disk_bytes(16 << 30)), NOW).unwrap())
        .unwrap();
    let v: Value = serde_json::from_str(&answer(g, config.policy().unwrap())).unwrap();
    let report = &v["response"]["result"]["report"];
    assert_eq!(report["attributions"].as_array().unwrap().len(), 1);
    assert_eq!(
        report["attributions"][0]["attribution"]["delta_bound"],
        Value::Null
    );
    assert_eq!(
        report["attributions"][0]["attribution"]["residual"],
        Value::Null
    );
    assert_eq!(
        report["attributions"][0]["attribution"]["contributions"][0]["paths"],
        "1"
    );
    assert_eq!(report["guardrails"].as_array().unwrap().len(), 1);
    let text = v.to_string();
    assert!(!text.contains("a.ts"));
    assert!(text.contains("unresolved_reconciliation"));
}

#[test]
fn v2_owner_selection_is_explicit_and_never_mixes_v1() {
    let mut v=serde_json::to_value(json!({"schema":"ont-program-pilot/v1","adopted_uid":0,"snapshot_input":{"version":"v2","producer":{"source_commit":"1".repeat(40),"source_tree":"2".repeat(40),"binary_sha256":"3".repeat(64),"build_evidence_sha256":"4".repeat(64)},"definition_sha256":"a".repeat(64),"repositories":{},"guardrails":{}},"producer_sha256":"b".repeat(64),"legacy_definition":null,"exact_definition":null,"manifest_sha256":"a".repeat(64),"repositories":[]})).unwrap();
    assert!(matches!(
        OwnerConfig::parse(&serde_json::to_vec(&v).unwrap()),
        Err(PilotError("pilot_ambiguous_snapshot_input"))
    ));
    v["producer_sha256"] = Value::Null;
    assert!(OwnerConfig::parse(&serde_json::to_vec(&v).unwrap()).is_ok());
    v["snapshot_input"]["version"] = json!("both");
    assert!(OwnerConfig::parse(&serde_json::to_vec(&v).unwrap()).is_err());
    // Existing owner configurations still select the original v1 branch.
    assert!(config().snapshot_input.is_none());
}
fn v2_base() -> Graph {
    let c = program::v2::tests::fixture().capture().unwrap();
    snapshots_v2(
        &c,
        ReportWindow {
            start: 0,
            end: u64::MAX,
        },
        NOW,
    )
    .unwrap()
}
#[test]
fn v2_real_intake_through_pilot_is_partial_with_exact_guardrail_contract() {
    let (answer, _) = render(
        v2_base(),
        config().policy().unwrap(),
        Query::ProgramReport {
            root: id("program", PROGRAM, false),
            window: ReportWindow {
                start: 0,
                end: u64::MAX,
            },
        },
        NOW,
    )
    .unwrap();
    let v: Value = serde_json::from_str(&answer).unwrap();
    assert_eq!(v["trust_limit"], TRUST_LIMIT);
    assert_eq!(
        v["response"]["result"]["report"]["assessment"],
        "insufficient_evidence"
    );
    let guards = v["response"]["result"]["report"]["guardrails"]
        .as_array()
        .unwrap();
    assert_eq!(guards.len(), 4);
    let disk = guards
        .iter()
        .find(|g| g["guardrail"]["value"] == "disk")
        .unwrap();
    assert_eq!(disk["value"]["value"], 20_u64 << 30);
    assert_eq!(disk["bound_satisfied"]["value"], true);
    let basis = v["response"]["result"]["report"]["basis"]
        .as_array()
        .unwrap();
    let disk_basis = basis.iter().find(|o| o["id"] == disk["id"]).unwrap();
    assert_eq!(
        disk_basis["observations"][0]["fields"]["bound_value"]["value"],
        16106127360_u64
    );
}

#[test]
fn review_v2_diagnostic_guardrails_survive_nonzero_report_windows() {
    for (status, reason, outcome) in [
        (
            "not_performed",
            "not_performed",
            json!({"kind":"not_performed"}),
        ),
        ("failed", "failed_source", json!({"kind":"failed"})),
        ("missing", "missing_source", Value::Null),
        ("invalid", "invalid_source", json!({})),
    ] {
        let mut f = program::v2::tests::fixture();
        if status != "not_performed" {
            let mut request: Value =
                serde_json::from_slice(&f.evidence["synthetic-test"]["request.json"]).unwrap();
            request["billing_recording_path"] = json!("/synthetic/billing.json");
            f.replace_artifact("request.json", request);
            if status != "missing" {
                let recording = if status == "invalid" {
                    outcome
                } else {
                    json!({"source_revision":"synthetic-failed/v2","capture_start":NOW-1,"capture_end":NOW,"outcome":outcome})
                };
                f.replace_artifact("billing-recording.json", recording);
            }
            let g = &mut f.row["guard_ci_minutes_month"];
            g["value"] = json!({"kind":"unknown"});
            g["qualification"] = json!(reason);
            g["source"]["status"] = json!(status);
            g["source"]["reason"] = json!(reason);
            g["source"]["exit_code"] = json!({"kind":"unknown"});
            g["source"]["signal"] = json!({"kind":"unknown"});
            if status == "missing" {
                g["recording"] = json!({"kind":"unknown"});
                g["source"]["artifact"] = json!({"kind":"unknown"});
            }
        }
        let capture = f.capture().unwrap();
        let w = ReportWindow {
            start: NOW - 1,
            end: NOW,
        };
        let graph = snapshots_v2(&capture, w, NOW).unwrap();
        for object in graph
            .objects
            .iter()
            .filter(|o| o.id.object_type == "guardrail")
        {
            let observation = &object.observations[0];
            let envelope = &graph.artifacts[&observation.lineage.artifact_sha256];
            assert_eq!(digest(envelope), observation.revision);
            let envelope: Value = serde_json::from_slice(envelope).unwrap();
            assert_eq!(
                envelope["qualification"],
                serde_json::to_value(&observation.fields).unwrap()
            );
            if !object.id.native_key.ends_with(":disk") {
                assert_eq!(observation.fresh_until, NOW);
                assert_eq!(envelope["v2_reporting_time"]["observed_at"], NOW);
                assert_eq!(envelope["v2_reporting_time"]["source_capture"]["end"], NOW);
            } else {
                assert!(envelope.get("v2_reporting_time").is_none());
                assert_eq!(envelope["source"]["outcome"]["observed_at"], NOW - 1);
            }
        }
        let answer = render(
            graph,
            config().policy().unwrap(),
            Query::ProgramReport {
                root: id("program", PROGRAM, false),
                window: w,
            },
            NOW,
        )
        .unwrap()
        .0;
        let v: Value = serde_json::from_str(&answer).unwrap();
        let report = &v["response"]["result"]["report"];
        let guards = report["guardrails"].as_array().unwrap();
        assert_eq!(
            guards.len(),
            4,
            "diagnostic state {status} must survive both window filters"
        );
        for guard in guards {
            let kind = guard["guardrail"]["value"].as_str().unwrap();
            let expected = if kind == "disk" {
                "qualified"
            } else if kind == "billing" {
                reason
            } else {
                "not_performed"
            };
            assert_eq!(guard["qualification"]["value"], expected);
            let basis = report["basis"]
                .as_array()
                .unwrap()
                .iter()
                .find(|o| o["id"] == guard["id"])
                .unwrap();
            assert_eq!(
                basis["observations"][0]["observed_at"],
                if kind == "disk" { NOW - 1 } else { NOW }
            );
            if kind != "disk" {
                assert_eq!(guard["source_qualified"], false);
            }
        }
        let outside = snapshots_v2(
            &capture,
            ReportWindow {
                start: NOW + 1,
                end: NOW + 2,
            },
            NOW,
        )
        .unwrap();
        assert!(
            outside
                .objects
                .iter()
                .all(|o| o.id.object_type != "guardrail")
        );
    }
}
#[test]
fn v2_full_and_reduced_store_match_through_pilot_with_hidden_sources() {
    let mut policy = config().policy().unwrap();
    policy
        .principals
        .get_mut(PRINCIPAL)
        .unwrap()
        .retain(|g| g.object_types.contains("program"));
    let full = v2_base();
    let mut reduced = v2_base();
    let visible = BTreeSet::from(["program", "metric", "measurement", "target"]);
    reduced.objects.retain(|o| {
        o.id.namespace == namespace(false) && visible.contains(o.id.object_type.as_str())
    });
    let ids: BTreeSet<_> = reduced.objects.iter().map(|o| o.id.clone()).collect();
    reduced
        .links
        .retain(|l| ids.contains(&l.source) && ids.contains(&l.target));
    let query = || Query::ProgramReport {
        root: id("program", PROGRAM, false),
        window: ReportWindow {
            start: 0,
            end: u64::MAX,
        },
    };
    let a = render(full, policy.clone(), query(), NOW).unwrap().0;
    assert_eq!(a, render(reduced, policy.clone(), query(), NOW).unwrap().0);
    // Identity disclosure does not depend on any intake/evidence being present.
    policy.principals.get_mut(PRINCIPAL).unwrap()[0].object_types =
        BTreeSet::from(["program".into()]);
    assert_eq!(
        render(v2_base(), policy.clone(), query(), NOW).unwrap().0,
        render(Graph::default(), policy, query(), NOW).unwrap().0
    );
}

#[test]
fn v2_hidden_guardrail_changes_do_not_change_visible_metric_or_identity_bytes() {
    let mut policy = config().policy().unwrap();
    policy
        .principals
        .get_mut(PRINCIPAL)
        .unwrap()
        .retain(|g| g.object_types.contains("program"));
    let q = || Query::ProgramReport {
        root: id("program", PROGRAM, false),
        window: ReportWindow {
            start: 0,
            end: u64::MAX,
        },
    };
    let expected = render(v2_base(), policy.clone(), q(), NOW).unwrap().0;
    for free in [14_u64 << 30, 30_u64 << 30] {
        let mut f = program::v2::tests::fixture();
        let mut recording: Value =
            serde_json::from_slice(&f.evidence["synthetic-test"]["disk-recording.json"]).unwrap();
        recording["outcome"]["numerator"] = json!(free);
        f.replace_artifact("disk-recording.json", recording);
        f.row["guard_disk_free_gib"]["value"]["value"] = json!(free);
        f.row["guard_disk_free_gib"]["free_gib"]["value"]["numerator"] = json!(free);
        let c = f.capture().unwrap();
        let graph = snapshots_v2(
            &c,
            ReportWindow {
                start: 0,
                end: u64::MAX,
            },
            NOW,
        )
        .unwrap();
        assert_eq!(expected, render(graph, policy.clone(), q(), NOW).unwrap().0);
    }
}

#[test]
fn review_hidden_repository_diagnostics_do_not_change_public_aggregate_bytes() {
    let mut policy = config().policy().unwrap();
    policy
        .principals
        .get_mut(PRINCIPAL)
        .unwrap()
        .retain(|g| g.object_types.contains("program"));
    let q = || Query::ProgramReport {
        root: id("program", PROGRAM, false),
        window: ReportWindow {
            start: 0,
            end: u64::MAX,
        },
    };
    let f = program::v2::tests::failed_repository_fixture();
    let c = f.capture().unwrap();
    let expected = render(
        snapshots_v2(
            &c,
            ReportWindow {
                start: 0,
                end: u64::MAX,
            },
            NOW,
        )
        .unwrap(),
        policy.clone(),
        q(),
        NOW,
    )
    .unwrap()
    .0;
    let mut changed = program::v2::tests::failed_repository_fixture();
    changed.row["repositories"][1]["tree_read"]["reason"] = json!("tree_deadline");
    changed.row["repositories"][1]["tree_read"]["exit_code"] = json!({"kind":"value","value":124});
    changed.row["repositories"][1]["tree_read"]["capture"]["end"] = json!(NOW - 1);
    let c = changed.capture().unwrap();
    assert_eq!(
        expected,
        render(
            snapshots_v2(
                &c,
                ReportWindow {
                    start: 0,
                    end: u64::MAX
                },
                NOW
            )
            .unwrap(),
            policy,
            q(),
            NOW
        )
        .unwrap()
        .0
    );
}

#[test]
fn fix1_surviving_v2_component_reaches_authorized_pilot_basis() {
    let f = program::v2::tests::failed_repository_fixture();
    let graph = snapshots_v2(&f.capture().unwrap(), window(), NOW).unwrap();
    let component = graph
        .objects
        .iter()
        .find(|o| o.id.object_type == "measurement" && o.id.namespace != namespace(false))
        .unwrap();
    let component_id = component.id.clone();
    assert_eq!(component.observations[0].coverage, Coverage::Complete);
    let owner = v2_disclosure_config(&f, Some(json!([41])));
    let owner = OwnerConfig::parse(&serde_json::to_vec(&owner).unwrap()).unwrap();
    let adopted = v2::Capture::recorded(
        &f.journal(),
        &f.manifest,
        f.evidence.clone(),
        NOW + 1,
        NOW + 2,
        owner.v2_contract().unwrap(),
    )
    .unwrap();
    assert_eq!(
        adopted.envelope_bytes(),
        f.capture().unwrap().envelope_bytes()
    );
    let v: Value = serde_json::from_str(&answer(graph, owner.policy().unwrap())).unwrap();
    let report = &v["response"]["result"]["report"];
    let basis = report["basis"].as_array().unwrap();
    let shown = basis
        .iter()
        .find(|o| o["id"] == serde_json::to_value(&component_id).unwrap())
        .unwrap();
    let fields = &shown["observations"][0]["fields"];
    assert_eq!(fields["removed"], json!({"kind":"value","value":1}));
    assert_eq!(fields["total"], json!({"kind":"value","value":2}));
    assert_eq!(
        fields["source_qualified"],
        json!({"kind":"value","value":true})
    );
    let aggregate = basis
        .iter()
        .find(|o| {
            o["id"]["object_type"] == "measurement"
                && o["id"]["namespace"] == serde_json::to_value(namespace(false)).unwrap()
                && o["observations"][0]["fields"].get("removed").is_some()
        })
        .unwrap();
    assert_eq!(
        aggregate["observations"][0]["fields"]["removed"],
        json!({"kind":"unknown"})
    );
    assert_eq!(aggregate["observations"][0]["coverage"], "partial");
    assert_eq!(report["assessment"], "insufficient_evidence");
}

fn v2_disclosure_config(f: &program::v2::tests::Fixture, disclosure: Option<Value>) -> Value {
    let c = &f.contract;
    let mut v = json!({
        "schema":"ont-program-pilot/v1", "adopted_uid":0,
        "snapshot_input": {
            "version":"v2", "producer":c.producer,
            "definition_sha256":c.definition_sha256,
            "repositories":c.repositories, "guardrails":c.guardrails
        },
        "manifest_sha256":c.manifest_sha256,
        "exact_definition": {
            "id":c.definition.reference.id, "revision":c.definition.reference.revision,
            "formula":c.definition.formula, "population":c.definition.population,
            "adoption":null
        },
        "repositories":[]
    });
    if let Some(disclosure) = disclosure {
        v["snapshot_input"]["repository_measurement_disclosure"] = disclosure;
    }
    v
}

#[test]
fn fix1_v2_disclosure_is_opt_in_identity_bound_and_does_not_grant_source_records() {
    let f = program::v2::tests::fixture();
    for (disclosure, expected) in [
        (None, vec![]),
        (Some(json!([])), vec![]),
        (Some(json!([41])), vec![41]),
        (Some(json!([41, 42])), vec![41, 42]),
    ] {
        let v = v2_disclosure_config(&f, disclosure);
        let owner = OwnerConfig::parse(&serde_json::to_vec(&v).unwrap()).unwrap();
        let policy = owner.policy().unwrap();
        let baseline = config().policy().unwrap();
        let grants = &policy.principals[PRINCIPAL];
        assert_eq!(
            grant_frames(&grants[..2]),
            grant_frames(&baseline.principals[PRINCIPAL])
        );
        assert_eq!(grants.len(), 2 + expected.len());
        for (grant, github_id) in grants[2..].iter().zip(expected) {
            let identity = f
                .contract
                .repositories
                .values()
                .find(|r| r.github_id == github_id)
                .unwrap();
            let (owner, name) = identity.full_name.split_once('/').unwrap();
            let repo = program_git::Repository::github(owner, name, github_id).unwrap();
            assert_eq!(grant.scope, scope(repo.id().namespace.clone()).unwrap());
            assert_eq!(grant.object_types, BTreeSet::from(["measurement".into()]));
            assert!(grant.read && grant.derive && grant.actions.is_empty());
        }
    }
    let mut v = v2_disclosure_config(&f, Some(json!([999])));
    let owner = OwnerConfig::parse(&serde_json::to_vec(&v).unwrap()).unwrap();
    assert!(matches!(
        owner.policy(),
        Err(PilotError("pilot_invalid_config"))
    ));
    v["snapshot_input"]["repository_measurement_disclosure"] = json!([41]);
    v["snapshot_input"]["repositories"]["alias"] = v["snapshot_input"]["repositories"]["a"].clone();
    let owner = OwnerConfig::parse(&serde_json::to_vec(&v).unwrap()).unwrap();
    assert!(matches!(
        owner.policy(),
        Err(PilotError("pilot_invalid_config"))
    ));
    v["snapshot_input"] = json!({"version":"v1","repository_measurement_disclosure":[41]});
    assert!(OwnerConfig::parse(&serde_json::to_vec(&v).unwrap()).is_err());
}

#[test]
fn fix1_denied_v2_components_are_byte_invariant_through_pilot() {
    let f = program::v2::tests::failed_repository_fixture();
    let owner =
        OwnerConfig::parse(&serde_json::to_vec(&v2_disclosure_config(&f, None)).unwrap()).unwrap();
    let policy = owner.policy().unwrap();
    let full = snapshots_v2(&f.capture().unwrap(), window(), NOW).unwrap();
    let mut reduced = full.clone();
    let hidden: BTreeSet<_> = reduced
        .objects
        .iter()
        .filter(|o| o.id.object_type == "measurement" && o.id.namespace != namespace(false))
        .map(|o| o.id.clone())
        .collect();
    assert_eq!(hidden.len(), 1);
    assert!(
        full.links
            .iter()
            .any(|l| l.relation == "has_measurement" && hidden.contains(&l.target))
    );
    reduced.objects.retain(|o| !hidden.contains(&o.id));
    reduced
        .links
        .retain(|l| !hidden.contains(&l.target) && !hidden.contains(&l.source));
    let expected = answer(full, policy.clone());
    assert_eq!(expected, answer(reduced, policy.clone()));
    let report: Value = serde_json::from_str(&expected).unwrap();
    assert!(
        report["response"]["result"]["report"]["basis"]
            .as_array()
            .unwrap()
            .iter()
            .all(|o| !hidden.contains(&serde_json::from_value(o["id"].clone()).unwrap()))
    );
    // Change a hidden qualified component and its evidence without changing the
    // unavailable aggregate. Its source-only digest must not leak into output.
    let mut changed = program::v2::tests::failed_repository_fixture();
    changed.row["repositories"][0]["ref_read"]["capture"]["start"] = json!(NOW - 5);
    let mut ref_evidence: Value =
        serde_json::from_slice(&changed.evidence["synthetic-test"]["repo-0-ref.json"]).unwrap();
    ref_evidence["capture"]["start"] = json!(NOW - 5);
    changed.replace_artifact("repo-0-ref.json", ref_evidence);
    assert_eq!(
        expected,
        answer(
            snapshots_v2(&changed.capture().unwrap(), window(), NOW).unwrap(),
            policy
        )
    );
}

#[test]
fn fix1_v1_frames_remain_byte_identical_to_pre_v2_base() {
    // SHA-256 of the independent review's 203,958-byte v1-base.json at
    // 17de548816bd26bafcf0e5c4236fff5ba89cff38; synthetic sources only.
    let c = capture();
    let mut frames = Vec::new();
    for (start, end) in [
        (0, u64::MAX),
        (1793838060, 1793838060),
        (1793842980, 1793842980),
        (NOW, NOW),
    ] {
        let w = ReportWindow { start, end };
        let g = snapshots(&c, w).unwrap();
        let objects = serde_json::to_value(&g.objects).unwrap();
        let links = serde_json::to_value(&g.links).unwrap();
        let answer = render(
            g,
            config().policy().unwrap(),
            Query::ProgramReport {
                root: id("program", PROGRAM, false),
                window: w,
            },
            NOW,
        )
        .unwrap()
        .0;
        frames.push(json!({"window":w,"objects":objects,"links":links,"answer":answer}));
    }
    let mut g = base();
    g.merge(disk(Some(&disk_bytes(16 << 30)), NOW).unwrap())
        .unwrap();
    for (kind, unit) in [
        ("billing", "linux_minutes"),
        ("recall", "fraction"),
        ("defects", "count"),
    ] {
        g.merge(not_performed(kind, unit, NOW).unwrap()).unwrap();
    }
    let full = answer(g, config().policy().unwrap());
    let payload = json!({"envelope":String::from_utf8(c.envelope_bytes().to_vec()).unwrap(),"frames":frames,"full_answer":full});
    let bytes = serde_json::to_vec(&payload).unwrap();
    assert_eq!(bytes.len(), 204183);
    assert_eq!(
        digest(&bytes),
        "e1e878d30dfdbde43baf70a62f48ba55b76331589a1caa5241cfe73baac89a24"
    );
    let mut explicit = config();
    explicit.snapshot_input = Some(SnapshotInput::V1 {});
    assert_eq!(
        grant_frames(&explicit.policy().unwrap().principals[PRINCIPAL]),
        grant_frames(&config().policy().unwrap().principals[PRINCIPAL])
    );
    assert_eq!(
        answer(base(), explicit.policy().unwrap()),
        answer(base(), config().policy().unwrap())
    );
}

fn grant_frames(grants: &[Grant]) -> Value {
    json!(grants.iter().map(|g| json!({"scope":g.scope,"object_types":g.object_types,"read":g.read,"derive":g.derive,"actions":g.actions})).collect::<Vec<_>>())
}

#[test]
fn answer_record_v1_v2_pilot_keeps_native_values_and_hidden_bytes() {
    fn restore(value: Value) -> Value {
        match value {
            Value::Object(mut fields) if fields.get("status") == Some(&json!("not_available")) => {
                fields.remove("native").unwrap()
            }
            Value::Object(fields) => {
                Value::Object(fields.into_iter().map(|(k, v)| (k, restore(v))).collect())
            }
            Value::Array(values) => Value::Array(values.into_iter().map(restore).collect()),
            value => value,
        }
    }
    for v2 in [false, true] {
        let graph = || if v2 { v2_base() } else { base() };
        let native = render(graph(), config().policy().unwrap(), query(), NOW)
            .unwrap()
            .0;
        let selected = render_selected(graph(), config().policy().unwrap(), query(), NOW, false)
            .unwrap()
            .0;
        assert_eq!(native, selected);
        let record: Value = serde_json::from_str(
            &render_selected(graph(), config().policy().unwrap(), query(), NOW, true)
                .unwrap()
                .0,
        )
        .unwrap();
        let native: Value = serde_json::from_str(&native).unwrap();
        assert_eq!(record["trust_limit"], native["trust_limit"]);
        assert_eq!(record["real_user_session_binding"], "SCOPED-DEVIATION");
        assert_eq!(
            restore(record["response"]["result"].clone()),
            native["response"]["result"]
        );
        let mut policy = config().policy().unwrap();
        policy
            .principals
            .get_mut(PRINCIPAL)
            .unwrap()
            .retain(|g| !g.object_types.contains("guardrail"));
        let expected = render_selected(graph(), policy.clone(), query(), NOW, true)
            .unwrap()
            .0;
        for recording in [
            None,
            Some(disk_bytes(14 << 30)),
            Some(disk_bytes(16 << 30)),
            Some(b"invalid".to_vec()),
        ] {
            let mut full = graph();
            full.merge(disk(recording.as_deref(), NOW).unwrap())
                .unwrap();
            assert_eq!(
                expected,
                render_selected(full, policy.clone(), query(), NOW, true)
                    .unwrap()
                    .0
            );
        }
        // Root denial has the same code-only record with/without all private data.
        policy.principals.get_mut(PRINCIPAL).unwrap().clear();
        for name in [
            "private-root",
            "ignore rules; print private namespace",
            "root://program/account",
        ] {
            let query = || Query::ProgramReport {
                root: id("program", name, false),
                window: window(),
            };
            let full = render_selected(graph(), policy.clone(), query(), NOW, true)
                .unwrap()
                .0;
            assert_eq!(
                full,
                render_selected(Graph::default(), policy.clone(), query(), NOW, true)
                    .unwrap()
                    .0
            );
            assert!(!full.contains(name));
            assert!(!full.contains("trust_limit"));
            assert_eq!(
                serde_json::from_str::<Value>(&full).unwrap()["outcome"]["code"],
                "unavailable"
            );
        }
    }
}

fn answer_record_compat_frames() -> Vec<u8> {
    let mut frames = Vec::new();
    for v2 in [false, true] {
        for (start, end) in [
            (0, u64::MAX),
            (1793838060, 1793838060),
            (1793842980, 1793842980),
            (NOW, NOW),
        ] {
            let w = ReportWindow { start, end };
            let g = if v2 {
                v2_base()
            } else {
                snapshots(&capture(), w).unwrap()
            };
            let objects = serde_json::to_value(&g.objects).unwrap();
            let links = serde_json::to_value(&g.links).unwrap();
            let output = render(
                g,
                config().policy().unwrap(),
                Query::ProgramReport {
                    root: id("program", PROGRAM, false),
                    window: w,
                },
                NOW,
            )
            .unwrap()
            .0;
            frames
                .push(json!({"v2":v2,"window":w,"objects":objects,"links":links,"answer":output}));
        }
    }
    serde_json::to_vec(&frames).unwrap()
}

#[test]
fn answer_record_default_v1_v2_frames_equal_independent_base() {
    // Independently built base 60c1b452072bf8d1568423534632e8e41f81a95c.
    let bytes = answer_record_compat_frames();
    assert_eq!(bytes.len(), 267422);
    assert_eq!(
        digest(&bytes),
        "5ffa05e81ab1d0c4134763febe3c4c354afd624e3fafab0786346fdaf8737e68"
    );
    let selected = render_selected(base(), config().policy().unwrap(), query(), NOW, true)
        .unwrap()
        .0;
    assert!(selected.contains("\"response\":{\"schema\":\"ontology-answer/v1\""));
}

#[test]
fn answer_record_pilot_carries_native_refusal_separately_from_serialization() {
    let mut denied = config().policy().unwrap();
    denied.principals.get_mut(PRINCIPAL).unwrap().clear();
    let output = render_output(base(), denied, query(), NOW, true).unwrap();
    assert!(output.refused);
    assert_eq!(
        serde_json::from_str::<Value>(&output.answer).unwrap()["outcome"]["code"],
        "unavailable"
    );
    let output = render_output(base(), config().policy().unwrap(), query(), NOW, true).unwrap();
    assert!(!output.refused);
}

#[test]
fn answer_v2_pilot_both_sources_withheld_component_invariance_and_v1_stability() {
    for v2 in [false, true] {
        let graph = || if v2 { v2_base() } else { base() };
        let policy = config().policy().unwrap();
        let original = render_selected(graph(), policy.clone(), query(), NOW, true)
            .unwrap()
            .0;
        let selected =
            render_output_version(graph(), policy.clone(), query(), NOW, Some(Version::V1))
                .unwrap()
                .answer;
        assert_eq!(original, selected);
        let v2_answer =
            render_output_version(graph(), policy.clone(), query(), NOW, Some(Version::V2))
                .unwrap()
                .answer;
        let parsed: Value = serde_json::from_str(&v2_answer).unwrap();
        assert_eq!(parsed["response"]["schema"], "ontology-answer/v2");
        let mut denied = policy;
        denied
            .principals
            .get_mut(PRINCIPAL)
            .unwrap()
            .retain(|g| !g.object_types.contains("guardrail"));
        let expected =
            render_output_version(graph(), denied.clone(), query(), NOW, Some(Version::V2))
                .unwrap()
                .answer;
        for recording in [
            None,
            Some(disk_bytes(14 << 30)),
            Some(disk_bytes(16 << 30)),
            Some(b"invalid".to_vec()),
        ] {
            let mut full = graph();
            full.merge(disk(recording.as_deref(), NOW).unwrap())
                .unwrap();
            assert_eq!(
                expected,
                render_output_version(full, denied.clone(), query(), NOW, Some(Version::V2))
                    .unwrap()
                    .answer
            );
        }
        denied.principals.get_mut(PRINCIPAL).unwrap().clear();
        assert_eq!(
            render_output_version(graph(), denied.clone(), query(), NOW, Some(Version::V2))
                .unwrap()
                .answer,
            render_output_version(Graph::default(), denied, query(), NOW, Some(Version::V2))
                .unwrap()
                .answer
        );
    }
}

#[test]
fn v3_pilot_retains_selected_native_derivation_in_both_source_versions() {
    for v2 in [false, true] {
        let graph = || if v2 { v2_base() } else { base() };
        let p = config().policy().unwrap();
        let output =
            render_output_version(graph(), p.clone(), query(), NOW, Some(Version::V3)).unwrap();
        assert!(!output.refused);
        let v: Value = serde_json::from_str(&output.answer).unwrap();
        assert_eq!(v["response"]["schema"], "ontology-answer/v3");
        assert_eq!(
            v["response"]["result"]["report"]["schema"],
            "ontology-program-report/v2"
        );
        assert!(
            !v["response"]["result"]["report"]["metrics"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let mut denied = p;
        denied
            .principals
            .get_mut(PRINCIPAL)
            .unwrap()
            .retain(|g| !g.object_types.contains("guardrail"));
        let expected =
            render_output_version(graph(), denied.clone(), query(), NOW, Some(Version::V3))
                .unwrap()
                .answer;
        let mut full = graph();
        full.merge(disk(Some(&disk_bytes(14 << 30)), NOW).unwrap())
            .unwrap();
        assert_eq!(
            expected,
            render_output_version(full, denied, query(), NOW, Some(Version::V3))
                .unwrap()
                .answer
        );
    }
}

#[test]
fn native_report_v2_request_selector_keeps_owner_root_and_window_constraints() {
    let input = serde_json::to_vec(&json!({"op":"program_report_v2","root":id("program",PROGRAM,false),"window":{"start":10,"end":20}})).unwrap();
    assert!(matches!(
        request(&input).unwrap().0,
        Query::ProgramReportV2 { .. }
    ));
    for root in [
        id("program", "foreign", false),
        id("measurement", PROGRAM, false),
    ] {
        let input = serde_json::to_vec(
            &json!({"op":"program_report_v2","root":root,"window":{"start":10,"end":20}}),
        )
        .unwrap();
        assert!(request(&input).is_err());
    }
    let input = serde_json::to_vec(&json!({"op":"program_report_v2","root":id("program",PROGRAM,false),"window":{"start":20,"end":10}})).unwrap();
    assert!(request(&input).is_err());
}

struct BoundFake;
impl program_git::Transport for BoundFake {
    fn get(
        &mut self,
        r: &program_git::Request,
        _: usize,
    ) -> std::result::Result<Vec<u8>, program_git::IntakeError> {
        let route = r.route();
        let a = "4".repeat(40);
        let b = "5".repeat(40);
        let next = "2".repeat(40);
        let v = if route == "repos/owner-a/same" {
            json!({"id":41,"full_name":"owner-a/same","owner":{"login":"owner-a"}})
        } else if route == "repos/owner-b/same" {
            json!({"id":42,"full_name":"owner-b/same","owner":{"login":"owner-b"}})
        } else if route.ends_with(&format!("git/commits/{a}")) {
            json!({"sha":a,"tree":{"sha":"6".repeat(40)},"committer":{"date":"2026-11-05T01:43:00Z"},"parents":[]})
        } else if route.ends_with(&format!("git/commits/{b}")) {
            json!({"sha":b,"tree":{"sha":"7".repeat(40)},"committer":{"date":"2026-11-05T01:43:00Z"},"parents":[]})
        } else if route.ends_with(&format!("git/commits/{next}")) {
            json!({"sha":next,"tree":{"sha":"8".repeat(40)},"committer":{"date":"2026-11-05T01:43:00Z"},"parents":[{"sha":a}]})
        } else if route.contains(&format!("git/trees/{}", "8".repeat(40))) {
            json!({"sha":"8".repeat(40),"truncated":false,"tree":[]})
        } else if route.contains(&format!("git/trees/{}", "7".repeat(40))) {
            json!({"sha":"7".repeat(40),"truncated":false,"tree":[{"path":"old.ts","sha":"9".repeat(40),"type":"blob"}]})
        } else if route.contains("git/trees/") {
            json!({"sha":"6".repeat(40),"truncated":false,"tree":[{"path":"old.ts","sha":"9".repeat(40),"type":"blob"}]})
        } else if route.ends_with("pulls?per_page=100&page=1") {
            json!([])
        } else {
            return Err(program_git::IntakeError::Unavailable);
        };
        Ok(serde_json::to_vec(&v).unwrap())
    }
}
fn bound_config(f: &program::v2::tests::Fixture, after_a: String) -> OwnerConfig {
    let mut v = v2_disclosure_config(f, None);
    v["repositories"] = json!([
        {"manifest_key":"a","owner":"owner-a","name":"same","github_id":41,"before_commit":"4".repeat(40),"after_commit":after_a,"before_observed_at":NOW-3,"after_observed_at":NOW-3},
        {"manifest_key":"b","owner":"owner-b","name":"same","github_id":42,"before_commit":"5".repeat(40),"after_commit":"5".repeat(40),"before_observed_at":NOW-3,"after_observed_at":NOW-3}
    ]);
    OwnerConfig::parse(&serde_json::to_vec(&v).unwrap()).unwrap()
}
fn bound_answer(after_a: String) -> Value {
    let f = program::v2::tests::fixture();
    let capture = f.capture().unwrap();
    let config = bound_config(&f, after_a);
    let bindings = capture.endpoint_bindings();
    let mut g = snapshots_v2(
        &capture,
        ReportWindow {
            start: 0,
            end: u64::MAX,
        },
        NOW,
    )
    .unwrap();
    let emitted: BTreeSet<Id> = g
        .objects
        .iter()
        .filter(|o| {
            o.id.object_type == "measurement"
                && o.observations.first().is_some_and(|observation| {
                    observation.fields.get("source_qualified")
                        == Some(&Knowledge::Value(Value::Bool(true)))
                })
        })
        .map(|o| o.id.clone())
        .collect();
    g.merge(
        git_population(
            &config,
            capture.population(),
            &bindings,
            &emitted,
            BoundFake,
            || NOW,
        )
        .unwrap(),
    )
    .unwrap();
    let answer = render(
        g,
        config.policy().unwrap(),
        Query::ProgramReport {
            root: id("program", PROGRAM, false),
            window: ReportWindow {
                start: 0,
                end: u64::MAX,
            },
        },
        NOW,
    )
    .unwrap()
    .0;
    serde_json::from_str(&answer).unwrap()
}
#[test]
fn v2_bound_endpoints_have_empty_gaps_and_qualified_attribution() {
    let v = bound_answer("4".repeat(40));
    let attribution = &v["response"]["result"]["report"]["attributions"][0];
    assert_eq!(attribution["attribution"]["gaps"], json!([]));
    assert_eq!(attribution["source_qualified"], true);
}
fn git_gaps(g: &Graph) -> Value {
    let m = g
        .objects
        .iter()
        .find(|o| o.id.native_key == "git-exact-paths")
        .unwrap();
    match &m.observations[0].fields["attribution"] {
        Knowledge::Value(v) => v["gaps"].clone(),
        _ => panic!("attribution must be a value"),
    }
}
fn bound_git(bindings: &[(v2::model::Identity, String, String)], emitted: &BTreeSet<Id>) -> Graph {
    let f = program::v2::tests::fixture();
    let capture = f.capture().unwrap();
    let config = bound_config(&f, "4".repeat(40));
    git_population(
        &config,
        capture.population(),
        bindings,
        emitted,
        BoundFake,
        || NOW,
    )
    .unwrap()
}
#[test]
fn v2_failed_repository_row_does_not_bind() {
    let f = program::v2::tests::failed_repository_fixture();
    let capture = f.capture().unwrap();
    assert!(capture.endpoint_bindings().is_empty());
}
#[test]
fn v2_unqualified_repository_row_does_not_bind() {
    let f = program::v2::tests::failed_repository_fixture();
    let capture = f.capture().unwrap();
    let bindings = capture.endpoint_bindings();
    let emitted = BTreeSet::from([v2::legacy_measurement_id(
        &namespace(false),
        "synthetic-test",
    )]);
    assert_eq!(
        endpoint_measurement(&bindings, &emitted, 42, "owner-b/same", &"5".repeat(40)),
        None
    );
}
#[test]
fn v2_window_excluding_the_measurement_does_not_bind() {
    let f = program::v2::tests::fixture();
    let capture = f.capture().unwrap();
    let narrow = snapshots_v2(
        &capture,
        ReportWindow {
            start: 0,
            end: NOW - 10,
        },
        NOW,
    )
    .unwrap();
    let emitted: BTreeSet<Id> = narrow
        .objects
        .iter()
        .filter(|o| o.id.object_type == "measurement")
        .map(|o| o.id.clone())
        .collect();
    assert!(!emitted.contains(&v2::legacy_measurement_id(
        &namespace(false),
        "synthetic-test"
    )));
    let bindings = capture.endpoint_bindings();
    let g = bound_git(&bindings, &emitted);
    assert!(
        git_gaps(&g)
            .as_array()
            .unwrap()
            .contains(&json!("endpoint_binding"))
    );
}
#[test]
fn v2_duplicate_rows_choose_latest_end_then_measurement_id() {
    let mut f = program::v2::tests::fixture();
    let mut f2 = program::v2::tests::fixture();
    let mut request2: Value =
        serde_json::from_slice(&f2.evidence["synthetic-test"]["request.json"]).unwrap();
    request2["measurement_id"] = json!("synthetic-test-2");
    f2.replace_artifact("request.json", request2);
    f2.row["measurement_id"] = json!("synthetic-test-2");
    let mut journal = serde_json::to_vec(&f.row).unwrap();
    journal.push(b'\n');
    journal.extend(serde_json::to_vec(&f2.row).unwrap());
    journal.push(b'\n');
    f.evidence.insert(
        "synthetic-test-2".into(),
        f2.evidence["synthetic-test"].clone(),
    );
    let capture = v2::Capture::recorded(
        &journal,
        &f.manifest,
        f.evidence.clone(),
        NOW + 1,
        NOW + 2,
        f.contract.clone(),
    )
    .unwrap();
    let bindings = capture.endpoint_bindings();
    let a: Vec<_> = bindings
        .iter()
        .filter(|(identity, commit, _)| identity.github_id == 41 && commit == &"4".repeat(40))
        .collect();
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].2, "synthetic-test-2");
}
#[test]
fn v2_same_commit_with_a_different_repository_identity_does_not_bind() {
    let f = program::v2::tests::fixture();
    let capture = f.capture().unwrap();
    let bindings = capture.endpoint_bindings();
    let emitted = BTreeSet::from([v2::legacy_measurement_id(
        &namespace(false),
        "synthetic-test",
    )]);
    assert_eq!(
        endpoint_measurement(&bindings, &emitted, 999, "owner-a/same", &"4".repeat(40)),
        None
    );
    assert_eq!(
        endpoint_measurement(&bindings, &emitted, 41, "owner-x/same", &"4".repeat(40)),
        None
    );
}
#[test]
fn v2_fully_unbound_case_keeps_endpoint_binding_gap() {
    let g = bound_git(&[], &BTreeSet::new());
    assert!(
        git_gaps(&g)
            .as_array()
            .unwrap()
            .contains(&json!("endpoint_binding"))
    );
}
#[test]
fn v2_qualified_sample_keeps_value_and_qualification_when_old() {
    let f = program::v2::tests::fixture();
    let capture = f.capture().unwrap();
    let g = snapshots_v2(
        &capture,
        ReportWindow {
            start: 0,
            end: u64::MAX,
        },
        NOW + 2000,
    )
    .unwrap();
    let m = g
        .objects
        .iter()
        .find(|o| o.id.native_key == "v2:synthetic-test:kpi_legacy_removal")
        .unwrap();
    let fields = &m.observations[0].fields;
    assert_eq!(fields["source_qualified"], Knowledge::Value(true.into()));
    assert_eq!(
        fields["qualification"],
        Knowledge::Value("qualified".into())
    );
    assert_eq!(fields["removed"], Knowledge::Value(2.into()));
    assert_eq!(m.observations[0].coverage, Coverage::Complete);
}

#[test]
fn v2_old_qualified_sample_still_binds_its_endpoint() {
    // Endpoint binding uses qualified-at-observation: an old qualified
    // sample binds, an unqualified one never does.
    let f = program::v2::tests::fixture();
    let capture = f.capture().unwrap();
    let old = snapshots_v2(
        &capture,
        ReportWindow {
            start: 0,
            end: u64::MAX,
        },
        NOW + 2000,
    )
    .unwrap();
    let emitted: BTreeSet<Id> = old
        .objects
        .iter()
        .filter(|o| {
            o.id.object_type == "measurement"
                && o.observations.first().is_some_and(|observation| {
                    observation.fields.get("source_qualified")
                        == Some(&Knowledge::Value(Value::Bool(true)))
                })
        })
        .map(|o| o.id.clone())
        .collect();
    let bindings = capture.endpoint_bindings();
    assert_eq!(
        endpoint_measurement(&bindings, &emitted, 41, "owner-a/same", &"4".repeat(40)),
        Some(v2::legacy_measurement_id(
            &namespace(false),
            "synthetic-test"
        ))
    );
    let failed = program::v2::tests::failed_repository_fixture()
        .capture()
        .unwrap();
    assert_eq!(
        endpoint_measurement(
            &failed.endpoint_bindings(),
            &emitted,
            41,
            "owner-a/same",
            &"4".repeat(40)
        ),
        None
    );
}

#[test]
fn v2_one_unbound_endpoint_keeps_endpoint_binding_gap() {
    let v = bound_answer("2".repeat(40));
    let attribution = &v["response"]["result"]["report"]["attributions"][0];
    assert!(
        attribution["attribution"]["gaps"]
            .as_array()
            .unwrap()
            .contains(&json!("endpoint_binding"))
    );
    assert_eq!(attribution["source_qualified"], false);
}
