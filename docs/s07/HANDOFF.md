# S07 bounded source handoff (not an acceptance)

## Authority / exact target

Overall initiative, goal, task STATE, build slot, integration and P8 verdict remain
Lead-owned. Current user/STATE/adopted ZIP override dated memory/WP/harness.
Read `/root/.agents/AGENTS.md` and `/home/palantirkc/work/ws-replan/STATE.md` on resume.
This worker writes only the assigned source checkout; no task-note writes.

- Checkout: `/home/palantirkc/work/ws-replan/wt/ontology-platform`.
- Common Git: `/home/palantirkc/platforms/ontology-platform/.git`.
- Branch: `rust/s07-local-actions-20261102`.
- Exact accepted prerequisite: `224864887d19f2c52e5026568f5e9b169314d3be` (S06).
- Frozen contract commit: `430d186` (before Rust implementation).
- Forward-only strict-judge correction: `83c25ae`; original v1 literals unchanged.
- Final exact candidate revision is returned by the worker with `git rev-parse HEAD`.

Goal: S07 only, one deterministic authorized function and persistent LOCAL FAKE
action, physical-field dispatch fence, authoritative readback and explicit recovery.
No production authentication, real Academy/Hindsight DB/locks, HTTP/GET, private
data, print/send, global config, service/deployment, push/merge or child models.
No compiler was granted to the author. No S12/P8/live acceptance claim.

## Returned source / pinned contract

New core `action` module; named-grant opt-in; additive store actions/fence/basis-hold
schema; separate concrete fake SQLite target with atomic receipts and absence
sealing; Service composition and separate `ont-local-fake` binary; focused tests,
literal subprocess judge, SQL design probe and real old-reader roundtrip tooling.
Existing S06 query code and frozen oracle/fixtures/pins remain intact. Only its
synthetic grants acquire an empty action set. `default-run=ont-daemon` preserves the old
Cargo invocation after adding a second binary. No new dependencies or lock changes.

`CONTRACT-v1.md` SHA256:
`aaea3ac7bf7909035ce2fff4b1ef00a92c7514ba303b88c69a43c5407082652c`.
Source fixture SHA256:
`c0f61e899b944597ef95a63d09ee9e52a2a408980524cd00b9ae875bba2493a2`.
24-scenario/106-step literal oracle SHA256:
`e8c18de131f900c01326e7b3a1be8bbb5d1dbb6a11d5a977bef24d2225dca809`.
v2 comparator SHA256:
`8b42b1c0c219c89dbe1152f378922a5f50cfc7dc9f719f7ef7404c93afa4f339`.
Manifests: `conformance/s07/pins-v1.json`, `pins-v2.json`; original S06
`conformance/contract-pins-v2.json` still has all 10 matching pins. Cargo.lock SHA256
remains `b52588aff0741668d076a85684a02f4194afa0182132eaac51a76608d48c3d47`.

## Actual permitted evidence

- `conformance/s07/results/static-final-01/report.json`: static fmt, locked offline
  metadata/fetch, S06 and S07 judge self-checks, Python syntax, old-helper rustfmt,
  staged/unstaged diff checks. Raw stdout/stderr and actual child statuses retained;
  source hashes bind this check to the inspected content. No Rust compilation.
- `results/static-s06-01/static-report.json`: earlier S06 static checks, 10 pins,
  26 cases, 14 negative controls, 9 S05 exact audit-pointer rows. All five statuses 0.
- `results/design-02/report.json`: Python SQLite DESIGN experiment only, actual
  child exits `[81,82,83,84,85,86,89,87,88]`, reopened target/local readbacks and
  integrity checks, field fences, sealed no-effect recovery, preserved old rows,
  idempotent additive installation and rollback of an old append transaction while
  pending. Parent status 0. `design-01` and its original probe source are preserved.
- S07 v2 judge self-check: 2 positive / 18 rejected negative controls. Actual Python
  child statuses `[7,85]`; parent 0. This is not a Rust candidate result or an
  independent review. v1's Python boolean/duplicate-key defect is documented in
  `JUDGE-v2-CORRECTION.md`; v1 itself is immutable.
- 8 new Service integration tests and 3 private target primitive tests are authored,
  including current/foreign/revoked sessions, forbidden output, stale source,
  target/journal identity, failed fence commit, actual child death, pending source
  hold, narrow reauthorization, no-effect retry proof and S06 data preservation.
  They have NOT been compiled or run here.

## Precise Lead-only build / fault / restart / rollback commands

