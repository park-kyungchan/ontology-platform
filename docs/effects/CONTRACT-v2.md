# Effect contract v2

Normative words MUST, MUST NOT and MAY set conformance requirements. This is a
per-item external-effect contract, not a claim of atomicity across services.
S07 v1 remains frozen; this version defines a new boundary and the gaps in today's
implementations. Implementations remain separate until a real target qualifies.

## 1. Terms and identities

| Term | Exact meaning |
| --- | --- |
| request | One immutable approved intent, identified within an owner/account scope; it contains one or more items. |
| item | One physical field or one document of a request, with its own outcome; several remote phases use separate linked attempts. |
| approval | Domain authority bound to the exact request content, targets and source basis; not an execution identifier. |
| pre-effect record | Durable owner record binding request/item/attempt identities, target instance/account/physical key, operation, before and desired value/hash (including desired absence), approval reference, evidence profile/version and serialization scope. |
| dispatch | The single invocation that may change the external target for one attempt; it requires a committed dispatch fence in the pre-effect record. Read-only polling is not dispatch. |
| target's answer | The response to that invocation: returned acknowledgement, completed response, timeout, acceptance with operation identifier, or definite rejection; loss of an answer is a separate durable fact. |
| evidence | Bound facts the target's pinned evidence profile permits using to decide an outcome; a success message alone is insufficient. |
| reconciliation | Read-only classification of one attempt using its durable facts and target evidence. A fake-only durable seal is an explicitly qualified exception to read-only work. |
| attempt | One immutable execution identity for an item/phase; at most one dispatch. A retry is another attempt, never replay of this one. |
| settled | A historical outcome fixed forever: exactly applied, not_applied, rejected or conflict. It is not a promise that the target's current value never changes. |

Request, item, attempt, target operation identifier and target idempotency token
MUST be distinct fields, even when deterministically derived. A phase change or
new run MUST NOT escape an unresolved physical-item fence. An action may dispatch
several items once each, with separate fences/outcomes and partial success; it
MUST NOT report all items applied from one successful field. A batch transport
MUST persist every included item's fence first and bind each returned field.

## 2. Closed item-state vocabulary

These are the ONLY contract state names. Facts such as a returned answer remain
in history when a later state supersedes their classification. The implementation
MUST durably write the record in the last column before publishing that state.

| State | Settled | Required durable record |
| --- | --- | --- |
| unrecorded | no | No pre-effect record; authoritative journal lookup finds none. No compliant dispatch is possible. |
| pre_effect_recorded | no | Complete pre-effect record committed, without its dispatch fence. |
| dispatch_possible | no | Dispatch fence committed before invocation; no durable answer is known. |
| answer_returned | no | Fence plus exact bound returned answer and completion/acknowledgement classification. |
| answer_lost | no | Fence plus explicit transport-loss observation, or a recorded reconciliation finding of missing answer. |
| timeout_returned | no | Fence plus the returned timeout; no implied cancellation or no-effect proof. |
| async_accepted | no | Fence plus accepted/discovered operation identifier, phase, target binding and pending observation. |
| rejected | yes | Fence plus terminal bound rejection that excludes partial and delayed effects. |
| applied | yes | Fence plus qualified evidence of application/convergence and the evidence kind; attribution strength explicitly retained. |
| not_applied | yes | Fence plus qualified proof of no partial effect and no possible delayed effect. |
| conflict | yes | Fence plus qualified terminal conflicting current value/operation result and observed basis; does not itself prove no effect. |
| not_observable | no | Fence, all known answer/operation facts and failed/incomplete read with scope, coverage and reason. |
| manual_reconciliation | no | Fence and unresolved evidence/reason; if the original record/fence is corrupt or absent, a durable quarantine record preserves the fault without fabricating approval. |

A successful pre-dispatch equality check is a domain no-op, not an applied attempt;
no attempt need be created. It MUST NOT create fictitious evidence of dispatch.
State transitions follow the table, not enum ordering. Settled records are returned
unchanged even if later evidence disagrees; record that disagreement separately.
Missing/corrupt settled evidence is an integrity error, never a valid settled input.

## 3. Evidence belongs to the target

An evidence profile pins target identity, scope, read authority, coverage and
ordering/finality guarantees. It MAY declare several supported kinds; every
classification chooses one kind and keeps the proof. Exactly five kinds exist:

