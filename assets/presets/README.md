# Built-in Presets

The current embedded catalog contains these Procedure v3 bases and Execution
Group v1 default proposals:

| Preset | Base | Default group |
| --- | --- | --- |
| `analysis-v3` | [analysis-v3.yaml](analysis-v3.yaml) | [analysis-default-v3.yaml](analysis-default-v3.yaml) |
| `bug-fix-v3` | [bug-fix-v3.yaml](bug-fix-v3.yaml) | [bug-fix-default-v3.yaml](bug-fix-default-v3.yaml) |
| `small-change-v3` | [small-change-v3.yaml](small-change-v3.yaml) | [small-change-default-v3.yaml](small-change-default-v3.yaml) |
| `sw-dev-v3` | [sw-dev-v3.yaml](sw-dev-v3.yaml) | [sw-dev-default-v3.yaml](sw-dev-default-v3.yaml) |

Both definitions use the same parser and semantic admission as custom
sources. Each embedded canonical definition has a pinned digest. Change its
version when behavior changes. The four `*-v2.yaml` files preserve historical
sources and fixture identities for the previous writer.

Repository contributors may prepare a candidate with `make preset-create` or
admit an existing YAML Procedure with `make preset-import`. Both commands validate
through the real Podway binary, write only to this canonical directory by default,
and refuse to replace an existing file. Creating or importing a file does not add
it to the embedded catalog: the contributor must still complete the canonical
documentation, embedded catalog, and test changes listed in the
[built-in preset specification](../../docs/specs/domain/built-in-presets.md).
