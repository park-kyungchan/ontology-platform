# S06 synthetic lineage v2 (forward hardening)

The preimplementation v1 oracle used an all-zero synthetic artifact digest. That
literal sentinel is inadequate as evidence of actual source artifact bytes.
V1 files/contract pins remain immutable. synthetic-source.json is a finite synthetic
source snapshot (objects, observation values and links before Ontology lineage).
Its exact digest now populates every synthetic lineage in synthetic-v2.json and
cases-v2.json. Synthetic pointers now resolve to exact source observation locations. All other CLI expectations and all 26 case identities are unchanged.
The adapter checks both the source bytes' SHA-256 and structural equality with its
input fixture after removing only Ontology lineage metadata. This prevents a
plausible provenance label from standing in for source evidence.

Active comparison inputs are cases-v2.json and contract-pins-v2.json. The latter
checks original frozen v1 artifacts AND v2 files; library-cases-v2.json records the
DELETE-journal clarification separately. This hardening was identified during
source inspection, before any candidate compilation or execution. It is a new
reviewable version, not a rewrite to fit candidate output. Independent reviewer/
Lead acceptance of these forward clarifications and the candidate remains pending.
The original design/knowledge/authorization contract and all recorded S05 bytes
remain unchanged. Nothing establishes live source/account authority.
