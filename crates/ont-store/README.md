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
same way). Format 1 or 2 files refuse with `store_failure`, and a format 3
file refuses in the old modes; no mode converts another. Its tables are
`objects`, `observations` (the v1 columns plus `ingested_at`, `schema_sha256`
and `change_kind`), `links` (created, unused), `schema_installs` and
`action_log` (see `schema-v3.sql`). The S07 tables are not created here; they
arrive additively only when a schema declares an effect action (packet O10).

`install_schema(bytes, installed_at)` keeps, per install, the domain, version,
sha256, the exact schema bytes and the caller-given install time. A domain's
version only grows; the same (domain, version) with a different sha256 refuses
with `schema_version_conflict`; installing the same bytes again is a no-op;
a candidate is checked against the type names of the other installed domains
before it is written. Opening the store rebuilds the `SchemaSet` from each
domain's latest stored bytes and re-verifies each pin; stored bytes that no
longer hash to their row's sha256 refuse the open with
`schema_pin_mismatch`. `schema_pins()` returns the current `domain ->
SchemaPin` map.

`append_schema(items, ingested_at)` is the typed ingest path (the legacy
`seed`/`append_batch` refuse with `schema_violation` in this mode). The whole
batch runs in one IMMEDIATE transaction or nothing is written. Per item: the
object type must be declared and its `fields` must pass
`SchemaSet::validate_fields`; the scope must be `Scope::Domain` of the
declaring domain, in that schema's namespace, with a key set equal to the
domain's `scope_keys` exactly; the object `Id.namespace` must equal the
schema's namespace; and every `ref` value's `Id.namespace` must equal the
namespace of the schema that declares its target type. A reference to an
object that does not exist in the store is accepted: sources are indexed
independently, and the reference is only a value. Refusals are
`StoreRefusal { code, violation }`: validation failures carry code
`schema_violation` with the violation code and pointer available, while a
moved head carries `revision_conflict`. Change rules: `initial` only for an
object with no observation (or after a retraction head, which is treated as
having no head; the full history is kept); `update`, `refresh`, `correction`
and `retraction` only when `supersedes` equals the current head; `refresh`
must carry fields equal to the head's fields. A `retraction` observation has
empty `fields`, so field validation is skipped for it. Each observation row
stores the sha256 of the schema pin it was written under (`schema_sha256`),
because installs grow versions and a later pin must not be read as the pin
of an older observation.

Reads: `head(id)` returns the head and reports a retracted head with
`retracted: true` instead of as absent; `history(id)` returns every
observation in append order with its `ingested_at` and `schema_sha256`, and
the unchanged `Snapshot::get` still returns the full `Object` history;
`scan_heads(object_type, limit, after)` returns non-retracted heads ordered
by `native_key` with `after` as the exclusive continuation key, in one query
per page. `append_action_log(entry)` stores one `action_log` row and returns
the store-assigned `seq`; no action logic lives here.

The busy timeout is 250 ms for the old modes (unchanged) and 10 s for mode
`Schema` at open (`open_schema_with_busy_timeout` selects another value). A
writer that cannot take the lock within it refuses with `store_busy`; it is
not retried automatically.
