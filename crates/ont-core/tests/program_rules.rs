//! Synthetic contract challenges only; no slice evaluation inputs or oracles.
#![allow(clippy::unwrap_used)]
use ont_core::program::*;
use ont_core::{Change, Coverage, Id, Knowledge, Lineage, Namespace, Observation};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

fn id(kind: &str, key: &str) -> Id {
    Id {
        object_type: kind.into(),
        namespace: Namespace {
            instance: "synthetic://program".into(),
            account: "A".into(),
        },
        native_key: key.into(),
    }
}
fn reference(key: &str) -> DefinitionRef {
    DefinitionRef {
        id: id("definition", key),
        revision: "v1".into(),
    }
}
fn sha(n: usize) -> String {
    format!("{n:040x}")
}
fn set(paths: &[&str]) -> BTreeSet<String> {
    paths.iter().map(|p| (*p).into()).collect()
}
fn population(paths: BTreeSet<String>) -> PathPopulation {
    PathPopulation {
        basis: PathBasis {
            definition: reference("path-absence"),
            manifest_sha256: "a".repeat(64),
            adapter_revision: "synthetic-v1".into(),
            algorithm: PathAlgorithm::ExactUtf8PathsV1,
            denominator: paths.len() as u64,
        },
        repositories: BTreeMap::from([(id("repository", "repo"), paths)]),
    }
}
fn tree(n: usize, paths: BTreeSet<String>) -> Tree {
    Tree {
        commit: sha(n),
        tree: sha(n + 10000),
        paths,
        coverage: Coverage::Complete,
    }
}
fn candidate(repository: &Id, number: u64, commit: &str) -> PrCandidate {
    PrCandidate {
        id: PrId {
            repository: repository.clone(),
            number,
        },
        merged: true,
        merge_commit: Some(commit.into()),
    }
}
fn change(repository: &Id, before: &Tree, after: &Tree, number: u64) -> MainlineChange {
    MainlineChange {
        first_parent: before.clone(),
        accepted: after.clone(),
        prs: PrSearch {
            coverage: Coverage::Complete,
            candidates: vec![candidate(repository, number, &after.commit)],
        },
    }
}
fn endpoint(population: &PathPopulation, tree: &Tree, key: &str) -> Endpoint {
    Endpoint {
        basis: population.basis.clone(),
        tree: tree.clone(),
        source_measurement: Some(id("measurement", key)),
        observed_at: 10,
        capture: CaptureInterval { start: 10, end: 11 },
    }
}
fn interval(population: &PathPopulation, trees: &[Tree]) -> RepositoryInterval {
    let repository = population.repositories.keys().next().unwrap().clone();
    RepositoryInterval {
        repository: repository.clone(),
        before: endpoint(population, &trees[0], "before"),
        after: endpoint(population, trees.last().unwrap(), "after"),
        changes: trees
            .windows(2)
            .enumerate()
            .map(|(i, t)| change(&repository, &t[0], &t[1], i as u64 + 1))
            .collect(),
        coverage: Coverage::Complete,
    }
}
fn observation() -> Observation {
    Observation {
        revision: "r1".into(),
        source_revision: "source-v1".into(),
        observed_at: 10,
        fresh_until: 20,
        change: Change::Initial,
        supersedes: None,
        coverage: Coverage::Complete,
        fields: BTreeMap::from([("count".into(), Knowledge::Value(json!(0)))]),
        lineage: Lineage {
            artifact_sha256: "b".repeat(64),
            pointer: "/rows/0".into(),
        },
    }
}
fn definition() -> MetricDefinition {
    MetricDefinition {
        reference: reference("count"),
        formula: "accepted answers in selected window".into(),
        population: "synthetic program".into(),
        unit: Unit::Count,
        direction: Direction::Increase,
        role: MetricRole::Indicator,
    }
}

