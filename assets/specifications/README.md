# Implementation Specifications

- `error-codes.json`: stable public error and exit-code catalog, including reserved typed-guard failures.
- `authoring-diagnostics.json`: stable authoring-time diagnostic catalog with ambiguity-based option replacements.
- `command-catalog.yaml`: command classification, direct execution behavior, and preconditions.
- `sqlite-v14.sql`: complete direct-store production baseline; older generations are rejected input fixtures only.
- `state-transition-matrix.csv`: compact transition reference.
- `canonicalization-v1.json`: machine-readable canonical JSON and Procedure normalization rules.

The Aquarium producer and service controller are executable repository tools,
not duplicate machine specifications. Their closed external documents are
registered under `assets/schemas/`, and their durable behavior is specified in
`docs/specs/operations/aquarium-development-service.md`.

`command-catalog.yaml` records every closed result discriminator in `result_schemas`;
multiple entries represent command-selected variants. Runtime errors in
`error-codes.json` and authoring findings in `authoring-diagnostics.json` are
separate automation namespaces. `error-codes.json` uses `details_schema` only for
errors with a closed public detail family or a discriminated family whose typed
variants are closed. The
transition matrix binds each state-changing route to its terminal `result_schema`
and uses `none` where no closed terminal result is defined. A
`procedure_digest_file_source_only` precondition is an optional caller guard that
applies to Procedure-file starts and is invalid for preset starts.
The `cli` field identifies one representative route spelling; it is not an
exhaustive inventory of mutually exclusive flags. The CLI specification and
generated help own the complete command grammar.

Narrative rationale and operational rules are in the
[contributor specifications](../../docs/specs/). These files are canonical build
inputs and should be consumed directly by tests where practical. Their logical
release path remains `spec/` for compatibility.
