#![allow(clippy::unwrap_used)]
//! Synthetic responses only. No test instantiates the live gh transport.
use ont_core::program::{
    DefinitionRef, PathAlgorithm, PathBasis, PathPopulation, PrAssociation, ReconciliationGap,
};
use ont_core::{
    Authority, Coverage, Grant, Id, Knowledge, Namespace, Policy, Query, Scope, Snapshot,
};
use ontd::program_git::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

fn pin(n: u64) -> String {
    format!("{n:040x}")
}
fn repo() -> Repository {
    Repository::github("fixture-owner", "fixture-repo", 42).unwrap()
}
fn id(kind: &str, key: &str) -> Id {
    Id {
        object_type: kind.into(),
        namespace: Namespace {
            instance: "synthetic://pr4".into(),
            account: "owner".into(),
        },
        native_key: key.into(),
    }
}
fn population(repository: &Repository, paths: &[&str]) -> PathPopulation {
    PathPopulation {
        basis: PathBasis {
            definition: DefinitionRef {
                id: id("kpi", "legacy"),
                revision: "exact-v2".into(),
            },
            manifest_sha256: "a".repeat(64),
            adapter_revision: "manifest-v1".into(),
            algorithm: PathAlgorithm::ExactUtf8PathsV1,
            denominator: paths.len() as u64,
        },
        repositories: BTreeMap::from([(
            repository.id().clone(),
            paths.iter().map(|s| s.to_string()).collect(),
        )]),
    }
}
fn binding(n: u64, bound: bool) -> EndpointBinding {
    EndpointBinding {
        commit: pin(n),
        source_measurement: bound.then(|| id("measurement", &format!("source-{n}"))),
        observed_at: 90,
    }
}
fn spec() -> ProjectionSpec {
    ProjectionSpec {
        program: id("program", "pilot"),
        measurement: id("measurement", "delta"),
        program_key: "pilot".into(),
        now: 100,
    }
}
#[derive(Clone, Default)]
struct Fake {
    responses: BTreeMap<String, Result<Vec<u8>, IntakeError>>,
}
impl Fake {
    fn put(&mut self, route: &str, value: Value) {
        self.responses
            .insert(route.into(), Ok(serde_json::to_vec(&value).unwrap()));
    }
    fn fail(&mut self, route: &str, error: IntakeError) {
        self.responses.insert(route.into(), Err(error));
    }
    fn route(suffix: &str) -> String {
        format!("repos/fixture-owner/fixture-repo/{suffix}")
    }
    fn commit(&mut self, n: u64, parents: &[u64], paths: &[&str]) {
        self.put(&Self::route(&format!("git/commits/{}",pin(n))),json!({"sha":pin(n),"tree":{"sha":pin(100+n)},"committer":{"date":"1970-01-01T00:00:50Z"},"parents":parents.iter().map(|p|json!({"sha":pin(*p)})).collect::<Vec<_>>(),"message":"not retained"}));
        self.put(&Self::route(&format!("git/trees/{}?recursive=1",pin(100+n))),json!({"sha":pin(100+n),"truncated":false,"tree":paths.iter().map(|p|json!({"path":p,"sha":pin(999),"type":"blob"})).collect::<Vec<_>>() }));
        self.pulls(n, json!([]));
    }
    fn pulls(&mut self, n: u64, pulls: Value) {
        self.put(
            &Self::route(&format!("commits/{}/pulls?per_page=100&page=1", pin(n))),
            pulls,
        );
    }
    fn check(&mut self, n: u64) {
        self.put(&Self::route(&format!("commits/{}/check-runs?filter=all&per_page=100&page=1",pin(500+n))),json!({"total_count":1,"check_runs":[{"id":n,"name":"rust","head_sha":pin(500+n),"status":"completed","conclusion":"success","output":{"text":"private body must be discarded"}}]}));
    }
}
impl Transport for Fake {
    fn get(&mut self, request: &Request, _: usize) -> Result<Vec<u8>, IntakeError> {
        // Any unplanned route fails; a PR-head tree fetch would fail this test.
        self.responses
            .get(&request.route())
            .cloned()
            .unwrap_or(Err(IntakeError::Unavailable))
    }
}
fn pull(number: u64, merge: u64) -> Value {
    json!({"number":number,"state":"closed","merged_at":"2026-11-05T00:00:00Z","merge_commit_sha":pin(merge),"head":{"sha":pin(500+number)},"base":{"repo":{"id":42,"full_name":"fixture-owner/fixture-repo"}},"body":"SECRET_MARKER_OMITTED","token":"SECRET_MARKER_OMITTED"})
}
fn ordinary() -> Fake {
    let mut fake = Fake::default();
    fake.put("repos/fixture-owner/fixture-repo",json!({"id":42,"full_name":"fixture-owner/fixture-repo","owner":{"login":"fixture-owner"},"description":"SECRET_MARKER_OMITTED"}));
    fake.commit(1, &[], &["old 한글\n\".rs", "unchanged"]);
    fake.commit(2, &[1, 900], &["unchanged"]);
    fake.pulls(2, json!([pull(7, 2)]));
    fake.check(7);
    fake
}
fn collect(
    fake: Fake,
    first: u64,
    last: u64,
    bound: bool,
    limits: Limits,
) -> Result<CollectedInterval, IntakeError> {
    Collector::new(fake, || 100, limits).unwrap().collect(
        &repo(),
        &population(&repo(), &["old 한글\n\".rs"]).basis,
        binding(first, bound),
        binding(last, bound),
    )
}
fn projection(c: CollectedInterval) -> Projection {
    build_projection(&population(&repo(), &["old 한글\n\".rs"]), &[c], spec()).unwrap()
}

