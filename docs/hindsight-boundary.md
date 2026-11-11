# Hindsight and Ontology boundary

Status: proposed design contract, 2026-11-04. This document defines ownership and
provenance conventions; it adds no integration, schema, service or deployment.
The current Rust baseline is `1aab8c13c37a663dd35611ea1291e2d640d5c8f2`.
The authorized deliverable is autonomy goal 7's contract, under the ws-replan
Lead. Implementation and production admission require separate work.

## Two kinds of knowledge

**User target:** Hindsight serves as the shared mid/long-term context memory for
**all four agents**: Claude Code, Codex, Muse and Antigravity CLI (`agy`). Its
subject matter is decisions, rationale, conversation-derived knowledge, lessons
and corrections. Retained documents preserve the selected text; extracted facts
and observations reconcile that material; mental models and knowledge pages
synthesize answers to standing questions. A knowledge page is backed by a mental
model. These are official Hindsight features, not Ontology tables. [H1][H2][H3]

**Configured behaviour observed on 2026-11-04:** the host's global defaults are
`retainSessions:false` and `gitIngest:none`; they do not automatically retain
sessions or ingest Git history. The current `coding-agent.json` has a specific
`paths["/home/palantirkc/work/ws-replan"].retainSessions:true` exception, permitting
session retention through supported lifecycle hooks for that task. Muse has MCP
tools but no configured lifecycle injection/retention hook. MCP registration,
page injection and an enabled setting do not prove a transcript was retained.
Explicit selected-document retention remains possible when authorized. Thus the
target is not a claim of universal automatic retention or fresh context in every
running agent. See the host runtime-loading map and official configuration
lifetime/retention guidance. [H4]

Ontology is the local operational model: source-qualified objects and links,
restricted reads and derivations, named actions, transactional state and effect
recovery. An object has an `Id` consisting of `object_type`,
`namespace.{instance,account}` and `native_key`; its access `Scope` also names
class and student keys. Each observation carries revision, source revision,
observation/freshness times, coverage, change/supersession and artifact lineage.
Missing, null, blank, unknown and not-performed remain distinct. Memory prose
cannot satisfy those requirements. [O1]

Current implementation is a bounded synthetic/recorded vertical, not a deployed
Palantir product or a production-authenticated Ontology service. `ont-store`
holds an isolated SQLite store; `ontd::Service` holds current `Admission` through
the store snapshot, derivation and serialization. The embedding owner must
authenticate before binding a session. A principal string, CLI label, host UID
or memory tag is not authentication. [O1][O2][O3]

## Authority by question

| Question | Authority and required check |
| --- | --- |
| What did the user decide, and why? | Dated Hindsight source document and its provenance, reconciled with newer corrections and current user/task instructions. A synthesized page is a navigation aid, not a new permission grant. |
| What does the program implement now? | Current native source, Git revision, compiler/runtime observations and relevant tests. A memory that a repair passed is a past report. |
| What does an operational object currently say? | An authorized Ontology read under current admission, tied to source revision, coverage and freshness. The native domain source remains the authority for the underlying real-world fact. |
| May this caller read, derive or act? | Current authenticated domain policy and named action grant, plus the user's actual task authority. Hindsight never supplies a grant. |
| Did an external effect happen, or may it be retried? | The domain's durable effect state and authoritative target reconciliation. A conversation, mental model or binary rollback cannot settle an uncertain effect. |

Academy lesson archive exception (user decision 2026-11-06): because the academy
server removes older rows, Hindsight archives dated Academy teacher statements and
source observations under real student names, as education-platform
`content/skills/academy-student-context/references/lesson-archive.md` specifies.
An archived observation is a dated snapshot and history, not current operational
state. It grants no admission or action, and a fresh owner read wins while the
source still shows the row. When the Ontology cutover lands, its admission design
decides whether this archive becomes an ingestion source; nothing here promotes
prose into objects.

If memory and current source disagree, preserve the dated memory as history and
correct its interpretation through Hindsight. If a source-qualified Ontology
observation is wrong, its owner appends the appropriate correction/backfill with
explicit supersession through the trusted ingestion boundary. Do not rewrite
one system to make it appear to agree with the other. Unknown or stale evidence
stays unknown or stale. The adopted migration proposal separates Hindsight's
database, Ontology projections and domain dispatch journals. [O1][O2][Z1]

## Explicit non-goals

No `ont-*` component opens, queries or shares Hindsight's PostgreSQL database,
schema, service credentials or internal tables. Hindsight is used only through
its official supported surfaces. There is no Ontology object/state replica in
Hindsight and no automatic promotion of recalled prose into operational facts,
policy, grants or actions. These boundaries apply independently of whether the
services happen to run on the same VPS. [Z1]

## Provenance references in both directions

