# S07 independently reviewed limits

Executable source `edfebb6ce9e71d253c8cfee60ab9c279da7d9f75` passed native
tests, strict Clippy, S06 regression, 24 scenarios with 106 real Rust children,
and two real archived-S06 reader roundtrips. Independent Wonder reran these
checks and additional account/date/fence/value-only probes. This document
records three Low findings without changing frozen contracts or expectations.

- The public dispatch precheck returns `target_conflict` before a physical
  fence exists when the target drifted. That request remains `reviewed` and
  holds no fence; a fresh request can proceed. The `conflict_no_effect` state
  is exercised by a private target primitive, not reachable through that
  public dispatch path. Do not cite the primitive as public-path coverage.
- The archived S06 binary is a read-only helper over unchanged S06 library
  code. It proves old reads preserve unresolved S07 side tables and state.
  Old append rollback is supported by the database trigger and an identical
  append-path test in the new binary, not by an old-binary append execution.
- Review may return `store_failure` after its local review commit if the
  target read transaction fails to commit. Inspect before assuming absence.
  Retrying the identical review is idempotent; this does not authorize replay
  of a business action. A failed value-only reconciliation likewise retains
  the physical fence and `possible_dispatch`.

Target absence sealing and receipts are capabilities of this local fake
target, not evidence about Academy HTTP. Production authentication, policy,
clock and file custody remain embedding responsibilities. Moving a bound
root or copying/replacing journals is not an authorized recovery shortcut.
Generic ingestion remains trusted; a future untrusted adapter must bind its
lineage content before admission. Source acceptance is not deployment, an
actual P8 user verdict or live authority.
