# Stable PR5 Observation interface (v1)

Owner: Lead w1J:p43. Consumer: PR6 via ordinary authorized Program objects, not a module import. Adapter revision: `program-guardrails/v1`. This is trusted intake, not an untrusted request protocol. No source acquisition or authorization is implemented here.

All fields below are existing `ont_core::Knowledge` JSON: `{"kind":"value","value":...}`, `{"kind":"unknown"}` or `{"kind":"not_performed"}`. Numeric zero is only `Value(0)` after successful source qualification. Optional fields are omitted when inapplicable, never synthesized as zero. Malformed, absent, failed, incomplete, stale, future or unverified sources set primary `value` and `bound_satisfied` to Unknown; explicit unattempted capture sets both NotPerformed. Bounds are trusted Contract metadata and remain present on source failure. Every Observation is Initial, supersedes=None, with a content-bound retained sanitized envelope; projection/correction ownership remains with the embedding owner.

## Common fields

| Field | Knowledge value JSON type | Unit / meaning / bounds |
|---|---|---|
| `guardrail` | string | `billing`, `disk`, `recall`, `defects` |
| `value` | unsigned integer | Linux minutes, bytes, supported answers, escaped incidents respectively; Unknown/NotPerformed as above |
| `unit` | string | `linux_minutes`, `bytes`, `fraction`, `count` respectively |
| `source_qualified` | boolean | True only for validated complete current evidence; false otherwise |
| `qualification` | string | `qualified`, `missing_source`, `not_performed`, `failed_source`, `invalid_source`, `incomplete_source`, `settlement_lag`, `unverified_baseline`, `future_source`, `stale_source`, `unresolved_disk_unit` |
| `bound_direction` | string | `at_most` for billing/defects; `at_least` for disk/recall |
| `bound_value` | unsigned integer | Contract limit in the primary unit; recall baseline numerator. Unknown for unresolved disk threshold unit |
| `bound_satisfied` | boolean | Exact bound comparison when qualified; Unknown/NotPerformed otherwise. Disk may be provably above/below both candidate thresholds while unit remains unresolved |

## Kind-specific fields

| Kind | Field | Knowledge value JSON type | Unit / meaning / states |
|---|---|---|---|
| Billing | `account` | string | Exact sanitized source account identity from Contract |
| Billing | `window_start`, `window_end` | unsigned integer | Contract UTC calendar month, Unix seconds, half-open |
| Billing | `source_cutoff` | unsigned integer | Source usage cutoff, within the month; present after typed parsing |
| Billing | `settlement_lag_seconds` | unsigned integer | Explicit settlement lag; Unknown when not supplied. Only known zero qualifies |
| Billing | `recorded_value` | unsigned integer | Validated page sum before lag qualification; diagnostic only, never a bound input |
| Disk | `source_unit` | string | `bytes`, `gb`, `gib`, preserving the source conversion basis |
| Disk | `bound_lower_bytes`, `bound_upper_bytes` | unsigned integer | Only for unresolved GB/GiB meaning; inclusive candidate bounds in exact bytes |
| Recall | `total` | unsigned integer | Evaluation denominator, >0, value <= total; Unknown/NotPerformed with primary value |
| Recall | `bound_total` | unsigned integer | Baseline denominator, >0; no regression compares exact fractions |
| Recall | `evaluation_set_sha256`, `baseline_result_revision`, `baseline_artifact_sha256` | string | Contract provenance; evaluation set must match source exactly |
| Recall | `result_revision` | string | Actual executed result revision after typed parsing |
| Defects | `window_start`, `window_end` | unsigned integer | Contract UTC Monday-to-Monday week, Unix seconds, half-open |
| Defects | `source_cutoff` | unsigned integer | Incident ledger cutoff, within the week |
| Defects | `fixed_count` | unsigned integer | Fixed subset of escaped incidents; does not reduce primary escaped count |
| Any baseline | `recorded_value`, optional `recorded_total` | unsigned integer | Explicit unverified producer assertion, never a qualified value |

`Observation.observed_at` is the actual measurement/cutoff time; capture/import time never refreshes it. `fresh_until` is source time + 86400 seconds for billing, +900 disk, +3600 recall/defects. Expiry equality remains fresh; future source/capture timestamps do not qualify. Coverage is Complete only for qualified evidence with resolved disk bound meaning; otherwise Partial. Consumers must re-evaluate freshness using the current trusted clock and existing `ont_core::program::qualify_basis`, and honor source_qualified/coverage/bound states. No universal all-clear follows from a partial authorized result.

## Synthetic examples (Knowledge wrappers shown for decision fields)

Billing Contract: account=`synthetic-account`, month=2026-11, linux_skus={`synthetic-linux`}, limit=3000. A complete one-page capture containing 12 linux_minutes at a current cutoff with settlement_lag_seconds=0 emits `guardrail=Value("billing")`, `unit=Value("linux_minutes")`, `value=Value(12)`, `bound_direction=Value("at_most")`, `bound_value=Value(3000)`, `bound_satisfied=Value(true)`, source_qualified=true, qualification=qualified, Complete coverage. Empty successful one-page usage emits Value(0); absent pages emit Unknown.

Disk Contract: minimum=15 GiB (16,106,127,360 bytes). A current exact 16 GiB observation emits `guardrail=Value("disk")`, `unit=Value("bytes")`, `source_unit=Value("gib")`, `value=Value(17179869184)`, `bound_direction=Value("at_least")`, `bound_value=Value(16106127360)`, `bound_satisfied=Value(true)`, source_qualified=true, qualification=qualified. If the minimum's GB/GiB meaning is unresolved, 15,500,000,000 bytes emits bound_value=Unknown, bound_satisfied=Unknown and Partial coverage.

Recall Contract: synthetic evaluation-set digest=`a` repeated 64, baseline=15/33, baseline result revision=`synthetic-baseline-v1`, artifact digest=`b` repeated 64. An executed current matching-set result=16/33 at result revision=`synthetic-result-v2` emits `guardrail=Value("recall")`, `unit=Value("fraction")`, `value=Value(16)`, `total=Value(33)`, `bound_direction=Value("at_least")`, `bound_value=Value(15)`, `bound_total=Value(33)`, `bound_satisfied=Value(true)`, source_qualified=true. An unverified hardcoded 15/33 assertion emits primary value/total/bound_satisfied=Unknown, recorded_value=Value(15), recorded_total=Value(33), source_qualified=false.

Defects Contract: UTC week starting Monday 2026-11-02, limit=0. One escaped incident in that week, subsequently fixed before the current cutoff, emits `guardrail=Value("defects")`, `unit=Value("count")`, `value=Value(1)`, `fixed_count=Value(1)`, `bound_direction=Value("at_most")`, `bound_value=Value(0)`, `bound_satisfied=Value(false)`, source_qualified=true. Fixing the incident does not erase its occurrence.

Source-only qualification records must live behind their own explicit Program-scope/type grant. Do not copy values, coverage, timestamps, digest, qualification or failures onto an independently readable program root. Hidden guardrails must not change authorized bytes. `QualifiedCapture::object` creates no root, links, policy or grant.
