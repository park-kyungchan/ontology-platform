# Effect contract v2: source mapping and present gaps

This is a source inspection, not a conformance test run. The closed contract names
are in CONTRACT-v2.md section 2. A mapping is conditional on recorded proof, not
just an enum rename. Row identifiers refer to its ordered outcome table.

Source anchors: engine paths are relative to ontology-platform at
50dd391239a880dcbabbe775321c0355588c9de1. C is
/home/palantirkc/platforms/education-platform/crates/edu-academy-core/src/workflow.
TF is /home/palantirkc/work/workspace-strategy-reassessment-20261109.
BP is TF/academy-context-20261111/BUILD-PLAN-P8-S3.md;
NA is TF/academy-context-20261111/NATIVE-ANSWERS-P8.md;
MP is TF/memory-20261111/BUILD-PLAN-P10-P1b.md;
MD is TF/memory-20261111/DESIGN-P10-MEMORY-v3.md.
BP/MP are plans, C is code; they are not evidence that the redesign is implemented.
NA is private host evidence: only these non-payload conclusions/locators are cited;
no captures, identifiers, narratives or lightly renamed private values belong here.

## Engine sequence

| Today's name / durable fact | Contract states | Source path:line and qualification |
| --- | --- | --- |
| No local_actions row | unrecorded | crates/ont-store/src/actions.rs:166; absent before review commit, docs/s07/CONTRACT-v1.md:74. |
| reviewed | pre_effect_recorded | crates/ont-store/src/actions.rs:181; review/token persisted before dispatch, but no v2 evidence-profile field. |
| possible_dispatch | dispatch_possible | crates/ont-store/src/actions.rs:265; durable fence commit at :271 precedes apply at :273. |
| persisted | applied | crates/ont-store/src/actions.rs:317; matching token/version/value, proof persisted at :331. |
| no_effect | not_applied | crates/ont-store/src/actions.rs:325; durable target seal in crates/ont-store/src/fake.rs:263 prevents future apply. |
| conflict_no_effect | conflict | crates/ont-store/src/actions.rs:326; target token records compare-and-set rejection at crates/ont-store/src/fake.rs:250. It is not proof that this attempt applied another writer's value. |
| manual_reconciliation | manual_reconciliation | crates/ont-store/src/actions.rs:324; own application proof with conflicting subsequent value/version retains fence. |
| Evidence::ValueOnly | dispatch_possible | crates/ont-store/src/actions.rs:310; no proof is promoted even when desired value matches; v2 kind none. |

Cannot express as distinct item states today: answer_returned, answer_lost,
timeout_returned, async_accepted, rejected and not_observable. The six-value enum
is crates/ont-core/src/action.rs:78; its settled set is :97. Missing-record faults
return errors rather than a durable manual_reconciliation quarantine (:305).

Rows it would fail or cannot represent if asked for the v2 inputs, from reading:
R06/R08/R09 integrity quarantine; R14 distinct rejection; R16-R23/R33 async and
complete-absence results; R25/R26 definitive returned answers; R27-R31/R35-R39
returned/lost/timeout distinctions and value-profile results. These are absent
capabilities, not evidence its fake token path failed. R11-R13/R15 are compatible
proof mappings; all cross-target proof inputs still need R24's explicit denial.
There is no multi-item action (crates/ont-core/src/action.rs:68), no history ceiling
(crates/ont-store/src/actions.rs:1), and no network-safe public adapter boundary.

## Academy workflow: all nine FieldState values

| Today's name | Contract states | Source path:line and qualification |
| --- | --- | --- |
| available | unrecorded | C/state.rs:138; no Execution exists; existing review/proposal is outside the attempt. |
| dispatch_outcome_unknown | dispatch_possible, answer_lost | C/state.rs:141; Execution exists without Outcome. It cannot tell a pre-call crash from loss without another durable fact. |
| reconciliation_required | answer_returned, answer_lost, timeout_returned, not_observable, manual_reconciliation | C/state.rs:135; other Outcome status collapses these; C/service.rs:672 records returned versus unresolved but not finality. |
| settled_persisted | applied | C/state.rs:132 and C/service.rs:689; valid only with v2 qualified proof; trusting status alone cannot establish final returned completion. |
| settled_noop | unrecorded, not_applied, manual_reconciliation | C/state.rs:133; no Execution means domain equality no-op, not an effect. If fenced, requires no-effect proof; otherwise cannot release ambiguity. |
| settled_not_persisted | rejected, not_applied, manual_reconciliation | C/state.rs:134; C/service.rs:673 requires transport returned, which alone is weaker than a final zero-effect answer. |
| reconciled_persisted | applied, answer_lost | C/state.rs:124; C/service.rs:928,951 accepts equality even without returned transport. Only final-return equality qualifies for applied in v2; otherwise R30 preserves answer_lost. |
| reconciled_not_persisted | not_applied, manual_reconciliation | C/state.rs:125; C/service.rs:930,940,955 accepts before-value after return or user instruction after loss; neither alone meets no-effect proof. |
| reconciled_conflict | conflict, manual_reconciliation | C/state.rs:126; C/service.rs:933,951. Qualified final-return third value maps conflict; user instruction after loss leaves manual_reconciliation. C/state.rs:113 labels this consumed_without_effect, but third value does not prove zero historical effect. |

Execution durability precedes the callback's dispatch permission
(C/approval.rs:44,63,71); ledger-only publication is not a fence
(C/state.rs:158). Source read failures/unknowns must retain ambiguity
(C/service.rs:888,923). BP:70-74 continues core and retains a held field after loss;
BP:71 makes returned third-value conflict terminal. However core still exposes a
user_instruction resolution path (C/service.rs:940-960), unlike BP:70's stronger
rule that a teacher assertion does not release the hold. That path is a genuine
source/plan divergence; a mapping must not hide it.

