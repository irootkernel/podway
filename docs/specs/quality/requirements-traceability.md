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
| One Unicode-scalar scale envelope binds authoring, runtime, persistence, protocol, and documentation | ADR-0024; Procedure and item specification | `V2SCL-003` domain, config, schema-equality, and protocol slice tests |
| Declared current evidence is readable one bounded page at a time without raising the 1 MiB frame | ADR-0024; IPC protocol | `V2SCL-002` reserved-contract and `V2SCL-004` store, daemon, CLI, and token tests |
| Every windowed response collection reports an exact total and its truncation state | ADR-0024; IPC response budgets | `V2SCL-005` budget, observation-composition, and maximum-size tests |
| External check results are structurally bound attempt-local items without becoming verification, execution, or a second evidence ledger | ADR-0025; Procedure and item specification | `V2AST-003`–`006` domain, contract, migration, runtime, observation, CLI, and trust-language tests |
| Typed conditions derive required items and guarded options from bounded current state without adding an expression engine or weakening freshness | ADR-0028; Procedure, transition, automation, and IPC specifications | `V2GRD-002`–`006` schema, domain, authoring, runtime, budget, CLI, and compatibility tests |
| Exactly four distinct v2 presets ship with pinned identity, exact graphs, and bounded path inventories | Built-in preset spec | Preset embedding, digest, CLI, production tests, and executable reference-path fixture |
| Public assets and documentation agree | Contract manifest; docs precedence | Contract, quality, documentation, and architecture checks |

The active roadmap owns completion state. Test success proves only the scope of the
named check; `make test` is the development gate for the combined revision.
