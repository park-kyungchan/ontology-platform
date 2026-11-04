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
