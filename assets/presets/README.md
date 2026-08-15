# Built-in Presets

This directory contains the canonical built-in Procedure v2 sources:

- `sw-dev-v2.yaml`
- `bug-fix-v2.yaml`
- `small-change-v2.yaml`

The implementation embeds these exact files. Each preset is admitted through
the same v2 parser, schema, canonicalizer, semantic validator, and vetting path
as a custom Procedure. The sources are bound to independently pinned shipped
digests. A preset update applies only to new sessions and requires a
procedure-version change when behavior changes.

Repository contributors may prepare a candidate with `make preset-create` or
admit an existing YAML Procedure with `make preset-import`. Both commands validate
through the real Podway binary, write only to this canonical directory by default,
and refuse to replace an existing file. Creating or importing a file does not add
it to the embedded catalog: the contributor must still complete the canonical
documentation, embedded catalog, and test changes listed in the
[built-in preset specification](../../docs/specs/domain/built-in-presets.md).