The following are **citation conventions**, not new Hindsight API fields or an
implemented Ontology wire extension. Names and human titles are display labels;
identifiers plus immutable evidence establish what a citation means.

### Ontology object to memory

A provenance note can explain why an object interpretation or modeling decision
was chosen by naming:

1. The exact Ontology `Id` and observation `revision` being discussed, with its
   `source_revision` and existing `lineage.{artifact_sha256,pointer}` unchanged.
2. The curator-recorded service/deployment identifier and tenant/API namespace
   (explicitly record when no separate namespace applies), plus the resource
   kind (`document`, `knowledge_page` or `mental_model`), owning `bank_id` and
   exact returned resource ID. The service/tenant identifiers come from the
   deployment owner, not invented API response fields. A document response
   returns `id`, `bank_id`, `content_hash` and timestamps; a page read returns
   its page `id`, and its bank comes from the request context. For a page also
   record its backing `mental_model_id` from the tree. A hostname or title alone
   is insufficient. [H1][H2]
3. The relation in plain text, such as “rationale for modeling” or “decision
   discussed by”; this must not assert “operational source” or “authorized by.”
4. Retrieval time, the content selector and a **SHA-256** digest, encoded as
   lowercase hexadecimal, of the UTF-8 bytes of the decoded API text string:
   `original_text` for a document, `body` for a knowledge page, or `content` for
   a mental model. Do not hash the JSON response, JSON string escapes or a
   page's frontmatter-bearing `markdown`; refresh timestamps in frontmatter
   are not content. Do not normalize Unicode, whitespace or line endings, trim,
   or add a terminal newline. A metadata-only response is insufficient. Record
   returned update timestamps/history references separately. A document's native
   `content_hash` is opaque and is recorded separately; do not assume it is
   this SHA-256. [H1][H2][H3]

Documents can be replaced under the same `document_id`; pages and mental models
refresh under stable IDs. An ID alone therefore identifies a resource, not
immutable text. A digest detects a changed version; it does not recover the old
text. Mental-model content history is enabled by default, capped at **50 entries
per kind per model** (version snapshots and refresh-failure records have separate
caps). `HINDSIGHT_API_MENTAL_MODEL_HISTORY_MAX_ENTRIES` changes that cap;
`HINDSIGHT_API_ENABLE_MENTAL_MODEL_HISTORY=false` disables tracking. A knowledge
page's content history belongs to its backing mental model under these same
limits. Document history instead records event IDs/counts, never prior text;
it also depends on retained operations and whether an operation was recorded.
Use the owner's retained original or an available content-history version for a
historical reading, applying the same text-digest convention to that version
(select `previous_content` for a mental-model history entry).
If history is disabled, pruned or otherwise unavailable, report the provenance
gap rather than silently using the latest text. [H1][H2][H3]

**Today's representation:** an ordinary reviewed evidence/design note binds the
object/revision to that memory reference. `Object`, `Observation` and `Link` do
not currently have a Hindsight reference field. `Link` joins two Ontology IDs;
it is not an external URL. `Lineage.pointer` remains the source-artifact pointer,
not an HTTP address. Do not insert memory into operational `fields`, forge a
domain object for a conversation or repurpose the operational source hash. [O1]

### Hindsight correction to an Ontology snapshot

An authorized memory curator writes a **new** Correction document containing the
prior claim, the corrected interpretation, its date/scope and the native evidence
that justified it. To cite Ontology evidence, name:

- The owning deployment and exact object/revision identifiers, source revision
  and source-artifact lineage. Include only identifiers permitted in the shared
  bank; use an approved opaque evidence handle if keys themselves are sensitive.
- An access-controlled artifact locator and pointer, with the canonical
  object/observation digest defined below. The artifact stays with its
  operational owner; its payload is not retained in Hindsight.
- Each cited observation's `observed_at`, the trusted read time used for
  freshness, retrieval time and the returned `authorization_revision`, recorded
  as citation metadata separately from the content pin. That authorization
  revision describes the policy generation of the past read; it cannot authorize
  a later dereference. Native `observed_at` is also an intrinsic observation
  field in the pinned subset; it is not the clock time of a later read.

**Canonical Ontology content pin (`ont-evidence-v1`).** For each cited object,
select exactly `id`, `scope`, `head` and `observations` from its authorized view.
The citation must enumerate the selected observation revisions; keep them in
their stored append order. Each selected observation contains exactly
`revision`, `source_revision`, `observed_at`, `fresh_until`, `change`,
`supersedes`, `coverage`, `fields` and `lineage`, including all nested values and
the tagged `Knowledge` representation. This is an explicit projection of current
`Object`/`Observation` types, not a hash of the response envelope. [O1]

