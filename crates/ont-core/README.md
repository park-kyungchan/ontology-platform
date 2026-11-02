# ont-core

Consumers: linked ontd Service and future explicitly pinned domain consumers.
Public contract: Id/Namespace/Scope, observation knowledge/lineage, opaque Session,
Authority/Admission, Snapshot and restricted Query/Response. No internal-platform
dependency, host user mapping, transport, Academy parser or action transitions.
Read and derive grants are independently required. Service keeps Admission alive
through serialization; raw Snapshot and Authority construction are trusted APIs.