| Kind | What must be read/stored | States it may prove by reconciliation |
| --- | --- | --- |
| target_token | Target stores/returns attempt token atomically with outcome; bound read verifies review, value/version; absence requires a durable seal excluding later apply. | applied, not_applied, rejected, conflict; applied with later divergence requires manual_reconciliation. |
| returned_value | Fresh authoritative equality/difference after a durably returned FINAL completed answer for this attempt; exact field identity and raw value, not display text or a reused digest. | applied as convergence only; third value gives conflict. Before-value alone never proves not_applied. Lost answer/timeout alone cannot use this kind to settle. |
| operation_record | Readable operation bound to attempt/phase/payload; pending/terminal/zero-partial-effect semantics and all target-required projection barriers. | async_accepted, applied, not_applied, rejected (explicit terminal rejection), conflict; unobservable or incomplete gives not_observable. |
| complete_absence | Complete authoritative enumeration of every affected view plus a closure barrier excluding outstanding/late work; no-effect additionally requires complete history excluding past application. | Desired absence gives applied as convergence; undesired absence for creation gives not_applied only when no partial effect anywhere is proved. |
| none | No qualifying read proof; transport receipt and elapsed time do not upgrade it. | No settled state from readback. |

A durably recorded definitive no-effect ANSWER may prove rejected/not_applied
regardless of available reconciliation kind (R25/R26). A generic HTTP error,
returned timeout, failed job after partial progress, missing token without a seal,
or top-k empty search MUST NOT be normalized to such an answer/proof. An explicit
terminal operation rejection normalizes to operation_rejected; R19 preserves
rejected. Terminal partial failures
normalize to operation_done_unverified, not operation_no_effect.

For complete_absence, "closed" requires no queued work, no possible late
completion and no other partial projection; a complete read at one instant is
insufficient. A read of desired absence while deletion is in flight remains
absence_open. Equality cannot identify who wrote a value; an applied outcome
using returned_value/complete_absence MUST state convergence, not causal proof.
For target_token, identical value with a changed version still counts as divergence.

Today's targets (paths use aliases defined in MAPPING-v2.md):

| Target | Current kind and qualification |
| --- | --- |
| Engine S07 fake | target_token, source-proved for this fake only (crates/ont-store/src/fake.rs:244,263). ValueOnly is none. Real-target transfer is an assumption, forbidden by docs/s07/CONTRACT-v1.md:51. |
| Academy daily-record workflow | returned_value is the intended profile, still an assumption for save/finality/causality; the workflow also accepts equality after loss, a conformance gap. NA:53-54 records read-only evidence, no mutation/recovery measurement. Native sourceRevision/wireDigest do not establish content equality (NA:41); raw qualified values must be compared. No native token, operation or complete-absence capability is established. |
| Memory publisher | operation_record plus complete_absence are intended; target_token/idempotent submission is an assumption (MP:92, MD:22); U3/U10/U14 remain unqualified (MP:90-93). MEASURE-U1-U15.md absent at source inspection; native runtime capability is none until measured. Source-described endpoints and synthetic tests are not native qualification. |

## 4. Outcome table and its data form

The table and vectors-v2.json have the same ordered rows. Inputs are normalized
DURABLE facts, not whatever a caller remembers receiving. A returned answer lost
before the local commit is none on restart, or lost after that loss is recorded.
A committed fence with no dispatch is indistinguishable from dispatch with a lost
answer until target evidence settles it. Reconciliation MUST NOT infer dispatch
absence from a local crash point. No committed fence means first dispatch is still
possible ONLY after fresh domain admission; "nothing" below adds no recovery work.

Recorded stage is absent, prepared (pre-effect record only), fenced or settled;
saved_state is null except for settled, where it is one of the four settled names.
Kind is one of section 3's five. Answer is exactly none, lost, returned_ack,
completed_returned, returned_timeout, accepted, rejected_no_effect or
completed_no_effect. completed_returned promises execution is terminal, unlike an
acknowledgement. accepted requires the bound operation identifier durably saved;
a lost identifier can be recovered only by a qualified lookup bound to the attempt,
not by resubmission. An unbound/ambiguous lookup is contradictory.

Later-read values are exactly those in vectors-v2.json domains.later_read.
desired/before/other compare qualified raw values for the same item; desired wins
if before equals desired. token_* and operation_* require the bindings and full
profile guarantees in section 3. operation_applied_verified includes projection
barriers; operation_done_unverified does not. absence_undesired_closed applies
only to creation with a complete effect-history read proving no past application,
zero partial effects and no late work; current-value absence alone is absence_open. unavailable includes permission and
unknown-field failures; identity_mismatch is never a read of a different item.
contradictory includes incompatible bound proofs/answers, malformed identifiers,
missing required durable facts and contradictory operation terminal results.
budget_exhausted is a durable escalation after the implementation's bounded reads.

