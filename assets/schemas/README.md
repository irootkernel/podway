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

`output-v3.schema.json` is retained exclusively for the unchanged `version`
command identity through V2DFC-009. `version-result-v1.schema.json`,
`version-summary-v1.schema.json`, `contract-manifest-v1.schema.json`,
`managed-runtime-v3.schema.json` and the Aquarium development-service schemas
remain under that separate release-identity transition. Their retention does not
restore daemon command routes. Reserved version v2 and manifest v2 schemas remain
unemitted until their owning task.

Delegation documents, receipts, current projections and authoring views retain
their semantic and byte bounds. `x-podway-max-utf8-bytes` records byte caps that
JSON Schema `maxLength`, which counts Unicode characters, cannot enforce alone.
Unsigned integer inputs must use integer JSON literals; the parser rejects
zero-fraction forms such as `1.0` even where JSON Schema accepts them.

JSON Schema does not express every semantic rule, including duplicate identifiers,
min/max relationships, graph liveness, path containment and canonicalization.
The [procedure specification](../../docs/specs/domain/procedure-and-item-specification.md)
defines those additional checks.
