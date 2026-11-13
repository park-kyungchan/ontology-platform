# ont-core

Consumers: `ont-store`, the `ont` command and domain crates that pin this repository.
Public contract: Id/Namespace/Scope, observation knowledge/lineage, opaque Session,
Authority/Admission, Snapshot and restricted Query/Response. No internal-platform
dependency, host user mapping, transport or Academy parser.
Read and derive grants are independently required. Service keeps Admission alive
through serialization; raw Snapshot and Authority construction are trusted APIs.

`Scope` is a closed `Education { namespace, class_key, student_key }` or
`Program { namespace, program_key }` enum. `Scope::education` and `Scope::program`
validate nonempty exact keys and the same namespace bounds as `Id`; `namespace()`
borrows the namespace for either variant. No whitespace or UTF-8 normalization is
performed. Education JSON retains namespace/class_key/student_key order; Program
JSON uses namespace/domain/program_key order with `domain: "program"`. Decoding
rejects duplicate, mixed, unknown, missing, null or invalid fields. Grants compare
the entire enum value and object type. This is a Rust source API change requiring
consumers to recompile; legacy Education wire bytes remain compatible. Existing
schema-v1 stores and the local fake target refuse Program intake; Program storage
is a separate change.

S07's action module derives `copy_recommendation` from complete/fresh evidence,
binds reviewed source/target inputs and requires explicit named action grants.
Review, dispatch, recovery and receipt publication keep exact account/class/student
scope. Opaque current Session and generation checks remain Authority-owned; no
Host UID or principal-string authentication, identity nonce or registry was added.

Answer publication retains `ontology-answer/v1` through every existing entrypoint.
`answer::Version::V2` explicitly selects publication-only aggregate and exact
fixed-decimal fields; native `Response` and `ProgramReport` serialization stay
unchanged. The README of the removed crate `ont-daemon` (Git history) documented
field names, arithmetic units, rounding, unavailable reasons and disclosure
rules. The closed constructors accept typed native results, never request text.

`Query::ProgramReportV2` explicitly selects the additive native typed derivation
schema `ontology-program-report/v2` (rule `program-report-derivation/v1`). The
existing ProgramReport builder still selects one authorized basis; the new pure
derivation reads only that basis and its admitted metric membership. Exact
samples, endpoint identity pins, changes, contributions and residuals carry
`Available<T>` with closed unavailable reasons. `Version::V3` publishes these
rational locations with the existing exact decimal renderings. Source qualification
and the original assessment remain conservative. The removed `ont-daemon` README (Git history) documented selectors,
population/period comparability and the unchanged unbound-attribution-endpoint rule.