Each cell is a literal, an OR-list, or * (any value of that column). Evaluate TOP
TO BOTTOM and use ONLY the FIRST matching row. Saved state is matched only in R01
through R04. Thus every valid finite Cartesian combination has exactly one selected
outcome, including unsupported/inconsistent combinations (quarantine). Conformance
requires the selected result for every combination in the finite domains, with
priority preserved; * is coverage, not an omitted case. Consumers choose their
own test design. R06/R08 are integrity
cases, not permission to dispatch without a record. No new table row may silently
extend the closed state or evidence vocabulary.

Allowed next steps: nothing; read_again (bounded read-only reconciliation);
new_attempt_new_identity (permission for a later, newly admitted attempt ONLY after
no-effect proof); never_retry (this attempt/intent has no retry permission);
ask_person (retain fence; seek qualified proof or target repair). None authorizes
automatic replay. A person saying "it failed" does not prove no effect. A new
independently approved intent after conflict is subject to current target state,
existing fences and domain admission; it MUST NOT be disguised as a failed retry.

| Row | Recorded stage / saved state | Kind | Answer | Later read | Result state | Allowed next step |
| --- | --- | --- | --- | --- | --- | --- |
| R01 | settled/applied | * | * | * | applied | nothing |
| R02 | settled/not_applied | * | * | * | not_applied | new_attempt_new_identity |
| R03 | settled/rejected | * | * | * | rejected | new_attempt_new_identity |
| R04 | settled/conflict | * | * | * | conflict | never_retry |
| R05 | absent | * | none | not_read, unavailable, desired, before, other | unrecorded | nothing |
| R06 | absent | * | * | * | manual_reconciliation | ask_person |
| R07 | prepared | * | none | not_read, unavailable, desired, before, other | pre_effect_recorded | nothing |
| R08 | prepared | * | * | * | manual_reconciliation | ask_person |
| R09 | fenced | * | * | identity_mismatch, contradictory, budget_exhausted | manual_reconciliation | ask_person |
| R10 | fenced | * | rejected_no_effect, completed_no_effect | token_applied_current, token_applied_diverged, operation_applied_verified, operation_pending, operation_done_unverified | manual_reconciliation | ask_person |
| R11 | fenced | target_token | * | token_applied_current | applied | nothing |
| R12 | fenced | target_token | * | token_applied_diverged | manual_reconciliation | ask_person |
| R13 | fenced | target_token | * | token_sealed | not_applied | new_attempt_new_identity |
| R14 | fenced | target_token | * | token_rejected | rejected | new_attempt_new_identity |
| R15 | fenced | target_token | * | token_conflict_no_effect | conflict | never_retry |
| R16 | fenced | operation_record | * | operation_applied_verified | applied | nothing |
| R17 | fenced | operation_record | * | operation_no_effect | not_applied | new_attempt_new_identity |
| R18 | fenced | operation_record | * | operation_conflict_no_effect | conflict | never_retry |
| R19 | fenced | operation_record | * | operation_rejected | rejected | new_attempt_new_identity |
| R20 | fenced | operation_record | * | operation_pending | async_accepted | read_again |
| R21 | fenced | operation_record | * | operation_done_unverified, operation_unobservable | not_observable | read_again |
| R22 | fenced | complete_absence | * | absence_desired_closed | applied | nothing |
| R23 | fenced | complete_absence | * | absence_undesired_closed | not_applied | new_attempt_new_identity |
| R24 | fenced | * | * | token_applied_current, token_applied_diverged, token_sealed, token_rejected, token_conflict_no_effect, operation_pending, operation_applied_verified, operation_no_effect, operation_conflict_no_effect, operation_rejected, operation_done_unverified, operation_unobservable, absence_desired_closed, absence_undesired_closed | manual_reconciliation | ask_person |
| R25 | fenced | * | rejected_no_effect | * | rejected | new_attempt_new_identity |
| R26 | fenced | * | completed_no_effect | * | not_applied | new_attempt_new_identity |
| R27 | fenced | returned_value | completed_returned | desired | applied | nothing |
| R28 | fenced | returned_value | completed_returned | before | manual_reconciliation | ask_person |
| R29 | fenced | returned_value | completed_returned | other | conflict | never_retry |
| R30 | fenced | returned_value | none, lost | desired | answer_lost | read_again |
| R31 | fenced | returned_value | returned_timeout | desired | timeout_returned | read_again |
| R32 | fenced | * | * | unavailable, absence_open, incomplete_absence | not_observable | read_again |
| R33 | fenced | operation_record | accepted | not_read, desired, before, other | async_accepted | read_again |
| R34 | fenced | * | accepted | * | manual_reconciliation | ask_person |
| R35 | fenced | * | returned_timeout | * | timeout_returned | read_again |
| R36 | fenced | * | lost | * | answer_lost | read_again |
| R37 | fenced | * | none | * | dispatch_possible | read_again |
| R38 | fenced | * | returned_ack, completed_returned | not_read | answer_returned | read_again |
| R39 | fenced | * | returned_ack, completed_returned | * | manual_reconciliation | ask_person |

