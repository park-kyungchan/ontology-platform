# Ontology platform — bounded S06 source candidate

Three packages implement a source-qualified restricted read vertical:

- `ont-core`: identity, evidence, local principal/grants, authorization and deterministic decisions.
- `ont-store`: small isolated SQLite store with atomic append/head updates and snapshot reads.
- `ontd`: linked composition, narrow recorded S05 adapter and explicit synthetic stdin CLI.

This new repository has no production source or remote. Contract and independent
literal expectations were frozen at `1933f9b48f6efc01a7011d9559c9b66169d2fe69`
before Rust implementation. See [contract](docs/S06-CONTRACT.md),
[snapshot clarification](docs/S06-SNAPSHOT-CLARIFICATION.md),
[lineage hardening](docs/S06-LINEAGE-CLARIFICATION.md) and
[validation/effect limits](docs/S06-VALIDATION.md). Source-only delivery is not an
operational/compiler result. Lead owns the serialized build slot and independent
review. No S07 action, production authentication, network listener, live source
connection, Academy write or S12 user verdict is implemented or claimed.

The synthetic principal is fixed in `ontd::synthetic`; stdin cannot select a
principal or policy. A real embedding application must authenticate independently
before calling `Authority::bind_trusted`, supply a trusted clock and use `Service`.
The direct Store interface is a trusted internal API, not a permission facade.

After the Lead grants a build slot, use the commands in S06-VALIDATION.md. Example
stdin (against the resulting binary with `--synthetic`):

```json
{"op":"decide","root":{"object_type":"student","namespace":{"instance":"synthetic://same-host","account":"A"},"native_key":"001"},"relation":"learning"}
```

It should produce a review-required decision with permitted linked evidence,
revision history, explicit partial coverage and per-observation freshness. The
recorded fixture account has instance `recording://s05-synthetic`, account
`fixture-A`, student `synthetic-student-1`, relations `section` and `attempt`.
These are synthetic recording identities, not native account assertions.
