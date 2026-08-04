# Schemas

These JSON Schema Draft 2020-12 files define Podway's versioned structural contracts.

- `workspace-v1.schema.json`: `.podway/config.yaml` after YAML parsing.
- `procedure-v1.schema.json`: custom procedures and built-in presets.
- `procedure-v2.schema.json`: closed and bounded Procedure v2 YAML/JSON authoring shape.
- `registry-v1.schema.json`: minimal daemon path registry.
- `ipc-request-v1.schema.json`: framed daemon request payload.
- `output-v1.schema.json`: success envelope.
- `error-v1.schema.json`: error envelope.
- `endpoint-error-details-v1.schema.json`: daemon endpoint and availability failures.
- `socket-endpoint-error-details-v1.schema.json`: invalid explicit Unix socket paths.
- `daemon-contract-mismatch-details-v1.schema.json`: CLI/daemon contract identity mismatch.
- `revision-conflict-details-v1.schema.json`: session and item revision conflicts.
- `attempt-conflict-details-v1.schema.json`: stale current-attempt conflicts.
- `blocker-limit-details-v1.schema.json`: active-attempt open-blocker limit failures.
- `idempotency-key-reused-details-v1.schema.json`: rejected idempotency-key reuse.
- `job-wait-timeout-details-v1.schema.json`: query and admitted mutation wait timeouts.
- `status-result-v1.schema.json`: `status` result object.
- `compact-status-result-v1.schema.json`: bounded `status --wait-for-idle --compact` result object.
- `daemon-status-result-v1.schema.json`: merged local-service and live-daemon status result.
- `version-summary-v1.schema.json`: compact public product name and version.
- `version-result-v1.schema.json`: detailed static build and contract identity.
- `procedure-validation-result-v1.schema.json`: validated Procedure digest and canonical form.
- `session-start-result-v1.schema.json`: terminal and dry-run session start outcomes.
- `next-result-v1.schema.json`: `next` result object.
- `item-mutation-result-v1.schema.json`: terminal item mutation outcome.
- `stage-transition-result-v1.schema.json`: terminal stage transition outcome.
- `detached-admission-result-v1.schema.json`: durable detached mutation admission.
- `workspace-init-result-v1.schema.json`: terminal workspace initialization outcome.
- `job-result-v1.schema.json`: `job status` and `job wait` result object.
- `job-lookup-result-v1.schema.json`: idempotency-key reconciliation result.
- `procedure-validation-result-v2.schema.json`: metadata-only Procedure v2 validation success.
- `detached-admission-result-v2.schema.json`: Procedure v2 durable mutation admission.
- `session-start-result-v2.schema.json`: Procedure v2 dry-run and live start outcomes.
- `compact-status-result-v2.schema.json`: bounded value-free Procedure v2 status.
- `status-result-v2.schema.json`: standard status and verbose status with six independently bounded history windows, including decisions and rework.
- `next-result-v2.schema.json`: bounded action, decision, and goal-aware machine guidance.
- `stage-transition-result-v2.schema.json`: Procedure v2 action and administrative transitions.
- `item-mutation-result-v2.schema.json`: Procedure v2 item mutation outcome.
- `authoring-diagnostic-v1.schema.json`: standalone bounded authoring diagnostic.
- `procedure-source-result-v1.schema.json`: format, scaffold, and convert source output.
- `procedure-diagnostics-result-v1.schema.json`: shared bounded diagnostics for every Procedure v2 authoring command.
- `procedure-graph-result-v1.schema.json`: deterministic graph projection output.
- `procedure-preview-result-v1.schema.json`: closed read-only preview report with checks, diagnostics, graph, Mermaid, digest, and an admissible start suggestion.
- `decision-result-v1.schema.json`: immutable routed decision outcome.
- `rework-result-v1.schema.json`: manual rework outcome.
- `goal-definition-result-v1.schema.json`: initial session goal record.
- `goal-revision-result-v1.schema.json`: revised session goal and rework outcome.
- `criterion-assessment-result-v1.schema.json`: one criterion assessment outcome.
- `v2-result-components-v1.schema.json`: shared closed and bounded Procedure v2 result components.
- `contract-manifest-v1.schema.json`: deterministic integration contract inventory.

JSON Schema does not express every semantic rule, including duplicate stage IDs,
min/max relationships, return-destination existence, path containment, or
procedure canonicalization. The [procedure specification](../../docs/specs/domain/procedure-and-item-specification.md)
defines those additional checks.
