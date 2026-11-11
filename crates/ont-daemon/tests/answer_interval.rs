//! FIX-1: synthetic graph adapted from independent review; no source acquisition.
#![allow(clippy::unwrap_used)]
use ont_core::{
    Authority, Change, Coverage, Grant, Id, Knowledge, Lineage, Link, Namespace, Object,
    Observation, Policy, Query, Scope, Session, answer::Version, report::ReportWindow,
};
use ont_daemon::Service;
use ont_store::Store;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
fn id(kind: &str, key: &str, account: &str) -> Id {
    Id {
        object_type: kind.into(),
        namespace: Namespace {
            instance: "review://pr16-service".into(),
            account: account.into(),
        },
        native_key: key.into(),
    }
}
fn object(kind: &str, key: &str, account: &str, at: u64, fields: Value) -> Object {
    let id = id(kind, key, account);
    Object {
        scope: Scope::program(id.namespace.clone(), "p16").unwrap(),
        id,
        head: "r16".into(),
        observations: vec![Observation {
            revision: "r16".into(),
            source_revision: "synthetic-review16".into(),
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
                pointer: "/review16".into(),
            },
        }],
    }
}
fn link(source: Id, relation: &str, target: Id) -> Link {
    Link {
        source,
        relation: relation.into(),
        target,
    }
}
fn graph() -> (Vec<Object>, Vec<Link>) {
    let mut objects = vec![
        object("program", "root", "owner", 0, json!({"program_key":"p16"})),
        object(
            "metric",
            "m",
            "owner",
            0,
            json!({"definition":{"id":id("metric","m","owner"),"revision":"d16","formula":"absent / adopted","population":"review16","unit":"fraction","direction":"increase","role":{"kind":"indicator"}}}),
        ),
    ];
    let mut links = vec![link(
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
            json!({"removed":n,"total":17,"source_unit":"fraction","definition_revision":"d16","manifest_sha256":"m16","algorithm":"exact_utf8_paths_v1"}),
        );
        links.push(link(objects[1].id.clone(), "has_measurement", o.id.clone()));
        objects.push(o);
    }
    let mut cs = vec![];
    for (account, n) in [("repo-a", 2), ("repo-b", 3)] {
        let repo = object("repository", "repository", account, 0, json!({"value":n}));
        let change = object("change", "change", account, 0, json!({"value":n}));
        links.push(link(objects[0].id.clone(), "repository", repo.id.clone()));
        links.push(link(
            id("measurement", "attribution", "owner"),
            "supported_by",
            change.id.clone(),
        ));
        links.push(link(change.id.clone(), "in_repository", repo.id.clone()));
        cs.push(json!({"repository":repo.id,"commit":format!("{account}-commit"),"first_parent":"parent","newly_absent_count":n,"restored_count":0,"path_set_sha256":"digest","paths":n.to_string(),"association":"unique","changes":[change.id]}));
        objects.push(repo);
        objects.push(change);
    }
    let a = object(
        "measurement",
        "attribution",
        "owner",
        20,
        json!({"attribution":{"rule":"attribution-v1","adapter":"synthetic-review16","definition":{"id":id("metric","m","owner"),"revision":"d16"},"manifest_sha256":"m16","population_adapter_revision":"review16","algorithm":"exact_utf8_paths_v1","denominator":17,"delta_bound":"5","residual":"0","qualification":"qualified","gaps":[],"contributions":cs}}),
    );
    links.push(link(objects[0].id.clone(), "has_measurement", a.id.clone()));
    objects.push(a);
    (objects, links)
}
fn policy(objects: &[Object], mask: u32, extra: bool) -> Policy {
    let mut grants: Vec<_> = objects
        .iter()
        .filter(|o| {
            o.id.namespace.account == "owner"
                || (mask
                    >> (if o.id.namespace.account == "repo-a" {
                        0
                    } else {
                        1
                    }))
                    & 1
                    == 1
        })
        .map(|o| Grant {
            scope: o.scope.clone(),
            object_types: BTreeSet::from([o.id.object_type.clone()]),
            read: true,
            derive: true,
            actions: BTreeSet::new(),
        })
        .collect();
    if extra {
        grants.push(Grant {
            scope: Scope::program(
                Namespace {
                    instance: "review://unrelated".into(),
                    account: "absent".into(),
                },
                "unrelated",
            )
            .unwrap(),
            object_types: BTreeSet::from(["absent".into()]),
            read: true,
            derive: true,
            actions: BTreeSet::new(),
        });
    }
    Policy {
        principals: BTreeMap::from([("reviewer".into(), grants)]),
    }
}
fn service(objects: &[Object], links: &[Link], policy: Policy) -> (Service, Session) {
    let mut s = Store::memory_program().unwrap();
    s.seed(objects, links).unwrap();
    let a = Authority::new(policy);
    let session = a.bind_trusted("reviewer").unwrap();
    (Service::new(s, a), session)
}
fn render(objects: &[Object], links: &[Link], p: Policy, q: Query) -> (String, bool) {
    let (mut s, session) = service(objects, links, p);
    s.render_answer_record_version_with_status(&session, q, 50, Version::V2)
        .unwrap()
}

