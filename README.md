# ontology-platform

The Ontology engine of this host: one generic engine, no domain knowledge. A domain describes its world in a schema file kept in the domain's own repository (object types with typed properties, link types, action types with submission criteria, views); the engine stores observations of those objects, answers reads over them and applies actions.

## Crates

| Crate | What it is |
| --- | --- |
| `ont-core` | The schema language and the data model: identity, observations with their knowledge state, coverage, freshness and lineage, scopes, grants |
| `ont-store` | The store: one SQLite file per store, atomic appends, snapshot reads |
| `ont` | The command `ont` and its library: `schema check`, `schema install`, `describe`, `get`, `search`, `around`, `aggregate`, `render`, `index`, `action list`, `action validate`, `action apply`, `backup`, `compact`, and `mcp` (the same reads and actions as an MCP server over standard input and output) |

Every command reads one JSON request on standard input and prints one JSON answer; `ont --help` lists the request fields. The contract of each command is in [crates/ont/README.md](crates/ont/README.md).

## Who uses it

- Agents of every CLI on this host, through the installed command `ont` and its MCP server.
- `agent-platform`: its schema `ontology/work.schema.json` and the command `ap`, which calls the installed `ont`.
- Domain crates in other repositories, which pin this repository by Git revision and link `ont`, `ont-core` and `ont-store` as libraries.

A change of an answer's shape or of the schema language is a change of a contract those users rely on.

## Check, install

- `./check` is the one check: format, lints as errors, all tests, the test of the install script. The CI job `rust` runs the same file and is the required check of `main`.
- `scripts/install-ont.sh install <commit>` builds and installs the command from a clean checkout of a commit on `main`; `check` compares the installed binary with it; `rollback` restores the previous one. It installs no schema and runs no index.

## What else is in the tree

- `docs/effects/`: the contract for effects on external systems (vocabulary and vectors).
- `conformance/`, `docs/S06-*.md`, `docs/s07/`: the record of the first two verticals. Tests of `ont-core` and `ont-store` still read fixtures from `conformance/`. The crate those verticals composed, `ont-daemon`, was removed when nothing consumed it; its source is in the Git history.
- `AGENTS.md`: how an agent works in this repository. `REVIEW.md`: what blocks a pull request.
