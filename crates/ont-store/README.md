# ont-store

Consumer: ontd composition. Public contract: isolated memory/file Store, trusted
atomic seed/append_batch, transaction-bound ReadSnapshot. Uses rusqlite 0.32.1 with
bundled libsqlite3-sys 0.30.1 (SQLite source 3.46.0); no system install.
Schema version 1, application ID 1330533430, composite BINARY keys, FULL synchronous,
DELETE journal, 250ms busy timeout, foreign keys. Unknown existing databases refuse;
S06 opens only memory or disposable synthetic files. Not an Academy journal,
Hindsight database or authorization entrypoint.

S07 explicitly adds action tables and a pending-basis UPDATE hold trigger without
changing S06 tables/application/user version. A separate closed fake target has
its own application ID, journal binding and transactional dispatch evidence.
All S07 files are isolated synthetic local state. See docs/s07/IMPLEMENTATION.md
for the lock order, sealed no-effect capability and old-reader compatibility.

`Store::open` / `Store::memory` retain the Education-only schema-v1 contract.
`Store::open_program` / `Store::memory_program` explicitly select a separate
Program-only schema-v2 projection. V2 uses the same application ID and object,
observation and link tables; it installs no action tables and refuses S07 action
preparation. Both modes validate the selected scope domain, required keys and
exact Id namespace during intake and snapshot reads, including link endpoints.
A failed seed/append rolls back the entire transaction.

Opening the other mode's existing version, a future version or an unknown database
refuses with `store_failure` before reading object rows. Neither mode upgrades,
downgrades, rewrites scopes or eagerly repairs corrupt rows. Use only an explicitly
task-owned new path for Program projections. Old S06/S07 binaries reject v2 at open;
rollback disables the Program route and retains its projection/source captures.
Keep Education action files and unresolved fences intact during reader rollback.
