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
