# Hindsight and Ontology boundary

Status: proposed design contract, 2026-11-04. This document defines ownership and
provenance conventions; it adds no integration, schema, service or deployment.
The current Rust baseline is `1aab8c13c37a663dd35611ea1291e2d640d5c8f2`.
The authorized deliverable is autonomy goal 7's contract, under the ws-replan
Lead. Implementation and production admission require separate work.

## Two kinds of knowledge

Hindsight is the shared mid/long-term context memory for **all four agents**:
Claude Code, Codex, Muse and Antigravity CLI (`agy`). It records decisions,
rationale, conversation-derived knowledge, lessons and corrections. Retained
documents preserve what was said; extracted facts and observations reconcile
that material; mental models and knowledge pages synthesize answers to standing
questions. A knowledge page is backed by a mental model. These are official
Hindsight features, not Ontology tables. [H1][H2][H3]

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
2. A Hindsight service/deployment identifier, tenant/API namespace, `bank_id`,
   resource kind (`document`, `knowledge_page` or `mental_model`) and the exact
   returned resource ID. These identify the resource within its owning service;
   a hostname or page title alone is insufficient.
3. The relation in plain text, such as “rationale for modeling” or “decision
   discussed by”; this must not assert “operational source” or “authorized by.”
4. Retrieval time and a digest of the exact cited content. Record a returned
   update timestamp/history reference when available. A document's native
   `content_hash` is opaque; do not assume its algorithm is SHA-256.

Documents can be replaced under the same `document_id`; pages and mental models
refresh under stable IDs. An ID alone therefore identifies a resource, not
immutable text. A digest detects a changed version; it does not recover the old
text. Mental models retain content history, but document history records event
IDs/counts rather than prior text. Use the owner's retained original or an
available content-history version for a historical reading; if unavailable,
report the provenance gap rather than silently using the latest text. [H1][H2][H3]

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
- An access-controlled artifact locator, SHA-256 of the exact authorized
  serialized read result and a pointer to the relevant part. The artifact stays
  with its operational owner; its payload is not retained in Hindsight.
- Observation time, the trusted read time used for freshness, and the returned
  `authorization_revision`. That revision describes the policy generation of
  the past read; it cannot authorize a later dereference.

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

Use **one shared bank**, with stable literal topic tags `platform:agent`,
`platform:education` and `platform:ontology`. These are three alternatives, not
one literal pipe-delimited tag. Retain the owning platform tag; cross-platform
decisions may carry each applicable tag. Preserve repository provenance using
`project:<repo>` and native document/agent attribution. Do not invent or overwrite
the coding-agents plugin's reserved `source:` and `harness:` tags. Modules use
their owning platform tag plus their own repository provenance. [H4]

Maintain platform-focused mental models whose `source_query` explicitly names
the platform, asks for decisions/rationale and points current status questions
to native sources. Keep cross-platform ownership guidance separate from mutable
operational state. This is a topic organization convention, not a per-CLI role
assignment or an authorization system.

**Tag filtering needs an actual source-scope check.** The official coding-agents
integration can consolidate into shared untagged observations. A strict tagged
model/page can then see no observations even when source documents have platform
tags. Official tagged models default to `all_strict`; `all` also includes
untagged material. A curator must verify the actual observation scopes and model
readback before claiming platform isolation. A platform-focused `source_query`
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
No service/runtime verification or Hindsight write was performed.

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
`/root/.agents/runtime-references/RUNTIME-LOADING.md`. These are local operational
guidance and dated observations; the pinned upstream links above document the
official APIs. The canonical skill used on this host is also available at
`/root/.agents/skills/hindsight-coding-agent/SKILL.md`.