#[test]
fn kpi_requires_explicit_strategy_adoption_and_complete_definition() {
    let mut d = definition();
    assert_eq!(d.validate(), Ok(()));
    assert!(matches!(d.role, MetricRole::Indicator));
    d.role = MetricRole::Kpi {
        strategy: reference("strategy"),
        adoption: observation().lineage,
    };
    assert_eq!(d.validate(), Ok(()));
    if let MetricRole::Kpi { adoption, .. } = &mut d.role {
        adoption.artifact_sha256.clear();
    }
    assert_eq!(d.validate(), Err(RuleError::InvalidDefinition));
    d.role = MetricRole::Indicator;
    d.formula.clear();
    assert_eq!(d.validate(), Err(RuleError::InvalidDefinition));
    d = definition();
    d.reference.revision.clear();
    assert_eq!(d.validate(), Err(RuleError::InvalidDefinition));
}

#[test]
fn target_and_guardrail_require_exact_definition_unit_and_window() {
    let d = definition();
    let mut target = Target {
        definition: d.reference.clone(),
        unit: Unit::Count,
        window: Window::Interval { start: 10, end: 20 },
        direction: BoundDirection::AtLeast,
        threshold: Fraction::new(3, 1).unwrap(),
    };
    assert_eq!(target.validate(&d), Ok(()));
    assert_eq!(
        target.validate_measurement(&d.reference, Unit::Count, &target.window),
        Ok(())
    );
    assert_eq!(
        target.validate_measurement(&d.reference, Unit::LinuxMinutes, &target.window),
        Err(RuleError::IncompatibleUnit)
    );
    assert_eq!(
        target.validate_measurement(
            &d.reference,
            Unit::Count,
            &Window::Interval { start: 9, end: 20 }
        ),
        Err(RuleError::InvalidWindow)
    );
    let mut other_revision = d.reference.clone();
    other_revision.revision = "v2".into();
    assert_eq!(
        target.validate_measurement(&other_revision, Unit::Count, &target.window),
        Err(RuleError::InvalidDefinition)
    );
    let guardrail = Guardrail {
        bound: target.clone(),
        non_regression_baseline: Some(Fraction::new(1, 1).unwrap()),
    };
    assert_eq!(guardrail.validate(&d), Ok(()));
    target.unit = Unit::Bytes;
    assert_eq!(target.validate(&d), Err(RuleError::IncompatibleUnit));
    target.unit = Unit::Count;
    target.window = Window::Interval { start: 20, end: 10 };
    assert_eq!(target.validate(&d), Err(RuleError::InvalidWindow));
}

#[test]
fn relative_target_keeps_unknown_deadline_until_observed_trigger() {
    let relative = Window::AfterTrigger {
        trigger: id("change", "cutover"),
        within_seconds: 14 * 86400,
    };
    assert_eq!(relative.deadline(None), Ok(None));
    assert_eq!(relative.deadline(Some(100)), Ok(Some(100 + 14 * 86400)));
    assert_eq!(
        relative.deadline(Some(u64::MAX)),
        Err(RuleError::ArithmeticOverflow)
    );
    assert_eq!(
        Window::Interval { start: 10, end: 10 }.validate(),
        Err(RuleError::InvalidWindow)
    );
    assert_eq!(
        Window::AfterTrigger {
            trigger: id("change", "cutover"),
            within_seconds: 0
        }
        .validate(),
        Err(RuleError::InvalidWindow)
    );
}

#[test]
fn program_basis_rejects_future_partial_and_expired_current_claims() {
    let mut o = observation();
    assert!(qualify_basis(&o, 10).unwrap().is_current());
    assert!(qualify_basis(&o, 20).unwrap().is_current());
    assert!(qualify_basis(&o, 9).unwrap().future);
    assert!(!qualify_basis(&o, 9).unwrap().is_current());
    assert!(qualify_basis(&o, 21).unwrap().stale);
    o.coverage = Coverage::Partial;
    assert!(qualify_basis(&o, 10).unwrap().partial);
    assert!(!qualify_basis(&o, 10).unwrap().is_current());
    o.fresh_until = 9;
    assert_eq!(qualify_basis(&o, 10), Err(RuleError::InvalidEvidence));
}

