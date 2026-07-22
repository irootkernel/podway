# Schemas

These JSON Schema Draft 2020-12 files define Podway v1 structural contracts.

- `workspace-v1.schema.json`: `.podway/config.yaml` after YAML parsing.
- `procedure-v1.schema.json`: custom procedures and built-in presets.
- `registry-v1.schema.json`: minimal daemon path registry.
- `ipc-request-v1.schema.json`: framed daemon request payload.
- `output-v1.schema.json`: success envelope.
- `error-v1.schema.json`: error envelope.
- `status-result-v1.schema.json`: `status` result object.
- `next-result-v1.schema.json`: `next` result object.

JSON Schema does not express every semantic rule, including duplicate stage IDs, min/max relationships, return-destination existence, path containment, or procedure canonicalization. The procedure specification defines those additional checks.
