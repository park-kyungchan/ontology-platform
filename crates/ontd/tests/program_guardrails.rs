#![allow(clippy::unwrap_used)]
//! Synthetic sanitized owner recordings only; no source or network operations.
use ont_core::program::qualify_basis;
use ont_core::{Authority, Coverage, Grant, Id, Knowledge, Link, Namespace, Policy, Query, Scope};
use ont_store::Store;
use ontd::program_guardrails::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const AT: u64 = 1_793_836_800; // 2026-11-05 UTC
const MONDAY: u64 = AT - 3 * 86400;

fn billing() -> Contract {
    Contract::Billing {
        account: "synthetic-account".into(),
        month: UtcMonth {
            year: 2026,
            month: 11,
        },
        linux_skus: BTreeSet::from(["synthetic-linux".into()]),
        limit: 3000,
    }
}
fn disk(minimum: DiskMinimum) -> Contract {
    Contract::Disk { minimum }
}
fn exact_disk() -> Contract {
    disk(DiskMinimum::Exact {
        quantity: 15,
        unit: DiskUnit::Gib,
    })
}
fn recall() -> Contract {
    Contract::Recall {
        baseline: RecallBaseline {
            supported: 15,
            total: 33,
            evaluation_set_sha256: "a".repeat(64),
            result_revision: "synthetic-baseline-v1".into(),
            artifact_sha256: "b".repeat(64),
        },
    }
}
fn defects() -> Contract {
    Contract::Defects {
        week_start: MONDAY,
        limit: 0,
    }
}
fn recording(outcome: Value) -> Value {
    json!({"source_revision":"owner-synthetic-v1","capture_start":AT,"capture_end":AT,"outcome":outcome})
}
fn usage(n: u64) -> Value {
    json!({"id":"usage-1","sku":"synthetic-linux","unit":"linux_minutes","quantity":n})
}
fn billing_outcome(items: Value) -> Value {
    let (start, end) = UtcMonth {
        year: 2026,
        month: 11,
    }
    .bounds()
    .unwrap();
    json!({"kind":"billing","account":"synthetic-account","month":{"year":2026,"month":11},
        "period_start":start,"period_end":end,"cutoff":AT,"settlement_lag_seconds":0,
        "population_complete":true,"pages":[{"number":1,"has_more":false,"items":items}]})
}
fn disk_outcome(n: u64, d: u64, unit: &str) -> Value {
    json!({"kind":"disk","observed_at":AT,"numerator":n,"denominator":d,"unit":unit})
}
fn recall_outcome(n: u64) -> Value {
    json!({"kind":"recall","observed_at":AT,"evaluation_set_sha256":"a".repeat(64),"result_revision":"synthetic-result-v2","supported":n,"total":33})
}
fn defect_outcome(incidents: Value) -> Value {
    json!({"kind":"defects","week_start":MONDAY,"week_end":MONDAY+604800,"cutoff":AT,"population_complete":true,"incidents":incidents})
}
fn capture(value: Value, contract: &Contract, now: u64) -> QualifiedCapture {
    qualify_recording(Some(&serde_json::to_vec(&value).unwrap()), contract, now).unwrap()
}
fn field(c: &QualifiedCapture, key: &str) -> Knowledge {
    c.observation().fields[key].clone()
}
fn value(v: impl Into<Value>) -> Knowledge {
    Knowledge::Value(v.into())
}
fn qualified(c: &QualifiedCapture, expected: u64, bound: bool) {
    assert_eq!(field(c, "source_qualified"), value(true));
    assert_eq!(field(c, "value"), value(expected));
    assert_eq!(field(c, "bound_satisfied"), value(bound));
    assert_eq!(c.observation().coverage, Coverage::Complete);
}
fn unresolved(c: &QualifiedCapture, reason: &str) {
    assert_eq!(field(c, "source_qualified"), value(false));
    assert_eq!(field(c, "value"), Knowledge::Unknown);
    assert_eq!(field(c, "bound_satisfied"), Knowledge::Unknown);
    assert_eq!(field(c, "qualification"), value(reason));
    assert_eq!(c.observation().coverage, Coverage::Partial);
}

