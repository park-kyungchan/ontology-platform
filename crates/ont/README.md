# ont

The `ont` command: `schema check|install`, `describe`, `get`, `backup`, `index`, `search`, `around`, `aggregate`, `action list|validate|apply` over a format 4 store (format 3 migrates on open).

`describe` lists readable views with their `name`, `args` and `object_type`,
including through `ont_describe`. Empty or whitespace-only stdin defaults to `{}`
for `describe` and `action list`; commands with required fields report the first
missing field. Reference search filters require a full Id, obtainable by searching
the referenced type. Refusals may include a top-level `hint`; submission hints use
fixed recovery text selected by failed criterion codes (unknown codes get a generic
repair), without request values. Store creation and existing-store opens refuse
symbolic links at the store file, its containing directory, or that directory's
parent (exactly two directory levels; aliases above them are allowed), SQLite sidecars
(`-journal`, `-wal`, `-shm`), `edits.jsonl` or its lock with
`store_path_is_symlink` and field `--store`. SQLite opens use
`SQLITE_OPEN_NOFOLLOW` against a swapped final database path; the parent path is
canonicalized after the guard so host aliases above the two checked directory
levels can open normally. Parent and adjacent file checks are pre-open checks: concurrent replacement after inspection remains
outside this guarantee; edit-log descriptors separately use `O_NOFOLLOW`. Default-store aliases retain
`install_requires_commit`. Git provenance strips userinfo only when its host boundary is
certain. Raw-slash credentials and ambiguous port/path-`@` forms use the checkout
path as provenance instead; `strip_credentials` returns `None` for ambiguity.
Scp-like userinfo requires exactly one `@` and no `/` before the first `:`;
a `:` before the first `@` falls back rather than assuming a password boundary.
Ordinary ports and local paths remain intact.

## actions
- Requests are one JSON object on stdin, one JSON line on stdout; exit 0 ok, 2 refused, 1 internal. `ont [--store PATH] [--client NAME] action list` takes `{"op":"list_actions","domain"?}` and lists each granted action type with its parameters (name, type, required, allowed values), criteria codes, messages and condition expressions (including `present`), and the object types its rules edit. `action validate` takes `{"op":"validate_action","action_type","parameters":{…},"agent"?,"session"?,"request_id"?}` and answers `valid`, every criterion with `passed`, `basis` (the heads read with their revisions), `edits` (what apply would write; with a `request_id`, the same revisions apply would write) and the schema pin in the envelope; it writes nothing. `action apply` takes the same fields plus `"expected":[{"id","revision"}]?` and `"request_id"?`.
- Apply runs in ONE IMMEDIATE transaction: grant check (`action_forbidden`), parameter typing (`invalid_parameter` with the parameter name), every head named in `expected` re-read and compared (`revision_conflict` naming the id), every criterion re-evaluated (`submission_criteria` with the failed codes and their declared messages), action id allocated, the observations of every rule appended, the `action_log` row inserted, commit; then the `edits.jsonl` append. When `expected` is absent, the transaction itself still guarantees that the criteria were evaluated on the heads it wrote against, because both happen inside the same IMMEDIATE transaction.
- Criteria forms evaluated: the criterion arms `all`/`any`/`none` and the conditions `present`, `exists`, `fresh`, `eq`, `ne`, `in`, `lt`, `le`, `gt`, `ge`, `linked` (on the reference properties of the current heads; no link table), `actor`, and nested `all`/`any`/`none`. Rules: create object (key from a parameter or the action id), modify, create-or-modify, delete (appends a `retraction`), set and clear a reference property. Only Ontology-owned types are edited; a source-owned target refuses defensively at apply with `action_edits_source_owned`. An Ontology-owned value has no freshness horizon (`fresh_until` is 9999-12-31); its lineage points to the action-log record (`/edits/<n>`).
- Idempotent retry: with `request_id` (1 to 64 characters of `[A-Za-z0-9._-]`), the action id is `act-rq-` plus the first 32 hex characters of the SHA-256 over the client, the agent (empty when absent) and the `request_id`, each field prefixed by its byte length, so the same `request_id` from another client or agent is another request; a second apply with the same `request_id`, action type and canonical parameters returns the first result with `"outcome":"already_applied"` and writes nothing; the same derived id with different content is the refusal `request_id_reused`.
- Attribution: the actor `{principal, client, agent, session}` reaches the log: `client` from `--client NAME` of the command (default `cli`), `agent` from the request field or the environment variable `ONT_AGENT`, `session` if given; all self-reported. The log row and the apply answer mark this with `"actor_trust": "self_reported"`: the actor is a claim, not authentication. On the library path `agent`, `session` and `client` are each 1 to 64 characters of `[A-Za-z0-9._:-]`, and the total size of `parameters` is bounded as on the command path (`invalid_request` otherwise).
- `edits.jsonl` beside the store (0600, opened without following a symlink): one canonical JSON line per applied action after the commit, with `fsync`. After the commit the action stands whatever happens to the file: the answer is `"outcome":"applied"` with `"edits_log"` = `written`, `pending` (the append failed; the next write repairs it) or `rebuilt`. Every write takes an exclusive `flock` on `edits.jsonl.lock` beside the store around its read-compare-append, writes each line with its newline in one write, and truncates an incomplete trailing line before appending. The file is valid only if its lines equal the table's rows in order, by sequence and by content; a diverged file (extra or changed lines, invalid UTF-8, an unparseable complete line) is renamed to `edits.jsonl.diverged-<UTC yyyymmddThhmmssZ>-<n>` (`n` counting up from 0 until the name is free, never replacing an existing aside) and a fresh file is written from the table (the database wins, nothing is lost). The table is read only after the `flock` is held, so no caller passes a reading taken before the lock. An existing file that is not 0600 owned by the caller is not appended to (`edits_log: pending`, one warning on stderr).
- Creating an object over a retracted key is allowed: it starts a new history for that key with an `initial` observation, and the retracted history stays in the object's history.
- Snapshot: apply and validate load parameter objects (except parameters used only by bare `present` and not needed for rule planning), expected-revision objects, one reference hop from the initially loaded heads, the objects a rule edits, and paged reverse `linked` scans. Admission reads only identity, scope and liveness before loading observation payloads; only admitted live heads are traversed or start reverse scans. Heads a scan filters out are never loaded or counted against the hard snapshot bound; every admitted loaded head counts, and exceeding it refuses with `snapshot_too_large` and writes nothing. Snapshot loading includes reverse-link scans from admitted sources in every written criterion operand, including an operand that ordered `all` later skips; the bound limits loading, not just evaluation.

