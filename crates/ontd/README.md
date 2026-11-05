# ontd

Consumer: harmless local demonstration and process conformance. Service links
ont-core and ont-store; `ontd --synthetic` reads one <=64KiB strict JSON query until
EOF with a fixed synthetic principal/clock. No daemon, socket, credential lookup,
root probe or caller-supplied source path. See repository contract for the narrow
S05 recorded-frame adapter; the copied S05 expectations are input evidence and
are distinct from independent S06 conformance expectations.

`Service::render_action` holds current Session admission through the local fake
effect and serialized output. The separate `ont-local-fake --root ABSOLUTE_PATH`
binary initializes/opens only explicit synthetic files. Its selectable fixture
profiles and administrative `perturb` command are test facilities, not production
authentication or public domain APIs. Default `cargo run -p ontd` still selects
the original ontd binary. S07's separate CLI cannot perform network/Academy calls.

## Program pilot (PR7)

`ont-program-pilot` is a separate one-shot Linux executable. It verifies numeric
real/effective UID 0, requires equality, and binds the fixed `program-owner`
principal. No profile, user name, environment identity, source path, trusted time,
action or listener can be selected through its bounded stdin ProgramReport query.

OS-account principal only; on this host every agent shares this UID, so the CLI does not authenticate a human.

The real-user session binding milestone is **SCOPED-DEVIATION**, not satisfied.
Only the user's explicit acceptance in the Lead conversation establishes human
identity for acceptance; running this CLI or passing tests never increments K1.

The fixed owner config is
`/home/palantirkc/work/ontology-coengineering/pr7-pilot/evidence/owner-config.json`
(directory 0700, file 0600, numeric owner UID 0). Its closed schema is
`ont-program-pilot/v1`: `adopted_uid` (exactly 0), `producer_sha256`,
`manifest_sha256`, `legacy_definition`, `exact_definition`, `repositories`.
Null source pins/definition leave the prepared config explicitly not ready.
Definitions contain `id`, `revision`, `formula`, `population`, and optional
`adoption` (`strategy_id`, `strategy_revision`, `evidence` lineage). They describe
an increasing fraction; an absent adoption leaves an indicator. No adoption is
inferred from a field name. Repository entries contain exact `manifest_key`,
`owner`, `name`, numeric `github_id`, full `before_commit` / `after_commit`, and
`before_observed_at` / `after_observed_at`. Endpoint source measurement binding
remains absent for historical v1. A separate exact-path definition is mandatory
for PR4 reconstruction and cannot turn the v1 basis into a qualified interval.

A separately released live run reads the existing three fixed PR3 sources without
executing the producer, validates the configured source pins, replays/collects PR4
through its allowlisted GET collector, and qualifies exact disk bytes against an
adopted 15 GiB minimum. Billing, recall and defects remain explicitly NotPerformed
without invented account/month/SKU/baseline Contracts. Until sanitized sources
are separately supplied, no all-clear is possible. The separate snapshot-v2
adapter is still required before consuming the new producer format.

All adapters are combined into one freshly seeded **in-memory v2 Program Store**.
A source-independent identity-only root cannot reveal private capture state.
Source observations retain their original v1 identities/lineage. Conflicting
objects, missing link endpoints and invalid lineage refuse; the native report
refuses more than 256 visible objects without truncation. Only the admitted
`Service::render` response is returned, wrapped with the verbatim trust limit,
SCOPED-DEVIATION and `qualified merge contributions + unresolved reconciliation`.
The inclusive query window selects original source time; the actual trusted clock
still marks stale/future data, and later Git/disk captures cannot acquire an old
measurement timestamp. Choose separate explicit query windows for historical
measurements and later reconstruction/guardrails as needed.

Live output/evidence is retained in create-new owner-only `evidence/run-*` files,
with content-addressed sanitized lineage artifacts and request/config/answer
hashes. No database file is created. Evidence retention must finish before stdout
publication; failure leaves a possibly partial directory for owner reconciliation.
No automatic retry/deletion or accepted-answer record is created. Rollback disables
the executable route and additive composition, preserving all retained sources,
answers and existing S06/S07 stores. Step 2 runs only synthetic internal seams and
invalid-input process controls; live acquisition/presentation requires a later
Lead release.

## Typed answer records

`ontd --synthetic --answer-record` and `ont-program-pilot --answer-record`
explicitly select `ontology-answer/v1`. `--help` describes the contract: **prose
answers may only restate record fields**. Defaults retain native output bytes.
The pilot option has the same fixed identity, source acquisition, live-run release
requirement and evidence retention as its default mode. Successful pilot output
keeps its trust/binding/attribution metadata and places the record in `response`;
its refused output is the bare record. Retention hashes the selected output bytes.

The fixed record envelope is `schema`, `outcome`, `authorization_revision`,
`result`. `outcome` is `{status:ok}` or `{status:refused,code:<native enum>}`.
All native Objects, Decision and ProgramReport fields are emitted, including the
complete authorized observation history, native exact numerator/denominator,
counts, booleans, native decimals, qualifications, limitations and attribution.
Object keys inside result are lexical; native array order is retained. Rationals
are not rounded, normalized or inferred; decimals appear only if native supplied
them. Empty arrays remain empty. A native optional null is
`{status:not_available,reason:native_null,native:null}`. Native Knowledge states
missing/null/blank/unknown/not_performed have explicit unavailable cells with that
reason and the original Knowledge in `native`. Known Knowledge payloads, including
null or Knowledge-shaped application JSON, remain verbatim. No more specific
reason is invented where native supplied none.