#[test]
fn successful_empty_billing_period_is_zero_but_absent_failed_and_unattempted_are_not() {
    qualified(
        &capture(recording(billing_outcome(json!([]))), &billing(), AT),
        0,
        true,
    );
    unresolved(
        &qualify_recording(None, &billing(), AT).unwrap(),
        "missing_source",
    );
    unresolved(
        &capture(recording(json!({"kind":"failed"})), &billing(), AT),
        "failed_source",
    );
    let unattempted = capture(recording(json!({"kind":"not_performed"})), &billing(), AT);
    assert_eq!(field(&unattempted, "value"), Knowledge::NotPerformed);
    assert_eq!(
        field(&unattempted, "bound_satisfied"),
        Knowledge::NotPerformed
    );
    assert_eq!(field(&unattempted, "source_qualified"), value(false));
}

#[test]
fn billing_exact_sum_and_limit_inclusive_crossing() {
    for n in [2999, 3000, 3001] {
        let c = capture(
            recording(billing_outcome(json!([usage(n)]))),
            &billing(),
            AT,
        );
        qualified(&c, n, n <= 3000);
        assert_eq!(field(&c, "unit"), value("linux_minutes"));
        assert_eq!(field(&c, "bound_value"), value(3000));
    }
}

#[test]
fn billing_pagination_requires_order_terminal_page_and_population() {
    let mut source = billing_outcome(json!([usage(12)]));
    source["pages"][0]["has_more"] = json!(true);
    unresolved(
        &capture(recording(source.clone()), &billing(), AT),
        "incomplete_source",
    );
    source["pages"].as_array_mut().unwrap().push(json!({"number":2,"has_more":false,"items":[{"id":"usage-2","sku":"synthetic-linux","unit":"linux_minutes","quantity":8}]}));
    qualified(
        &capture(recording(source.clone()), &billing(), AT),
        20,
        true,
    );
    source["pages"][1]["number"] = json!(3);
    unresolved(
        &capture(recording(source), &billing(), AT),
        "invalid_source",
    );
    let mut source = billing_outcome(json!([]));
    source["pages"] = json!([]);
    unresolved(
        &capture(recording(source), &billing(), AT),
        "incomplete_source",
    );
    let mut source = billing_outcome(json!([]));
    source["population_complete"] = json!(false);
    unresolved(
        &capture(recording(source), &billing(), AT),
        "incomplete_source",
    );
}

#[test]
fn billing_rejects_wrong_account_month_period_sku_unit_duplicate_and_overflow() {
    for (pointer, replacement) in [
        ("/account", json!("foreign")),
        ("/month/month", json!(12)),
        ("/period_start", json!(AT)),
        ("/period_end", json!(AT)),
        ("/pages/0/items/0/sku", json!("unknown-linux")),
        ("/pages/0/items/0/unit", json!("credits")),
        ("/pages/0/items/0/unit", json!("job_wall_minutes")),
        ("/pages/0/items/0/quantity", json!(-1)),
        ("/pages/0/items/0/quantity", json!(1.5)),
        ("/pages/0/items/0/quantity", json!("0")),
        ("/pages/0/items/0/quantity", json!(true)),
        ("/pages/0/items/0/quantity", Value::Null),
    ] {
        let mut source = billing_outcome(json!([usage(12)]));
        *source.pointer_mut(pointer).unwrap() = replacement;
        unresolved(
            &capture(recording(source), &billing(), AT),
            "invalid_source",
        );
    }
    unresolved(
        &capture(
            recording(billing_outcome(json!([usage(12), usage(12)]))),
            &billing(),
            AT,
        ),
        "invalid_source",
    );
    let mut second = usage(1);
    second["id"] = json!("usage-2");
    unresolved(
        &capture(
            recording(billing_outcome(json!([usage(u64::MAX), second]))),
            &billing(),
            AT,
        ),
        "invalid_source",
    );
}

