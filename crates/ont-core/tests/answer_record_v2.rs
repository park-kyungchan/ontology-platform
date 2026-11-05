//! Independently specified synthetic v2 arithmetic and publication contracts.
#![allow(clippy::unwrap_used)]
use ont_core::{
    Output, Response,
    action::ActionOutput,
    answer::{AnswerRecord, Version},
};
use serde_json::{Value, json};

fn id(kind: &str, key: &str) -> Value {
    json!({"object_type":kind,"namespace":{"instance":"synthetic","account":"owner"},"native_key":key})
}
fn native() -> Response {
    let repo = |key| json!({"id":id("repository",key),"scope":{"domain":"program","namespace":{"instance":"synthetic","account":"owner"},"program_key":"p"},"head":"r1","observations":[]});
    let sample = |key, at, n| {
        json!({"id":id("measurement",key),"revision":"r1","observed_at":at,"fresh_until":99,
        "source_unit":{"kind":"value","value":"fraction"},"definition_revision":{"kind":"value","value":"d1"},
        "manifest_sha256":{"kind":"value","value":"manifest"},"algorithm":{"kind":"value","value":"paths"},
        "fraction":{"numerator":n,"denominator":10},"source_qualified":true})
    };
    let contribution = |key, n| {
        json!({"repository":id("repository",key),"commit":"c","first_parent":"p","newly_absent_count":n,
        "restored_count":0,"path_set_sha256":"digest","paths":"sha256_only","association":"missing","changes":[]})
    };
    serde_json::from_value(json!({"status":"ok","authorization_revision":1,"result":{"kind":"program_report","report":{
        "window":{"start":10,"end":20},"assessment":"review_required","basis":[repo("a"),repo("b")],
        "metrics":[{"metric":id("metric","m"),"definition":{"kind":"value","value":{"id":id("metric","m"),"revision":"d1"}},
            "samples":[sample("a",10,"1"),sample("b",20,"4")],"delta":{"numerator":"3","denominator":10},
            "percentage_point_delta":{"numerator":"300","denominator":10},"source_qualified":true}],
        "attributions":[{"id":id("measurement","bound"),"revision":"r1","source_qualified":true,"attribution":{
            "rule":"attribution-v1","adapter":"synthetic","definition":{"id":id("metric","m"),"revision":"d1"},
            "manifest_sha256":"manifest","population_adapter_revision":"p1","algorithm":"paths","denominator":10,
            "delta_bound":"3","residual":"0","qualification":"qualified","gaps":[],"contributions":[contribution("a",1),contribution("b",2)]}}],
        "guardrails":[],"guardrail_availability":{"kind":"unknown"},"limitations":["authorized_basis_only"]}}})).unwrap()
}
fn record(r: &Response) -> Value {
    serde_json::to_value(AnswerRecord::from_response_version(r, Version::V2).unwrap()).unwrap()
}
fn report_mut(r: &mut Response) -> &mut ont_core::report::ProgramReport {
    let Response::Ok {
        result: Output::ProgramReport { report },
        ..
    } = r
    else {
        panic!("fixture");
    };
    report
}
fn frac(n: &str, d: u64, values: [&str; 4]) -> Value {
    json!({"numerator":n,"denominator":d,"native":{"numerator":n,"denominator":d},"rounding_mode":"half_away_from_zero",
        "decimal_renderings":{"2":values[0],"3":values[1],"4":values[2],"6":values[3]}})
}
#[test]
fn literal_v2_goldens_for_every_native_result_kind_and_refusal() {
    for (input, golden) in [
        (
            json!({"status":"ok","authorization_revision":1,"result":{"kind":"objects","objects":[]}}),
            include_str!("answer-goldens/v2/objects.json"),
        ),
        (
            json!({"status":"ok","authorization_revision":2,"result":{"kind":"decision","decision":"insufficient_evidence","basis":[]}}),
            include_str!("answer-goldens/v2/decision.json"),
        ),
        (
            json!({"status":"ok","authorization_revision":3,"result":{"kind":"program_report","report":{"window":{"start":10,"end":20},"assessment":"insufficient_evidence","basis":[],"metrics":[],"attributions":[],"guardrails":[],"guardrail_availability":{"kind":"unknown"},"limitations":["authorized_basis_only","not_available_in_authorized_result"]}}}),
            include_str!("answer-goldens/v2/program-report.json"),
        ),
    ] {
        let r: Response = serde_json::from_value(input).unwrap();
        assert_eq!(
            AnswerRecord::from_response_version(&r, Version::V2)
                .unwrap()
                .render()
                .unwrap(),
            golden.trim_end()
        );
    }
    let action: Value = serde_json::from_str(include_str!("answer-goldens/action.json")).unwrap();
    let action: ActionOutput = serde_json::from_value(action["result"]["action"].clone()).unwrap();
    assert_eq!(
        AnswerRecord::from_action_version(&action, Version::V2)
            .unwrap()
            .render()
            .unwrap(),
        include_str!("answer-goldens/v2/action.json").trim_end()
    );
    assert_eq!(
        AnswerRecord::from_error_version(ont_core::Error::RevisionConflict, Version::V2)
            .render()
            .unwrap(),
        include_str!("answer-goldens/v2/revision-conflict.json").trim_end()
    );
}
#[test]
fn complete_aggregate_endpoints_and_explicit_zero_residual() {
    let r = native();
    let v = record(&r);
    let report = &v["result"]["report"];
    let expected = json!({"population":10,"start_count":"1","end_count":"4","count_change":"3",
        "start_ratio":frac("1",10,["0.10","0.100","0.1000","0.100000"]),
        "end_ratio":frac("4",10,["0.40","0.400","0.4000","0.400000"]),
        "ratio_change":frac("3",10,["0.30","0.300","0.3000","0.300000"]),
        "percentage_point_change":frac("300",10,["30.00","30.000","30.0000","30.000000"]),"source_qualified":true});
    assert_eq!(report["metrics"][0]["aggregate"], expected);
    let a = &report["attributions"][0]["aggregate"];
    // Population pins do not prove the attribution's source endpoint identities.
    assert_eq!(
        a["endpoints"],
        json!({"status":"not_available","reason":"unbound_endpoints","native":null})
    );
    assert_eq!(
        a["component_contribution_sum"],
        frac("3", 10, ["0.30", "0.300", "0.3000", "0.300000"])
    );
    assert_eq!(
        a["reconciliation_residual"],
        frac("0", 10, ["0.00", "0.000", "0.0000", "0.000000"])
    );
    assert_eq!(
        report["metrics"][0]["delta"],
        frac("3", 10, ["0.30", "0.300", "0.3000", "0.300000"])
    );
    assert_eq!(record(&r), v);
}
#[test]
fn residual_uses_exact_native_binding_not_native_display_or_a_float() {
    let mut r = native();
    let report = report_mut(&mut r);
    report.attributions[0].attribution.delta_bound = Some("2".into());
    // Deliberately inconsistent source display: v2 must compute, never copy it.
    report.attributions[0].attribution.residual = Some("999".into());
    let v = record(&r);
    assert_eq!(
        v["result"]["report"]["attributions"][0]["aggregate"]["reconciliation_residual"],
        frac("-1", 10, ["-0.10", "-0.100", "-0.1000", "-0.100000"])
    );
    report_mut(&mut r).attributions[0].attribution.delta_bound = Some("4".into());
    assert_eq!(
        record(&r)["result"]["report"]["attributions"][0]["aggregate"]["reconciliation_residual"]["numerator"],
        "1"
    );
}
#[test]
fn missing_empty_and_incomparable_aggregates_never_guess() {
    let mut r = native();
    report_mut(&mut r).metrics[0].samples.clear();
    assert_eq!(
        record(&r)["result"]["report"]["metrics"][0]["aggregate"]["reason"],
        "missing_endpoints"
    );
    let mut r = native();
    report_mut(&mut r).metrics[0].samples[0]
        .fraction
        .as_mut()
        .unwrap()
        .denominator = 0;
    assert_eq!(
        record(&r)["result"]["report"]["metrics"][0]["aggregate"]["reason"],
        "empty_denominator"
    );
    assert_eq!(
        record(&r)["result"]["report"]["metrics"][0]["samples"][0]["fraction"]["reason"],
        "empty_denominator"
    );
    let mut r = native();
    report_mut(&mut r).metrics[0].delta = None;
    assert_eq!(
        record(&r)["result"]["report"]["metrics"][0]["aggregate"]["reason"],
        "incomparable_components"
    );
    let mut r = native();
    report_mut(&mut r).attributions[0].attribution.delta_bound = None;
    assert_eq!(
        record(&r)["result"]["report"]["attributions"][0]["aggregate"]["reconciliation_residual"]["reason"],
        "unbound_aggregate_change"
    );
    let mut r = native();
    report_mut(&mut r).attributions[0]
        .attribution
        .definition
        .revision = "different".into();
    assert_eq!(
        record(&r)["result"]["report"]["attributions"][0]["aggregate"]["endpoints"]["reason"],
        "unbound_endpoints"
    );
}
#[test]
fn rounding_boundaries_at_each_precision_negative_zero_carry_and_extreme_integers() {
    for (n, d, expected) in [
        ("1", 200, ["0.01", "0.005", "0.0050", "0.005000"]),
        ("-1", 200, ["-0.01", "-0.005", "-0.0050", "-0.005000"]),
        ("1", 2000, ["0.00", "0.001", "0.0005", "0.000500"]),
        ("-1", 2000, ["0.00", "-0.001", "-0.0005", "-0.000500"]),
        ("1", 20000, ["0.00", "0.000", "0.0001", "0.000050"]),
        ("-1", 20000, ["0.00", "0.000", "-0.0001", "-0.000050"]),
        ("1", 2000000, ["0.00", "0.000", "0.0000", "0.000001"]),
        ("-1", 2000000, ["0.00", "0.000", "0.0000", "-0.000001"]),
        (
            "19999999",
            20000000,
            ["1.00", "1.000", "1.0000", "1.000000"],
        ),
        (
            "-170141183460469231731687303715884105728",
            1,
            [
                "-170141183460469231731687303715884105728.00",
                "-170141183460469231731687303715884105728.000",
                "-170141183460469231731687303715884105728.0000",
                "-170141183460469231731687303715884105728.000000",
            ],
        ),
    ] {
        let mut r = native();
        let m = &mut report_mut(&mut r).metrics[0];
        m.delta = Some(ont_core::report::ExactFraction {
            numerator: n.into(),
            denominator: d,
        });
        assert_eq!(
            record(&r)["result"]["report"]["metrics"][0]["delta"],
            frac(n, d, expected)
        );
    }
}
#[test]
fn withheld_components_omit_aggregate_without_hidden_reason_or_count() {
    let mut r = native();
    report_mut(&mut r).basis.pop();
    let v = record(&r);
    assert!(
        v["result"]["report"]["attributions"][0]
            .get("aggregate")
            .is_none()
    );
    // A visible native fraction does not acquire fields inside opaque JSON.
    report_mut(&mut r).metrics[0].definition =
        ont_core::Knowledge::Value(json!({"numerator":"1","denominator":2}));
    assert_eq!(
        record(&r)["result"]["report"]["metrics"][0]["definition"],
        json!({"kind":"value","value":{"numerator":"1","denominator":2}})
    );
}

#[test]
fn native_integer_strings_are_lossless_beyond_machine_range() {
    for (n, d, rendered) in [
        ("0001", 2, "0.500000"),
        ("-0000", 1, "0.000000"),
        (
            "99999999999999999999999999999999999999999999999999",
            1,
            "99999999999999999999999999999999999999999999999999.000000",
        ),
        ("18446744073709551614", u64::MAX, "1.000000"),
        ("1", u64::MAX, "0.000000"),
    ] {
        let mut r = native();
        report_mut(&mut r).metrics[0].delta = Some(ont_core::report::ExactFraction {
            numerator: n.into(),
            denominator: d,
        });
        let v = record(&r);
        let delta = &v["result"]["report"]["metrics"][0]["delta"];
        assert_eq!(delta["numerator"], n);
        assert_eq!(delta["native"], json!({"numerator":n,"denominator":d}));
        assert_eq!(delta["decimal_renderings"]["6"], rendered);
    }
}
