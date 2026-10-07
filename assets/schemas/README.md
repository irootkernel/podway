# Schemas

These JSON Schema Draft 2020-12 files define Podway's versioned structural contracts.

The current daemon-free public envelopes are `output-v4.schema.json` and
`error-v2.schema.json`. Current result and detail families are selected by the
protocol registries and the command-to-result branches in those envelopes.
`v2-result-components-v4.schema.json` owns their shared bounded components.
IPC requests, daemon status, durable jobs, admission metadata, runtime modes and
legacy result generations were retired by V2DFC-008.

`workspace-v1.schema.json` and `workspace-v2.schema.json` describe configuration
input after YAML parsing. A legacy `job_queue` value is accepted within global
parser bounds and ignored. Workspace v2 accepts only the optional legacy `prod`
mode. Both legacy members are omitted from canonical serialization.
`procedure-v2.schema.json` defines the closed, bounded authoring shape.

`version-result-v2.schema.json` and `contract-manifest-v2.schema.json` own the
current single-binary version and manifest identity. Version emits Output v4.
The manifest has no IPC IDs. Output v3/version v1/manifest v1, managed-runtime and
Aquarium development-service schemas are retired by V2DFC-009; no managed service
replacement is adopted.

Delegation documents, receipts, current projections and authoring views retain
their semantic and byte bounds. `x-podway-max-utf8-bytes` records byte caps that
JSON Schema `maxLength`, which counts Unicode characters, cannot enforce alone.
Unsigned integer inputs must use integer JSON literals; the parser rejects
zero-fraction forms such as `1.0` even where JSON Schema accepts them.

JSON Schema does not express every semantic rule, including duplicate identifiers,
min/max relationships, graph liveness, path containment and canonicalization.
The [procedure specification](../../docs/specs/domain/procedure-and-item-specification.md)
defines those additional checks.
