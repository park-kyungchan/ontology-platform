# ont-store

Consumer: ontd composition. Public contract: isolated memory/file Store, trusted
atomic seed/append_batch, transaction-bound ReadSnapshot. Uses rusqlite 0.32.1 with
bundled libsqlite3-sys 0.30.1 (SQLite source 3.46.0); no system install.
Schema version 1, application ID 1330533430, composite BINARY keys, FULL synchronous,
DELETE journal, 250ms busy timeout, foreign keys. Unknown existing databases refuse;
S06 opens only memory or disposable synthetic files. Not an Academy journal or
Hindsight database, external-effect ledger or authorization entrypoint.
