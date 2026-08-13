# Built-in Presets

This directory contains the canonical built-in Procedure sources. The retained
v1 catalog contains four YAML files:

- `sw-dev.yaml`
- `bug-fix.yaml`
- `docs-only.yaml`
- `analysis.yaml`

The v0.2.0 catalog contains two additional YAML files:

- `sw-dev-v2.yaml`
- `bug-fix-v2.yaml`

The implementation embeds these exact files. Each v1 or v2 preset is admitted
through the same parser, schema, canonicalizer, and semantic validator as a
custom Procedure of the same schema. The v2 sources are additionally vet-clean
and bound to independently pinned shipped digests. A preset update applies only
to new sessions and requires a procedure-version change when behavior changes.

The released v0.1 catalog remains exactly the four v1 presets. The two v2 presets are
implemented and included in the shipped v0.2.0 artifact. Normal v2 session admission
is enabled in those bytes, and the development-only admission unlock is absent.

Repository contributors may prepare a candidate with `make preset-create` or
admit an existing YAML Procedure with `make preset-import`. Both commands validate
through the real Podway binary, write only to this canonical directory by default,
and refuse to replace an existing file. Creating or importing a file does not add
it to either embedded catalog: the contributor must still complete the canonical
documentation, embedded catalog, and test changes listed in the
[built-in preset specification](../../docs/specs/domain/built-in-presets.md).
