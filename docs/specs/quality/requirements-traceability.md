# Requirements Traceability

| Requirement | Authority | Primary evidence |
| --- | --- | --- |
| Procedure v2 is the only admitted procedure model | ADR-0019; Procedure schema | Config, CLI, preset, and daemon admission tests |
| Successful commands use `podway.output/v3` | ADR-0019; output schema | Protocol schema, codec, CLI, daemon, and release-contract tests |
| One active graph attempt and declared movement | Domain model; transition matrix | Core v2 model/property and daemon runtime tests |
| Required items, blockers, evidence, and goals gate advancement | Lifecycle spec | V2 runtime, goal, decision, and rework tests |
| Mutations are atomic, ordered, and idempotent | Transaction spec | Store transaction, concurrency, crash, and reconciliation tests |
| Prepared is a persistent cursor-free session state and begin atomically creates the first running attempt | ADR-0021; lifecycle and transition specs | `V2LIF-003` core/store and `V2LIF-004` daemon/CLI tests |
| Default deletion uses only prepared state or current terminal disposition; force deletion requires confirmation and progress summary | ADR-0021; lifecycle and automation specs | `V2LIF-004` reset/replacement and `V2LIF-005` E2E tests |
| Schema v5 preserves v4 sessions while adding prepared state and terminal disposition | SQLite model; sqlite-v5 DDL | `V2LIF-003` migration, reconstruction, restart, and downgrade tests |
| Workspace UUID and root ownership remain unique while confirmed reset converges proven legacy duplicate-root metadata | Recovery, retention, and maintenance; SQLite model | `V2REC-001` registry and reset-recovery integration tests |
| Exactly three v2 presets ship with pinned identity | Built-in preset spec | Preset embedding, digest, CLI, and production tests |
| Public assets and documentation agree | Contract manifest; docs precedence | Contract, quality, documentation, and architecture checks |

The active roadmap owns completion state. Test success proves only the scope of the
named check; `make test` is the development gate for the combined revision.
