//! Synthetic native report derivation contracts. No source acquisition.
#![allow(clippy::unwrap_used)]
use ont_core::{
    Authority, Change, Coverage, Grant, Id, Knowledge, Lineage, Link, Namespace, Object,
    Observation, Policy, Query, Scope, answer::Version, report::ReportWindow,
};
use ont_store::Store;
use ontd::Service;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
fn id(kind: &str, key: &str, account: &str) -> Id {
    Id {
        object_type: kind.into(),
        namespace: Namespace {
            instance: "synthetic://metrics".into(),
            account: account.into(),
        },
        native_key: key.into(),
    }
}
fn object(kind: &str, key: &str, account: &str, at: u64, fields: Value) -> Object {
    let id = id(kind, key, account);
    Object {
        scope: Scope::program(id.namespace.clone(), "p").unwrap(),
        id,
        head: "r1".into(),
        observations: vec![Observation {
            revision: "r1".into(),
            source_revision: "synthetic/v1".into(),
            observed_at: at,
            fresh_until: 100,
            change: Change::Initial,
            supersedes: None,
            coverage: Coverage::Complete,
            fields: fields
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), Knowledge::Value(v.clone())))
                .collect(),
            lineage: Lineage {
                artifact_sha256: "a".repeat(64),
                pointer: "/fixture".into(),
            },
        }],
    }
}
fn edge(source: Id, relation: &str, target: Id) -> Link {
    Link {
        source,
        relation: relation.into(),
        target,
    }
}
fn graph(repos: usize) -> (Vec<Object>, Vec<Link>) {
    let mut objects = vec![
        object("program", "root", "owner", 0, json!({})),
        object(
            "metric",
            "legacy",
            "owner",
            0,
            json!({"definition":{"id":id("metric","legacy","owner"),"revision":"d1","formula":"removed / total","population":"adopted","unit":"fraction","direction":"increase","role":{"kind":"indicator"}}}),
        ),
    ];
    let mut links = vec![edge(
        objects[0].id.clone(),
        "tracks_indicator",
        objects[1].id.clone(),
    )];
    for (key, t, n) in [("start", 10, 2), ("middle", 15, 4), ("end", 20, 7)] {
        let o = object(
            "measurement",
            key,
            "owner",
            t,
            json!({"removed":n,"total":17,"source_unit":"fraction","definition_revision":"d1","manifest_sha256":"m1","algorithm":"exact_utf8_paths_v1"}),
        );
        links.push(edge(objects[1].id.clone(), "has_measurement", o.id.clone()));
        objects.push(o);
    }
    let mut contributions = vec![];
    for i in 0..repos {
        let account = format!("repo-{i}");
        let repo = object("repository", "r", &account, 0, json!({}));
        let change = object(
            "change",
            "1",
            &account,
            0,
            json!({"merged":true,"merge_commit":format!("commit-{i}")}),
        );
        contributions.push(json!({"repository":repo.id,"commit":format!("commit-{i}"),"first_parent":"parent","newly_absent_count":if i==0{2}else{3},"restored_count":0,"path_set_sha256":"p1","paths":if i==0{"2"}else{"3"},"association":"unique","changes":[change.id]}));
        links.push(edge(objects[0].id.clone(), "repository", repo.id.clone()));
        links.push(edge(
            id("measurement", "attribution", "owner"),
            "supported_by",
            change.id.clone(),
        ));
        links.push(edge(change.id.clone(), "in_repository", repo.id.clone()));
        objects.extend([repo, change]);
    }
    let a = object(
        "measurement",
        "attribution",
        "owner",
        20,
        json!({"attribution":{"rule":"attribution-v1","adapter":"synthetic/v1","definition":{"id":objects[1].id,"revision":"d1"},"manifest_sha256":"m1","population_adapter_revision":"p1","algorithm":"exact_utf8_paths_v1","denominator":17,"delta_bound":"5","residual":(5-if repos==0{0}else if repos==1{2}else{5}).to_string(),"qualification":"qualified","gaps":[],"contributions":contributions}}),
    );
    links.push(edge(objects[0].id.clone(), "has_measurement", a.id.clone()));
    objects.push(a);
    (objects, links)
}
fn policy(objects: &[Object], hidden: &[&str], derive: bool) -> Policy {
    Policy {
        principals: BTreeMap::from([(
            "reader".into(),
            objects
                .iter()
                .filter(|o| !hidden.contains(&o.id.namespace.account.as_str()))
                .map(|o| Grant {
                    scope: o.scope.clone(),
                    object_types: BTreeSet::from([o.id.object_type.clone()]),
                    read: true,
                    derive,
                    actions: BTreeSet::new(),
                })
                .collect(),
        )]),
    }
}
fn render(
    objects: &[Object],
    links: &[Link],
    p: Policy,
    start: u64,
    end: u64,
    version: Option<Version>,
) -> String {
    let mut store = Store::memory_program().unwrap();
    store.seed(objects, links).unwrap();
    let authority = Authority::new(p);
    let session = authority.bind_trusted("reader").unwrap();
    let mut service = Service::new(store, authority);
    let query = Query::ProgramReport {
        root: id("program", "root", "owner"),
        window: ReportWindow { start, end },
    };
    match version {
        Some(v) => {
            service
                .render_answer_record_version_with_status(&session, query, 50, v)
                .unwrap()
                .0
        }
        None => service
            .render(
                &session,
                Query::ProgramReportV2 {
                    root: id("program", "root", "owner"),
                    window: ReportWindow { start, end },
                },
                50,
            )
            .unwrap(),
    }
}
fn report(objects: &[Object], links: &[Link], start: u64, end: u64) -> Value {
    let v: Value = serde_json::from_str(&render(
        objects,
        links,
        policy(objects, &[], true),
        start,
        end,
        None,
    ))
    .unwrap();
    v["result"]["report"].clone()
}
fn rational(n: &str, d: u64) -> Value {
    json!({"status":"known","value":{"numerator":n,"denominator":d}})
}
fn unavailable(reason: &str) -> Value {
    json!({"status":"not_available","reason":reason})
}
#[test]
fn native_kpi_and_single_multiple_repository_goldens() {
    for repos in [1, 2] {
        let (o, l) = graph(repos);
        let r = report(&o, &l, 10, 20);
        let m = &r["metrics"][0];
        let a = &r["attributions"][0];
        let actual = json!({"schema":r["schema"],"rule":r["derivation_rule"],"sample_values":m["samples"].as_array().unwrap().iter().map(|s|s["value"].clone()).collect::<Vec<_>>(),"start":m["endpoints"]["value"]["start"]["basis"],"end":m["endpoints"]["value"]["end"]["basis"],"change":m["change"],"percentage_point_change":m["percentage_point_change"],"attribution_change":a["change"],"sum":a["component_contribution_sum"],"residual":a["reconciliation_residual"],"attribution_endpoints":a["endpoints"]});
        let expected: Value = serde_json::from_str(if repos == 1 {
            include_str!("report-metrics-goldens/single.json")
        } else {
            include_str!("report-metrics-goldens/multiple.json")
        })
        .unwrap();
        assert_eq!(actual, expected);
    }
}
#[test]
fn intervals_never_bind_attribution_endpoints_and_equal_values_are_not_identity() {
    let (mut o, l) = graph(2);
    for (start, end, n) in [
        (10, 20, Some("5")),
        (15, 20, Some("3")),
        (0, 20, Some("5")),
        (11, 21, Some("3")),
        (20, 20, None),
    ] {
        let r = report(&o, &l, start, end);
        assert_eq!(
            r["metrics"][0]["change"],
            n.map(|n| rational(n, 17))
                .unwrap_or_else(|| unavailable("missing_endpoints"))
        );
        assert_eq!(
            r["attributions"][0]["endpoints"],
            unavailable("unbound_endpoints")
        );
        assert_eq!(r["attributions"][0]["change"], rational("5", 17));
    }
    o.iter_mut()
        .find(|o| o.id.native_key == "middle")
        .unwrap()
        .observations[0]
        .fields
        .insert("removed".into(), Knowledge::Value(2.into()));
    assert_eq!(
        report(&o, &l, 15, 20)["attributions"][0]["endpoints"],
        unavailable("unbound_endpoints")
    );
    let end = o.iter_mut().find(|o| o.id.native_key == "end").unwrap();
    end.observations[0]
        .fields
        .insert("algorithm".into(), Knowledge::Value("different".into()));
    assert_eq!(
        report(&o, &l, 10, 20)["metrics"][0]["change"],
        unavailable("incomparable_components")
    );
    end_time(&mut o, "end", 15);
    assert_eq!(
        report(&o, &l, 10, 20)["metrics"][0]["change"],
        unavailable("ambiguous_timestamp")
    );
}
fn end_time(objects: &mut [Object], key: &str, at: u64) {
    objects
        .iter_mut()
        .find(|o| o.id.native_key == key)
        .unwrap()
        .observations[0]
        .observed_at = at;
}
#[test]
fn unresolved_ambiguous_negative_zero_changes_and_missing_merged_evidence() {
    for repos in [0, 1, 2] {
        let (mut o, l) = graph(repos);
        let a = o
            .iter_mut()
            .find(|o| o.id.native_key == "attribution")
            .unwrap();
        let Knowledge::Value(v) = a.observations[0].fields.get_mut("attribution").unwrap() else {
            panic!()
        };
        v["delta_bound"] = Value::Null;
        v["residual"] = Value::Null;
        if repos > 0 {
            v["contributions"][0]["association"] = "ambiguous".into();
            v["contributions"][0]["restored_count"] = 4.into();
            v["contributions"][0]["paths"] = "-2".into();
        }
        let r = report(&o, &l, 10, 20);
        let a = &r["attributions"][0];
        assert_eq!(a["change"], unavailable("unbound_aggregate_change"));
        assert_eq!(
            a["reconciliation_residual"],
            unavailable("unbound_aggregate_change")
        );
        assert_eq!(
            a["component_contribution_sum"],
            rational(
                if repos == 0 {
                    "0"
                } else if repos == 1 {
                    "-2"
                } else {
                    "1"
                },
                17
            )
        );
        if repos > 0 {
            assert_eq!(
                a["contributions"][0]["assignment"],
                unavailable("ambiguous_association")
            );
        }
    }
    let (mut o, l) = graph(0);
    let a = o
        .iter_mut()
        .find(|o| o.id.native_key == "attribution")
        .unwrap();
    let Knowledge::Value(v) = a.observations[0].fields.get_mut("attribution").unwrap() else {
        panic!()
    };
    v["delta_bound"] = "0".into();
    v["residual"] = "0".into();
    let r = report(&o, &l, 10, 20);
    assert_eq!(r["attributions"][0]["change"], rational("0", 17));
    assert_eq!(
        r["attributions"][0]["reconciliation_residual"],
        rational("0", 17)
    );
    let (mut o, mut l) = graph(1);
    o.retain(|o| o.id.native_key != "attribution");
    l.retain(|l| l.source.native_key != "attribution" && l.target.native_key != "attribution");
    l.push(edge(
        o[0].id.clone(),
        "has_measurement",
        id("measurement", "support", "owner"),
    ));
    o.push(object("measurement", "support", "owner", 20, json!({})));
    l.push(edge(
        id("measurement", "support", "owner"),
        "supported_by",
        id("change", "1", "repo-0"),
    ));
    let r = report(&o, &l, 10, 20);
    assert_eq!(
        r["attributions"][0]["change"],
        unavailable("missing_contribution_evidence")
    );
}
#[test]
fn billing_periods_exact_values_and_zero_are_typed_without_savings_attribution() {
    let (mut o, mut l) = graph(0);
    for (key, at, n) in [("bill-a", 10, 302), ("bill-b", 20, 335)] {
        let b = object(
            "guardrail",
            key,
            "billing",
            at,
            json!({"guardrail":"billing","unit":"linux_minutes","account":"test-account","window_start":0,"window_end":100,"value":n,"source_qualified":true,"qualification":"qualified","bound_satisfied":true}),
        );
        l.push(edge(o[0].id.clone(), "guardrail", b.id.clone()));
        o.push(b);
    }
    let r = report(&o, &l, 10, 20);
    let b = r["metrics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["kind"] == "billing")
        .unwrap();
    let actual = json!({"unit":b["unit"],"values":b["samples"].as_array().unwrap().iter().map(|s|s["value"].clone()).collect::<Vec<_>>(),"change":b["change"],"percentage_point_change":b["percentage_point_change"],"population":b["population"]});
    assert_eq!(
        actual,
        serde_json::from_str::<Value>(include_str!("report-metrics-goldens/billing.json")).unwrap()
    );
    let last = o.last_mut().unwrap();
    last.observations[0]
        .fields
        .insert("value".into(), Knowledge::Value(0.into()));
    let r = report(&o, &l, 10, 20);
    let b = r["metrics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["kind"] == "billing")
        .unwrap();
    assert_eq!(b["samples"][1]["value"], rational("0", 1));
    assert_eq!(b["change"], rational("-302", 1));
    o.last_mut().unwrap().observations[0]
        .fields
        .insert("window_end".into(), Knowledge::Value(101.into()));
    let r = report(&o, &l, 10, 20);
    assert_eq!(
        r["metrics"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["kind"] == "billing")
            .count(),
        2
    );
}
fn billing_metric(stale_first_id: bool, stale_latest: bool) -> Value {
    let (mut o, mut l) = graph(0);
    let (stale_key, fresh_key) = if stale_first_id {
        ("a-stale", "z-fresh")
    } else {
        ("z-stale", "a-fresh")
    };
    let samples = if stale_latest {
        [(fresh_key, 10, 100, 302), (stale_key, 20, 25, 335)]
    } else {
        [(stale_key, 10, 15, 302), (fresh_key, 20, 100, 335)]
    };
    for (key, at, fresh, n) in samples {
        let mut b = object(
            "guardrail",
            key,
            "billing",
            at,
            json!({"guardrail":"billing","unit":"linux_minutes","account":"test-account","window_start":0,"window_end":100,"value":n,"source_qualified":true,"qualification":"qualified","bound_satisfied":true}),
        );
        b.observations[0].fresh_until = fresh;
        l.push(edge(o[0].id.clone(), "guardrail", b.id.clone()));
        o.push(b);
    }
    let r = report(&o, &l, 0, 100);
    r["metrics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["kind"] == "billing")
        .unwrap()
        .clone()
}
fn component_metric(stale_first_id: bool, stale_latest: bool) -> Value {
    let (mut o, mut l) = graph(0);
    o.retain(|o| o.id.object_type != "metric" && o.id.native_key != "attribution");
    l.clear();
    let (stale_key, fresh_key) = if stale_first_id {
        ("a-stale", "z-fresh")
    } else {
        ("z-stale", "a-fresh")
    };
    let samples = if stale_latest {
        [(fresh_key, 10, 100, 1), (stale_key, 20, 25, 3)]
    } else {
        [(stale_key, 10, 15, 1), (fresh_key, 20, 100, 3)]
    };
    for (key, at, fresh, n) in samples {
        let mut m = object(
            "measurement",
            key,
            "owner",
            at,
            json!({"removed":n,"total":4,"source_unit":"fraction","definition_revision":"d1","manifest_sha256":"m1","algorithm":"exact_utf8_paths_v1"}),
        );
        m.observations[0].fresh_until = fresh;
        l.push(edge(o[0].id.clone(), "has_measurement", m.id.clone()));
        o.push(m);
    }
    let r = report(&o, &l, 0, 100);
    r["metrics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["kind"] == "snapshot_component")
        .unwrap()
        .clone()
}
#[test]
fn headless_series_take_head_condition_from_the_latest_sample_in_both_id_orders() {
    for stale_first_id in [true, false] {
        assert_eq!(
            billing_metric(stale_first_id, false)["source_qualified"],
            true
        );
        assert_eq!(
            component_metric(stale_first_id, false)["source_qualified"],
            true
        );
        assert_eq!(
            billing_metric(stale_first_id, true)["source_qualified"],
            false
        );
        assert_eq!(
            component_metric(stale_first_id, true)["source_qualified"],
            false
        );
    }
}

#[test]
fn reduced_full_hidden_mutations_and_query_differencing_are_byte_invariant() {
    let (o, l) = graph(2);
    for hidden in [vec!["repo-0"], vec!["repo-1"], vec!["repo-0", "repo-1"]] {
        let reduced: Vec<_> = o
            .iter()
            .filter(|o| !hidden.contains(&o.id.namespace.account.as_str()))
            .cloned()
            .collect();
        let ids: BTreeSet<_> = reduced.iter().map(|o| o.id.clone()).collect();
        let links: Vec<_> = l
            .iter()
            .filter(|l| ids.contains(&l.source) && ids.contains(&l.target))
            .cloned()
            .collect();
        for (start, end) in [(10, 20), (15, 20), (0, 20), (20, 20)] {
            for version in [None, Some(Version::V3)] {
                let expected = render(
                    &reduced,
                    &links,
                    policy(&o, &hidden, true),
                    start,
                    end,
                    version,
                );
                assert_eq!(
                    render(&o, &l, policy(&o, &hidden, true), start, end, version),
                    expected
                );
                let mut changed = o.clone();
                for v in changed
                    .iter_mut()
                    .filter(|o| hidden.contains(&o.id.namespace.account.as_str()))
                {
                    v.observations[0].observed_at = 99;
                    v.observations[0]
                        .fields
                        .insert("secret".into(), Knowledge::Value(999.into()));
                    v.observations[0].revision = "hidden-replacement".into();
                    v.head = "hidden-replacement".into();
                }
                assert_eq!(
                    render(&changed, &l, policy(&o, &hidden, true), start, end, version),
                    expected
                );
            }
        }
    }
    let denied = render(&o, &l, policy(&o, &[], false), 10, 20, Some(Version::V3));
    let v: Value = serde_json::from_str(&denied).unwrap();
    assert_eq!(
        v["outcome"],
        json!({"status":"refused","code":"derive_forbidden"})
    );
    assert!(!denied.contains("root"));
}
#[test]
fn v3_selects_native_derivation_and_retains_exact_decimal_renderings() {
    let (o, l) = graph(2);
    let v: Value = serde_json::from_str(&render(
        &o,
        &l,
        policy(&o, &[], true),
        10,
        20,
        Some(Version::V3),
    ))
    .unwrap();
    assert_eq!(v["schema"], "ontology-answer/v3");
    assert_eq!(v["result"]["kind"], "program_report_v2");
    let change = &v["result"]["report"]["metrics"][0]["change"]["value"];
    assert_eq!(change["numerator"], "5");
    assert_eq!(
        change["decimal_renderings"],
        json!({"2":"0.29","3":"0.294","4":"0.2941","6":"0.294118"})
    );
    for version in [Version::V1, Version::V2] {
        let v: Value = serde_json::from_str(&render(
            &o,
            &l,
            policy(&o, &[], true),
            10,
            20,
            Some(version),
        ))
        .unwrap();
        assert_eq!(v["result"]["kind"], "program_report");
        assert!(v["result"]["report"].get("derivation_rule").is_none());
    }
}

#[test]
fn flat_authorized_snapshot_rows_derive_without_metric_handles_or_global_population_guess() {
    let (mut o, mut l) = graph(0);
    o.retain(|o| o.id.object_type != "metric" && o.id.native_key != "attribution");
    l.clear();
    for key in ["start", "middle", "end"] {
        l.push(edge(
            o[0].id.clone(),
            "has_measurement",
            id("measurement", key, "owner"),
        ));
    }
    let r = report(&o, &l, 10, 20);
    assert_eq!(r["metrics"].as_array().unwrap().len(), 1);
    assert_eq!(r["metrics"][0]["change"], rational("5", 17));
    assert_eq!(r["metrics"][0]["kind"], "snapshot_component");
    assert_eq!(r["metrics"][0]["definition"]["value"]["role"], "indicator");
    let mut store = Store::memory_program().unwrap();
    store.seed(&o, &l).unwrap();
    let a = Authority::new(policy(&o, &[], true));
    let session = a.bind_trusted("reader").unwrap();
    let mut service = Service::new(store, a);
    let old: Value = serde_json::from_str(
        &service
            .render(
                &session,
                Query::ProgramReport {
                    root: o[0].id.clone(),
                    window: ReportWindow { start: 10, end: 20 },
                },
                50,
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(old["result"]["report"]["metrics"], json!([]));
    // A second repository has its own component population, not an inferred sum.
    for (key, at, n) in [("start", 10, 1), ("end", 20, 3)] {
        let v = object(
            "measurement",
            key,
            "other",
            at,
            json!({"removed":n,"total":5,"source_unit":"fraction","definition_revision":"d1","manifest_sha256":"m1","algorithm":"exact_utf8_paths_v1"}),
        );
        l.push(edge(o[0].id.clone(), "has_measurement", v.id.clone()));
        o.push(v);
    }
    let r = report(&o, &l, 10, 20);
    assert_eq!(r["metrics"].as_array().unwrap().len(), 2);
    assert!(
        !r["metrics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["population"]["value"] == 22)
    );
}

#[test]
fn multiple_metrics_population_changes_and_duplicate_attribution_do_not_invent_joins() {
    let (mut o, mut l) = graph(2);
    let mut other = o[1].clone();
    other.id.native_key = "independent".into();
    l.push(edge(o[0].id.clone(), "tracks_indicator", other.id.clone()));
    for key in ["start", "middle", "end"] {
        l.push(edge(
            other.id.clone(),
            "has_measurement",
            id("measurement", key, "owner"),
        ));
    }
    o.push(other);
    let r = report(&o, &l, 15, 20);
    assert_eq!(r["metrics"].as_array().unwrap().len(), 2);
    assert_eq!(
        r["attributions"][0]["endpoints"],
        unavailable("unbound_endpoints")
    );
    let a = o
        .iter_mut()
        .find(|o| o.id.native_key == "attribution")
        .unwrap();
    let Knowledge::Value(v) = a.observations[0].fields.get_mut("attribution").unwrap() else {
        panic!()
    };
    let duplicate = v["contributions"][0].clone();
    v["contributions"].as_array_mut().unwrap().push(duplicate);
    v["residual"] = "-2".into();
    let r = report(&o, &l, 10, 20);
    assert_eq!(
        r["attributions"][0]["component_contribution_sum"],
        unavailable("invalid_attribution")
    );
    assert!(
        r["attributions"][0]["contributions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["assignment"] == unavailable("invalid_attribution"))
    );
    o.iter_mut()
        .find(|o| o.id.native_key == "end")
        .unwrap()
        .observations[0]
        .fields
        .insert("total".into(), Knowledge::Value(18.into()));
    let r = report(&o, &l, 10, 20);
    assert_eq!(
        r["metrics"][0]["population"],
        unavailable("incomparable_components")
    );
    assert_eq!(
        r["metrics"][0]["change"],
        unavailable("incomparable_components")
    );
}

#[test]
fn complete_native_report_serialization_has_literal_forward_goldens() {
    for (repos, golden) in [
        (1, include_str!("report-metrics-goldens/single-report.json")),
        (
            2,
            include_str!("report-metrics-goldens/multiple-report.json"),
        ),
    ] {
        let (o, l) = graph(repos);
        let actual = report(&o, &l, 10, 20);
        let mut expected: Value = serde_json::from_str(golden).unwrap();
        // Existing basis serialization is copied from the authored source objects,
        // independently of the report builder; every new report field is literal.
        let mut basis = o.clone();
        basis.sort_by(|a, b| a.id.cmp(&b.id));
        expected["basis"] =
            serde_json::to_value(basis.into_iter().map(|o| o.view(50)).collect::<Vec<_>>())
                .unwrap();
        assert_eq!(actual, expected);
    }
}

fn set_field(objects: &mut [Object], key: &str, field: &str, value: Value) {
    objects
        .iter_mut()
        .find(|o| o.id.native_key == key)
        .unwrap()
        .observations[0]
        .fields
        .insert(field.into(), Knowledge::Value(value));
}

#[test]
fn two_numerators_bind_supported_definition_across_the_whole_series() {
    let (mut o, l) = graph(2);
    let Knowledge::Value(d) = o[1].observations[0].fields.get_mut("definition").unwrap() else {
        panic!()
    };
    d["formula"] = "supported / total".into();
    for (key, n) in [("start", 10), ("middle", 11), ("end", 12)] {
        set_field(&mut o, key, "supported", json!(n));
    }
    let r = report(&o, &l, 10, 20);
    let m = &r["metrics"][0];
    assert_eq!(m["samples"][0]["value"], rational("10", 17));
    assert_eq!(m["samples"][2]["value"], rational("12", 17));
    assert_eq!(m["change"], rational("2", 17));
    assert_eq!(m["source_qualified"], true);
    // The v3 Service selector must use the same binding.
    let v: Value = serde_json::from_str(&render(
        &o,
        &l,
        policy(&o, &[], true),
        10,
        20,
        Some(Version::V3),
    ))
    .unwrap();
    assert_eq!(
        v["result"]["report"]["metrics"][0]["change"]["value"]["numerator"],
        "2"
    );
}

#[test]
fn numerator_kind_missing_conflicting_and_unsupported_bindings_are_unavailable() {
    let (mut o, l) = graph(2);
    o.iter_mut()
        .find(|o| o.id.native_key == "end")
        .unwrap()
        .observations[0]
        .fields
        .remove("removed");
    set_field(&mut o, "end", "supported", json!(7));
    let r = report(&o, &l, 10, 20);
    assert_eq!(r["metrics"][0]["change"], unavailable("missing_value"));
    assert_eq!(
        r["metrics"][0]["samples"][2]["value"],
        unavailable("missing_value")
    );
    assert_eq!(r["metrics"][0]["source_qualified"], false);
    for (formula, selector, reason) in [
        ("unsupported / total", None, "unsupported_fraction_binding"),
        (
            "removed / total",
            Some("kpi_mcp_parity"),
            "conflicting_fraction_binding",
        ),
    ] {
        let (mut o, l) = graph(2);
        let Knowledge::Value(d) = o[1].observations[0].fields.get_mut("definition").unwrap() else {
            panic!()
        };
        d["formula"] = formula.into();
        if let Some(selector) = selector {
            set_field(&mut o, "legacy", "selector", json!(selector));
        }
        let r = report(&o, &l, 10, 20);
        assert_eq!(r["metrics"][0]["change"], unavailable(reason));
        assert_eq!(r["metrics"][0]["source_qualified"], false);
    }
    // No explicit definition or recognized selector establishes no binding.
    let (mut o, l) = graph(2);
    o[1].observations[0].fields.remove("definition");
    assert_eq!(
        report(&o, &l, 10, 20)["metrics"][0]["change"],
        unavailable("missing_definition")
    );
    // Flat rows with several candidate fields cannot establish a unique rule.
    let (mut o, _) = graph(0);
    o.retain(|o| o.id.object_type != "metric" && o.id.native_key != "attribution");
    let mut links = vec![];
    for key in ["start", "middle", "end"] {
        set_field(&mut o, key, "supported", json!(10));
        links.push(edge(
            o[0].id.clone(),
            "has_measurement",
            id("measurement", key, "owner"),
        ));
    }
    for m in report(&o, &links, 10, 20)["metrics"].as_array().unwrap() {
        assert_eq!(m["change"], unavailable("conflicting_fraction_binding"));
        assert_eq!(m["source_qualified"], false);
    }
}

#[test]
fn billing_requires_explicit_true_marker_on_every_sample() {
    for marker in [
        None,
        Some(Value::Null),
        Some(json!("true")),
        Some(json!(1)),
        Some(json!({})),
        Some(json!(false)),
        Some(json!(true)),
    ] {
        for marker_key in ["bill-a", "bill-b"] {
            let (mut o, mut l) = graph(0);
            for (key, at, n) in [("bill-a", 10, 302), ("bill-b", 20, 335)] {
                let b = object(
                    "guardrail",
                    key,
                    "billing",
                    at,
                    json!({"guardrail":"billing","unit":"linux_minutes","account":"test-account","window_start":0,"window_end":100,"value":n,"source_qualified":true,"qualification":"qualified","bound_satisfied":true}),
                );
                l.push(edge(o[0].id.clone(), "guardrail", b.id.clone()));
                o.push(b);
            }
            let fields = &mut o
                .iter_mut()
                .find(|o| o.id.native_key == marker_key)
                .unwrap()
                .observations[0]
                .fields;
            fields.remove("source_qualified");
            if let Some(v) = &marker {
                fields.insert("source_qualified".into(), Knowledge::Value(v.clone()));
            }
            let r = report(&o, &l, 10, 20);
            let m = r["metrics"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["kind"] == "billing")
                .unwrap();
            assert_eq!(m["change"], rational("33", 1));
            let expected = marker == Some(json!(true));
            assert_eq!(m["source_qualified"], expected);
            let sample = m["samples"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["basis"]["id"]["native_key"] == marker_key)
                .unwrap();
            assert_eq!(sample["source_qualified"], expected);
            let native = r["guardrails"]
                .as_array()
                .unwrap()
                .iter()
                .find(|g| g["id"]["native_key"] == marker_key)
                .unwrap();
            assert_eq!(sample["source_qualified"], native["source_qualified"]);
        }
    }
}

#[test]
fn attribution_source_qualification_does_not_claim_reconciliation() {
    let (o, l) = graph(1);
    let r = report(&o, &l, 10, 20);
    let a = &r["attributions"][0];
    assert_eq!(a["source_qualified"], true);
    assert_eq!(a["qualification"], "unresolved_reconciliation");
    assert_eq!(a["reconciliation_residual"], rational("3", 17));
    assert_eq!(a["endpoints"], unavailable("unbound_endpoints"));
}
