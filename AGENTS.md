# Working in ontology-platform

Shared rules of this host: `/root/.agents/AGENTS.md`. This page adds only what is particular to this repository. What the repository is: `README.md`.

## Before you change anything

- Work in a Git worktree of your task on a branch from `origin/main`, never in the main checkout.
- The engine is generic. A change that names a domain (a lesson, a student, a packet) belongs in that domain's schema or repository, not here.
- The engine changes only for a named use: say in the pull request which consumer needs it and which real run will show it.

## The check

`./check` at the repository root, until it prints `check: ok`. It is the same file the CI job `rust` runs. On the host it takes the build lock by itself (do not wrap it in `flock`) and builds into `target/` inside your worktree. Do not push before it passes.

## Contracts you can break

| Contract | Where it is stated | What relies on it |
| --- | --- | --- |
| The schema language | `crates/ont-core`, described in `crates/ont/README.md` | Every installed schema |
| The JSON answers of each command and of the MCP tools | `crates/ont/README.md` | `ap` in `agent-platform`, every agent that reads an answer |
| The store format | `crates/ont-store`; `crates/ont/README.md` section "compact" | Every store file on the host; a format change needs an upgrade path and a design note before the build |
| The library boundary (`ont`, `ont-core`, `ont-store` as dependencies) | The crates' public items | Domain crates that pin a revision of this repository |

A change of one of these says so in the pull request and comes with the test that fails on the old behaviour.

## Pull request

- One pull request per unit of work, able to merge alone. Its description is your plan: files, order, risks, proof.
- Never force-push. A repair is a new commit.
- The required check is `rust`. Review policy: `REVIEW.md`.
- Installation on the host is a separate step of the Lead (`scripts/install-ont.sh`); a merged change is not an installed one.

## Not here

No daemon, no scheduler, no second copy of the action log, no domain parser. No student data, credentials or host paths of private data in source, tests or fixtures: fixtures are synthetic.