#[test]
fn fractions_preserve_denominator_and_label_rounding_without_float_error() {
    let exact = Fraction::new(2096, 5124).unwrap();
    let rounded = exact.rounded_percent(1).unwrap();
    assert_eq!(rounded.exact.numerator(), 2096);
    assert_eq!(rounded.exact.denominator(), 5124);
    assert_eq!(rounded.text, "40.9%");
    assert_eq!(rounded.rule, "nearest_ties_away_from_zero");
    assert_eq!(
        Fraction::new(1, 32)
            .unwrap()
            .rounded_percent(2)
            .unwrap()
            .text,
        "3.13%"
    );
    assert_eq!(
        Fraction::new(-1, 32)
            .unwrap()
            .rounded_percent(2)
            .unwrap()
            .text,
        "-3.13%"
    );
    assert_eq!(
        Fraction::new(1, 100000)
            .unwrap()
            .rounded_percent(1)
            .unwrap()
            .text,
        "0.0%"
    );
    assert_eq!(
        Fraction::new(-1, 100000)
            .unwrap()
            .rounded_percent(1)
            .unwrap()
            .text,
        "0.0%"
    );
    assert_eq!(Fraction::new(1, 0), Err(RuleError::InvalidDefinition));
    assert_eq!(
        Fraction::new(i128::MAX, 1).unwrap().rounded_percent(0),
        Err(RuleError::ArithmeticOverflow)
    );
    assert_eq!(exact.rounded_percent(10), Err(RuleError::InvalidDefinition));
}

#[test]
fn synthetic_107_786_897_chain_telescopes_once_to_exact_1790_over_5124() {
    let paths: BTreeSet<String> = (0..5124).map(|i| format!("legacy/{i:04}.rs")).collect();
    let p = population(paths.clone());
    let trees = [
        tree(1, paths.clone()),
        tree(2, paths.iter().skip(107).cloned().collect()),
        tree(3, paths.iter().skip(893).cloned().collect()),
        tree(4, paths.iter().skip(1790).cloned().collect()),
    ];
    let i = interval(&p, &trees);
    let result = attribute_v1(&p, &[i], 30).unwrap();
    assert_eq!(
        result
            .contributions
            .iter()
            .map(|c| c.paths)
            .collect::<Vec<_>>(),
        [107, 786, 897]
    );
    assert_eq!(result.delta_bound, Some(1790));
    assert_eq!(result.residual, Some(0));
    assert!(result.has_unique_pr_explanation());
    assert_eq!(
        result.contributions[2].fraction,
        Fraction::new(897, 5124).unwrap()
    );
    let value = path_absence(
        &p,
        &BTreeMap::from([(id("repository", "repo"), trees[3].clone())]),
    )
    .unwrap();
    assert_eq!(value, Fraction::new(1790, 5124).unwrap());
    assert_eq!(value.rounded_percent(2).unwrap().text, "34.93%");
}

#[test]
fn exact_paths_include_unicode_newlines_spaces_and_quoted_display_lookalikes() {
    let paths = set(&["경로.rs", "a b", "a\nb", "\"quoted\"", "a\\b"]);
    let p = population(paths.clone());
    let mut shown = paths.clone();
    shown.remove("경로.rs");
    shown.insert("\"\\352\\262\\275\\353\\241\\234.rs\"".into());
    let absent = path_absence(
        &p,
        &BTreeMap::from([(id("repository", "repo"), tree(1, shown))]),
    )
    .unwrap();
    assert_eq!(absent, Fraction::new(1, 5).unwrap());
    assert_eq!(
        path_absence(
            &p,
            &BTreeMap::from([(id("repository", "repo"), tree(2, paths))])
        )
        .unwrap()
        .numerator(),
        0
    );
}