## Action values and parameter descriptions

A rule's `set` value or `set_reference.value` may use `$parameter.property`
when `parameter` is a reference. For example,
`"set":{"task":"$decision.task","topic":"$decision.topic"}` copies the
reference and topic from the decision. Only one property level is supported;
unknown properties, non-reference parameters and incompatible destination
property types refuse at `schema check`. The value comes from the original
snapshot, even when an earlier rule in the same action modifies that object.
The referenced head and revision are included in `validate`'s `basis` and the
applied action's log. Pass that `basis` as `expected` to apply: any intervening
change refuses with `revision_conflict`. Without `expected`, apply reads the
current value inside its transaction and logs that application basis. A copied
value needs a fresh, readable source: every grant listing the action must include
the referenced type in `read` (`reference_read_forbidden` at the expression's
schema pointer otherwise). At runtime the native read admission policy also
checks the object's grant scope. At the reference-admission boundary, missing,
retracted and inadmissible sources refuse `rule_reference_unavailable` with the
same detail-free answer. Availability of all rule reference inputs is checked before staleness or
expected revisions. Criterion `$parameter.property` admission and freshness
checks run only when reached in evaluation order, after snapshot loading. A
present object whose requested property has no value refuses `rule_value_unavailable`. A source past `fresh_until` refuses
`rule_value_stale`; `validate` returns `valid:false`, no edits and a failed
`preconditions` entry with that code, while retaining the criteria results and
computed basis. A criterion that needs a stale property is marked `passed:false`
with `precondition:rule_value_stale`; independent criteria are still evaluated.
The freshness boundary is inclusive; Ontology-owned objects retain their existing
indefinite freshness.

`"set":{"closed_at":"$now"}` writes the trusted application time as a UTC
string such as `"2026-11-12T00:00:00Z"`. It uses the same time as the existing
log's `submitted_at` and the observations' `observed_at`; no log or store format
changes. Validation returns `"application_time":"set_at_application"` and does
not fix that time. A retry with the same `request_id` returns the original edits
and leaves the original value and time intact. `$now` requires a string or
string-based value-type destination, and reserves the parameter name `now`.
Times beyond year 9999 refuse with
`application_time_out_of_range`. For a patterned string value type, schema
checking accepts the canonical UTC shape
`[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z` (also with `\d`,
optional `^`/`$`), or `.*`. Other patterns refuse `invalid_rule` at the `$now`
expression: arbitrary regex/time-language intersection is not supported.
The actual applied value must still satisfy the value type's other constraints.

Submission comparisons `eq`, `ne`, `lt`, `le`, `gt`, and `ge` also accept `$now`
on either side, for example `{"le":["$observed_at","$now"]}`. The other operand
must be a string parameter, a string `$parameter.property`, or a string literal
(string-based value types are accepted, including patterned ones). In these
comparisons unprefixed strings on either side are literals; name a referenced
property explicitly with `$parameter.property`: `["$source.observed_at","$now"]`
reads the source property, while `["observed_at","$now"]` compares the literal
word `observed_at`. A literal equal to a declared action parameter name refuses
`invalid_criterion` with a message to write `$<name>` instead.
Comparisons use the existing string ordering and the same full UTC text that a
rule value `$now` stores.
`validate` evaluates against its validation time and returns
`"application_time":"set_at_application"`, including when the criterion fails;
it does not promise the same result at a later apply. Apply evaluates against
its application time, once inside the write transaction. An applied request's
retry returns `already_applied` with its original edits without evaluating the
criteria again. Both `--now` and the library's injected clock drive this value.
`$now` remains unavailable in view filters, anywhere inside `linked ... where`
(including nested `all`/`any`/`none`), and other criterion forms.

String ordering has chronological meaning only when both operands use the same
full UTC form, such as `2026-11-12T00:00:00Z`. With that application time, `le`
accepts the empty string `""`, the date without time `"2026-11-12"`, and the offset
time `"2026-11-11T23:30:00-02:00"`. The last value represents a later instant;
the language compares text and does not parse or normalize times. A schema that
needs chronological safety gives the parameter a string value type with the
full UTC pattern, for example
`"utc_observed_time":{"base":"string","pattern":"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$"}`
and `"observed_at":{"type":{"value_type":"utc_observed_time"},"required":true}`.
This fixes the representation; calendar validity requires tighter ranges too.

