#![allow(clippy::unwrap_used)]
use super::*;
use ont_core::program::{DefinitionRef, Direction, MetricRole, qualify_basis};
use ont_core::{Authority, Grant, Policy, Query, Snapshot};

fn contract() -> IntakeContract {
    let namespace = Namespace {
        instance: "synthetic://program".into(),
        account: "owner".into(),
    };
    IntakeContract {
        namespace: namespace.clone(),
        program_key: "pilot".into(),
        producer_sha256: digest(b"synthetic producer; never executed"),
        legacy_definition: MetricDefinition {
            reference: DefinitionRef {
                id: Id {
                    object_type: "metric".into(),
                    namespace,
                    native_key: "legacy".into(),
                },
                revision: "legacy/v1".into(),
            },
            formula: "recorded non-KEEP absence / adopted manifest population".into(),
            population: "synthetic manifest".into(),
            unit: Unit::Fraction,
            direction: Direction::Increase,
            role: MetricRole::Indicator,
        },
    }
}
fn manifest() -> Value {
    json!({"schema_version":1,"repos":{"repo-a":{"head":"a".repeat(40),"tracked_file_count":4,"groups":[
        {"disposition":"DELETE_NOW","count":3,"files":["a.ts","한글.ts","line\nquote\".ts"]},
        {"disposition":"KEEP","count":1,"files":["keep.rs"]}]}},"verification":{"exact_non_keep_removal_paths":3}})
}
fn row() -> Value {
    json!({"measured_at":"2026-11-05T00:21Z",
        "kpi_legacy_removal":{"removed":1,"total":3,"pct":33.3,"per_repo":{"repo-a":[1,3]}},
        "kpi_mcp_parity":{"qualified":0,"total":44,"bounded_implemented":13,"source":"recorded baseline"},
        "guard_ci_minutes_month":{"value":0,"limit":3000},
        "guard_disk_free_gb":{"value":24.5,"min":15},
        "guard_escaped_defects_week":{"value":1,"note":"historical occurrence"},
        "guard_recall_regression":{"supported":15,"total":33,"source":"recorded baseline"}})
}
fn lines(rows: &[Value]) -> Vec<u8> {
    rows.iter()
        .flat_map(|r| {
            let mut b = serde_json::to_vec(r).unwrap();
            b.push(b'\n');
            b
        })
        .collect()
}
fn capture(rows: &[Value]) -> Capture {
    Capture::recorded(
        &lines(rows),
        &serde_json::to_vec(&manifest()).unwrap(),
        b"synthetic producer; never executed",
        100,
        101,
        contract(),
    )
    .unwrap()
}
fn admit_bytes(s: &[u8], m: &Value) -> Result<Capture> {
    Capture::recorded(
        s,
        &serde_json::to_vec(m).unwrap(),
        b"synthetic producer; never executed",
        100,
        101,
        contract(),
    )
}
fn find<'a>(objects: &'a [Object], selector: &str) -> &'a Object {
    objects
        .iter()
        .find(|o| o.id.native_key.ends_with(selector))
        .unwrap()
}
#[test]
fn exact_raw_file_lines_envelope_and_pointer_are_bound() {
    let mut a = row();
    a["measured_at"] = "2026-11-05T00:21:17Z".into();
    let mut b = row();
    b["measured_at"] = "2026-11-05T01:43Z".into();
    let rows = [a, b];
    let bytes = lines(&rows);
    let c = capture(&rows);
    let envelope: Value = serde_json::from_slice(c.envelope_bytes()).unwrap();
    assert_eq!(digest(c.envelope_bytes()), c.envelope_sha256());
    assert_eq!(envelope["source"]["snapshots_sha256"], digest(&bytes));
    let sources = envelope["row_sources"].as_array().unwrap();
    let reconstructed: String = sources
        .iter()
        .map(|s| s["raw_utf8"].as_str().unwrap())
        .collect();
    assert_eq!(reconstructed.as_bytes(), bytes);
    for (i, source) in sources.iter().enumerate() {
        assert_eq!(source["line"], i + 1);
        assert_eq!(
            source["sha256"],
            digest(source["raw_utf8"].as_str().unwrap().as_bytes())
        );
        let parsed: Value = serde_json::from_str(source["raw_utf8"].as_str().unwrap()).unwrap();
        assert_eq!(envelope["rows"][i], parsed);
    }
    for object in c.objects().unwrap() {
        let observation = &object.observations[0];
        assert_eq!(observation.lineage.artifact_sha256, c.envelope_sha256());
        let bound = envelope.pointer(&observation.lineage.pointer).unwrap();
        let selector = object.id.native_key.split_once(':').unwrap().1;
        let source_group = &bound[selector];
        let leaf = if selector == "kpi_legacy_removal" {
            "removed"
        } else if selector == "kpi_mcp_parity" {
            "qualified"
        } else if selector == "guard_recall_regression" {
            "supported"
        } else {
            "value"
        };
        assert_eq!(
            observation.fields[leaf],
            Knowledge::Value(source_group[leaf].clone())
        );
        assert_eq!(
            timestamp(bound["measured_at"].as_str().unwrap()).unwrap(),
            observation.observed_at
        );
    }
    // Raw private-looking paths never appear in retained envelope bytes.
    let envelope_text = std::str::from_utf8(c.envelope_bytes()).unwrap();
    assert!(!envelope_text.contains("한글.ts"));
    assert!(envelope_text.contains(&digest("한글.ts".as_bytes())));
    assert_eq!(
        c.population().repositories.values().next().unwrap().len(),
        3
    );
}
#[test]
fn recapture_and_overlapping_files_replay_without_observation_or_link_growth() {
    let a = row();
    let c = capture(std::slice::from_ref(&a));
    let mut p = Projection::new(&c).unwrap();
    assert_eq!(p.ingest(&c).unwrap(), Replay::Unchanged);
    let mut b = a.clone();
    b["measured_at"] = "2026-11-05T01:43Z".into();
    let next = capture(&[a.clone(), b]);
    assert_eq!(p.ingest(&next).unwrap(), Replay::Added);
    assert_eq!(p.ingest(&next).unwrap(), Replay::Unchanged);
    let recapture = Capture::recorded(
        &lines(&[a]),
        &serde_json::to_vec(&manifest()).unwrap(),
        b"synthetic producer; never executed",
        200,
        201,
        contract(),
    )
    .unwrap();
    assert_eq!(p.ingest(&recapture).unwrap(), Replay::Unchanged);
    let root = p.root().clone();
    let mut store = p.into_store();
    store
        .snapshot(|s| {
            let metrics = s.targets(&root, "tracks_indicator")?;
            assert_eq!(metrics.len(), 6);
            let mut targets = Vec::new();
            for metric in metrics {
                targets.extend(s.targets(&metric, "has_measurement")?);
            }
            assert_eq!(targets.len(), 12);
            for target in targets {
                assert_eq!(s.get(&target)?.unwrap().observations.len(), 1);
            }
            Ok(())
        })
        .unwrap();
}
#[test]
fn different_manifest_does_not_relabel_existing_row_and_failure_is_atomic() {
    let a = row();
    let c = capture(std::slice::from_ref(&a));
    let mut p = Projection::new(&c).unwrap();
    let mut m = manifest();
    m["repos"]["repo-a"]["groups"][0]["files"][0] = "replacement.ts".into();
    let changed = admit_bytes(&lines(&[a]), &m).unwrap();
    assert_eq!(p.ingest(&changed), Err(IntakeError::InvalidData));
    assert_eq!(p.ingest(&c).unwrap(), Replay::Unchanged);
    assert_eq!(p.objects.len(), 6);
}
#[test]
fn absent_null_blank_unknown_not_performed_and_zero_stay_distinct() {
    let mut r = row();
    r.as_object_mut()
        .unwrap()
        .remove("guard_escaped_defects_week");
    r["guard_disk_free_gb"]["value"] = Value::Null;
    r["guard_recall_regression"]["supported"] = "".into();
    r["kpi_mcp_parity"]["qualified"] = json!({"kind":"unknown"});
    r["kpi_legacy_removal"]["removed"] = json!({"kind":"not_performed"});
    let objects = capture(&[r]).objects().unwrap();
    for (selector, field, expected) in [
        ("guard_escaped_defects_week", "value", Knowledge::Missing),
        ("guard_disk_free_gb", "value", Knowledge::Null),
        ("guard_recall_regression", "supported", Knowledge::Blank),
        ("kpi_mcp_parity", "qualified", Knowledge::Unknown),
        ("kpi_legacy_removal", "removed", Knowledge::NotPerformed),
        (
            "guard_ci_minutes_month",
            "value",
            Knowledge::Value(0.into()),
        ),
    ] {
        assert_eq!(
            find(&objects, selector).observations[0].fields[field],
            expected
        );
    }
}
#[test]
fn malformed_duplicate_truncated_empty_and_unrecognized_lines_refuse() {
    let m = manifest();
    for s in [b"".as_slice(),b"{}",b"{}\n\n",b"{\"measured_at\":\"2026-11-05T00:21Z\",\"measured_at\":\"2026-11-05T00:21Z\"}\n",b"[1,2]\n",b"{\"measured_at\":\"2026-11-05T00:21Z\",\"guard_ci_minutes_month\":{\"value\":0,\"value\":1}}\n",b"{}\n{}"] {
        assert!(admit_bytes(s,&m).is_err());
    }
    let r = row();
    let bytes = lines(&[r.clone(), r]);
    assert!(admit_bytes(&bytes, &m).is_err());
    let mut r = row();
    r["arbitrary_path"] = "/ignored".into();
    assert!(admit_bytes(&lines(&[r]), &m).is_err());
    assert_eq!(
        strict::parse(b"{\"unused\":{\"x\":1,\"x\":2}}"),
        Err(IntakeError::InvalidData)
    );
}
#[test]
fn numeric_type_range_denominator_units_and_component_mismatches_refuse() {
    for (pointer, value) in [
        ("/kpi_legacy_removal/removed", json!(-1)),
        ("/kpi_legacy_removal/removed", json!(1.5)),
        ("/kpi_legacy_removal/removed", json!(true)),
        ("/kpi_legacy_removal/removed", json!("1")),
        ("/kpi_legacy_removal/total", json!(0)),
        ("/kpi_legacy_removal/total", json!(4)),
        ("/kpi_legacy_removal/pct", json!(101)),
        ("/kpi_mcp_parity/qualified", json!(45)),
        ("/guard_disk_free_gb/value", json!(-0.1)),
        ("/guard_recall_regression/total", json!(0)),
        ("/kpi_legacy_removal/per_repo/repo-a/0", json!(2)),
        ("/kpi_legacy_removal/per_repo/repo-a/1", json!(4)),
        (
            "/guard_ci_minutes_month/value",
            json!({"kind":"value","value":0}),
        ),
    ] {
        let mut r = row();
        *r.pointer_mut(pointer).unwrap() = value;
        assert!(admit_bytes(&lines(&[r]), &manifest()).is_err(), "{pointer}");
    }
    for selector in SELECTORS {
        let mut r = row();
        r[selector]["unit"] = "wrong".into();
        assert!(admit_bytes(&lines(&[r]), &manifest()).is_err());
    }
    let mut r = row();
    r["guard_disk_free_gb"]["unit"] = "GiB".into();
    assert!(admit_bytes(&lines(&[r]), &manifest()).is_ok());
}
#[test]
fn manifest_unique_population_and_producer_contract_pins_are_required() {
    for (pointer, value) in [
        ("/repos/repo-a/groups/0/files/1", json!("a.ts")),
        ("/repos/repo-a/groups/0/count", json!(2)),
        ("/repos/repo-a/groups/0/disposition", json!("unknown")),
        ("/repos/repo-a/tracked_file_count", json!(99)),
        ("/verification/exact_non_keep_removal_paths", json!(4)),
    ] {
        let mut m = manifest();
        *m.pointer_mut(pointer).unwrap() = value;
        assert!(admit_bytes(&lines(&[row()]), &m).is_err());
    }
    let mut c = contract();
    c.producer_sha256 = "0".repeat(64);
    assert!(
        Capture::recorded(
            &lines(&[row()]),
            &serde_json::to_vec(&manifest()).unwrap(),
            b"synthetic producer; never executed",
            0,
            1,
            c
        )
        .is_err()
    );
    let mut c = contract();
    c.legacy_definition.unit = Unit::Paths;
    assert!(
        Capture::recorded(
            &lines(&[row()]),
            &serde_json::to_vec(&manifest()).unwrap(),
            b"synthetic producer; never executed",
            0,
            1,
            c
        )
        .is_err()
    );
}
#[test]
fn time_validation_future_expiry_and_reimport_preserve_partial_original_evidence() {
    assert_eq!(timestamp("1970-01-01T00:00Z").unwrap(), 0);
    assert_eq!(timestamp("2000-02-29T00:00:00Z").unwrap(), 951782400);
    for time in [
        "2026-02-29T00:00Z",
        "2026-11-05T24:00Z",
        "2026-11-05T00:60Z",
        "2026-11-05T00:00:60Z",
        "2026-11-05T00:00+00:00",
        "2026-00-05T00:00Z",
    ] {
        assert!(timestamp(time).is_err());
    }
    let c = capture(&[row()]);
    for o in c.objects().unwrap() {
        let obs = &o.observations[0];
        let future = qualify_basis(obs, obs.observed_at - 1).unwrap();
        assert!(future.future && future.partial && future.unresolved && !future.is_current());
        assert!(!qualify_basis(obs, obs.fresh_until).unwrap().stale);
        assert!(qualify_basis(obs, obs.fresh_until + 1).unwrap().stale);
        assert_eq!(obs.coverage, Coverage::Partial);
        assert_eq!(obs.fields["original_git_pins"], Knowledge::Unknown);
    }
    let o = c.objects().unwrap();
    assert!(
        find(&o, "guard_disk_free_gb").observations[0].fields["source_unit"].clone()
            == Knowledge::Value(unit("guard_disk_free_gb").into())
    );
}
#[test]
fn streaming_bounds_and_io_failure_never_become_empty_success() {
    assert_eq!(bounded_read(&b"12345"[..], 4), Err(IntakeError::Bounds));
    assert_eq!(bounded_read(&b"1234"[..], 4).unwrap(), b"1234");
    struct Fails;
    impl Read for Fails {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("synthetic failure"))
        }
    }
    assert_eq!(bounded_read(Fails, 4), Err(IntakeError::SourceUnavailable));
    assert_eq!(
        admit_bytes(&vec![b' '; MAX_SNAPSHOTS + 1], &manifest()).err(),
        Some(IntakeError::Bounds)
    );
    assert!(admit_bytes(&[vec![b' '; MAX_LINE], vec![b'\n']].concat(), &manifest()).is_err());
}
#[test]
fn projection_is_program_v2_and_reads_require_existing_service_authorization() {
    let c = capture(&[row()]);
    let p = Projection::new(&c).unwrap();
    let root = p.root().clone();
    let scope = Scope::program(contract().namespace, "pilot").unwrap();
    let grant = Grant {
        scope,
        object_types: BTreeSet::from(["program".into(), "metric".into(), "measurement".into()]),
        read: true,
        derive: true,
        actions: BTreeSet::new(),
    };
    let authority = Authority::new(Policy {
        principals: BTreeMap::from([("owner".into(), vec![grant]), ("worker".into(), vec![])]),
    });
    let session = authority.bind_trusted("owner").unwrap();
    let denied = authority.bind_trusted("worker").unwrap();
    let mut service = crate::Service::new(p.into_store(), authority);
    let allowed = service
        .render(
            &session,
            Query::Traverse {
                root: root.clone(),
                relation: "tracks_indicator".into(),
            },
            0,
        )
        .unwrap();
    assert!(allowed.contains("metric"));
    assert_eq!(
        service.render(&denied, Query::Get { id: root }, 0),
        Err(ont_core::Error::Unavailable)
    );
    let mut education = Store::memory().unwrap();
    assert!(education.seed(&c.objects().unwrap(), &[]).is_err());
}