#[test]
fn billing_settlement_lag_retains_recorded_quantity_without_qualified_bound() {
    for lag in [Value::Null, json!(1), json!(86400)] {
        let mut source = billing_outcome(json!([usage(12)]));
        source["settlement_lag_seconds"] = lag;
        let c = capture(recording(source), &billing(), AT);
        unresolved(&c, "settlement_lag");
        assert_eq!(field(&c, "recorded_value"), value(12));
    }
}

#[test]
fn billing_utc_month_covers_leap_century_year_boundary_and_invalid_months() {
    let duration = |year, month| {
        let (s, e) = UtcMonth { year, month }.bounds().unwrap();
        (e - s) / 86400
    };
    assert_eq!(duration(2024, 2), 29);
    assert_eq!(duration(2100, 2), 28);
    assert_eq!(duration(2000, 2), 29);
    assert_eq!(duration(2026, 11), 30);
    assert_eq!(duration(2026, 12), 31);
    assert_eq!(
        UtcMonth {
            year: 2026,
            month: 12
        }
        .bounds()
        .unwrap()
        .1,
        UtcMonth {
            year: 2027,
            month: 1
        }
        .bounds()
        .unwrap()
        .0
    );
    for month in [0, 13] {
        assert!(UtcMonth { year: 2026, month }.bounds().is_err());
    }
    assert!(
        UtcMonth {
            year: 1969,
            month: 12
        }
        .bounds()
        .is_err()
    );
}

#[test]
fn disk_exact_source_conversion_and_inclusive_threshold_do_not_use_floats() {
    let bound = 15_u64 << 30;
    for n in [bound - 1, bound, bound + 1] {
        qualified(
            &capture(recording(disk_outcome(n, 1, "bytes")), &exact_disk(), AT),
            n,
            n >= bound,
        );
    }
    qualified(
        &capture(recording(disk_outcome(16, 1, "gib")), &exact_disk(), AT),
        16 << 30,
        true,
    );
    qualified(
        &capture(recording(disk_outcome(155, 10, "gb")), &exact_disk(), AT),
        15_500_000_000,
        false,
    );
    for source in [
        disk_outcome(1, 0, "gb"),
        disk_outcome(1, 3, "gb"),
        disk_outcome(u64::MAX, 1, "gib"),
        disk_outcome(1, 1, "GB"),
    ] {
        unresolved(
            &capture(recording(source), &exact_disk(), AT),
            "invalid_source",
        );
    }
}

#[test]
fn disk_unresolved_gb_gib_boundary_preserves_meaning_gap() {
    let contract = disk(DiskMinimum::UnresolvedGbOrGib { quantity: 15 });
    for (n, expected) in [
        (14_999_999_999, Some(false)),
        (15_000_000_000, None),
        (15_500_000_000, None),
        ((15 << 30) - 1, None),
        (15 << 30, Some(true)),
    ] {
        let c = capture(recording(disk_outcome(n, 1, "bytes")), &contract, AT);
        assert_eq!(field(&c, "value"), value(n));
        assert_eq!(field(&c, "bound_value"), Knowledge::Unknown);
        assert_eq!(
            field(&c, "bound_satisfied"),
            expected.map_or(Knowledge::Unknown, value)
        );
        assert_eq!(field(&c, "qualification"), value("unresolved_disk_unit"));
        assert_eq!(c.observation().coverage, Coverage::Partial);
        assert!(!qualify_basis(c.observation(), AT).unwrap().is_current());
    }
}