Run sequentially only after Lead grants the sole build slot. Preserve each real
command's status/stdout/stderr using the Lead's native build evidence route. Stop
at failure; fix forward, never change expectations to match a candidate. Select
fresh output directories if `build-01` is already occupied. No network required.

```bash
cd /home/palantirkc/work/ws-replan/wt/ontology-platform
source /home/palantirkc/work/ws-replan/preparation/final-review/execution-env.sh
git status --short --branch
git rev-parse HEAD
S07_EVIDENCE=/home/palantirkc/work/ws-replan/preparation/slices/S07/build-01
S07_TARGET="$S07_EVIDENCE/target"
mkdir -p "$S07_EVIDENCE"
cargo test --locked --offline --workspace --all-targets -j1 --target-dir "$S07_TARGET"
cargo clippy --locked --offline --workspace --all-targets -j1 --target-dir "$S07_TARGET" -- -D warnings
cargo build --locked --offline --workspace --bins -j1 --target-dir "$S07_TARGET"
python3 conformance/verify.py --candidate "$S07_TARGET/debug/ont-daemon" --out "$S07_EVIDENCE/s06-process"
python3 conformance/s07/verify_v2.py --binary "$S07_TARGET/debug/ont-local-fake" --results "$S07_EVIDENCE/s07-process"
python3 conformance/s07/prepare_old_reader.py --out "$S07_EVIDENCE/old-reader-prep"
cargo build --locked --offline --manifest-path "$S07_EVIDENCE/old-reader-prep/source/Cargo.toml" -p ont-daemon --bin s07-old-reader -j1 --target-dir "$S07_EVIDENCE/old-reader-target"
python3 conformance/s07/rollback_v1.py --binary "$S07_TARGET/debug/ont-local-fake" --old-reader "$S07_EVIDENCE/old-reader-target/debug/s07-old-reader" --results "$S07_EVIDENCE/old-reader-roundtrip"
```

The process judge runs all 106 steps as separate real Rust children: review
commit, fence commit, target commit, readback and receipt faults/restarts. It checks
raw exit codes and authoritative SQLite state as well as complete output/scopes.
It tests new request+date+logical-occurrence fencing, distinct same-key accounts,
value-only uncertainty, conflicting target readback and explicit supported recovery.
`rollback_v1.py` runs exact accepted S06 Store::open/snapshot in a separately built
archive through two unresolved S07 states, compares full old output and preserved
side tables, and then proves new restart/reconciliation without duplicate effects.
The added helper is read-only; the archived S06 library source is unchanged.

For a single manual restart: initialize an absent absolute root using input
`{"op":"init"}`, review r1 from `cases-v1.json`, then use
`ont-local-fake --root ROOT --fault after_target_commit` with that case's dispatch.
Expected actual child exit is 86, with local possible_dispatch/fence=1 and one
target effect. A NEW `inspect` process must retain possible_dispatch; a NEW
`reconcile` with evidence=value_only must retain it; a NEW `dispatch` must refuse
retry_forbidden/exit2. Only explicit evidence=durable reconciliation may establish
persisted. The fixed judge already contains these exact inputs/outputs/statuses.

## Remaining uncertainty / handoff

Lead must compile, run the native tests/106-step faults/S06 regression and actual
old-binary roundtrip, then obtain independent semantic review (including the
forward-only v2 judge correction and source-basis hold trigger). The author has
not self-accepted the slice and cannot release S10/S12 dependencies.
Filesystem/media failure, hostile file replacement, arbitrary journal copying or
deletion and real external transports are not demonstrated. Trusted authentication,
policy generation ownership and clock remain embedding responsibilities; fixture
profiles do not establish them in production. Recovery may use a current narrower
session only within the original owner/scope/mapping. There is intentionally no
manual fence-clearing API. Target absence sealing is fake-only, not Academy proof.

All launched author command/probe children finished; no builds/background workers
or model children were launched. Synthetic probe directories were disposable and
removed; raw logs/readbacks remain source-owned. No real business uncertainty was
created. Lead owns final native process/MCP retention reconciliation and pane close;
the next bounded task should start a fresh native session.

Existing Rust Hindsight initiative was searched/read and reused, not recreated or
claimed by this worker. Official per-call guide:
`/docker/hindsight/HINDSIGHT_OFFICIAL_PER_CALL.md`; supplied coding-agents 0.8.2 /
Hindsight 1.1.0 revision `a5a619b5ca59c5c39e0f96be6f7a31fabd136bd5` was checked
against the pinned official Git source. This worker performed no Hindsight
configuration or migration/data mutation; native session retention is runtime-owned.