Encode that projection as a JSON value with object keys recursively sorted in
Unicode scalar-value order, arrays in their original order, and no insignificant
whitespace or trailing newline. Use the string escaping and number serialization
of `serde_json` **1.0.145**, pinned by the baseline's `Cargo.lock`; preserve native
integer/number values and strings without coercion or Unicode normalization.
The pin is SHA-256 of those UTF-8 bytes, rendered as lowercase hexadecimal.
Record the encoding label and baseline with the digest; an incompatible encoding
requires a new label, never silent reinterpretation of an existing pin. This is
a proposed citation convention; no encoder or export is implemented here.

Exclude response `status`, `authorization_revision`, computed `stale` flags,
query/decision wrappers and read/retrieval clocks from that projection. An
identical selected object/observation subset then has the same content pin across
clock or policy-generation changes. A pin does not establish freshness or access.
For multiple objects, record a separate pin and revision selection per object;
do not imply that one object's pin captures links or the entire query graph.
If an artifact also has an exact-file SHA-256 for custody, label it separately
from `ont-evidence-v1`; it is not the object-version pin.

`Store::snapshot` is currently a transaction-scoped consistent read, **not a
durable globally addressable snapshot ID**. A retained result artifact can pin
the selected authorized view when separately permitted; it is not a full database
snapshot or proof of present freshness. No export is created by this document.
An object head alone does not pin the linked objects selected by a query. [O2][O3]

The correction discusses the earlier conclusion and its provenance. It stores
neither object fields nor query-result rows, action receipts, journals, student
payloads or a parallel “current state” table. For example, “the earlier answer
treated incomplete evidence as complete; see the scoped snapshot reference” is
memory; copying the snapshot's operational values is an Ontology replica and is
outside this boundary.

Bidirectional means two independently authorized citations, not an automatic
two-phase write. Either citation can exist without the other. A missing or
inaccessible target remains a disclosed gap; resolution never creates a target
or repairs state. Knowledge-page synthesis must not ingest its own citations as
new operational evidence or create a feedback loop. [H2]

## Governance of the shared bank

The user owns decisions and effect authority. Each agent can contribute memory
within its assigned task; an authorized curator owns correction/refresh work.
The domain source adapter/trusted ingestion owner owns Ontology observations;
the authenticated embedding owner controls policy, sessions and named actions.
An agent's access to a shared memory bank gives it none of those domain powers.

The target is **one shared bank** across agents and repositories. The official
default `coding-agent::{gitProject}` shares a bank across agents only within a
repository. The configuration owner must explicitly converge destinations in
`~/.hindsight/coding-agent.json`: use a static `bankId`, a `mapPathToBank` mapping
whose longest absolute-path prefix wins (even over `bankId`), or
`banks.<resolved-id>.bank` redirects to one literal destination. Redirects are
single-hop; the target bank's config section is not consulted. Verify each
intended runtime/repository's resolved destination; a stated target or common
service URL is not proof of convergence. No routing is changed here. [H4]

Use stable literal topic tags `platform:agent`, `platform:education` and
`platform:ontology`. These are three alternatives, not one literal pipe-delimited
tag. The configuration owner sets the applicable literals in `retainTags`
alongside **`"project:{gitProject}"`**, whose placeholder expands to the
worktree-aware repository name; `project:<repo>` describes the expanded tag and
is not the literal template to configure. Select the owning platform through
the appropriate path/bank overrides; cross-platform documents may carry each
applicable platform tag. Modules use their owning platform tag plus their own
repository provenance. Preserve native document/agent attribution and do not
invent or overwrite the plugin's reserved `source:` and `harness:` tags. [H4]

For **direct-API retains**, the authorized caller/curator must explicitly supply
the concrete `platform:*` and expanded `project:<repo>` tags in the retain
request. The server does not apply the coding-agents client's `retainTags` or
expand its templates on behalf of a direct caller. A direct caller must not
impersonate plugin-generated attribution.

Maintain platform-focused synthesis whose `source_query` names the platform,
asks for decisions/rationale and routes current status questions to native
sources. The query defines the question; tags and trigger tag filters define
the source scope. For a standalone API mental model, the authorized curator owns
its query, tags and trigger through the mental-model API. For plugin-managed
pages, the configuration owner maintains seeded queries under `pages` and new
pages under `customPages` in `coding-agent.json`; these are re-synced from config,
so an API-only query edit can be overwritten next session. `customPages.tags`
selects source facts, not display labels. Keep cross-platform ownership guidance
separate from mutable operational state. This is topic organization, not a
per-CLI role assignment or an authorization system. [H2][H3][H4]

**Tag filtering needs an actual source-scope check.** The official coding-agents
integration can consolidate into shared untagged observations. A strict tagged
model/page can then see no observations even when source documents have platform
tags. Direct API tagged pages/models default to `all_strict`; `all` also includes
untagged material, and plugin-managed custom pages use `all`. A curator must
verify the actual observation scopes and model readback before claiming platform
isolation. A platform-focused `source_query`
over shared observations, using documented non-strict matching where needed,
provides topic routing only. It is not a privacy boundary. This document changes
no consolidation mode, tags, routing configuration or model. [H2][H3][H4]