#[test]
fn recall_exact_baseline_comparison_and_provenance_are_required() {
    for n in [14, 15, 16] {
        let c = capture(recording(recall_outcome(n)), &recall(), AT);
        qualified(&c, n, n >= 15);
        assert_eq!(field(&c, "total"), value(33));
        assert_eq!(
            field(&c, "baseline_result_revision"),
            value("synthetic-baseline-v1")
        );
        assert_eq!(field(&c, "baseline_artifact_sha256"), value("b".repeat(64)));
    }
    for (key, v) in [
        ("evaluation_set_sha256", json!("c".repeat(64))),
        ("total", json!(34)),
        ("supported", json!(34)),
        ("total", json!(0)),
        ("result_revision", json!("")),
    ] {
        let mut source = recall_outcome(15);
        source[key] = v;
        let c = capture(recording(source), &recall(), AT);
        unresolved(&c, "invalid_source");
        assert_eq!(field(&c, "total"), Knowledge::Unknown);
    }
}

#[test]
fn hardcoded_recall_defect_and_billing_assertions_never_become_executed_sources() {
    for (contract, n, total) in [
        (recall(), 15, Some(33)),
        (defects(), 1, None),
        (billing(), 302, None),
    ] {
        let c = capture(
            recording(json!({"kind":"baseline","observed_at":AT,"value":n,"total":total})),
            &contract,
            AT,
        );
        unresolved(&c, "unverified_baseline");
        assert_eq!(field(&c, "recorded_value"), value(n));
    }
}

#[test]
fn fixed_incident_still_counts_as_escaped_in_the_week() {
    let source = defect_outcome(json!([{"id":"incident-1","escaped_at":MONDAY+1,"fixed_at":AT-1}]));
    let c = capture(recording(source), &defects(), AT);
    qualified(&c, 1, false);
    assert_eq!(field(&c, "fixed_count"), value(1));
    qualified(
        &capture(recording(defect_outcome(json!([]))), &defects(), AT),
        0,
        true,
    );
}

#[test]
fn defects_require_exact_week_complete_population_and_consistent_incidents() {
    let good = json!({"id":"incident-1","escaped_at":MONDAY+1,"fixed_at":AT-1});
    for (pointer, replacement) in [
        ("/week_start", json!(MONDAY + 1)),
        ("/week_end", json!(MONDAY + 1)),
        ("/incidents/0/escaped_at", json!(MONDAY - 1)),
        ("/incidents/0/escaped_at", json!(AT)),
        ("/incidents/0/fixed_at", json!(MONDAY)),
        ("/incidents/0/fixed_at", json!(AT + 1)),
    ] {
        let mut source = defect_outcome(json!([good.clone()]));
        *source.pointer_mut(pointer).unwrap() = replacement;
        unresolved(
            &capture(recording(source), &defects(), AT),
            "invalid_source",
        );
    }
    unresolved(
        &capture(
            recording(defect_outcome(json!([good.clone(), good]))),
            &defects(),
            AT,
        ),
        "invalid_source",
    );
    let mut source = defect_outcome(json!([]));
    source["population_complete"] = json!(false);
    unresolved(
        &capture(recording(source), &defects(), AT),
        "incomplete_source",
    );
}

#[test]
fn clock_expiry_equality_future_and_reimport_are_source_based_for_each_kind() {
    for (contract, source, budget) in [
        (billing(), billing_outcome(json!([])), 86400),
        (exact_disk(), disk_outcome(16, 1, "gib"), 900),
        (recall(), recall_outcome(16), 3600),
        (defects(), defect_outcome(json!([])), 3600),
    ] {
        let c = capture(recording(source.clone()), &contract, AT + budget);
        assert!(
            qualify_basis(c.observation(), AT + budget)
                .unwrap()
                .is_current()
        );
        unresolved(
            &capture(recording(source.clone()), &contract, AT + budget + 1),
            "stale_source",
        );
        assert!(
            !qualify_basis(c.observation(), AT + budget + 1)
                .unwrap()
                .is_current()
        );
        let mut imported = recording(source.clone());
        imported["capture_start"] = json!(AT + budget + 1);
        imported["capture_end"] = json!(AT + budget + 1);
        let old = capture(imported, &contract, AT + budget + 1);
        unresolved(&old, "stale_source");
        assert_eq!(old.observation().observed_at, AT);
        let mut future = recording(source);
        future["capture_end"] = json!(AT + 1);
        unresolved(&capture(future, &contract, AT), "future_source");
    }
}

