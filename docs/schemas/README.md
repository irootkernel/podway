# Schemas

These JSON Schema Draft 2020-12 files define Podway v1 structural contracts.

- `workspace-v1.schema.json`: `.podway/config.yaml` after YAML parsing.
- `procedure-v1.schema.json`: custom procedures and built-in presets.
- `registry-v1.schema.json`: minimal daemon path registry.
- `ipc-request-v1.schema.json`: framed daemon request payload.
- `output-v1.schema.json`: success envelope.
- `error-v1.schema.json`: error envelope.
- `status-result-v1.schema.json`: `status` result object.
- `daemon-status-result-v1.schema.json`: merged local-service and live-daemon status result.
- `version-result-v1.schema.json`: static product and embedded contract identity.
- `procedure-validation-result-v1.schema.json`: validated Procedure digest and canonical form.
- `session-start-result-v1.schema.json`: terminal and dry-run session start outcomes.
- `next-result-v1.schema.json`: `next` result object.
- `item-mutation-result-v1.schema.json`: terminal item mutation outcome.
- `stage-transition-result-v1.schema.json`: terminal stage transition outcome.
- `detached-admission-result-v1.schema.json`: durable detached mutation admission.
- `job-result-v1.schema.json`: `job status` and `job wait` result object.
- `job-lookup-result-v1.schema.json`: idempotency-key reconciliation result.
- `contract-manifest-v1.schema.json`: deterministic integration contract inventory.

JSON Schema does not express every semantic rule, including duplicate stage IDs, min/max relationships, return-destination existence, path containment, or procedure canonicalization. The procedure specification defines those additional checks.
