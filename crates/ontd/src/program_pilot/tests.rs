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
    assert_eq!(bytes.len(), 203958);
    assert_eq!(
        digest(&bytes),
        "133ee7f01653b740a7376a68fda09ee2eb1101838a7e09fb3b500d0134096bc2"
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