#[test]
fn restored_reverted_and_same_path_replacement_keep_signed_net_sets() {
    let p = population(set(&["a", "b", "c"]));
    let before = tree(1, set(&["a", "b", "c"]));
    let removed = tree(2, set(&["c"]));
    let restored = tree(3, set(&["a", "c"]));
    // Different blob/tree contents at the same paths do not increase absence.
    let replacement = tree(4, set(&["a", "c"]));
    let reverted = tree(5, set(&["a", "b", "c"]));
    let result = attribute_v1(
        &p,
        &[interval(
            &p,
            &[before, removed, restored, replacement, reverted],
        )],
        30,
    )
    .unwrap();
    assert_eq!(
        result
            .contributions
            .iter()
            .map(|c| c.paths)
            .collect::<Vec<_>>(),
        [2, -1, 0, -1]
    );
    assert_eq!(result.contributions[1].restored, set(&["a"]));
    assert!(result.contributions[2].newly_absent.is_empty());
    assert_eq!(result.delta_bound, Some(0));
    assert_eq!(result.residual, Some(0));
}

#[test]
fn absence_requires_full_population_complete_trees_and_matching_denominator() {
    let mut p = population(set(&["a"]));
    let repository = id("repository", "repo");
    assert_eq!(
        path_absence(&p, &BTreeMap::new()),
        Err(RuleError::IncompleteEvidence)
    );
    let mut t = tree(1, BTreeSet::new());
    t.coverage = Coverage::Partial;
    assert_eq!(
        path_absence(&p, &BTreeMap::from([(repository.clone(), t)])),
        Err(RuleError::IncompleteEvidence)
    );
    p.basis.denominator = 2;
    assert_eq!(p.validate(), Err(RuleError::InvalidDefinition));
    p.basis.denominator = 1;
    p.basis.algorithm = PathAlgorithm::ProducerV1RenderedPaths;
    assert_eq!(p.validate(), Err(RuleError::UnsupportedAlgorithm));
    p.basis.algorithm = PathAlgorithm::ExactUtf8PathsV1;
    let mut malformed = tree(1, set(&["a\0b"]));
    assert_eq!(
        path_absence(
            &p,
            &BTreeMap::from([(repository.clone(), malformed.clone())])
        ),
        Err(RuleError::InvalidEvidence)
    );
    malformed = tree(1, set(&["a"]));
    malformed.commit = "short".into();
    assert_eq!(
        path_absence(&p, &BTreeMap::from([(repository, malformed)])),
        Err(RuleError::InvalidEvidence)
    );
}

#[test]
fn unbound_original_snapshots_keep_merge_facts_but_delta_and_residual_unknown() {
    let p = population(set(&["a", "b"]));
    let mut i = interval(&p, &[tree(1, set(&["a", "b"])), tree(2, set(&["b"]))]);
    i.before.source_measurement = None;
    let result = attribute_v1(&p, &[i], 30).unwrap();
    assert_eq!(result.contributions[0].paths, 1);
    assert_eq!(result.delta_bound, None);
    assert_eq!(result.residual, None);
    assert!(result.gaps.contains(&ReconciliationGap::EndpointBinding));
    assert_eq!(result.qualification(), UNRESOLVED_RECONCILIATION);
}

#[test]
fn incompatible_algorithm_manifest_revision_adapter_or_denominator_prevents_reconciliation() {
    let p = population(set(&["a"]));
    let i = interval(&p, &[tree(1, set(&["a"])), tree(2, set(&[]))]);
    let mut bases = vec![p.basis.clone(); 5];
    bases[0].algorithm = PathAlgorithm::ProducerV1RenderedPaths;
    bases[1].manifest_sha256 = "b".repeat(64);
    bases[2].definition.revision = "v2".into();
    bases[3].adapter_revision = "adapter-v2".into();
    bases[4].denominator = 2;
    for basis in bases {
        let mut incompatible = i.clone();
        incompatible.before.basis = basis;
        let result = attribute_v1(&p, &[incompatible], 30).unwrap();
        assert_eq!(result.contributions[0].paths, 1);
        assert!(result.gaps.contains(&ReconciliationGap::DefinitionMismatch));
        assert_eq!((result.delta_bound, result.residual), (None, None));
        assert!(!result.has_unique_pr_explanation());
    }
}