A parameter declared `"required":false` accepts omission or JSON null. A rule
value using that absent parameter leaves its destination unset on create and
unchanged on modify; if nested in an array or struct, the entire destination
property is skipped. A nonempty rule set whose values are all skipped produces
no edit or object revision. A create rule whose inputs are all absent creates no
object. Modify and reference rules still resolve their targets before skipping:
a missing or retracted target refuses just as it would with supplied inputs.
An action whose rules all skip still records one action-log entry with `edits:[]` for idempotency, with no per-object observation;
its retry returns `already_applied` with the same empty edits. An optional
parameter feeding a required property on create refuses `invalid_rule` at its
expression during schema checking. Required destination properties also undergo
the ordinary field check. Omission and null have the same canonical request content for
idempotency. Criteria use the existing `{"exists":"$label"}` form to test a
scalar or array parameter's presence (empty strings and empty arrays are present;
array elements are not tested for liveness). For a single reference, `exists`
checks that the referenced object exists and is live. An absent optional reference
therefore fails `exists`.
Bare-parameter `exists`/`fresh`, bare-property comparisons, `linked` source and
candidate reads, modify/delete/`set_reference`/`clear_reference` target reads, and
`expected` revision reads give byte-equal answers for a missing and an unreadable
object, including at the snapshot bound; creation inherits scope only from
admitted parameter objects. At the target-read boundary, missing, retracted and unreadable modify, delete and
reference targets refuse with the detail-free `rule_reference_unavailable` used
by `$parameter.property`. `create_or_modify_object` is an exception:
it creates when missing and refuses when unreadable, because creating a missing
object must not overwrite an existing object the caller cannot read. Comparisons
with an absent parameter remain unevaluable
and refuse when evaluated. `all` evaluates operands in written order and stops
at the first false operand, so a presence test can guard a later comparison:
`{"none":[{"all":[{"present":"$supersedes"},{"eq":["$record","$supersedes"]}]}]}`
permits an absent optional `supersedes` and refuses self-supersession, including
a brand-new `record` id naming itself before its target exists. Using `exists`
here would let that new id pass because its target is not live yet. `any` and
`none` keep evaluating every operand; a stale or unevaluable operand still fails
the criterion even after an earlier operand determines its Boolean result.
A requirement of freshness or read access must stand in its own criterion or
before any operand a caller can make false, because `all` skips everything after
its first false operand. Ordinary properties cannot be cleared by a `set` value:
absent/null parameters mean skip, not clear, and no scalar clear rule exists.
Reference properties can still use the existing explicit `clear_reference` rule.

Object properties and action parameters accept an optional `description` string
of at most 300 UTF-8 bytes, for example
`"reason":{"type":"string","required":false,"description":"Why the hold ended."}`.
`describe` shows both; `action list` and `ont_action_list` show parameter
descriptions. The existing MCP response and tool-list bounds still apply.

## Presence operators

| Operator | True when | Reads |
| --- | --- | --- |
| `{"present":"$name"}` | The request supplies a non-null parameter, including a required parameter, empty string or empty array. | None; a missing, retracted or unreadable reference target still counts as supplied. This operator adds no snapshot heads or basis. |
| `{"present":"$name.property"}` | The referenced head holds the declared property in the value state. | None if the parameter is omitted; otherwise the ordinary admitted reference read. Missing, retracted and unreadable targets refuse `rule_reference_unavailable` without an id. An absent property or any other knowledge state is false. Freshness is a separate check. |
| `{"exists":"$name"}` | A scalar/array parameter is supplied, or a single reference resolves to a live readable head. | Single reference targets are read. |
| `{"fresh":"$name"}` | The existing `exists` check passes and any referenced head is fresh. | Single reference targets are read. |

`present` accepts only `$parameter` or `$parameter.property`, declared on the
same action. It works in submission criteria, every nested `all`/`any`/`none`,
and `linked.where`; it never returns `criterion_not_evaluable`. Action rules
have no criterion guard grammar: `present` in that position is a located
`invalid_rule`. `action list` and `describe` include each criterion's expression
in `criteria`, for example
`{"code":"no_self","message":"Choose another predecessor.","none":[{"all":[{"present":"$supersedes"},{"eq":["$record","$supersedes"]}]}]}`.
A later operator or rule may independently require a head named by a bare
`present`; its usual reads and snapshot bound still apply.

## How a criterion names a property
In a comparison (`eq`, `ne`, `in`, `lt`, `le`, `gt`, `ge`) only the first operand is the property reference; the second operand is a literal (or a `$parameter`), and a literal is never replaced by a property value. A bare property name in the first operand resolves against the object of the action's reference parameter, and only when the action type has exactly one reference parameter; with more than one, the explicit form `$<parameter>.<property>` is required, a bare name is `criterion_not_evaluable` at evaluation, and `schema check` refuses such a schema where it can know. Inside a `linked` condition's `where`, a bare name names a property of the linked candidate object. A property reference that does not resolve (missing property, null, a retracted or absent object, or a type mismatch that cannot be ordered) is unevaluable: comparison is tri-state (true, false, unevaluable), and an evaluated unevaluable condition under `all`, `any`, `none` or a negation refuses with `criterion_not_evaluable` and never passes. Numbers are compared numerically with and without a candidate context, so 1 equals 1.0 in both.
- `get` and `search` on an Ontology-owned type return what actions wrote, with `basis` and history as for indexed types, and `describe` lists action types.

Wire behavior: a missing or retracted modify, delete, `set_reference` or
`clear_reference` target answers
`rule_reference_unavailable` without `id` (previously `revision_conflict` with
`id`); unreadable targets give the same refusal. Unavailable `expected` objects
now give that same detail-free refusal before their revisions are compared.
Bare-property comparisons on unreadable objects now give
`criterion_not_evaluable`, just as missing objects do, without testing the hidden
value. When an expected-revision mismatch coincides with a stale value, both
validate and apply refuse `rule_value_stale`. Store-link refusals use the expanded
`store_path_is_symlink` message and also apply to existing-store reads.