Cannot express distinctly in today's nine field states: pre_effect_recorded,
answer_returned, answer_lost, timeout_returned, async_accepted, rejected,
not_observable and manual_reconciliation. Some facts live in other records, but
the enum alone is insufficient. There is no operation identifier/pending-operation
state. C/service.rs:935 only checks transport returned, not execution terminal.

Predicted failing cases: R30 (lost-answer desired equality is settled today), R28
(before-value after a final returned answer is treated as no effect without an
explicit no-effect guarantee), R35 (returned timeout may be confused with returned
completion; C/service.rs:935), R09/R36 (user instruction after loss can settle a
before/third value), R20/R21/R33 (async results unexpressed), and R04/R29's
never_retry obligation if consumed_without_effect is treated as retry permission.
R02/R03 require actual no-effect proof, not that label. R27 is a conditional
convergence mapping, not causal attribution. No code tests were run for these claims.

Native qualification: returned_value after save is still an assumption. NA:53-54
explicitly records no mutation, returned-timeout, lost-answer or recovery trial;
NA:41 disproves using sourceRevision/wireDigest as a content hash. Its actual
read metadata does not qualify a mutation boundary. No native successful-absence
proof was established (NA:39). The native document itself is not vendored.

## Memory plan: projection status and operation facts

| Today's name / planned fact | Contract states | Source path:line and qualification |
| --- | --- | --- |
| pending | pre_effect_recorded, dispatch_possible, answer_returned, answer_lost, timeout_returned, async_accepted, not_observable, manual_reconciliation | MP:76-77; row hidden before remote work, but status alone collapses every attempt/phase and proof condition. |
| ready | applied | MP:78,85; projection convergence only after that row's document/fact/page/read-port barriers. Equal verified ready is a domain no-op (MP:70), not a new applied attempt. |
| superseded | applied | MP:86; for desired deletion, withdraw after complete absence/barriers. Final-id refusal (MP:88) is local admission, not a dispatched rejection. Final status alone is not a reusable effect proof. |
| Earlier operation discovered, nonterminal | async_accepted | MP:72; bind identifier to item/payload/phase, wait boundedly, never dispatch a second operation. Discovery is not evidence of success. |
| operation_terminal_unobservable | not_observable | MP:72; pending projection remains hidden, other rows continue. Terminal state without visible proofs is not a settled success or failure. |
| Retain/delete acknowledgement lost | answer_lost, not_observable | MP:77,93; inspect operation/projection before retry. A rerun's selected Git tip is not proof that the old attempt failed. |
| Operation complete with verified projection | applied | MP:77,90-93; operation_record plus every required domain barrier, not merely a successful job status. |
| Operation failed/rejected | rejected, not_applied, not_observable | MP:72,77; only qualified zero-partial/zero-late-effect proof permits either settled no-effect state. Partial failure remains not_observable. |
| DELETE returned; incomplete/top-k absence | not_observable | MP:91,93; complete_absence unavailable keeps pending. Absence alone cannot exclude a late operation. |

Cannot express independently through pending/ready/superseded today: unrecorded
(except missing row), dispatch_possible, answer_returned, answer_lost,
timeout_returned, async_accepted, rejected, not_applied, conflict,
not_observable and manual_reconciliation; pre_effect_recorded versus remote work
also needs phase facts. These statuses describe current projection visibility,
not immutable attempt history: ready -> pending repair is intentional (MP:87).
Do not map that repair as changing an old settled applied attempt.

Predicted gaps: R20/R21/R33 require durable operation/phase/attempt binding which
is not a field in MP:80-88's listed projection schema; R35/R36/R38 cannot be
reconstructed from pending alone. P19/P20 must retain these facts on the pending
object's edit record (assessment disposition), or provide a qualified bound target
lookup. P24/P25 must prove immutable prior-attempt results as well as visibility
(MP:195-196). R02/R03/R13/R17/R19/R23/R25/R26 require a NEW attempt identity after
no-effect, whereas MP:92's same-tip phase identity and same-UUID retry plan lack a
separate consumed-attempt identity. Same remote document ID is allowed; resending
an old attempt is not. No same-tip retry is authorized merely by a new run.
R22/R23 require complete closed coverage; MP:91 explicitly leaves U10 unresolved.
U14 operation finality and U3 submission idempotency remain assumptions; the
measurement file was absent. None of these are measured native failures.

## What each mapping must prove, without prescribing its test design

Each implementation pins the contract/data and accounts for all normalized table
inputs in its declared capability profile, including denial of unsupported proof
kinds. It proves durable pre-dispatch ordering, one dispatch per attempt, bounded
read-only recovery, correct returned/lost/timeout/accepted separation, no retry
without zero-effect proof, immutable settlement, and retained unresolved fences.
The engine additionally proves existing token/seal behavior; Academy per-field
partial results and its authority hook; memory operation-phase bindings plus
projection/page barriers without turning projection status into attempt history.
A synthetic mapping pass never qualifies a native target.

## Source corrections to the assessment

The broad structural assessment holds. Two qualifications matter: the plan's
"no teacher assertion releases a hold" is stronger than core's existing explicit
resolution API (C/service.rs:940-960); and native Academy answers now exist but
qualify reads only (NA:53-54), not writes. The assessment's "only Academy has
returned-vs-lost" is about implemented field records; MP:72,77,93 already specifies
lost-ack operation inspection as future work. No equivalence of mechanisms follows.
The packet's private-repository premise is contradicted by MP:146 and the current
GitHub repository metadata (ontology-platform visibility public); acquisition must
still fail closed if access changes, never silently request a new credential.