#[test]
fn explicit_adoption_contract_is_bound_and_field_names_never_adopt_kpis() {
    let ordinary = capture(&[row()]);
    let mut ordinary_store = Projection::new(&ordinary).unwrap().into_store();
    let root = Id {
        object_type: "program".into(),
        namespace: contract().namespace,
        native_key: "pilot".into(),
    };
    ordinary_store
        .snapshot(|s| {
            assert!(s.targets(&root, "adopts_kpi")?.is_empty());
            Ok(())
        })
        .unwrap();
    let mut adopted_contract = contract();
    adopted_contract.legacy_definition.role = MetricRole::Kpi {
        strategy: DefinitionRef {
            id: Id {
                object_type: "strategy".into(),
                namespace: contract().namespace,
                native_key: "adopted".into(),
            },
            revision: "decision-v1".into(),
        },
        adoption: Lineage {
            artifact_sha256: digest(b"synthetic accepted decision"),
            pointer: "/decision".into(),
        },
    };
    let adopted = Capture::recorded(
        &lines(&[row()]),
        &serde_json::to_vec(&manifest()).unwrap(),
        b"synthetic producer; never executed",
        100,
        101,
        adopted_contract,
    )
    .unwrap();
    let envelope: Value = serde_json::from_slice(adopted.envelope_bytes()).unwrap();
    let mut store = Projection::new(&adopted).unwrap().into_store();
    store
        .snapshot(|s| {
            let metrics = s.targets(&root, "adopts_kpi")?;
            assert_eq!(metrics.len(), 1);
            let metric = s.get(&metrics[0])?.unwrap();
            let obs = &metric.observations[0];
            assert_eq!(
                obs.fields["definition"],
                Knowledge::Value(envelope.pointer(&obs.lineage.pointer).unwrap().clone())
            );
            assert_eq!(s.targets(&metrics[0], "has_measurement")?.len(), 1);
            Ok(())
        })
        .unwrap();
}