Supersede stale decisions/status claims with a dated Correction that names the
prior document and current evidence; preserve the original historical source.
Refresh affected models/pages through official tools, then read their resulting
content and source scope back. A successful retain or queued refresh does not
prove extraction, consolidation, page convergence or an existing agent's context
has updated. Official convergence/consistency tests illustrate those separate
properties; they were read, not executed for this documentation change. [H5]
Only exact duplicates/noise may be deleted by the separately authorized curation
task through the official API. No deletion is requested here.

## Future: read-only provenance resolver crate

A possible `ont-provenance` crate is **unimplemented and unapproved for execution**.
It would consume caller-supplied citation identifiers and use the owner's
supported public read API or authenticated in-process facade to check existence,
revision/digest agreement and accessibility. Hindsight reads use its official
document/page/model APIs; Ontology reads stay behind fresh `Session`/`Admission`.
There is currently no general production Ontology HTTP endpoint to call. [O3]

Its result would be bounded reference metadata and an explicit match, changed,
unavailable or unresolved outcome, never operational fields or memory text
copied into the other store. It must not bypass either owner's access checks,
enumerate hidden objects, fetch arbitrary artifact URLs or treat an old
authorization generation as reusable admission. Missing versions fail resolution.
It would have no persistent mirror, refresh scheduler, ingestion pipeline,
write API, direct database connection or autonomous repair behavior.

This sketch describes a future consumer of official features, not permission to
build a replacement memory contract, proxy or data-transformation pipeline.
Before implementation, establish an actual consumer, supported official read
surfaces, authorized identifier visibility and a version-pinning/recovery rule.
If official features cannot meet those requirements, document the exact gap.

## Source basis

Local sources were read on 2026-11-04. The official Hindsight source checkout is
revision `a5a619b5ca59c5c39e0f96be6f7a31fabd136bd5`; local deployment documentation
identifies server 1.1.0 and the installed coding-agents package identifies 0.8.2.
Only selected non-secret retention configuration fields were read back; no
service calls, runtime tests or Hindsight writes were performed.

[O1]: ../crates/ont-core/src/lib.rs "Current identity, observations, lineage and admission"
[O2]: ../crates/ont-store/src/lib.rs "Trusted append and transaction-scoped snapshots"
[O3]: ../crates/ontd/src/lib.rs "Authenticated embedding boundary and guarded serialization"
[H1]: https://github.com/vectorize-io/hindsight/blob/a5a619b5ca59c5c39e0f96be6f7a31fabd136bd5/hindsight-docs/docs/developer/api/documents.mdx "Official documents, mutable IDs, hashes and history"
[H2]: https://github.com/vectorize-io/hindsight/blob/a5a619b5ca59c5c39e0f96be6f7a31fabd136bd5/hindsight-docs/docs/developer/api/knowledge-pages.mdx "Official knowledge-page IDs, backing mental models and tag filters"
[H3]: https://github.com/vectorize-io/hindsight/blob/a5a619b5ca59c5c39e0f96be6f7a31fabd136bd5/hindsight-docs/docs/developer/api/mental-models.mdx "Official mental-model scope, refresh and provenance"
[H4]: https://github.com/vectorize-io/hindsight/blob/a5a619b5ca59c5c39e0f96be6f7a31fabd136bd5/hindsight-integrations/coding-agents/README.md "Official shared-bank routing, retain tags and observation scopes"
[H5]: https://github.com/vectorize-io/hindsight/tree/a5a619b5ca59c5c39e0f96be6f7a31fabd136bd5/hindsight-system-evals/evals "Official tests 01 (convergence), 06 (reflect reads pages), 10 (consistency)"
[Z1]: /home/palantirkc/work/ws-replan/inputs/final-review-20261102/extracted/rust_migration_reassessment_2026-11-02/PROPOSAL.md "Adopted ZIP proposal sections 4.2, 5.3 and 6; original ZIP SHA256 964845e260a25350823fb42d38a60e7cba973f8af607e10b505727d3bf78915c"

Host policy references: `/docker/hindsight/HINDSIGHT_OFFICIAL_PER_CALL.md`,
`/docker/hindsight/AGENTS.md`, `/docker/hindsight/README.md`, the installed
`/root/.hindsight/coding-agents/skill/SKILL.md`, and
`/root/.agents/runtime-references/CLI-RUNTIMES.md`. These are local operational
guidance and dated observations; the pinned upstream links above document the
official APIs. The canonical skill used on this host is also available at
`/root/.agents/skills/hindsight-coding-agent/SKILL.md`.
