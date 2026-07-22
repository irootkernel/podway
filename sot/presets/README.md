# Built-in Presets

The four YAML files in this directory are the normative built-in procedure sources:

- `sw-dev.yaml`
- `bug-fix.yaml`
- `docs-only.yaml`
- `analysis.yaml`

The implementation embeds these exact files and validates them through the same schema and semantic validator as custom procedures. A preset update applies only to new sessions and requires a procedure-version change when behavior changes.

The v0.1 release catalog remains exactly these four presets. Repository contributors
may prepare a future candidate with `make preset-create` or admit an existing YAML
procedure with `make preset-import`. Both commands validate through the real Podway
binary, write only to the canonical `sot/presets/` source directory by default, and
refuse to replace an existing file. Creating or importing a file does not add it to
the shipped catalog: the contributor must still complete the SOT, embedded catalog,
documentation, and test changes listed in the built-in preset specification.