#[test]
fn actual_first_parent_and_accepted_tree_not_pr_head_drive_contribution() {
    let capture = collect(ordinary(), 1, 2, true, Limits::default()).unwrap();
    assert_eq!(capture.interval().changes[0].first_parent.commit, pin(1));
    assert_eq!(capture.interval().changes[0].accepted.commit, pin(2));
    let projected = projection(capture);
    assert_eq!(projected.attribution.delta_bound, Some(1));
    assert_eq!(projected.attribution.residual, Some(0));
    assert!(projected.attribution.has_unique_pr_explanation());
    assert_eq!(
        projected.attribution.contributions[0].association,
        PrAssociation::Unique(ont_core::program::PrId {
            repository: repo().id().clone(),
            number: 7
        })
    );
    let artifact: String = String::from_utf8(projected.artifact).unwrap();
    assert!(!artifact.contains("SECRET_MARKER_OMITTED"));
    assert!(!artifact.contains("private body"));
    assert!(!artifact.contains("old 한글"));
    assert!(artifact.contains("check-runs"));
    assert!(artifact.contains("first_parent"));
}
#[test]
fn restorations_reverts_and_already_applied_changes_keep_signed_counts() {
    let mut f = ordinary();
    f.commit(3, &[2], &["old 한글\n\".rs", "unchanged"]);
    f.commit(4, &[3], &["old 한글\n\".rs", "unchanged"]);
    let c = collect(f, 1, 4, true, Limits::default()).unwrap();
    let p = projection(c);
    assert_eq!(
        p.attribution
            .contributions
            .iter()
            .map(|c| c.paths)
            .collect::<Vec<_>>(),
        vec![1, -1, 0]
    );
    assert_eq!(p.attribution.delta_bound, Some(0));
    assert_eq!(p.attribution.residual, Some(0));
}
#[test]
fn second_parent_reachability_is_not_interval_membership() {
    let mut f = ordinary();
    f.commit(3, &[], &[]);
    f.commit(2, &[3, 1], &[]);
    assert_eq!(
        collect(f, 1, 2, true, Limits::default()).unwrap_err(),
        IntakeError::FirstParentGap
    );
}
#[test]
fn source_tree_failure_never_becomes_empty_complete_tree() {
    for error in [
        IntakeError::Unavailable,
        IntakeError::RateLimited,
        IntakeError::Timeout,
    ] {
        let mut f = ordinary();
        f.fail(
            &Fake::route(&format!("git/trees/{}?recursive=1", pin(102))),
            error,
        );
        assert_eq!(
            collect(f, 1, 2, true, Limits::default()).unwrap_err(),
            error
        );
    }
}
#[test]
fn truncated_wrong_oid_and_malformed_trees_refuse() {
    for tree in [
        json!({"sha":pin(102),"truncated":true,"tree":[]}),
        json!({"sha":pin(2),"truncated":false,"tree":[]}),
        json!({"sha":pin(102),"tree":[]}),
        json!({"sha":pin(102),"truncated":false,"tree":[{"sha":pin(8),"path":"../escape","type":"blob"}]}),
        json!({"sha":pin(102),"truncated":false,"tree":[{"sha":pin(8),"path":"x","type":"blob"},{"sha":pin(8),"path":"x","type":"blob"}]}),
    ] {
        let mut f = ordinary();
        f.put(
            &Fake::route(&format!("git/trees/{}?recursive=1", pin(102))),
            tree,
        );
        assert_eq!(
            collect(f, 1, 2, true, Limits::default()).unwrap_err(),
            IntakeError::InvalidResponse
        );
    }
}
#[test]
fn commit_tree_identity_is_resolved_before_recursive_tree_route() {
    let mut f = ordinary();
    f.put(
        &Fake::route(&format!("git/commits/{}", pin(2))),
        json!({"sha":pin(3),"tree":{"sha":pin(102)},"committer":{"date":"1970-01-01T00:00:50Z"},"parents":[{"sha":pin(1)}]}),
    );
    assert_eq!(
        collect(f, 1, 2, true, Limits::default()).unwrap_err(),
        IntakeError::InvalidResponse
    );
}
#[test]
fn absent_original_snapshot_pins_stay_unbound_after_successful_collection() {
    let p = projection(collect(ordinary(), 1, 2, false, Limits::default()).unwrap());
    assert_eq!(p.attribution.delta_bound, None);
    assert_eq!(p.attribution.residual, None);
    assert_eq!(p.attribution.contributions[0].paths, 1);
    assert!(
        p.attribution
            .gaps
            .contains(&ReconciliationGap::EndpointBinding)
    );
    assert_eq!(
        p.attribution.qualification(),
        ont_core::program::UNRESOLVED_RECONCILIATION
    );
    assert!(
        p.objects
            .iter()
            .filter(|o| matches!(o.id.object_type.as_str(), "program" | "measurement"))
            .all(|o| o.observations[0].coverage == Coverage::Partial)
    );
}
#[test]
fn denied_rate_limited_and_timed_out_pr_searches_are_unknown_not_missing() {
    for error in [
        IntakeError::Unavailable,
        IntakeError::RateLimited,
        IntakeError::Timeout,
    ] {
        let mut f = ordinary();
        f.fail(
            &Fake::route(&format!("commits/{}/pulls?per_page=100&page=1", pin(2))),
            error,
        );
        let p = projection(collect(f, 1, 2, true, Limits::default()).unwrap());
        assert!(matches!(
            p.attribution.contributions[0].association,
            PrAssociation::Unknown { .. }
        ));
        assert!(!p.attribution.has_unique_pr_explanation());
        assert_eq!(p.attribution.delta_bound, Some(1));
    }
}
#[test]
fn complete_empty_pr_search_is_missing_and_no_change_is_invented() {
    let mut f = ordinary();
    f.pulls(2, json!([]));
    let p = projection(collect(f, 1, 2, true, Limits::default()).unwrap());
    assert_eq!(
        p.attribution.contributions[0].association,
        PrAssociation::Missing
    );
    assert!(!p.objects.iter().any(|o| o.id.object_type == "change"));
}
fn hundred() -> Fake {
    let mut f = ordinary();
    f.pulls(2, json!((1..=100).map(|n| pull(n, 2)).collect::<Vec<_>>()));
    for n in 1..=100 {
        f.check(n);
    }
    f
}
#[test]
fn full_page_at_bound_remains_partial_and_counts_merge_once() {
    let p = projection(
        collect(
            hundred(),
            1,
            2,
            true,
            Limits {
                max_pages: 1,
                ..Limits::default()
            },
        )
        .unwrap(),
    );
    assert_eq!(p.attribution.contributions.len(), 1);
    assert_eq!(p.attribution.contributions[0].paths, 1);
    assert!(
        matches!(&p.attribution.contributions[0].association,PrAssociation::Unknown {eligible} if eligible.len()==100)
    );
}
#[test]
fn subsequent_page_failure_retains_eligible_candidates_as_unknown() {
    let mut f = hundred();
    f.fail(
        &Fake::route(&format!("commits/{}/pulls?per_page=100&page=2", pin(2))),
        IntakeError::RateLimited,
    );
    let p = projection(collect(f, 1, 2, true, Limits::default()).unwrap());
    assert!(
        matches!(&p.attribution.contributions[0].association,PrAssociation::Unknown {eligible} if eligible.len()==100)
    );
}
#[test]
fn complete_pagination_preserves_ambiguous_set_without_duplicating_contribution() {
    let mut f = hundred();
    f.put(
        &Fake::route(&format!("commits/{}/pulls?per_page=100&page=2", pin(2))),
        json!([]),
    );
    let p = projection(collect(f, 1, 2, true, Limits::default()).unwrap());
    assert!(
        matches!(&p.attribution.contributions[0].association,PrAssociation::Ambiguous(ids) if ids.len()==100)
    );
    assert_eq!(p.attribution.contributions.len(), 1);
    assert_eq!(p.attribution.residual, Some(0));
}
#[test]
fn duplicate_pr_page_and_foreign_repository_refuse() {
    let mut bad = pull(7, 2);
    bad["base"]["repo"]["full_name"] = json!("foreign/other");
    for pulls in [json!([pull(7, 2), pull(7, 2)]), json!([bad])] {
        let mut f = ordinary();
        f.pulls(2, pulls);
        assert_eq!(
            collect(f, 1, 2, true, Limits::default()).unwrap_err(),
            IntakeError::InvalidResponse
        );
    }
}
#[test]
fn unmerged_or_different_accepted_revision_is_not_eligible() {
    let mut open = pull(7, 2);
    open["state"] = json!("open");
    open["merged_at"] = Value::Null;
    for pr in [open, pull(7, 3)] {
        let mut f = ordinary();
        f.pulls(2, json!([pr]));
        let p = projection(collect(f, 1, 2, true, Limits::default()).unwrap());
        assert_eq!(
            p.attribution.contributions[0].association,
            PrAssociation::Missing
        );
    }
}
#[test]
fn wrong_check_head_inconsistent_total_and_missing_pull_state_refuse() {
    for check in [
        json!({"total_count":1,"check_runs":[{"id":7,"name":"rust","head_sha":pin(2),"status":"completed","conclusion":"success"}]}),
        json!({"total_count":2,"check_runs":[]}),
    ] {
        let mut f = ordinary();
        f.put(
            &Fake::route(&format!(
                "commits/{}/check-runs?filter=all&per_page=100&page=1",
                pin(507)
            )),
            check,
        );
        assert_eq!(
            collect(f, 1, 2, true, Limits::default()).unwrap_err(),
            IntakeError::InvalidResponse
        );
    }
    let mut f = ordinary();
    let mut pr = pull(7, 2);
    pr.as_object_mut().unwrap().remove("merged_at");
    f.pulls(2, json!([pr]));
    assert_eq!(
        collect(f, 1, 2, true, Limits::default()).unwrap_err(),
        IntakeError::InvalidResponse
    );
}
#[test]
fn failed_check_fetch_is_partial_status_without_erasing_merge_fact() {
    let mut f = ordinary();
    f.fail(
        &Fake::route(&format!(
            "commits/{}/check-runs?filter=all&per_page=100&page=1",
            pin(507)
        )),
        IntakeError::Unavailable,
    );
    let p = projection(collect(f, 1, 2, true, Limits::default()).unwrap());
    assert!(p.attribution.has_unique_pr_explanation());
    assert!(
        p.objects
            .iter()
            .filter(|o| matches!(o.id.object_type.as_str(), "program" | "measurement"))
            .all(|o| o.observations[0].coverage == Coverage::Partial)
    );
}
#[test]
fn byte_commit_and_tree_entry_limits_are_enforced_without_zero_filling() {
    assert_eq!(
        collect(
            ordinary(),
            1,
            2,
            true,
            Limits {
                max_bytes: 20,
                ..Limits::default()
            }
        )
        .unwrap_err(),
        IntakeError::LimitExceeded
    );
    assert_eq!(
        collect(
            ordinary(),
            1,
            2,
            true,
            Limits {
                max_tree_entries: 1,
                ..Limits::default()
            }
        )
        .unwrap_err(),
        IntakeError::InvalidResponse
    );
    let mut f = ordinary();
    f.commit(3, &[2], &[]);
    assert_eq!(
        collect(
            f,
            1,
            3,
            true,
            Limits {
                max_commits: 1,
                ..Limits::default()
            }
        )
        .unwrap_err(),
        IntakeError::LimitExceeded
    );
}
#[test]
fn endpoint_future_and_old_status_capture_do_not_gain_current_qualification() {
    let mut c = Collector::new(ordinary(), || 100, Limits::default()).unwrap();
    let mut after = binding(2, true);
    after.observed_at = 101;
    let p = projection(
        c.collect(
            &repo(),
            &population(&repo(), &["old 한글\n\".rs"]).basis,
            binding(1, true),
            after,
        )
        .unwrap(),
    );
    assert!(
        p.attribution
            .gaps
            .contains(&ReconciliationGap::FutureEndpoint)
    );
    let capture = collect(ordinary(), 1, 2, true, Limits::default()).unwrap();
    let mut s = spec();
    s.now = 10000;
    let p = build_projection(&population(&repo(), &["old 한글\n\".rs"]), &[capture], s).unwrap();
    assert_eq!(
        p.objects
            .iter()
            .find(|o| o.id.object_type == "measurement")
            .unwrap()
            .observations[0]
            .fresh_until,
        1000
    );
}
#[test]
fn identical_bound_endpoints_have_zero_interval_and_no_pr_attribution() {
    let p = projection(collect(ordinary(), 1, 1, true, Limits::default()).unwrap());
    assert_eq!(p.attribution.delta_bound, Some(0));
    assert!(p.attribution.contributions.is_empty());
    assert!(!p.attribution.has_unique_pr_explanation());
}
#[test]
fn route_and_clock_inputs_are_bounded_and_cannot_select_arbitrary_urls() {
    for name in ["../escape", "x/y", "x?token=bad", "", ".."] {
        assert_eq!(
            Repository::github("fixture-owner", name, 42).unwrap_err(),
            IntakeError::InvalidConfiguration
        );
    }
    let mut c = Collector::new(ordinary(), || 100, Limits::default()).unwrap();
    let mut before = binding(1, true);
    before.commit = "main".into();
    assert_eq!(
        c.collect(
            &repo(),
            &population(&repo(), &["a"]).basis,
            before,
            binding(2, true)
        )
        .unwrap_err(),
        IntakeError::InvalidConfiguration
    );
    let mut t = 200;
    let mut c = Collector::new(
        ordinary(),
        move || {
            t -= 1;
            t
        },
        Limits::default(),
    )
    .unwrap();
    assert_eq!(
        c.collect(
            &repo(),
            &population(&repo(), &["a"]).basis,
            binding(1, true),
            binding(2, true)
        )
        .unwrap_err(),
        IntakeError::InvalidResponse
    );
}
#[test]
fn projection_uses_v2_store_exact_lineage_and_finite_cardinality() {
    let p = projection(collect(ordinary(), 1, 2, true, Limits::default()).unwrap());
    for o in &p.objects {
        let observation = &o.observations[0];
        let bytes = &p.artifacts[&observation.lineage.artifact_sha256];
        assert_eq!(
            observation.lineage.artifact_sha256,
            format!("{:x}", Sha256::digest(bytes))
        );
        let own_envelope: Value = serde_json::from_slice(bytes).unwrap();
        let r = own_envelope.pointer(&observation.lineage.pointer).unwrap();
        assert_eq!(r["id"], serde_json::to_value(&o.id).unwrap());
        for (field, value) in &observation.fields {
            assert_eq!(value, &Knowledge::Value(r["fields"][field].clone()));
        }
    }
    let mut store = p.memory_store().unwrap();
    store
        .snapshot(|s| {
            for o in &p.objects {
                assert_eq!(s.get(&o.id)?, Some(o.clone()));
            }
            Ok(())
        })
        .unwrap();
    for change in p.objects.iter().filter(|o| o.id.object_type == "change") {
        assert_eq!(
            p.links
                .iter()
                .filter(|l| l.source == change.id && l.relation == "in_repository")
                .count(),
            1
        );
    }
    let ids: BTreeSet<_> = p.objects.iter().map(|o| &o.id).collect();
    assert!(p.links.iter().all(|l| {
        ids.contains(&l.source)
            && ids.contains(&l.target)
            && [
                "has_measurement",
                "repository",
                "in_repository",
                "supported_by",
            ]
            .contains(&l.relation.as_str())
    }));
}
#[test]
fn aggregate_has_separate_grant_and_hidden_repository_does_not_leak() {
    let p = projection(collect(ordinary(), 1, 2, false, Limits::default()).unwrap());
    let store = p.memory_store().unwrap();
    let grant = Grant {
        scope: Scope::Program {
            namespace: repo().id().namespace.clone(),
            program_key: "pilot".into(),
        },
        object_types: BTreeSet::from(["repository".into(), "change".into()]),
        read: true,
        derive: true,
        actions: BTreeSet::new(),
    };
    let authority = Authority::new(Policy {
        principals: BTreeMap::from([("worker".into(), vec![grant])]),
    });
    let session = authority.bind_trusted("worker").unwrap();
    let mut service = ontd::Service::new(store, authority);
    assert_eq!(
        service
            .render(
                &session,
                Query::Get {
                    id: id("measurement", "delta")
                },
                100
            )
            .unwrap_err(),
        ont_core::Error::Unavailable
    );
    let result = service
        .render(
            &session,
            Query::Get {
                id: repo().id().clone(),
            },
            100,
        )
        .unwrap();
    assert!(!result.contains("delta_bound"));
}

