# Schemas

These JSON Schema Draft 2020-12 files define Podway's versioned structural contracts.

- `workspace-v1.schema.json`: `.podway/config.yaml` after YAML parsing.
- `procedure-v2.schema.json`: closed and bounded Procedure v2 YAML/JSON authoring shape.
- `registry-v1.schema.json`: minimal daemon path registry.
- `ipc-request-v1.schema.json`: framed daemon request payload.
- `output-v3.schema.json`: unified success envelope with closed command-to-result selection.
- `error-v1.schema.json`: error envelope.
- `endpoint-error-details-v1.schema.json`: daemon endpoint and availability failures.
- `socket-endpoint-error-details-v1.schema.json`: invalid explicit Unix socket paths.
- `daemon-contract-mismatch-details-v1.schema.json`: CLI/daemon contract identity mismatch.
- `revision-conflict-details-v1.schema.json`: session and item revision conflicts.
- `attempt-conflict-details-v1.schema.json`: stale current-attempt conflicts.
- `blocker-limit-details-v1.schema.json`: active-attempt open-blocker limit failures.
- `idempotency-key-reused-details-v1.schema.json`: rejected idempotency-key reuse.
- `job-wait-timeout-details-v1.schema.json`: query and admitted mutation wait timeouts.
- `daemon-status-result-v1.schema.json`: merged local-service and live-daemon status result.
- `version-summary-v1.schema.json`: compact public product name and version.
- `version-result-v1.schema.json`: detailed static build and contract identity.
- `workspace-init-result-v1.schema.json`: terminal workspace initialization outcome.
- `detached-admission-result-v1.schema.json`: detached workspace initialization admission.
- `job-result-v3.schema.json`: job status/wait wrapper and terminal response.
- `job-lookup-result-v3.schema.json`: idempotency-key reconciliation result.
- `procedure-validation-result-v2.schema.json`: metadata-only Procedure v2 validation success.
- `detached-admission-result-v2.schema.json`: Procedure v2 durable mutation admission.
- `session-start-result-v2.schema.json`: Procedure v2 dry-run and live start outcomes.
- `compact-status-result-v2.schema.json`: bounded value-free Procedure v2 status.
- `status-result-v2.schema.json`: standard status and verbose status with six independently bounded history windows, including decisions and rework.
- `next-result-v2.schema.json`: bounded action, decision, and goal-aware machine guidance.
- `stage-transition-result-v2.schema.json`: Procedure v2 action and administrative transitions.
- `item-mutation-result-v2.schema.json`: Procedure v2 item mutation outcome.
- `authoring-diagnostic-v1.schema.json`: standalone bounded authoring diagnostic.
- `procedure-source-result-v1.schema.json`: format and scaffold source output.
- `procedure-diagnostics-result-v1.schema.json`: shared bounded diagnostics for every Procedure v2 authoring command.
- `procedure-graph-result-v1.schema.json`: deterministic graph projection output.
- `procedure-preview-result-v1.schema.json`: closed read-only preview report with checks, diagnostics, graph, Mermaid, digest, and an admissible start suggestion.
- `decision-result-v1.schema.json`: immutable routed decision outcome.
- `rework-result-v1.schema.json`: manual rework outcome.
- `goal-definition-result-v1.schema.json`: initial session goal record.
- `goal-revision-result-v1.schema.json`: revised session goal and rework outcome.
- `criterion-assessment-result-v1.schema.json`: one criterion assessment outcome.
- `v2-result-components-v1.schema.json`: shared closed and bounded Procedure v2 result components.
- `v2-runtime-error-details-v1.schema.json`: closed code-bound details for registered Procedure v2 runtime errors.
- `contract-manifest-v1.schema.json`: deterministic integration contract inventory.

JSON Schema does not express every semantic rule, including duplicate identifiers,
min/max relationships, graph liveness, path containment, or
procedure canonicalization. The [procedure specification](../../docs/specs/domain/procedure-and-item-specification.md)
defines those additional checks.