#[test]
fn future_measurement_time_does_not_hide_under_earlier_capture_time() {
    let mut source = disk_outcome(16, 1, "gib");
    source["observed_at"] = json!(AT + 1);
    unresolved(
        &capture(recording(source), &exact_disk(), AT),
        "future_source",
    );
    let mut source = recording(disk_outcome(16, 1, "gib"));
    source["capture_start"] = json!(AT + 1);
    unresolved(&capture(source, &exact_disk(), AT), "invalid_source");
}

#[test]
fn strict_recordings_reject_unknown_payload_fields_duplicates_truncation_and_size() {
    let bytes = serde_json::to_vec(&recording(billing_outcome(json!([])))).unwrap();
    let mut extra = recording(billing_outcome(json!([])));
    extra["credential"] = json!("SYNTHETIC_OMITTED_MARKER");
    let mut nested = recording(billing_outcome(json!([usage(1)])));
    nested["outcome"]["pages"][0]["items"][0]["raw_response"] = json!("SYNTHETIC_OMITTED_MARKER");
    for invalid in [serde_json::to_vec(&extra).unwrap(),serde_json::to_vec(&nested).unwrap(),
        bytes[..bytes.len()-1].to_vec(),[bytes.clone(),b" trailing".to_vec()].concat(),vec![b' ';1024*1024+1],
        br#"{"source_revision":"a","source_revision":"b","capture_start":0,"capture_end":0,"outcome":{"kind":"failed"}}"#.to_vec(),
        br#"{"source_revision":"a","capture_start":0,"capture_end":0,"outcome":{"kind":"failed","kind":"not_performed"}}"#.to_vec()] {
        let c=qualify_recording(Some(&invalid),&billing(),AT).unwrap();
        unresolved(&c,"invalid_source");
        assert!(!String::from_utf8_lossy(c.envelope_bytes()).contains("SYNTHETIC_OMITTED_MARKER"));
    }
}

#[test]
fn content_binding_replay_and_contract_change_are_exact_and_deterministic() {
    let input = recording(billing_outcome(json!([usage(12)])));
    let first = capture(input.clone(), &billing(), AT);
    let replay = capture(input.clone(), &billing(), AT);
    assert_eq!(first.envelope_bytes(), replay.envelope_bytes());
    assert_eq!(first.observation(), replay.observation());
    let envelope: Value = serde_json::from_slice(first.envelope_bytes()).unwrap();
    let fields = serde_json::to_value(&first.observation().fields).unwrap();
    assert_eq!(
        envelope.pointer(&first.observation().lineage.pointer),
        Some(&fields)
    );
    assert_eq!(
        first.observation().lineage.artifact_sha256,
        format!("{:x}", Sha256::digest(first.envelope_bytes()))
    );
    assert_eq!(
        envelope["source_sha256"],
        json!(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&input).unwrap())
        ))
    );
    let different = Contract::Billing {
        account: "synthetic-account".into(),
        month: UtcMonth {
            year: 2026,
            month: 11,
        },
        linux_skus: BTreeSet::from(["synthetic-linux".into()]),
        limit: 10,
    };
    let changed = capture(input, &different, AT);
    assert_ne!(first.observation().revision, changed.observation().revision);
    qualified(&changed, 12, false);
}

