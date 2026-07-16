# Built-in Presets

The four YAML files in this directory are the normative built-in procedure sources:

- `sw-dev.yaml`
- `bug-fix.yaml`
- `docs-only.yaml`
- `analysis.yaml`

The implementation embeds these exact files and validates them through the same schema and semantic validator as custom procedures. A preset update applies only to new sessions and requires a procedure-version change when behavior changes.
