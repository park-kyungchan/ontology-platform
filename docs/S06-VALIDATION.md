# S06 source validation and pending build

## Candidate boundary

Contract/oracle bootstrap commit: 1933f9b48f6efc01a7011d9559c9b66169d2fe69.
Contract pin manifest SHA-256: 6441ae200054780e64f8d0be3fc60c4aef5b4176ba9c5061ec76bd9754b1d34c.
Education prerequisite: 049f7c3da0af6c320c818d2afdab3da34bccd37a, read using Git
object bytes, never the concurrently advancing checkout. Copied S05 expected and
audit are byte-identical. Expected SHA-256 is aae41a0f4d79752a0b14d2d4504d5112d4bc576ba6827412675f6789eb6ee234;
audit SHA-256 is 4bdf5431cf2823cf707f9b73d2f7b07b47122369fde7ba7a0831bd9700922c3e.
The original S06 literal expected file conformance/cases.json was independently
authored before implementation. Active cases-v2.json changes only synthetic lineage
digests/pointers to verified source bytes/locations; see S06-LINEAGE-CLARIFICATION.md. Neither
S06 oracle is consumed by candidate Rust code. contract-pins-v2.json preserves
v1 pins and freezes the forward hardening files. Independent acceptance is pending.

## Actual checks performed (no compilation)

- cargo fmt, locked/offline metadata (three packages), frozen file/digest checks,
  exact nine S05 audit pointers and Git whitespace checks: raw stdout/stderr and
  actual statuses in conformance/results/static-run-*/ (initial top-level raw files also retained).
- Comparator self-check: 26 literal CLI cases; 14 known-wrong controls rejected,
  including actual Python child exit 7. No Rust candidate was executed.
- Python stdlib SQLite 3.45.1: schema/identity, explicit rollback, coherent read
  snapshot, bounded second writer, uncommitted child exit 23 and reopened integrity.
  conformance/results/sqlite-python.json records readbacks, not just exit status.
- Native Cargo resolution/fetch only, no build scripts/compiler invoked. Cargo.lock
  pins all 38 registry packages, including rusqlite 0.32.1/libsqlite3-sys 0.30.1.
  Bundled SQLite header reports 3.46.0, so the Python experiment is not evidence
  for the Rust binding's actual runtime behavior.

Initial `cargo generate-lockfile` failed before dependency resolution because ontd
had no source target yet (`missing_target@crates/ontd/Cargo.toml:1`). Source targets
were then added; native lock generation and fetch completed successfully. The
system `pkg-config --modversion sqlite3` probe found no sqlite3.pc. Bundled SQLite
was selected instead of installing a system package. Early observations and command
results are preserved in conformance/results/preparation-log.json. No failure is
relabeled a candidate pass.

## Lead-only compilation and candidate checks

No build slot was granted to this author. The following commands were NOT run.
First the Lead reconciles its sole build slot, current resources and exact candidate
HEAD. Preserve stdout/stderr/actual status for each command and bind reports to HEAD.
From the exclusive checkout, with a Lead-owned fresh target directory:

```bash
source /home/palantirkc/work/ws-replan/preparation/final-review/execution-env.sh
export CARGO_TARGET_DIR=/home/palantirkc/scratch/rust-s06-build/target
cargo +1.95.0 check --workspace --all-targets --locked --offline -j1
cargo +1.95.0 test --workspace --all-targets --locked --offline -j1
cargo +1.95.0 clippy --workspace --all-targets --locked --offline -j1 -- -D warnings
cargo +1.95.0 build -p ont-daemon --locked --offline -j1
python3 conformance/verify.py --candidate "$CARGO_TARGET_DIR/debug/ont-daemon" --out conformance/results/candidate-01
```

The process comparator preserves all 26 actual frames and exit statuses. Rust
library tests challenge stale/foreign/revoked sessions, read-versus-derive and linked
derive narrowing, forbidden-evidence invariance, publication/revocation ordering,
atomic batches/head conflicts, immutable scope, file reopen and writer contention.
Six test functions cover those challenges. Re-run static checks without compilation with
`python3 conformance/static_checks.py --out conformance/results/static-run-NEW`. Tests are authored, not run. Review the
forward snapshot/lineage clarifications v2 before accepting the corresponding expectations.

## Unproven and excluded

Compilation, Rust tests/clippy, bundled SQLite behavior, process conformance and
independent semantic review remain pending. Source delivery alone is not operational
success. No live account credential or teacher authentication, physical writable
field selection, root ownership, HTML/parser parity, live HTTP policy, durable
external action/recovery or production rollout is established. S05 remains bounded;
S07 has no implementation; S12 still needs the actual user's scenario verdict.
No child model agent was spawned, no expensive compiler process was started, and
all finite Cargo resolution/Python command children completed. Lead owns session
retention/evidence closure; task STATE/ORCHESTRATION, source EDU/AP, global/native
configuration and Hindsight service/database were not modified by author tools.