## Embedding
A domain server uses the library without the command: `Context::new(store_path, now)` is the supported constructor (`Context` is `#[non_exhaustive]`; `with_snapshot_bounds` is the documented test constructor) (then set its fields: the store path, a trusted `now`, `client`, `agent`, `session`), then call the public functions `ont::action::list_actions`, `ont::action::validate_action`, `ont::action::apply_action`, `ont::get::get` and `ont::search::search` with their public request types (`ListActionsRequest`, `ValidateActionRequest`, `ApplyActionRequest`, `GetRequest`, `SearchRequest`). Install the server's own schema into its own store file first (for example with `ont_store::Store::install_schema`).
- The uid rule is the caller's: the command checks real = effective uid 0 before binding principal `agent`; a library caller must do the same check itself (see `ont::check_uid`) before it trusts a `Context`. Attribution in the action log is self-reported; it is not authentication.
- The store file has one owner process family: every process opens the file itself, SQLite's write lock admits one writer at a time, and no second process family may write the SQLite file, `edits.jsonl`, or the schema installs directly.

## schema install

- `ont [--store PATH] schema install --from <repository checkout> --commit <40 lowercase hex> --path <path inside the repository>` reads the file with `git show <commit>:<path>` (`/usr/bin/git`, argv only, cleared environment, at most 16 MiB, a 10-second wait). It refuses a non-hex or uppercase commit and an absolute or `..` path with `invalid_request`, a commit that is not an ancestor of the checkout's `refs/remotes/origin/main` with `commit_not_merged`, a missing path at that commit with `schema_path_not_found`, and a missing `/usr/bin/git` or a failed ancestor check other than status 1 with `git_unavailable`. It then does what the file form does (check, version rule, backup before an accepted install that changes stored bytes when the store already holds an install, install) and records the provenance `(repository, commit, path)`: repository is `remote.origin.url` with any userinfo removed, else the checkout path.
- The caller must run `git fetch origin` before the commit form: `ont schema install --from` does not fetch, and its ancestor check uses the local `refs/remotes/origin/main`. When root reads a checkout owned by another user, Git must trust that exact checkout through `safe.directory` in root's protected Git configuration; otherwise the check refuses with `git_unavailable`.
- The file form `ont [--store PATH] schema install <file>` stays for temporary stores; on the default store path (`/var/lib/ontology-platform/ont/ontology.sqlite`) and aliases through file symlinks, hard links, symlinked parent directories, `..`, repeated slashes, trailing slashes or relative paths, it is refused with `install_requires_commit`.
- The installed schema bytes have one home: the `bytes` column of the store table `schema_installs` (with `domain`, `version`, `sha256`, `installed_at`, `source_repository`, `source_commit`, `source_path`). There is no second copy under `/opt/ontology-platform/schema/`. Drift is detected at load: the latest row's bytes are parsed and their sha256 must equal the row's `sha256` (`schema_pin_mismatch` otherwise), and the same bytes are the ones every process reads, so there is no external file that could drift from the record.
- `describe` and the MCP `ont_describe` show, for each installed domain, `version`, `sha256` and, when recorded, `repository`, `commit` and `path`.

