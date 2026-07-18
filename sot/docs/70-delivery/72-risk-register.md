# Risk Register

| ID | Risk | Likelihood | Impact | Trigger or signal | Mitigation | Primary owner |
|---|---|---:|---:|---|---|---|
| `R-01` | Queue implementation creates duplicate effects after crash | medium | critical | crash test produces two revisions or attempts | durable admission, atomic terminal commit, idempotency, crash matrix | Store/Queue |
| `R-02` | A stale queued command mutates a later stage | medium | critical | command succeeds after active attempt changed | require attempt and revision preconditions; never auto-retarget | Domain/Store |
| `R-03` | Same-item concurrent updates silently overwrite | medium | high | last writer wins without conflict | persistent item revisions and compare-and-set | Domain/Store |
| `R-04` | Worktree path or symlink escapes state boundary | low | critical | runtime or artifact opens outside root | canonical containment, no runtime symlink, adversarial fixtures | Git/Filesystem |
| `R-05` | Worktree-local queue cannot recover after daemon restart | medium | high | acknowledged jobs remain idle | minimal registry, startup scan, path repair | Daemon |
| `R-06` | Worktree move or copy causes split identity | medium | high | same UUID at two roots or moved jobs lost | Git fingerprints, conflict detection, repair, destructive reinit | Git/Daemon |
| `R-07` | LaunchAgent keep-alive conflicts with explicit stop or upgrades | medium | medium | daemon restarts unexpectedly or old binary persists | bootout/bootstrap lifecycle, install metadata, integration tests | Service |
| `R-08` | Procedure authoring becomes too complex | medium | high | users bypass Podway or use only confirm items | six simple item types, strong presets, `next` command suggestions | Product/CLI |
| `R-09` | Product drifts back into evidence/audit scope | medium | high | export/history/revocation features enter critical path | non-goals, ADR-0001/0007/0008, design review | Architecture |
| `R-10` | Artifact hashing blocks queues on large files | medium | medium | long queue latency | hash outside DB transaction, streaming I/O, report duration, keep artifacts focused | Daemon/Store |
| `R-11` | Required local artifact changes after attach | high | medium | completion uses stale metadata | rehash at complete with slot revision recheck | Domain/Store |
| `R-12` | YAML parser permits resource exhaustion or code-like behavior | low | high | large alias expansion or custom tags | strict parser limits, duplicate-key rejection, no tags/includes | Config/Security |
| `R-13` | JSON/IPC contracts drift between CLI and daemon | medium | high | upgrade incompatibility or golden failure | shared protocol crate, schemas in CI, compatibility tests | Protocol/CLI |
| `R-14` | Full macOS service tests are flaky or unavailable | medium | high | release lane cannot prove login startup | isolated test account/VM, deterministic service abstraction tests | Service/QA |
| `R-15` | SQLite schema and domain invariants diverge | medium | high | doctor detects impossible state | migration checks, invariant scan, transaction review | Store/Domain |
| `R-16` | Global registry accidentally accumulates task data | low | medium | registry fields expand | fixed schema, tests, architecture review | Daemon/Security |
| `R-17` | Logs leak task or item content | medium | medium | request debug output appears in logs | structured allowlist logging and redaction tests | Daemon/Security |
| `R-18` | Destructive reset is used accidentally | low | high | session lost without intent | TTY prompt, `--yes`, `--force`, dry run, explicit help | CLI |
| `R-19` | Apple Silicon release portability or toolchain issue delays delivery | medium | medium | `aarch64-apple-darwin` CI build, dependency, signing, or notarization failure | build and validate the Apple Silicon target in CI; resolve target-specific dependency, signing, and notarization issues before release | Release |
| `R-20` | Same-user threat is misunderstood as strong authentication | medium | medium | users rely on Podway for audit proof | explicit documentation, no “secure evidence” language, no access key | Product/Security |
| `R-21` | `next` suggestions are incomplete or unsafe to copy | medium | high | users still omit steps | structured argv suggestions, preset E2E assertions, UX dogfooding | CLI/Product |
| `R-22` | Reset-all crash leaves workspace unusable | low | high | marker or partial DB remains | marker protocol and crash cases C14-C16 | Store/Daemon |
| `R-23` | Dependency supply-chain or licensing issue blocks release | low | high | audit failure near release | early dependency review, lockfile, limited dependency surface | Release |
| `R-24` | Overengineering delays complete product | medium | high | long work on non-goals or generic frameworks | fixed scope, phased gates, reject speculative adapters/history | Project lead |

## Risk review cadence

- Review critical and high risks at every integration milestone.
- Add a conformance test when a risk is triggered by a defect.
- A new critical risk may block integration until an owner and mitigation are assigned.
- Closing a risk requires evidence in tests, release tooling, or accepted design, not only an implementation claim.
