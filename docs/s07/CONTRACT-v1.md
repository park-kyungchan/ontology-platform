# S07 new-design contract v1 (freeze before implementation)

Authority: current user / ws-replan STATE / adopted ZIP S07, ONT-03/05/06,
EDU-08/09/15 and PROPOSAL recovery rules. Base is accepted S06
224864887d19f2c52e5026568f5e9b169314d3be. This is a bounded local fake, not
Academy HTTP, production authentication, S12, P8 or a universal delivery guarantee.
The existing Rust initiative remains Lead-owned; no new initiative or goal.

## Useful operation and authority

`copy_recommendation` deterministically copies the current complete/fresh source
observation's string `recommendation` into a fake physical `note` field. The
source's string `physical_record` maps logical occurrences onto the physical
record. Review binds the whole current observation, object identity and scope,
target identity/path, target version/value, desired text, owner and authorization
generation. Partial/stale/unknown evidence is never write authority; S05 saved
subset records cannot pass this contract. Strings and source/account keys are
exact (no trim/coercion or delimiter concatenation).

Every function/output/dispatch/recovery requires an opaque current Session from
the same Authority, current read + derive + named action permission for the same
object type, source/account/class/student. An arbitrary principal string, root,
ap or Host UID is not authentication. Synthetic CLI profiles are trusted fixtures
only. Dispatch rejects a changed review generation or source observation. Recovery
uses current authorization, the original owner/scope and original physical mapping;
a new generation may narrowly reconcile, but cannot dispatch a stale review.
No forbidden object/receipt is published. Keep admission alive through serialized
output. Revocation linearizes at Authority's write lock as in S06.

## Durability and lock order

Order: Authority read guard -> local SQLite IMMEDIATE -> fake target SQLite
IMMEDIATE. Never reacquire the local DB while holding only a target transaction.
The target transaction is already held when committing the local dispatch fence;
target effect is attempted only after that successful local commit. Target change
and dispatch evidence commit atomically in a separate SQLite database. No network,
daemon, outbox, worker or automatic retry. SQLite FULL + DELETE and bounded lock
waits; kernel/SQLite transaction recovery, no PID lock or unlink/recreate.

States: `reviewed` (no dispatch), `possible_dispatch` (durable fence; ambiguity),
`persisted` (matching durable applied evidence AND authoritative version/value),
`no_effect` (durable sealed no-dispatch proof), `conflict_no_effect` (durable
target rejection proof), `manual_reconciliation` (applied but conflicting readback).
Only explicit reconciliation can settle possible dispatch; inspect never retries.
Physical fences key exact `(source instance, account, physical record, field)`;
request ID, lesson date and logical object are deliberately absent. Unresolved
possible/manual state blocks ALL other requests/occurrences for that field.
Settled requests are never dispatched again. A new explicitly reviewed request is
permitted only after a predecessor is conclusively settled.

The fake target can durably seal a missing token under its writer transaction,
preventing that token from ever being applied later. This, not current value or
elapsed time, proves no effect. Value-only readback leaves uncertainty intact.
Do not extrapolate sealed absence/idempotency to an Academy adapter. Applied
evidence with a later value/version conflict retains the fence and requires manual
reconciliation. Source/target identity or mapping changes refuse recovery rather
than inspecting a different physical record. No fence expiration or reset API.

## Compatibility

S06 schema/application/user version 1 and original tables remain unchanged. Action
tables are installed additively by explicit S07 preparation. S06's existing reader
ignores these tables and has no dispatch capability. Old reads may continue;
returning the old binary does not undo target effects or settle uncertainty. S07
reopening must recover the same fence after an old-reader round trip. Two different
journal domains cannot attach to the same fake target: persistent journal ID is
bound in target metadata. Files must stay together; no deletion/restore-to-empty
as rollback. This is not legacy Academy journal compatibility (S10).

## Fault contract (real child process exits, not returned fake errors)

| Boundary | Exit | Reopened local state | Target effect count |
|---|---:|---|---:|
| before_review_commit | 81 | absent | 0 |
| after_review_commit | 82 | reviewed | 0 |
| before_fence_commit | 83 | reviewed | 0 |
| after_fence_commit | 84 | possible_dispatch | 0 |
| before_target_commit | 85 | possible_dispatch | 0 (rollback) |
| after_target_commit | 86 | possible_dispatch | 1 |
| before_readback | 89 | possible_dispatch | 1 |
| before_receipt_commit | 87 | possible_dispatch | 1 |
| after_receipt_commit | 88 | persisted | 1 |

Explicit evidence reconciliation yields no_effect for exits 84/85 and persisted
for 86/89/87; value-only yields possible_dispatch in all of these. Retry of the
same request never dispatches again. Exit 83 permits its FIRST dispatch later.
Before/after commit means around the actual SQLite commit; before_target_commit
is after uncommitted target UPDATE/evidence insertion.

## Evidence and judge

Frozen `cases-v1.json` contains literal expectations for full normalized protocol
outputs, raw statuses and reopened SQL state/effect/fence counts. No Rust output
may author expectations. The Python judge must reject wrong values/scopes,
missing fields, protocol junk, plausible output with a nonzero child status and
wrong raw crash statuses; preserve stdout/stderr/status for each actual child.
Python SQLite probes are design experiments only, never Rust evidence. Lead alone
owns compilation, independent review, acceptance and task state. Oracle changes
require a new version plus independent contract review; do not edit v1 to fit code.