## index
- `ont index [--source NAME]...` runs the sources of the installed schemas, one write transaction per source. One run at a time per store (an advisory lock on `<store>.index.lock`, opened `O_NOFOLLOW`; a held lock is `index_busy`, a lock file that cannot be opened or created is the refusal `index_lock_unavailable`). The lock file stays after the run and is never removed.
- Source kinds `markdown_fields` (`glob`) and `file_digest` (`paths`) take `exclude`, `complete_listing`, `fresh_for_s`; `markdown_fields` also `head_lines`. Globs are absolute; `*` and `?` stay inside one segment; `**` is a whole segment (at most 8); no brace, class or escape syntax.
- `markdown_fields` `glob` accepts one glob string (for example `"/work/*/STATE.md"`) or an array of 1 to 16 absolute globs (for example `["/work/*/TASK*.md","/work/*/*/TASK*.md"]`). The listing is the union: a file matched by two globs is one record, `complete_listing` retracts against the union, and one failing glob fails the whole source with nothing appended. `file_digest` `paths` keeps its form: an array of 1 to 256 absolute glob strings or literal paths.
- `exclude`: an exclude pattern that matches a directory excludes everything under it (the directory is not read); a pattern that matches a file excludes that file. To prune a directory the pattern must name the directory itself (`/a/secret`): `/a/secret/*` does not match `/a/secret`, so the directory is still listed, and only its children are excluded one by one.
- A symlink is never followed. A wildcard segment skips symlinks and counts them (`skipped_symlinks`); a literal (wildcard-free) segment that is a symlink fails the source (`glob_root_is_symlink`), so a replaced root never retracts objects. With `complete_listing`, a wildcard-free directory of the glob (or the parent of a literal `file_digest` path) that does not exist fails the source (`glob_root_missing`) and appends nothing, so an unmounted or renamed root never retracts objects; the same prefix existing but not being a directory fails with `glob_root_not_directory`. Without `complete_listing` both are an empty listing. Files are read through one descriptor opened `O_NOFOLLOW|O_NONBLOCK` whose real path must equal the listed path (`path_not_canonical`).
- Bounds per run: 16 MiB per file, 1 GiB in all (`source_bytes_too_large`), 64 MiB retained parsed text (`source_text_too_large`), 10,000 records, 200,000 directory entries. The directory-entry bound counts a directory once per glob that lists it, so a directory shared by two globs is counted twice (the bound is conservative).
- Change kinds: `initial`, `update` (values differ), `refresh` (values equal and the head is stale, or the bytes read differ from the head's lineage), nothing otherwise, `retraction` (with `complete_listing`).
- `parse_integer` is strict: the whole trimmed string must parse as an `i64`, so `+5` and `007` are accepted (5 and 7) and `3 items` is unresolved.
- Value templates: besides `from` (one JSON pointer), a primary key or a property may declare `template` instead (never both), for example `"primary_key":{"template":"{dir}/{stem_id}"}`. A template is at most 256 bytes with at most 8 placeholders `{name}`, each naming a top-level record field of the source kind whose value is a string for that kind (`unknown_template_field`, `template_field_not_string`, `invalid_template` at `schema check`). The value is the template with each placeholder replaced by that record field; it is unresolved when a named field is absent from the record. A `transform` applies to the template result as it does for `from`.
- Transforms are `"first_word"`, `"parse_integer"`, `{"segment_after":"<absolute prefix ending in />"}` (the first path segment after the prefix; unresolved when the value does not start with the prefix, when nothing follows, or when that segment is `.` or `..`; when nothing but a file name follows the prefix, the segment is that file name) and `{"replace_prefix":["<from>","<to>"]}` (replaces a leading `<from>` by `<to>`; unresolved when the value does not start with `<from>`), for example `"transform":{"segment_after":"/home/palantirkc/work/"}`. Each object-form argument is at most 1,024 bytes (`invalid_transform` at `schema check`); `describe` shows transforms and templates as declared.
- References from record strings: a property of type `{"ref":T}` on a source-owned type may be filled by `from` or `template`, plus a transform. The indexer builds the one reference representation the store and `get` already use, the full `Id` object `{"object_type":T,"namespace":<the domain namespace>,"native_key":<the resolved string>}`; an empty or unresolved string gives the unresolved (`missing`) state, never a reference to an empty key. A reference whose target object does not exist is stored as it is (the target may be indexed later or by another source): `get` returns the full `Id` object for it and `search` can match it with `eq` on the full `Id` or find its absence with `unresolved`, even while the target object itself is not found. `schema check` refuses such a property when `T` is not declared in the domain (`unknown_type`, as the link checks use). `unresolved` on a reference property means the property has no value, not that its target is absent; a reference to a retracted or never indexed object stays as stored.

## search
- `qualified` covers the page returned (the scanned heads of this request), not the whole set when `continuation` is present. `stale` and `partial` count scanned admitted heads; `stale_sources` lists the sources of every stale scanned head. `scanned` now counts admitted candidate heads evaluated on this page; hidden and retracted heads consume neither the scan budget nor lookahead.


O22 part A applies candidate selection to `search` only. `around`, `aggregate`
and the renderer retain their existing scans until part B; renderer pagination
still has its raw-head preflight. Limits for `search`:

- `limit`: 1 to 1,000 matching returned objects, default 100. Continuation means
  another admitted candidate exists, not necessarily another matching row.
- Scan budget: 10,000 admitted candidates evaluated per page; an additional
  admitted candidate refuses `result_too_large` unless the row limit was reached.
- Candidates: the first `eq` or `in` leaf reachable through `all` nodes supplies
  a necessary property condition. `any`, `none`, other operators and no filter
  provide no condition and retain an unfiltered scan. Empty `in` is supported.
- Unresolved used properties, historical schema pins, partial coverage and stale
  heads remain candidates even if the selected equality cannot match, preserving
  their existing qualification/exclusion effects. Only `scanned` changes meaning;
  matching rows and other basis fields retain their definitions.
- Filter bounds remain depth 8, 64 leaves, 100 `in` values; request bytes remain
  bounded as documented above. No payload is loaded before metadata admission.

The store maintains a format-4 `head_candidates` projection transactionally.
`visibility` stores each matching grant's canonical namespace/domain/scope-key
predicate; `scope_keys` stores the actual object's key/value map. Omitted grant
keys keep native wildcard semantics. The primary key is
`(object_type,visibility,property,value,instance,account,native_key)`.
Current read admission (type, namespace, domain and scope-key equality) is fully
represented by these partitions; Authority/session checks still run before
payload loading. No inaccessible head needs a residual per-head work budget.

Equality/uncertain postings and 16 hexadecimal expiry prefixes per live head
permit at most 240 stale equality streams per readable scope. Every stream uses
an indexed keyset seek after the previous object key and returns at most 256
metadata rows. A bounded merge retains only the first 256 unique keys. There is
no UNION materialization or SQL sort; work is bounded by stream count times
admitted candidates plus fixed seek overhead, independently of hidden heads.
With 100,000 fresh superseded and 201 current heads, only current/uncertain/stale
postings are read and 201 payloads are loaded. Unsupported filter forms also use
the visibility-aware admission/lookahead loop: intentionally, hidden tails no
longer create continuations or consume the old raw-head allowance.

A nominally read-only open of format 3 performs a one-time writable migration,
then reopens read-only. This preserves automatic upgrade for existing callers;
unwritable storage cannot migrate. DDL, backfill and the format-4 marker commit
atomically; two openers serialize. Backfill/schema upgrades read live payloads
in 256-head pages. Repeated opens do not rewrite the projection. Appends replace
that head's postings in the write transaction; backfills do not replace them.

Older binaries reject format 4 on both read-only and writable opens. Before
migration, quiesce old writers and keep a consistent store copy and its matching
binary. Binary rollback requires restoring that pre-migration store copy, with
any subsequent edits handled by the operator; binary-only rollback cannot open
format 4. No db56e16 projection was installed outside disposable tests, so there
is one migration path from format 3; disposable predecessor projections rebuild
in that same migration. Do not manually lower the format marker.

Database triggers on every INSERT/UPDATE/DELETE of `objects`, `observations`
and `schema_installs` advance a mutation generation, even through an older
connection opened before migration. These tables contain every candidate input:
identity/scope/head, observation values/freshness/pins, and schema bytes including
grants. `links` and `action_log` are not inputs to candidate membership.
Native projecting writes record the projected generation in their transaction;
partial writes refuse if the starting projection is stale. Each candidate page
compares generations inside its read snapshot and refuses `projection_stale`
on mismatch. Searches do not acquire a write lock or start a rebuild.

Repair explicitly with `ont --store STORE schema install FILE`, using the exact
installed schema bytes (or its `--from CHECKOUT --commit SHA --path PATH` form).
An identical install repairs all domains under IMMEDIATE; `changed:false` still
refers to the schema pin. Rebuild and checkpoint commit together; interruption
rolls back and leaves the old correct projection or the stale refusal.
SQLite's schema cookie invalidates pre-migration prepared statements so their
next execution runs the newly installed triggers; the held-connection regression
executes and asserts this reprepare. The format barrier remains in place.

The compatibility guarantee covers every binary of this repository, opened
before or after migration. Trigger definitions are checked at open and before
candidate reads, cached against SQLite's schema cookie; missing/changed triggers
refuse until identical-schema repair recreates them and rebuilds atomically.
Arbitrary SQL modification (dropping triggers, disabling them on a connection,
or editing tables) is outside what the store can detect; presence checks do not
detect a connection-local trigger switch.

## mcp
`ont [--store PATH] mcp [--client NAME]` is a Model Context Protocol server over stdio: newline-delimited JSON-RPC 2.0, one request per line on stdin, one JSON line per response on stdout, nothing else on stdout; no listener, no threads. `--client`, when given, matches `^[a-z0-9][a-z0-9-]{0,31}$` (else the refusal `invalid_request`, before stdin is read); when `--client` is absent, `initialize` supplies `clientInfo.name` only when `valid_client` accepts it; absent, empty or invalid names use `mcp`. An explicit `--client` wins, except the reserved name `cli` also maps to `mcp`, so MCP writes never share the CLI action namespace. Actions record this client, the per-call `agent` (falling back to nonempty `ONT_AGENT`) and optional `session` through `Context`, subject to the library actor bounds. The uid check is that of the other subcommands.
- Register it in a CLI as `{"command":"/usr/local/bin/ont","args":["mcp","--client","<cli>"]}`. This crate registers and installs nothing; each CLI's own configuration is changed by a separate packet.
- Methods: `initialize` (answers the asked protocol version when it is `2025-11-25`, `2025-06-18`, `2025-03-26` or `2024-11-05`, else `2025-11-25`; `capabilities` `{"tools":{}}`; `serverInfo` `ont` and the build commit), `ping`, `tools/list`, `tools/call`. Notifications get no response. Another method is `-32601`; a line that is not JSON is `-32700` (id null); a JSON value that is not a request object, a batch array included, is `-32600`; an unknown tool or `arguments` that is not an object is `-32602`. Ids (string or number) are echoed. End of input exits 0.
- Tools: `ont_describe`, `ont_get`, `ont_search`, `ont_index`, `ont_action_list`, `ont_action_validate`, `ont_action_apply`. The arguments are the JSON object that the subcommand reads on stdin (for `ont_index`, the object `index` takes: `{"sources":[...]}`). Each call opens the store, runs the same operation as the subcommand, closes it and binds the principal for that call; the server never creates a store and holds nothing between calls. `ont_index` takes the same run lock (`index_busy`).
- Result: `{"content":[{"type":"text","text":T}],"isError":B}`. `T` is the line the subcommand prints for the same request, store and `now`, without its trailing newline; an internal failure (exit 1, which prints no stdout) is `{"status":"internal","code":"<code>@<file>:<line>"}`. `isError` is true exactly when the subcommand's exit code would not be 0: a refusal, `source_failed`, an internal failure (a panic inside one call is contained and answered `panic@<file>:<line>`). There is no `structuredContent`.
- Bounds: a request line over 1,048,576 bytes is answered `-32600` `request_too_large` and discarded as it is read; a `T` over 262,144 bytes (measured on the `text` bytes; the wire line is that text JSON-escaped and can be up to about twice as long) is replaced by the refusal `response_too_large` (with `bytes` and, for `ont_search`, `retry_with_smaller_limit`: true); `tools/list` is constant and at most 5,000 bytes.

| Tool | Operation |
| --- | --- |
| `ont_describe` | Describe the installed schema. |
| `ont_get` | Read one object and its history. |
| `ont_search` | Filter objects. |
| `ont_index` | Refresh indexed sources. |
| `ont_action_list` | List action types with parameters and criteria codes; optional `domain` only. |
| `ont_action_validate` | Preview criteria, basis revisions and edits without writing. |
| `ont_action_apply` | Apply an action with a required `request_id` and optional `expected` revisions. |

### Changing something through an action

Choose an action and its parameter names from `ont_action_list`, and object ids from `ont_search` when needed; call `ont_action_validate`, then pass its `basis` as `expected` to `ont_action_apply`. The following request is from `action_validate_apply_passthrough_and_refusals_write_nothing` using the shared test schema: `{"name":"ont_action_validate","arguments":{"action_type":"create_tag","parameters":{"label":"once"},"request_id":"tag-1","agent":"parity-agent"}}`; its `result` is shown below. Apply with the same arguments and `ont_action_apply` to receive `outcome: applied`; after a lost answer, repeat the same `request_id` with the same arguments, including `agent`, to receive `already_applied` without another change. Every value of `edits_log` means the change was made; a refusal carries `code`, and must not be retried unchanged. All notifications, including `notifications/cancelled`, get no answer and change nothing; the server completes a started apply before reading the next line.

```json
{"action_type":"create_tag","basis":[],"criteria":[],"edits":[{"change":"initial","id":{"namespace":{"account":"root","instance":"vps://palantirkc"},"native_key":"act-rq-4876aec9a158d5ecf55564d16c398d05","object_type":"tag"},"revision":"act-rq-4876aec9a158d5ecf55564d16c398d05#0"}],"valid":true}
```

MCP action `request_id` must match `^[A-Za-z0-9._-]{1,64}$`: choose a new unique value per intended change (for example a UUID); invalid values are refused with `invalid_argument` and a message naming the allowed form. The action schemas reject additional properties; `agent` and `session` match `^[A-Za-z0-9._:-]{1,64}$`, and validate accepts `expected` basis revisions. If an apply answer exceeds the response bound after commit, `response_too_large` retains `action_id` and `outcome` so the caller can identify the committed change.

## Generated blocks

A schema's optional `views` map declares a table over a type in the same domain.
For example, the synthetic `render-board.json` test fixture declares:

```json
"items": {
  "object_type": "item",
  "order": [{"property": "rank", "direction": "asc"}],
  "columns": [
    {"header": "Key", "property": "$key"},
    {"header": "Title", "property": "title"},
    {"header": "Rank", "property": "rank"}
  ]
}
```

`filter` accepts the search grammar; a string `$title` takes the string argument
`title`, declared in `args: ["title"]`. `limit` defaults to 200 (maximum 1000).
`order` and `columns` must be nonempty. Order ties use the object key. Retracted
objects are omitted; stale values have ` (stale)` appended. Unresolved knowledge
states appear as `(missing)`, `(null)`, `(blank)`, `(unknown)`, `(not_performed)`.
Cells escape pipes, line breaks and HTML comment delimiters. Rendering uses
search admission and its 10,000-head bound, with no hidden-object totals.

Three forms (use `--store` with a test store; omitting it selects the host store):

```sh
printf '%s\n' '{"op":"render","view":"items","args":{}}' | ont --store /tmp/demo/store.sqlite render
ont --store /tmp/demo/store.sqlite render --write /tmp/demo/BOARD.md --section named_items --arg title=equal --preimage-dir /tmp/demo/preimages
ont --store /tmp/demo/store.sqlite render --check /tmp/demo/BOARD.md
```

`--write` requires an existing regular file with exactly one matching, closed
block. It saves the previous bytes in the existing preimage directory with mode
0600 and a SHA-256 suffix, then renames a temporary file over the target, keeping
its mode and all bytes outside the block. An identical block causes no write and
answers `changed:false` with `preimage:null`. Argument values must be nonempty
and contain only ASCII letters, digits, `.`, `_`, `:`, `/` or `-`; opening-line
arguments are sorted by name. A check reports the opening line of each drifting,
unknown or unclosed block and exits 2; a clean check exits 0.

The `deterministic_order_ties_arguments_and_actions` test renders this exact block:

```markdown
<!-- ont:render items schema=board@1 -->
| Key | Title | Rank |
| --- | --- | --- |
| a | equal | 2 |
| b | equal | 2 |
| z | last | 10 |
<!-- /ont:render -->
```

An argument-bearing opening line has the form
`<!-- ont:render named_items title=equal schema=board@1 -->`.
More matching rows than the limit add a final table row containing
`(+N more; limit L)` with the exact omitted count. No timestamp is generated.

Rendering reads schema metadata and each search page in separate short read
transactions; it holds no read transaction while opening another connection.
A block is deterministic for an unchanged store and fixed `now`; it is not a
snapshot across pages: concurrent actions or schema installs can change later
pages, counts or values. For a coherent whole-block view, render after writers
have finished. The `(stale)` suffix still follows the trusted clock.

`--write` holds an exclusive advisory lock on the persistent
`<target file name>.ont-render.lock` sidecar beside the target, from before the
read through rename. Other render writers wait at most 250 ms and then refuse
with `render_busy`; an inaccessible or invalid sidecar is `render_lock_unavailable`.
Render leaves `<target>.ont-render.lock` beside the target; inside a Git worktree
it appears as an untracked file and may be deleted when no render runs.
Render never removes the sidecar itself, so rename does not change the lock inode.
Other editors must respect the same lock to be serialized. Targets with multiple hard
links are refused as `render_target_invalid`.

Fenced examples are prose: an opener has at most three leading spaces and a run
of at least three backticks or tildes. A closer has at most three leading spaces,
the same character repeated at least as many times, and only spaces after it;
backtick opener info strings cannot contain backticks. Markers inside the fence
are ignored by both check and write. An unclosed fence makes check exit 2 with
`refusal_code: render_fence_unclosed` at the opening fence's line (empty view and
arguments); write refuses `render_fence_unclosed` without changing the target.
Cells escape backslashes before pipes, line breaks and comment delimiters.
In a `render_drift` entry, `refusal_code` is present when rendering was refused
(for example `store_busy`, `store_not_found`, `unavailable`); pure byte drift
has no `refusal_code`. Argument placeholders are accepted only as complete
string comparison values on string properties (including string-based value
types), or individual elements of `in` on those properties; no coercion or
placeholders inside arrays/structs are supported. An `eq` filter on a single
reference property also accepts an argument, for example
`"filter":{"eq":["task","$task"]},"args":["task"]`. Its argument is the
referenced object's native key (such as `/synthetic/task-1`), recorded unchanged
in the generated block's opening line. Rendering builds the full reference Id
for search; no copied key property is needed and the target need not exist.
Reference arguments support `eq` only. Their characters follow the same ASCII
argument rules above; an unrepresentable key refuses at render time with
`reference_argument_unrepresentable`.
Unresolved sort values compare as JSON null; nonscalar values compare by compact
JSON text. In ascending boolean order, this places `false`, unresolved, `true`;
other mixed comparisons follow JSON text order. Order ties use the key.