`ont_core::answer::AnswerRecord::from_response`, `from_error`, `from_action` and
`render` are the typed library entry. `Service::render_answer_record` holds the
existing admission and snapshot through serialization. `from_action` publishes
an already authorized native ActionOutput, including `observed` post-condition;
it never executes/retries an action. `from_outcome` accepts only the fixed native
code vocabulary; an unknown String code publishes only `refused`. Refusals never
include caller IDs, names, namespaces, roots, requests or arbitrary diagnostics.
A failed append's `revision_conflict` is a refused record; an authorized Get or
ProgramReport retains the actual post-attempt head and complete native history.
No record creates extra permissions, source facts, availability counts, decimal
renderings, human authentication or operational acceptance.

New-mode CLI success exits 0; native refused/error outcomes exit 2 even when
record serialization succeeds. `Service::render_answer_record_with_status` and
pilot `execute_answer_record_with_status` carry the native refusal boolean with
the serialized bytes; callers need not parse record text to determine outcome.
The pilot retains those same bytes before publication for either outcome.

Both CLIs read OS arguments fallibly. With `--answer-record`, every invalid or
non-UTF-8 argument yields only `invalid_request`, empty stderr and exit 2,
regardless of argument/help order. Default pilot invalid arguments preserve the
base static diagnostic and exit 2. One explicit compatibility exception removes
a legacy leak: default ontd non-UTF-8 arguments now also yield the code-only
`invalid_request` record and exit 2 instead of a panic containing argument bytes.

Both CLIs use fallible stdout/stderr writes for help, results and diagnostics.
Any failed output write exits 1 without invoking the panic printer. A failed
stdout write attempts the static output-failure diagnostic; failure writing that
diagnostic is ignored. Working output channels retain their existing bytes and
exit status (help/success 0, refusal 2, operational/output error 1).

### Answer record v2 (explicit selector)

`ontd --synthetic --answer-record=v2` and `ont-program-pilot --answer-record=v2`
select `ontology-answer/v2`. `--answer-record` and `--answer-record=v1` select
v1; omitting the flag keeps native output. Existing help/native/v1 bytes stay
unchanged. The pilot retains the same selected bytes it publishes. Source
acquisition still requires its separately released owner authority.

Library callers select `ont_core::answer::Version::V2` through
`AnswerRecord::from_response_version`, `from_action_version`,
`from_error_version`, `from_outcome_version`, or
`Service::render_answer_record_version_with_status`. Existing entrypoints stay
v1. The returned boolean preserves native refusal exit status (0 success,
2 refused; failed output writes exit 1 with no panic).

V2 preserves the complete native record and adds these publication fields:

| Location | Added values |
| --- | --- |
| Every typed report fraction | `native` numerator/denominator, `rounding_mode`, `decimal_renderings` |
| `report.metrics[].aggregate` | `population`, `start_count`, `end_count`, `start_ratio`, `end_ratio`, `count_change`, `ratio_change`, `percentage_point_change`, `source_qualified` |
| `report.attributions[].aggregate` | `population`, `endpoints`, `count_change`, `ratio_change`, `percentage_point_change`, `component_contribution_sum`, `reconciliation_residual`, `reconciliation_residual_percentage_points` |

Counts remain exact integer strings and population is an integer. Each rational
keeps its exact numerator string and denominator integer. `decimal_renderings`
has string keys `2`, `3`, `4`, `6`, with exactly that many fractional digits.
`rounding_mode` is `half_away_from_zero`: round to nearest, with an exact halfway
value rounded away from zero. For example, 1/200 at precision 2 is `0.01`,
-1/200 is `-0.01`, and values rounded to zero have no minus sign. Decimal long
division uses only integer quotient/remainder operations, never float. Native
integer strings are preserved even beyond machine-integer range. Opaque
`Knowledge::Value` payloads remain opaque, including fraction-shaped user data.

A metric is a native aggregate time series: its population and endpoint counts
come from its admitted fractions and its native comparability decision. Samples
are endpoints over time, not components to add together. Different KPIs,
populations, recorded/exact algorithms and overlapping definitions are never
summed. An undefined aggregate has `status:not_available` with a closed reason:
`missing_endpoints`, `missing_endpoint_values`, `empty_denominator`,
`incomparable_components`, `invalid_rational` or `arithmetic_overflow`.

An attribution aggregate uses the native `delta_bound` count and denominator.
`component_contribution_sum` is the exact sum of newly absent minus restored
counts divided by that denominator. `reconciliation_residual` is aggregate
ratio change minus that sum (ratio units); the explicitly named percentage-point
residual multiplies it by 100. Exact zero is present as numerator `"0"`, never
silently omitted. Unbound change gives `unbound_aggregate_change`, not zero.
`endpoints` requires proof of the attribution's native source endpoint identities
and interval matching the selected metric samples. The current native attribution
projection does not carry that binding, so `endpoints` is always typed
`not_available` with reason `unbound_endpoints`. Matching population/definition
pins, query/observation times or numeric changes cannot establish it. Metric
endpoint aggregates and attribution change, component sum and residual remain
independently available.
Native source qualification and reconciliation gaps remain visible and are never
strengthened by decimal formatting or arithmetic.

Every contribution's repository and change references must occur in the admitted
basis before its attribution aggregate is published. When any reference is
absent, the aggregate is omitted without a hidden inventory/count/reason.
Metric aggregates use only their separately authorized native aggregate samples;
unknown totals are never reconstructed from a partially disclosed repository
population. No second Store pass occurs. Refusals remain code-only. Prose may
only restate fields present in the selected record; the new fields do not assert
operational acceptance, complete source qualification, or human authentication.