fn answer(objects: &[Object], links: &[Link], mask: u32, start: u64, end: u64) -> Value {
    let (bytes, refused) = render(
        objects,
        links,
        policy(objects, mask, false),
        Query::ProgramReport {
            root: objects[0].id.clone(),
            window: ReportWindow { start, end },
        },
    );
    assert!(!refused);
    serde_json::from_str(&bytes).unwrap()
}
fn assert_native_change(a: &Value, n: &str, residual: &str) {
    let aggregate = &a["aggregate"];
    assert_eq!(
        aggregate["endpoints"],
        json!({"status":"not_available","reason":"unbound_endpoints","native":null})
    );
    assert_eq!(a["attribution"]["delta_bound"], n);
    assert_eq!(aggregate["count_change"], n);
    assert_eq!(aggregate["ratio_change"]["numerator"], n);
    assert_eq!(aggregate["ratio_change"]["denominator"], 17);
    assert_eq!(aggregate["component_contribution_sum"]["numerator"], "5");
    assert_eq!(aggregate["reconciliation_residual"]["numerator"], residual);
    assert_eq!(aggregate["reconciliation_residual"]["denominator"], 17);
}
#[test]
fn full_and_narrow_review_intervals_never_attach_population_only_endpoints() {
    let (objects, links) = graph();
    for (start, count, delta) in [(10, "2", "5"), (15, "4", "3")] {
        let v = answer(&objects, &links, 3, start, 20);
        let r = &v["result"]["report"];
        assert_eq!(r["metrics"][0]["aggregate"]["start_count"], count);
        assert_eq!(r["metrics"][0]["aggregate"]["end_count"], "7");
        assert_eq!(r["metrics"][0]["aggregate"]["count_change"], delta);
        assert_native_change(&r["attributions"][0], "5", "0");
    }
}
fn variants(two_metrics: bool) -> (Vec<Object>, Vec<Link>) {
    let (mut objects, mut links) = graph();
    // Equal start values and equal shifted end values still have different IDs/times.
    for (key, at, n) in [("outer", 0, 0), ("same-start", 5, 2), ("same-end", 25, 7)] {
        let o = object(
            "measurement",
            key,
            "owner",
            at,
            json!({"removed":n,"total":17,"source_unit":"fraction","definition_revision":"d16","manifest_sha256":"m16","algorithm":"exact_utf8_paths_v1"}),
        );
        links.push(link(
            id("metric", "m", "owner"),
            "has_measurement",
            o.id.clone(),
        ));
        objects.push(o);
    }
    let mut a = objects
        .iter()
        .find(|o| o.id == id("measurement", "attribution", "owner"))
        .unwrap()
        .clone();
    a.id.native_key = "second-attribution".into();
    if let Knowledge::Value(v) = a.observations[0].fields.get_mut("attribution").unwrap() {
        v["delta_bound"] = "3".into();
        v["residual"] = "-2".into();
    }
    links.push(link(objects[0].id.clone(), "has_measurement", a.id.clone()));
    for account in ["repo-a", "repo-b"] {
        links.push(link(
            a.id.clone(),
            "supported_by",
            id("change", "change", account),
        ));
    }
    objects.push(a);
    if two_metrics {
        let mut m = objects[1].clone();
        m.id.native_key = "same-population-metric".into();
        links.push(link(
            objects[0].id.clone(),
            "tracks_indicator",
            m.id.clone(),
        ));
        for edge in links.clone() {
            if edge.source == objects[1].id && edge.relation == "has_measurement" {
                links.push(link(m.id.clone(), "has_measurement", edge.target));
            }
        }
        objects.push(m);
    }
    (objects, links)
}
#[test]
fn interval_variants_equal_deltas_and_multiple_metrics_keep_attributions_independent() {
    for two_metrics in [false, true] {
        let (objects, links) = variants(two_metrics);
        // Full, sub, super, shifted, equal-value/different-identity and single endpoint.
        for (start, end) in [
            (10, 20),
            (15, 20),
            (0, 25),
            (15, 25),
            (5, 20),
            (20, 20),
            (20, 25),
        ] {
            let v = answer(&objects, &links, 3, start, end);
            let r = &v["result"]["report"];
            assert_eq!(
                r["metrics"].as_array().unwrap().len(),
                if two_metrics { 2 } else { 1 }
            );
            assert_eq!(r["attributions"].as_array().unwrap().len(), 2);
            for a in r["attributions"].as_array().unwrap() {
                let second = a["id"]["native_key"] == "second-attribution";
                assert_native_change(
                    a,
                    if second { "3" } else { "5" },
                    if second { "-2" } else { "0" },
                );
            }
        }
    }
}
#[test]
fn interval_variants_full_reduced_and_hidden_mutations_are_byte_identical() {
    for two_metrics in [false, true] {
        let (objects, links) = variants(two_metrics);
        for mask in 0..4 {
            let p = policy(&objects, mask, false);
            let visible = |o: &Object| {
                p.principals["reviewer"].iter().any(|g| {
                    g.scope == o.scope && g.read && g.object_types.contains(&o.id.object_type)
                })
            };
            let reduced: Vec<_> = objects.iter().filter(|o| visible(o)).cloned().collect();
            let ids: BTreeSet<_> = reduced.iter().map(|o| o.id.clone()).collect();
            let reduced_links: Vec<_> = links
                .iter()
                .filter(|l| ids.contains(&l.source) && ids.contains(&l.target))
                .cloned()
                .collect();
            for (start, end) in [
                (10, 20),
                (15, 20),
                (0, 25),
                (15, 25),
                (5, 20),
                (20, 20),
                (20, 25),
            ] {
                let q = Query::ProgramReport {
                    root: objects[0].id.clone(),
                    window: ReportWindow { start, end },
                };
                let expected = render(&reduced, &reduced_links, p.clone(), q.clone());
                assert_eq!(expected, render(&objects, &links, p.clone(), q.clone()));
                assert_eq!(
                    expected,
                    render(&objects, &links, policy(&objects, mask, true), q.clone())
                );
                let v: Value = serde_json::from_str(&expected.0).unwrap();
                for a in v["result"]["report"]["attributions"].as_array().unwrap() {
                    assert_eq!(a.get("aggregate").is_some(), mask == 3);
                }
                for variant in 0..3 {
                    let mut changed = objects.clone();
                    for o in &mut changed {
                        if !visible(o) {
                            let h = &mut o.observations[0];
                            h.observed_at = variant;
                            h.fresh_until = 100 + variant;
                            h.fields.insert(
                                "withheld".into(),
                                Knowledge::Value(json!(u64::MAX - variant)),
                            );
                        }
                    }
                    assert_eq!(expected, render(&changed, &links, p.clone(), q.clone()));
                }
            }
        }
    }
}