#[test]
fn invalid_contracts_refuse_without_payload_and_no_model_debug_leaks() {
    let mut bad_recall = recall();
    if let Contract::Recall { baseline } = &mut bad_recall {
        baseline.artifact_sha256 = "SYNTHETIC_OMITTED_MARKER".into();
    }
    for contract in [
        bad_recall,
        Contract::Defects {
            week_start: MONDAY + 1,
            limit: 0,
        },
        disk(DiskMinimum::Exact {
            quantity: u64::MAX,
            unit: DiskUnit::Gib,
        }),
    ] {
        let error = qualify_recording(None, &contract, AT).err().unwrap();
        assert_eq!(error, IntakeError::InvalidContract);
        assert_eq!(error.to_string(), "invalid_guardrail_contract");
        assert!(!format!("{error:?}").contains("SYNTHETIC_OMITTED_MARKER"));
    }
}

fn id(kind: &str, key: &str) -> Id {
    Id {
        object_type: kind.into(),
        native_key: key.into(),
        namespace: Namespace {
            instance: "synthetic://pr5".into(),
            account: "owner".into(),
        },
    }
}
fn scope() -> Scope {
    Scope::program(id("program", "root").namespace, "pilot").unwrap()
}

#[test]
fn qualified_object_uses_program_scope_without_grant_or_root_construction() {
    let c = capture(recording(disk_outcome(16, 1, "gib")), &exact_disk(), AT);
    let object = c.object(id("guardrail", "disk"), scope()).unwrap();
    let mut store = Store::memory_program().unwrap();
    store.seed(&[object], &[]).unwrap();
    assert!(
        c.object(
            id("guardrail", "disk"),
            Scope::education(id("program", "root").namespace, "C1", "S1").unwrap()
        )
        .is_err()
    );
    let foreign = Scope::program(
        Namespace {
            instance: "synthetic://foreign".into(),
            account: "owner".into(),
        },
        "pilot",
    )
    .unwrap();
    assert!(c.object(id("guardrail", "disk"), foreign).is_err());
}

#[test]
fn independently_granted_guardrail_cannot_affect_public_root_bytes() {
    let root_capture = capture(recording(disk_outcome(16, 1, "gib")), &exact_disk(), AT);
    let root = root_capture
        .object(id("public_metadata", "root"), scope())
        .unwrap();
    let hidden = capture(
        recording(billing_outcome(json!([usage(3001)]))),
        &billing(),
        AT,
    )
    .object(id("guardrail", "billing"), scope())
    .unwrap();
    let grant = Grant {
        scope: scope(),
        object_types: BTreeSet::from(["public_metadata".into()]),
        read: true,
        derive: true,
        actions: BTreeSet::new(),
    };
    let mut results = vec![];
    for include_hidden in [false, true] {
        let authority = Authority::new(Policy {
            principals: BTreeMap::from([("worker".into(), vec![grant.clone()])]),
        });
        let session = authority.bind_trusted("worker").unwrap();
        let mut store = Store::memory_program().unwrap();
        let mut objects = vec![root.clone()];
        let mut links = vec![];
        if include_hidden {
            objects.push(hidden.clone());
            links.push(Link {
                source: root.id.clone(),
                relation: "guardrail".into(),
                target: hidden.id.clone(),
            });
        }
        store.seed(&objects, &links).unwrap();
        let mut service = ontd::Service::new(store, authority);
        let mut output = vec![];
        for query in [
            Query::Get {
                id: root.id.clone(),
            },
            Query::Traverse {
                root: root.id.clone(),
                relation: "guardrail".into(),
            },
            Query::Decide {
                root: root.id.clone(),
                relation: "guardrail".into(),
            },
        ] {
            output.push(service.render(&session, query, AT).unwrap());
        }
        assert_eq!(
            service
                .render(
                    &session,
                    Query::Get {
                        id: hidden.id.clone()
                    },
                    AT
                )
                .unwrap_err(),
            ont_core::Error::Unavailable
        );
        results.push(output);
    }
    assert_eq!(results[0], results[1]);
}
