# Built-in Procedures

Podway ships exactly three built-in Procedure v2 presets. Their YAML files under
[`assets/presets/`](../../../assets/presets/) are canonical and embedded into the
binary with pinned digests.

| ID | Intended use |
| --- | --- |
| `sw-dev-v2` | Goal-tracked software development with implementation, verification, review, and assessment paths. |
| `bug-fix-v2` | Reproduce, diagnose, correct, verify, and assess a defect. |
| `small-change-v2` | Inspect, implement, verify, review, and close a bounded change without goal tracking. |

`podway preset list`, `show`, and `explain` expose only this catalog. A workspace
created by `podway init` defaults to `sw-dev-v2`. A preset start admits the exact
embedded source and fails closed if its shipped digest does not match.

Custom Procedure v2 files remain supported through the configured safe relative
procedure paths. Built-in and custom procedures pass through the same v2 parser,
validator, canonicalizer, and runtime model.

`small-change-v2` follows `inspect -> implement -> verify -> review -> closeout`.
The review decision advances through `ready` or returns to `implement` through
`changes-requested`; manual rework may target `inspect`, `implement`, or `verify`.
The preset records bounded summaries, the exact verification command and integer
exit status, and a terminal note. It intentionally omits goal tracking, source
revisions, log digests, artifacts, and criterion assessment.
