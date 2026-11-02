# S07 local action implementation boundary

The named operation copies the current authorized `recommendation` string to a
fake `note`. The source maps a logical occurrence to `physical_record`; two
occurrences in the fixture share R-001. Review persists the exact ActionInput,
current observation (including lineage, freshness and coverage), object/scope,
expected target value/version/identity/canonical path, desired value, authenticated
owner and authorization generation. No caller supplies a desired value or physical
mapping. Fresh complete source evidence is required for the function and dispatch.

The same Authority Admission stays alive through data reads, target effect and
serialization. A current Session is checked against the existing owned Weak and
generation. `Grant.actions` is an explicit opt-in; S06 grants have an empty set,
and S06 read/derive semantics are unchanged. Review/dispatch requires one grant
with read+derive+copy_recommendation for the exact object type and scope. Recovery
can use a newer generation only with the original owner, current named permission,
unchanged scope and original physical mapping. It cannot dispatch a stale review.
The fixed synthetic CLI policies are test bindings; there is no production
authentication or persistent production policy service in this slice.

Local lock order is Authority read -> store IMMEDIATE -> target IMMEDIATE.
The target writer transaction validates identity/version/value before local fence
commit and remains held until target apply/evidence commit. The durable decision
linearizes at fence commit. `local_action_basis_hold` prevents UPDATE of the
admitted source object while its fence is present, including an old S06 append
transaction (which rolls back). This preserves the data decision across the
local commit/target gap. Revocation waits for Admission publication as in S06.
Target lock release precedes local reacquisition for readback, so there is no
target->store acquisition inversion. SQLite waits are bounded at 250ms.

`possible_dispatch` and `manual_reconciliation` retain a unique composite physical
fence; logical object, request/date and target file path are not part of this key.
Local receipt updates and fence release commit together. Target mutation and
dispatch evidence commit together in the other DB. Errors after local fence
commit return without replay; subsequent dispatch refuses. Readback is explicit
on restart. `inspect` publishes the authorized stored decision only; its `observed`
field is the last recorded review/readback value, not a new target observation.

Fake target metadata binds one persistent journal ID. Each operation checks the
store/target journal join, and each target transaction rereads target identity and
owner metadata. Review binds target identity and canonical path too. Target-side
tokens bind the exact serialized review. Sealing a missing token under the target
writer lock provides explicit durable no-effect evidence and prevents a late
apply of that token. Value-only readback cannot seal, settle or clear a fence.
An applied receipt with mismatching current version/value becomes manual state;
there is intentionally no override/expiry/reset path. Save/print/send do not exist.

## Compatible recovery matrix

| Reader / operation | S06-only store | S07 store with pending action |
|---|---|---|
| Exact S06 Store::open/get/snapshot | Existing behavior | Reads original tables; preserves action state |
| Exact S06 append of another object | Existing behavior | Existing transaction semantics |
| Exact S06 append of fenced basis | N/A | Trigger aborts; full append transaction rolls back |
| S07 reads | Existing behavior | Existing behavior |
| S07 action before explicit prepare_local_actions | Refuses missing action schema | Supported, current Session required |
| S07 reopen/reconcile | No pending state | Fence/proof persists; no automatic dispatch |

The old binary cannot execute S07 actions or settle S07 uncertainty. Switching to
it is a restricted-read rollback, not an undo of a target save. Keep the same local
and target files; do not remove action tables/trigger, delete the fence or restore
an empty database. Pending source-object ingestion resumes only after supported
reconciliation. `prepare_old_reader.py` exports the exact accepted S06 source and
adds only a read-only helper; `rollback_v1.py` will prove the native roundtrip after
Lead compiles it. The already executed Python probe establishes only SQL-level
compatibility, actual process exits and SQLite recovery, not binary compatibility.
No legacy Academy lock/journal compatibility is claimed; that belongs to S10.

S06 ingestion remains trusted. A future untrusted adapter must content-bind lineage
before admission. The S07 fixture loader binds its exact included bytes and JSON
pointer before parsing the admitted object. Frozen S05 partial evidence is never
converted into S07 write authority. No retired Work/Task/Run machinery was added.