#[test]
fn partial_tree_coverage_has_numeric_residual_without_guessing_a_pr_assignment() {
    let p = population(set(&["a", "b", "c"]));
    let mut missing = tree(3, set(&[]));
    missing.coverage = Coverage::Partial;
    let mut i = interval(
        &p,
        &[
            tree(1, set(&["a", "b", "c"])),
            tree(2, set(&["b", "c"])),
            missing,
            tree(4, set(&[])),
        ],
    );
    i.coverage = Coverage::Partial;
    let result = attribute_v1(&p, &[i.clone()], 30).unwrap();
    assert_eq!(result.contributions.len(), 1);
    assert_eq!((result.delta_bound, result.residual), (Some(3), Some(2)));
    assert_eq!(result.qualification(), UNRESOLVED_RECONCILIATION);
    i.coverage = Coverage::Complete;
    assert_eq!(
        attribute_v1(&p, &[i.clone()], 30),
        Err(RuleError::IncompleteEvidence)
    );
    // A prefix unconnected to the pinned after commit cannot establish membership.
    i.coverage = Coverage::Partial;
    i.changes.truncate(1);
    assert_eq!(attribute_v1(&p, &[i], 30), Err(RuleError::IntegrityFailure));
}

#[test]
fn second_parent_pr_head_disconnected_duplicate_and_conflicting_pins_refuse() {
    let p = population(set(&["a", "b"]));
    let i = interval(
        &p,
        &[
            tree(1, set(&["a", "b"])),
            tree(2, set(&["b"])),
            tree(3, set(&[])),
        ],
    );
    let mut wrong = i.clone();
    wrong.changes[0].first_parent = tree(99, set(&["a", "b"]));
    assert_eq!(
        attribute_v1(&p, &[wrong], 30),
        Err(RuleError::IntegrityFailure)
    );
    let mut wrong = i.clone();
    wrong.changes.reverse();
    assert_eq!(
        attribute_v1(&p, &[wrong], 30),
        Err(RuleError::IntegrityFailure)
    );
    let mut wrong = i.clone();
    wrong.changes.push(wrong.changes[1].clone());
    assert_eq!(
        attribute_v1(&p, &[wrong], 30),
        Err(RuleError::IntegrityFailure)
    );
    let mut wrong = i.clone();
    wrong.changes[1].first_parent.paths.clear();
    assert_eq!(
        attribute_v1(&p, &[wrong], 30),
        Err(RuleError::IntegrityFailure)
    );
    let mut wrong = i.clone();
    wrong.after.tree.tree = wrong.before.tree.tree.clone();
    assert_eq!(
        attribute_v1(&p, &[wrong], 30),
        Err(RuleError::IntegrityFailure)
    );
    assert_eq!(
        attribute_v1(&p, &[i.clone(), i], 30),
        Err(RuleError::IntegrityFailure)
    );
}

#[test]
fn association_cardinality_never_duplicates_or_divides_a_merge_contribution() {
    let p = population(set(&["a", "b"]));
    let mut i = interval(&p, &[tree(1, set(&["a", "b"])), tree(2, set(&[]))]);
    let second = candidate(&i.repository, 2, &i.after.tree.commit);
    i.changes[0].prs.candidates.extend([second.clone(), second]);
    let result = attribute_v1(&p, &[i], 30).unwrap();
    assert_eq!(result.contributions.len(), 1);
    assert_eq!(result.contributions[0].paths, 2);
    assert!(
        matches!(&result.contributions[0].association, PrAssociation::Ambiguous(ids) if ids.len() == 2)
    );
    assert_eq!(result.residual, Some(0));
    assert_eq!(result.qualification(), UNRESOLVED_RECONCILIATION);
}

