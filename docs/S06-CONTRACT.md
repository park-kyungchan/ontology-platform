# S06 restricted operational read v1

New design, bounded to S06. Authority: current user/STATE, adopted review ZIP
964845e260a25350823fb42d38a60e7cba973f8af607e10b505727d3bf78915c,
PROPOSAL sections 6/7 and S06, ONT-01/02/03/04/06. Old plan/03-architecture
supplies constraints only; no AP lifecycle, service, registry or action is adopted.

## Identity and evidence

Identity is the JSON tuple represented by `Id {object_type, namespace: {instance,
account}, native_key}`. All strings are nonempty exact UTF-8; no trim, split,
normalization, case fold, numeric cast or name resolution. SQLite uses separate
BINARY TEXT columns, never a joined key. Instance AND account are mandatory.
Synthetic accounts A/B deliberately share an instance and native key. A recording
without authenticated account provenance can only inhabit an explicitly synthetic
recording namespace; teacherLabel, filename, frame and Host UID confer no grant.

Each identity has an immutable class/student scope and explicit head revision.
An observation has revision, source_revision, observed_at/fresh_until Unix seconds,
initial/correction/backfill, supersedes, partial/complete coverage, fields and
artifact SHA-256/JSON pointer lineage. Initial has no predecessor. Corrections and
backfills name the current head and append history atomically; no revision sorting
or replacement by timestamp. Scope changes require a different identity, not merge.
Knowledge variants are missing, null, blank, unknown, not_performed and value;
zero is value(0). Raw recorded JSON is also preserved. Freshness is `now > fresh_until`;
expiry equality remains fresh. Backfill may be older than its predecessor.

## Trusted principal and publication boundary

ont-core defines domain principals and grants independently of host users or agent
roles. The embedding application owns Authority construction, credential validation,
clock and trusted session binding. `bind_trusted` is explicitly NOT authentication.
S06 has no production credential adapter. The only CLI is `ontd --synthetic`: a
fixed synthetic principal/policy, compiled fixtures and fixed clock, no caller
principal, grant, source path, role, UID, time or policy input. Unknown fields refuse.
It demonstrates no real teacher entitlement, root ownership or live permission.

A session is opaque, tied to one Authority instance and policy generation. Every
query, traversal and derived decision validates it against current Authority.
Revocation/replacement monotonically advances generation, invalidating old sessions.
A read guard spans SQLite snapshot, policy filtering, derivation AND serialized
response construction. Concurrent policy replacement waits for this short request;
a request admitted before revocation may finish, later requests cannot use it.
Already returned bytes cannot be retracted. This is a linearizable local boundary,
not a remote distributed revocation guarantee. Direct store APIs are trusted ingestion/
internal persistence APIs, not untrusted read entrypoints. Callers use Service.

A read grant must exactly match namespace/class/student and object type. Derive is
separate and requires read as well. Get/root forbidden and absent both refuse
`unavailable`. One-hop outgoing traversal rechecks each target; forbidden targets,
IDs, edges, counts and their evidence never enter output. No hidden total or filtered
count. Decision re-reads its basis under the same current policy/snapshot; it never
accepts caller-supplied objects. All included objects require derive as well as read;
missing derive on an otherwise visible basis refuses the entire decision. Forbidden
objects do not change decision or output. Root is included once, targets deduplicated
and ordered lexicographically by the structured Id (UTF-8 string order).

## Deterministic useful read

Queries are strict tagged JSON: get(id), traverse(root,relation), decide(root,relation).
Traverse returns only permitted targets. Decision returns `review_required` if any
included head is stale, partial, has no fields, or contains missing/null/blank/unknown/
not_performed. Otherwise it returns `evidence_available`. Neither value asserts
mastery, assignment, whole-book completion, nonperformance, save or teacher finalization.
History remains visible with each observation's independent freshness. The synthetic
student -> textbook/progress vertical contains corrected partial progress and older
backfill. A completely evidenced synthetic target supplies the other decision branch.

Response is exactly {status:ok,authorization_revision,result}; object results contain
kind:objects and objects; decisions contain kind:decision, decision and basis.
Object views contain id, scope, head and observations, with stale per observation.
Errors are {status:refused,code}. CLI writes one compact JSON line, no stderr for
expected refusals; exit 0 on ok, 2 on refusal. Unexpected IO/store failures exit 1
and print a located stable code without payload. Input <=64KiB; one request then EOF.
Library failures are stable codes; persistence is not exposed over network.

## Recorded S05 constraint

Pinned Education source: 049f7c3da0af6c320c818d2afdab3da34bccd37a.
Frozen expected bytes: aae41a0f4d79752a0b14d2d4504d5112d4bc576ba6827412675f6789eb6ee234.
The copied S05 expected file is recorded INPUT to a narrow demonstrator adapter,
not the S06 oracle. It imports only sections-edition-a and attempt-knowledge-states
for synthetic-student-1 into an explicit synthetic recording account. Sections keep
placeholder and edition IDs, attempts keep all seven raw rows and knowledge states.
It validates the whole file digest plus audit hash and each row's source pointer/value.
No HTML/parser, native account, physical-field, live operation-policy or root-ownership
proof is inherited. All imported observations stay partial and stale at the fixed
November clock; empty/exhausted source selection is never promoted to completeness.
The Academy-specific adapter lives in ontd; generic core/store have no Academy names.
No source request, journal, lock, database, save, print, PDF or parent send occurs.

## Store and scope

ont-store owns only isolated local SQLite schema, trusted ingest and snapshot reads.
S06 uses a memory database in CLI; file-backed reopen is tested on synthetic paths.
Atomic observation/head updates, duplicate rejection, foreign-key links and snapshot
coherence are required. One writer, short reads, FULL synchronous, DELETE journal,
bounded busy timeout are sufficient here. No external effect ledger or action recovery
claim. Python stdlib experiment is separate evidence from uncompiled rusqlite behavior.
Bundled SQLite avoids a host system sqlite3-dev install. No daemon or network listener.

## Conformance frozen before candidate

conformance/cases.json contains literal full expected values, authored from this
contract and fixture bytes before Rust implementation. `contract-pins.json` freezes
contract, fixture and oracle hashes. Candidate does not load cases.json. The Python
comparator requires actual exit, empty stderr, one compact JSON line and exact values,
including missing keys and array order; negative controls qualify the comparator.
Library conformance separately challenges stale sessions, cross-Authority sessions,
derive narrowing, policy replacement, head conflicts and reopen/rollback semantics.
Independent reviewer must challenge this contract/candidate. Author source delivery,
static checks and Python SQLite experiments are not compilation, S07 or S12 acceptance.
