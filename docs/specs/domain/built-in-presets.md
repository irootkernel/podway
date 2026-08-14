# Built-in Procedures

Podway ships exactly two built-in Procedure v2 presets. Their YAML files under
[`assets/presets/`](../../../assets/presets/) are canonical and embedded into the
binary with pinned digests.

| ID | Intended use |
| --- | --- |
| `sw-dev-v2` | Goal-tracked software development with implementation, verification, review, and assessment paths. |
| `bug-fix-v2` | Reproduce, diagnose, correct, verify, and assess a defect. |

`podway preset list`, `show`, and `explain` expose only this catalog. A workspace
created by `podway init` defaults to `sw-dev-v2`. A preset start admits the exact
embedded source and fails closed if its shipped digest does not match.

Custom Procedure v2 files remain supported through the configured safe relative
procedure paths. Built-in and custom procedures pass through the same v2 parser,
validator, canonicalizer, and runtime model.
