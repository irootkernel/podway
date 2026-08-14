# Requirements Traceability

| Requirement | Authority | Primary evidence |
| --- | --- | --- |
| Procedure v2 is the only admitted procedure model | ADR-0019; Procedure schema | Config, CLI, preset, and daemon admission tests |
| Successful commands use `podway.output/v3` | ADR-0019; output schema | Protocol schema, codec, CLI, daemon, and release-contract tests |
| One active graph attempt and declared movement | Domain model; transition matrix | Core v2 model/property and daemon runtime tests |
| Required items, blockers, evidence, and goals gate advancement | Lifecycle spec | V2 runtime, goal, decision, and rework tests |
| Mutations are atomic, ordered, and idempotent | Transaction spec | Store transaction, concurrency, crash, and reconciliation tests |
| Schema v4 preserves v2 state and rejects legacy task state | SQLite model; sqlite-v4 DDL | Focused migration and reset-recovery tests |
| Only two v2 presets ship with pinned identity | Built-in preset spec | Preset embedding, digest, CLI, and production tests |
| Public assets and documentation agree | Contract manifest; docs precedence | Contract, quality, documentation, and architecture checks |

The active roadmap owns completion state. Test success proves only the scope of the
named check; `make test` is the development gate for the combined revision.