#[test]
fn configured_repository_numeric_identity_must_match_slug_and_api_owner() {
    for response in [
        json!({"id":999,"full_name":"fixture-owner/fixture-repo","owner":{"login":"fixture-owner"}}),
        json!({"id":42,"full_name":"fixture-owner/fixture-repo","owner":{"login":"foreign"}}),
    ] {
        let mut f = ordinary();
        f.put("repos/fixture-owner/fixture-repo", response);
        assert_eq!(
            collect(f, 1, 2, true, Limits::default()).unwrap_err(),
            IntakeError::InvalidResponse
        );
    }
    let mut f = ordinary();
    let mut pr = pull(7, 2);
    pr["base"]["repo"]["id"] = json!(99);
    f.pulls(2, json!([pr]));
    assert_eq!(
        collect(f, 1, 2, true, Limits::default()).unwrap_err(),
        IntakeError::InvalidResponse
    );
}

#[test]
fn public_source_bytes_are_identical_when_an_unbound_hidden_repository_is_added() {
    let public = repo();
    let hidden = Repository::github("fixture-owner", "hidden-fixture", 43).unwrap();
    let mut pop = population(&public, &["old 한글\n\".rs"]);
    pop.repositories.insert(
        hidden.id().clone(),
        BTreeSet::from(["old 한글\n\".rs".into()]),
    );
    pop.basis.denominator = 2;
    let first = Collector::new(ordinary(), || 100, Limits::default())
        .unwrap()
        .collect(&public, &pop.basis, binding(1, true), binding(2, true))
        .unwrap();
    let mut second = ordinary();
    second.responses = second
        .responses
        .into_iter()
        .map(|(route, bytes)| {
            let route = route.replace("fixture-owner/fixture-repo", "fixture-owner/hidden-fixture");
            let bytes = bytes.map(|b| {
                let mut text = String::from_utf8(b)
                    .unwrap()
                    .replace("fixture-owner/fixture-repo", "fixture-owner/hidden-fixture");
                text = text.replace("\"id\":42", "\"id\":43");
                text.into_bytes()
            });
            (route, bytes)
        })
        .collect();
    let second = Collector::new(second, || 110, Limits::default())
        .unwrap()
        .collect(&hidden, &pop.basis, binding(1, false), binding(2, false))
        .unwrap();
    let reduced = build_projection(&pop, std::slice::from_ref(&first), spec()).unwrap();
    let full = build_projection(&pop, &[first, second], spec()).unwrap();
    let mut serialized = vec![];
    for projection in [&reduced, &full] {
        let authority = Authority::new(Policy {
            principals: BTreeMap::from([(
                "worker".into(),
                vec![Grant {
                    scope: Scope::Program {
                        namespace: public.id().namespace.clone(),
                        program_key: "pilot".into(),
                    },
                    object_types: BTreeSet::from(["repository".into(), "change".into()]),
                    read: true,
                    derive: true,
                    actions: BTreeSet::new(),
                }],
            )]),
        });
        let session = authority.bind_trusted("worker").unwrap();
        let mut service = ontd::Service::new(projection.memory_store().unwrap(), authority);
        serialized.push([
            service
                .render(
                    &session,
                    Query::Get {
                        id: public.id().clone(),
                    },
                    100,
                )
                .unwrap(),
            service
                .render(
                    &session,
                    Query::Get {
                        id: Id {
                            object_type: "change".into(),
                            namespace: public.id().namespace.clone(),
                            native_key: "7".into(),
                        },
                    },
                    100,
                )
                .unwrap(),
        ]);
    }
    assert_eq!(serialized[0], serialized[1]);
}