#[test]
fn foreign_repository_unmerged_and_wrong_merge_sha_are_not_pr_evidence() {
    let p = population(set(&["a"]));
    let mut i = interval(&p, &[tree(1, set(&["a"])), tree(2, set(&[]))]);
    let mut foreign = candidate(&i.repository, 1, &i.after.tree.commit);
    foreign.id.repository.namespace.account = "B".into();
    let mut unmerged = candidate(&i.repository, 2, &i.after.tree.commit);
    unmerged.merged = false;
    let wrong_head = candidate(&i.repository, 3, &sha(999));
    i.changes[0].prs.candidates = vec![foreign, unmerged, wrong_head];
    let result = attribute_v1(&p, &[i], 30).unwrap();
    assert_eq!(result.contributions[0].association, PrAssociation::Missing);
    assert_eq!(result.contributions[0].paths, 1);
    assert!(!result.has_unique_pr_explanation());
}

#[test]
fn failed_or_incomplete_pr_search_is_unknown_even_with_one_observed_match() {
    let p = population(set(&["a"]));
    let mut i = interval(&p, &[tree(1, set(&["a"])), tree(2, set(&[]))]);
    i.changes[0].prs.coverage = Coverage::Partial;
    let result = attribute_v1(&p, &[i.clone()], 30).unwrap();
    assert!(
        matches!(&result.contributions[0].association, PrAssociation::Unknown { eligible } if eligible.len() == 1)
    );
    i.changes[0].prs.candidates.clear();
    let result = attribute_v1(&p, &[i], 30).unwrap();
    assert!(
        matches!(&result.contributions[0].association, PrAssociation::Unknown { eligible } if eligible.is_empty())
    );
}

#[test]
fn future_endpoint_cannot_upgrade_historical_git_facts_to_qualified_pr_explanation() {
    let p = population(set(&["a"]));
    let mut i = interval(&p, &[tree(1, set(&["a"])), tree(2, set(&[]))]);
    i.after.observed_at = 31;
    let result = attribute_v1(&p, &[i.clone()], 30).unwrap();
    assert!(result.gaps.contains(&ReconciliationGap::FutureEndpoint));
    assert_eq!(result.contributions[0].paths, 1);
    assert!(!result.has_unique_pr_explanation());
    i.after.observed_at = 10;
    i.after.capture.end = 31;
    assert!(
        attribute_v1(&p, &[i], 30)
            .unwrap()
            .gaps
            .contains(&ReconciliationGap::FutureEndpoint)
    );
}

#[test]
fn multi_repository_population_uses_exact_identity_and_requires_all_endpoints() {
    let mut p = population(set(&["a"]));
    let mut other = id("repository", "repo");
    other.namespace.account = "B".into();
    p.repositories.insert(other.clone(), set(&["a", "b"]));
    p.basis.denominator = 3;
    let a = interval(&p, &[tree(1, set(&["a"])), tree(2, set(&[]))]);
    let mut b = interval(&p, &[tree(1, set(&["a", "b"])), tree(2, set(&["a"]))]);
    b.repository = other.clone();
    b.changes[0].prs.candidates = vec![candidate(&other, 1, &b.after.tree.commit)];
    // Reverse input order must still publish stable repository order.
    let result = attribute_v1(&p, &[b, a.clone()], 30).unwrap();
    assert_eq!((result.delta_bound, result.residual), (Some(2), Some(0)));
    assert_eq!(result.contributions[0].repository.namespace.account, "A");
    assert_eq!(result.contributions[1].repository.namespace.account, "B");
    assert_eq!(result.contributions[0].fraction.denominator(), 3);
    let missing = attribute_v1(&p, &[a], 30).unwrap();
    assert_eq!((missing.delta_bound, missing.residual), (None, None));
    assert!(!missing.has_unique_pr_explanation());
}

#[test]
fn identical_bound_endpoints_give_zero_and_empty_chain_unbound_rows_do_not() {
    let p = population(set(&["a"]));
    let mut i = interval(&p, &[tree(1, set(&["a"]))]);
    let result = attribute_v1(&p, &[i.clone()], 30).unwrap();
    assert!(result.contributions.is_empty());
    assert!(!result.has_unique_pr_explanation());
    assert_eq!(
        result.qualification(),
        "qualified empty interval; no PR attribution"
    );
    assert_eq!((result.delta_bound, result.residual), (Some(0), Some(0)));
    i.after.source_measurement = None;
    let result = attribute_v1(&p, &[i], 30).unwrap();
    assert_eq!((result.delta_bound, result.residual), (None, None));
}