## binary install script

`scripts/install-ont.sh install <commit>` prints and runs `git fetch origin +refs/heads/main:refs/remotes/origin/main --refmap=` in the checkout before checking ancestry; `--dry-run` prints the fetch step without executing it. Run the script directly or with `bash`. The build disk floor is checked on its temporary build directory's file system. Before replacing `ont`, the script verifies the retained previous copy's sha256. After writing `installed.json` it keeps at most three previous copies. The script unsets inherited `GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE` and `GIT_COMMON_DIR`. An unknown commit after fetch refuses with `commit_unknown`; Git execution failures remain `git_unavailable`. A state-write failure after replacement in either install or rollback exits nonzero and prints manual recovery commands. For rollback the printed command repeats rollback to record the restored binary. `rollback` validates the previous binary's version before replacement. Test directory overrides require `ONT_INSTALL_TEST=1` and must resolve outside the real bin and state directories.

## Usage and input diagnostics

`ont help`, `ont --help`, `ont -h`, and `ont COMMAND --help` return JSON usage
with the command forms, options, stdin fields and examples. Refusals for malformed
fields carry `field` (a JSON pointer, or option name) and `rule`, without the
rejected value; unknown command/option refusals also carry `hint: "ont help"`.

A render consumer records the required block count N and always passes
`ont render --check FILE --expect-blocks N`; a mismatch refuses with
`render_block_count`, `expected`, and `found`, including a balanced fence that
hides a real block from the parser.