#[test]
fn check_page_bound_never_fabricates_complete_status_and_run_ids_are_unique() {
    let runs:Vec<_>=(1..=100).map(|n|json!({"id":n,"name":"rust","head_sha":pin(507),"status":"completed","conclusion":"success"})).collect();
    let route = Fake::route(&format!(
        "commits/{}/check-runs?filter=all&per_page=100&page=1",
        pin(507)
    ));
    let mut f = ordinary();
    f.put(&route, json!({"total_count":100,"check_runs":runs}));
    let p = projection(
        collect(
            f,
            1,
            2,
            true,
            Limits {
                max_pages: 1,
                ..Limits::default()
            },
        )
        .unwrap(),
    );
    assert!(
        p.objects
            .iter()
            .find(|o| o.id.object_type == "measurement")
            .unwrap()
            .observations[0]
            .coverage
            == Coverage::Partial
    );
    for runs in [
        json!([{"id":7,"name":"rust","head_sha":pin(507),"status":"completed","conclusion":null}]),
        json!([{"id":7,"name":"rust","head_sha":pin(507),"status":"completed","conclusion":"success"},{"id":7,"name":"rust","head_sha":pin(507),"status":"completed","conclusion":"success"}]),
    ] {
        let mut f = ordinary();
        f.put(
            &route,
            json!({"total_count":runs.as_array().unwrap().len(),"check_runs":runs}),
        );
        assert_eq!(
            collect(f, 1, 2, true, Limits::default()).unwrap_err(),
            IntakeError::InvalidResponse
        );
    }
}
