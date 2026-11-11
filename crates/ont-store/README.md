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

## Schema mode (format 3)

`Store::open_schema(path)` / `Store::memory_schema()` explicitly select format
3 (`user_version` 3, the same `application_id` as formats 1 and 2, checked the
same way; a busy lock at open refuses with `store_busy`). Format 1 or 2 files
refuse with `store_failure`, and a format 3 file refuses in the old modes; no
mode converts another. Its tables are `objects`, `observations` (the v1
columns plus `ingested_at`, `schema_domain`, `schema_version` and
`change_kind`), `links` (created, unused), `schema_installs` and `action_log`
(see `schema-v3.sql`). The S07 tables are not created here; they arrive
additively only when a schema declares an effect action (packet O10).

An observation and an action-log row refer to the install they were written
under by `(schema_domain, schema_version)`, a foreign key to
`schema_installs(domain, version)`; the pin is read from that install row (one
home for the pin). Schema change in v1: installing a newer version does not
re-validate stored observations; an old observation keeps its own pin; a type
removed by the newer version is still readable by `head` and `history` and
refuses `unknown_type` for ingest and `scan_heads`.

`install_schema(bytes, InstallArgs { installed_at, source_repository,
source_commit })` keeps, per install, the domain, version, sha256, the exact
schema bytes, the caller-given install time and the optional source
repository/commit. A domain's version only grows; the same (domain, version)
with a different sha256 refuses with `schema_version_conflict`; installing
the exact bytes of the latest installed version again is a no-op; a candidate
is checked (including the type-name clash) against the set read inside its
IMMEDIATE transaction before it is written. Opening the store rebuilds the
`SchemaSet` from each domain's latest stored bytes and re-verifies each pin;
stored bytes that no longer hash to their row's sha256 refuse the open with
`schema_pin_mismatch`. The cached set is never trusted across a transaction
boundary: every transaction (read or write) compares the latest install rows
with the cache and rebuilds inside that transaction when they differ, and the
cache is replaced only after a successful commit. `schema_set()` and
`schema_pins()` refresh the same way.

One public write transaction covers observations and the action log:
`Store::write(now, |tx| ...)` runs the closure in one IMMEDIATE transaction
with a `SchemaWrite` handle (`append`, `log_action`, `head`, `schema_set`),
commits when the closure returns `Ok` and rolls back otherwise.
`append_schema(items, ingested_at)` and `append_action_log(entry)` are thin
callers of it. Ingest rules (per item, inside the transaction): the object
type must be declared and its `fields` must pass `SchemaSet::validate_fields`;
the scope must be `Scope::Domain` of the declaring domain, in that schema's
namespace, with a key set equal to the domain's `scope_keys` exactly (an empty
scope key or value is `scope_keys_mismatch`); the object `Id.namespace` must
equal the schema's namespace; and every `ref` value's `Id.namespace` must
equal the namespace of the schema that declares its target type. A reference
to an object that does not exist in the store is accepted: sources are
indexed independently, and the reference is only a value. The legacy
`seed`/`append_batch` refuse with `schema_violation` in this mode (the old
modes have no dedicated wrong-mode refusal besides `store_failure` at open,
so that code is kept). Refusals are `StoreRefusal { code, violation }`:
validation failures carry code `schema_violation` with the violation code and
pointer available; a moved head carries `revision_conflict`; a repeated
revision carries `duplicate_revision`. Change rules: `initial` only for an
object with no observation (or after a retraction head, which is treated as
having no head; the full history is kept), and it must not name a superseded
head; `update`, `refresh`, `correction` and `retraction` only when
`supersedes` equals the current head; `refresh` must carry fields equal to
the head's fields. A `retraction` observation has empty `fields`, so field
validation is skipped for it. `action_log.action_id` is UNIQUE (a duplicate
refuses with `duplicate_action_id`).

Reads: `head(id)` returns the head and reports a retracted head with
`retracted: true` instead of as absent; an objects row whose head observation
is missing, and decoded rows that fail validation, are `store_failure`, not
absence. `history(id)` returns every observation in append order with its
`ingested_at` and its own pin. `scan_heads(object_type, limit, after)` returns
non-retracted heads in primary-key order `(instance, account, native_key)`,
continuing from the last returned `Id`; a limit above 10,000 refuses with
`limit_too_large`. Every integer crossing into SQLite goes through
`i64::try_from` (`value_out_of_range`).

The busy timeout is 250 ms for the old modes (unchanged) and 10 s for mode
`Schema` at open (`open_schema_with_busy_timeout` selects another value). A
writer that cannot take the lock within it refuses with `store_busy`; it is
not retried automatically.