#[test]
fn unavailable_values_never_become_current_zero() {
    for unavailable in [
        Knowledge::Missing,
        Knowledge::Null,
        Knowledge::Blank,
        Knowledge::Unknown,
        Knowledge::NotPerformed,
    ] {
        let mut o = observation();
        o.fields.insert("count".into(), unavailable);
        let q = qualify_basis(&o, 10).unwrap();
        assert!(q.unresolved);
        assert!(!q.is_current());
    }
    let mut o = observation();
    o.fields.clear();
    assert!(!qualify_basis(&o, 10).unwrap().is_current());
    assert!(qualify_basis(&observation(), 10).unwrap().is_current());
}

#[test]
fn conflicting_or_malformed_pr_records_cannot_establish_complete_association() {
    let p = population(set(&["a"]));
    let mut i = interval(&p, &[tree(1, set(&["a"])), tree(2, set(&[]))]);
    let mut contradiction = i.changes[0].prs.candidates[0].clone();
    contradiction.merged = false;
    i.changes[0].prs.candidates.push(contradiction);
    assert_eq!(
        attribute_v1(&p, &[i.clone()], 30),
        Err(RuleError::IntegrityFailure)
    );
    i.changes[0].prs.candidates.pop();
    i.changes[0].prs.candidates[0].merge_commit = None;
    assert_eq!(attribute_v1(&p, &[i], 30), Err(RuleError::InvalidEvidence));
}

#[test]
fn one_pr_cannot_claim_different_merged_revisions_across_commit_searches() {
    let p = population(set(&["a", "b"]));
    let mut i = interval(
        &p,
        &[
            tree(1, set(&["a", "b"])),
            tree(2, set(&["b"])),
            tree(3, set(&[])),
        ],
    );
    let consistent = i.changes[0].prs.candidates[0].clone();
    // Seeing the same exact record in both complete searches is consistent.
    i.changes[1].prs.candidates.push(consistent.clone());
    assert!(
        attribute_v1(&p, &[i.clone()], 30)
            .unwrap()
            .has_unique_pr_explanation()
    );
    i.changes[1].prs.candidates.truncate(1);
    i.changes[1].prs.candidates[0].id = consistent.id.clone();
    assert_eq!(attribute_v1(&p, &[i], 30), Err(RuleError::IntegrityFailure));
}

#[test]
fn contradictory_pr_records_in_different_repository_searches_also_refuse() {
    let mut p = population(set(&["a"]));
    let other = id("repository", "other");
    p.repositories.insert(other.clone(), set(&["a"]));
    p.basis.denominator = 2;
    let mut a = interval(&p, &[tree(1, set(&["a"])), tree(2, set(&[]))]);
    a.repository = id("repository", "repo");
    a.changes[0].prs.candidates = vec![candidate(&a.repository, 1, &a.after.tree.commit)];
    let mut b = a.clone();
    b.repository = other.clone();
    b.changes[0].prs.candidates = vec![candidate(&other, 1, &b.after.tree.commit)];
    // Equal PR numbers in different repositories are separate identities.
    assert!(
        attribute_v1(&p, &[a.clone(), b.clone()], 30)
            .unwrap()
            .has_unique_pr_explanation()
    );
    let mut contradiction = a.changes[0].prs.candidates[0].clone();
    contradiction.merge_commit = Some(sha(99));
    b.changes[0].prs.candidates.push(contradiction);
    assert_eq!(
        attribute_v1(&p, &[a.clone(), b.clone()], 30),
        Err(RuleError::IntegrityFailure)
    );
    assert_eq!(
        attribute_v1(&p, &[b, a], 30),
        Err(RuleError::IntegrityFailure)
    );
}