The JSON envelope declares format, selection, closed states/kinds, finite domains,
allowed steps and rows. recorded_facts/answer/later_read are selectors as above;
expected_state/allowed_next_step are exact assertions; reason explains the row.
A consumer pins repository + full commit + docs/effects/vectors-v2.json + SHA256
of exact bytes, together with CONTRACT-v2.md and MAPPING-v2.md at that commit.
Verify hashes before running; missing/mismatched pins fail, never skip/fallback.
Vendor the three pinned data/doc files if necessary; no Rust dependency is needed.
The consumer owns its test implementation; it must prove state mapping, durable
pre-dispatch order, single dispatch, no retry on uncertainty, immutable settlement,
bounds/retention and every applicable row. Unsupported capabilities must be
explicitly denied and exercise R24/R34/R39, not silently counted as passing native
behavior. Every fake behavior cites native evidence or says "assumption".

## 5. Invariants, bounds and retention

1. The pre-effect record and fence MUST be durable before dispatch. Ambiguous local
   commit means no invocation in that process; restart still reads the journal.
   No local or target transaction is required to span a network call.
2. A single serialization scope MUST exclude concurrent first dispatch/recovery
   races. Once fenced, an attempt MUST NOT dispatch again, even when the target
   supports idempotency. A lost ack is reconciled by reads, never by resending.
3. A retry requires not_applied or rejected with no partial/delayed effect, a new
   attempt identity and current approval/admission. Stable remote document identity
   is allowed; it is distinct from an attempt token. No identity rotation releases
   ambiguity. Target idempotency is defense in depth, not replay permission.
4. Settled states MUST NOT change. Later target drift is a separate observation or
   newly approved request. Unresolved/manual fences MUST survive restart, code
   rollback, compaction and authorization loss; denied reads never release them.
5. Each implementation MUST declare limits for items, attempts/history bytes and
   count, bounded calls/lock waits, proof retention, archive/restore, and admission
   at capacity. Never purge an unresolved record or its dependencies. A settled
   summary must preserve identity consumption/no-replay guarantees before payload
   deletion; expiration alone is not proof. Restore must not erase known fences.

Current limits are observations, not new implementation:

| Implementation | Bounds/retention today and required gap |
| --- | --- |
| S07 fake | No effect-history count/retention limit or expiry (crates/ont-store/src/actions.rs:1; docs/s07/CONTRACT-v1.md:57). It therefore lacks the bounded-history declaration required for a real target. Preserve both databases/journal binding; no restore-to-empty. |
| Academy | BP:121-128 declares total 4,096 per-student executions, preflight for selected fields, 64MiB root, 1,024 draft generations, 7-day closed-draft retention, 24h idle expiry; core evidence is not purged. Compaction is a later required packet, not implemented evidence. Read-only reconciliation remains possible at capacity. These are plan limits, not a core-enforced universal bound. |
| Memory | MP:47-55 declares 1,000 current/10,000 scan ceilings and incomplete-read refusal; these are read admission bounds, not operation-history retention. MP:72,92-94 needs durable attempt/phase/operation bindings, capacity and retention declarations for pending work; Git/tree identity does not replace them. |

## 6. Domain policy and offered hooks

| Outside this contract | Hook the contract provides |
| --- | --- |
| Teacher approval content, immediate-call rule and time window | Opaque approval reference + pre-dispatch revalidation; missing/expired authority forbids the first dispatch, never abandons old fences. |
| Draft generations, expiry, parsing, selected fields and display | Request/item/basis hashes and per-item outcome; domain recomputes previews and distinguishes an equality no-op. |
| Account lock and source-specific key resolution | Explicit serialization scope and immutable physical identity; domain fixes lock order outside the journal lock. |
| Memory Git-tree union, pending visibility, independent rows and page ordering | Separate linked phase attempts + accepted-operation binding; domain supplies full projection/absence barriers and refresh dependencies before application/finalization. |

No cross-item atomicity, automatic retry, watcher, universal absence proof,
new approval UI, consumer test framework or implementation cutover is specified.
Historical S07 byte compatibility remains at v1; claiming v2 conformance requires
mapping plus the new proof and durability obligations, not a renamed enum alone.
