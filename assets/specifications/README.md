# Implementation Specifications

- `sqlite-v1.sql`: reference authoritative base database DDL.
- `sqlite-v2.sql`: response-context migration for durable terminal-envelope reconstruction.
- `sqlite-v3.sql`: parallel Procedure v2 relational state and recovery migration.
- `sqlite-v4.sql`: v2-only migration with fail-closed legacy Procedure state rejection.
- `sqlite-v5.sql`: prepared-session and terminal-disposition migration reservation.
- `launchagent.plist.template`: reference macOS user LaunchAgent.
- `error-codes.json`: stable public error and exit-code catalog.
- `authoring-diagnostics.json`: stable authoring-time diagnostic catalog.
- `command-catalog.yaml`: command classification, queue behavior, and preconditions.
- `state-transition-matrix.csv`: compact transition reference.
- `canonicalization-v1.json`: machine-readable canonical JSON and Procedure normalization rules.

`command-catalog.yaml` records every closed result discriminator in `result_schemas`;
multiple entries represent request-selected or detached variants. Runtime errors in
`error-codes.json` and authoring findings in `authoring-diagnostics.json` are
separate automation namespaces. `error-codes.json`
uses `details_schema` only for errors with a closed public detail family. The
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
