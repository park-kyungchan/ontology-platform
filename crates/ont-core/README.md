# ont-core

Consumers: linked ontd Service and future explicitly pinned domain consumers.
Public contract: Id/Namespace/Scope, observation knowledge/lineage, opaque Session,
Authority/Admission, Snapshot and restricted Query/Response. No internal-platform
dependency, host user mapping, transport or Academy parser.
Read and derive grants are independently required. Service keeps Admission alive
through serialization; raw Snapshot and Authority construction are trusted APIs.

S07's action module derives `copy_recommendation` from complete/fresh evidence,
binds reviewed source/target inputs and requires explicit named action grants.
Review, dispatch, recovery and receipt publication keep exact account/class/student
scope. Opaque current Session and generation checks remain Authority-owned; no
Host UID or principal-string authentication, identity nonce or registry was added.