## History

O17 renamed the composition crate and synthetic CLI from `ontd` to `ont-daemon`
(Rust import `ont_daemon`). It remains the consumer of `ont-core`/`ont-store`
used by `ont-program-pilot`, `ont-local-fake` and the synthetic integration tests.
The operational command is `ont`. Pinned contracts, recorded diagnostics and
commands over archived source retain their historical names. References in the immutable
`conformance/` evidence and the out-of-scope root README retain the earlier name.

## Linked object-set reads

`ont around` and `ont_search` with `"op":"search_around"` take
`{"from":{"object_type":"hold","where":{"eq":["status","active"]}},"path":["task"],"limit":100}`.
`from` uses the search filter grammar; each of 1 to 3 path entries is a forward
or reverse link name from `describe`. The whole path is checked before loading
heads. One SQLite snapshot and one native admission cover the request; metadata
admission precedes every payload load, including reverse scans. Hidden and missing
targets both contribute to `dangling`, with identical answers. Each hop removes
duplicate IDs. The final set is sorted by ID, capped at `limit` (default 100,
maximum 1,000); `matched` counts the full final set, with no continuation token.
At most 10,000 unique admitted heads per type may be loaded across all hops;
otherwise the operation refuses `result_too_large`. Basis `scanned`, `stale`
and `partial` count unique admitted heads used by starting/reverse scans or
forward lookups. Unresolved filter comparisons contribute `unresolved_excluded`;
unresolved used filter/link properties also prevent qualification. `qualified`
requires no stale, partial, unresolved-used/excluded or dangling input. Stale
source entries name the `ont index --source NAME` refresh command.

Traversal charges at most 10,000 reference entries across all hops, including
repeated IDs and missing targets. Each admitted source's entire reference array
is charged before iteration or target lookups, also in reverse scans; hidden
source heads consume no entries. Exceeding `MAX_REFERENCE_ENTRIES` refuses
`result_too_large` with `max_reference_entries: 10000`. Loaded heads are shared
without copying their reference arrays before this check.

## Count aggregation

`ont aggregate` and `ont_search` with `"op":"aggregate"` accept
`{"object_type":"instruction_conflict","group_by":["status"],"metrics":["count"]}`
and optional `where` using the search filter grammar. Only `metrics:["count"]`
is supported; other metrics refuse `unsupported_metric` at `/metrics`.
The answer has `kind: aggregation`, `groups:[{key:{status:"open"},count:0},...]`
and basis `scanned`, `unresolved_excluded`, `stale`, `partial`,
`oldest_observed_at` (null for no admitted heads), `qualified`, `stale_sources`.
Read and derive grants are required on the type and on each readable object's
scope, before loading payloads. Hidden objects affect no counts. Bounds are
10,000 admitted heads, 2 distinct group properties, 1 metric, 1,000 buckets
including enum zero buckets, and 1,024 compact JSON bytes per group value.
Enum keys expand all declared combinations, including on an empty store when
all keys are enums; mixed keys expand for observed non-enum combinations.
`group_by` may be omitted for zero keys. `where` uses search membership,
including `unresolved`, `any` and `none`; missing filter values are evaluated
by that grammar. Historical incompatible filter values are excluded and counted.
Only matching rows need resolved group values; oversized or incompatible
historical group values are excluded and counted, never a request-wide refusal.
Any exclusion, stale/partial head or unresolved used filter value prevents qualification.

Group properties must have a declared worst-case compact JSON size <=1,024 bytes;
otherwise preflight refuses `unsupported_group_property`, naming the property
and a usable declaration, independently of the stored rows:

| Declared type | Size guarantee and admission |
| --- | --- |
| Enum value type | Largest encoded member; all members must fit. |
| Boolean | 5 bytes; admitted. |
| Integer, timestamp | 20 bytes; admitted. |
| Date | 12 bytes; admitted. |
| Decimal, including non-enum value types | Unbounded spelling without `max_bytes`; admitted with `max_bytes <=1022` (plus two quotes). |
| String, including non-enum value types | Unbounded without `max_bytes`; admitted with `max_bytes <=170` (up to six encoded bytes per input byte, plus two quotes). Pattern-derived bounds are not inferred. |
| Reference | Id fields have length limits, but their worst-case escaped representation exceeds 1,024 bytes; refused. |
| Array | No declared item-count limit; refused. |
| Struct | Admitted if field-name overhead plus every field's recursively declared bound fits; inner strings have no `max_bytes` declaration. |

Bucket order compares keys in `group_by` order: enum components follow declared
enum order, other components sort by canonical encoded JSON. JSON object fields
are recursively sorted for identity, independently of map construction order.
