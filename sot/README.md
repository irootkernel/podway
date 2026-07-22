# Podway Design Specification

**Design version:** 1.0.1-design  
**Status:** implementation-ready source of truth  
**Product:** Podway  
**Implementation language:** Rust  
**Primary platform:** macOS  
**License:** MIT

Podway is a local procedure guard for one current task in a Git worktree. It keeps the task on an ordered procedure, shows the active stage and missing requirements, serializes all mutations through a user-scoped daemon, and forces affected stages to be repeated after retry or return.

Podway is intentionally focused on **current task execution**. It is not an evidence archive, review database, post-mortem system, project-management board, workflow server, command runner, or Git automation layer.

## Normative language and source precedence

The words **MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT**, and **MAY** are normative.

When documents disagree, use this precedence:

1. accepted Architecture Decision Records in [`adr/`](adr/);
2. machine-readable contracts in [`schemas/`](schemas/) and [`spec/`](spec/);
3. feature specifications in [`docs/`](docs/);
4. examples in [`examples/`](examples/).

Any contradiction should be fixed in all affected files before implementation proceeds. There are no intentionally open product decisions in this package.

## Handoff shortcuts

- [Implementation handoff](IMPLEMENTATION_HANDOFF.md): immediate kickoff sequence and prohibited shortcuts.
- [Implementation status](IMPLEMENTATION_STATUS.md): non-normative implementation and local release-gate status.
- [Manifest](MANIFEST.md): complete archive inventory.
- [Design version](DESIGN_VERSION): specification version consumed by the team.
- [Validation report](VALIDATION_REPORT.md): package consistency checks completed before handoff.
- [Checksums](checksums.sha256): SHA-256 integrity list for the package.

## Recommended reading order

1. [Product overview](docs/00-product/00-product-overview.md)
2. [Goals and non-goals](docs/00-product/01-goals-and-non-goals.md)
3. [Terminology and invariants](docs/00-product/02-terminology-and-invariants.md)
4. [User workflows](docs/00-product/03-user-workflows.md)
5. [System architecture](docs/10-architecture/10-system-architecture.md)
6. [Daemon and write queue](docs/10-architecture/11-daemon-and-write-queue.md)
7. [Domain model](docs/20-domain/20-domain-model.md)
8. [Procedure and item specification](docs/20-domain/21-procedure-and-item-specification.md)
9. [Built-in presets](docs/20-domain/24-built-in-presets.md)
10. [State transitions](docs/20-domain/22-state-transitions.md)
11. [CLI specification](docs/30-interfaces/30-cli-specification.md)
12. [IPC protocol](docs/30-interfaces/32-ipc-protocol.md)
13. [SQLite model](docs/40-storage/40-sqlite-model.md)
14. [Testing and conformance](docs/60-quality/60-testing-and-conformance.md)
15. [Implementation plan](docs/70-delivery/70-implementation-plan.md)
16. [Team work breakdown](docs/70-delivery/71-team-work-breakdown.md)

## Document index

### Product

| Document | Purpose |
|---|---|
| [Product overview](docs/00-product/00-product-overview.md) | Product statement, scope, confirmed decisions, and success model |
| [Goals and non-goals](docs/00-product/01-goals-and-non-goals.md) | Priority order, explicit exclusions, and product boundaries |
| [Terminology and invariants](docs/00-product/02-terminology-and-invariants.md) | Shared vocabulary and system-wide invariants |
| [User workflows](docs/00-product/03-user-workflows.md) | End-to-end human, script, and AI-assisted usage flows |

### Architecture

| Document | Purpose |
|---|---|
| [System architecture](docs/10-architecture/10-system-architecture.md) | Components, responsibilities, data ownership, and dependency boundaries |
| [Daemon and write queue](docs/10-architecture/11-daemon-and-write-queue.md) | Single-writer model, durable jobs, scheduling, cancellation, and restart recovery |
| [Git worktree and filesystem](docs/10-architecture/12-git-worktree-and-filesystem.md) | Workspace discovery, identity, layout, path safety, moves, copies, and deletion |
| [macOS service](docs/10-architecture/13-macos-service.md) | LaunchAgent installation, socket paths, service lifecycle, and logs |
| [Rust codebase architecture](docs/10-architecture/14-rust-codebase.md) | Cargo workspace, crate responsibilities, dependency rules, and implementation constraints |

### Domain model

| Document | Purpose |
|---|---|
| [Domain model](docs/20-domain/20-domain-model.md) | Workspace, session, procedure snapshot, stages, attempts, items, blockers, and jobs |
| [Procedure and item specification](docs/20-domain/21-procedure-and-item-specification.md) | Authoring schema, item types, validation rules, canonicalization, and limits |
| [State transitions](docs/20-domain/22-state-transitions.md) | Preconditions and effects for every session mutation |
| [Rework and lifecycle](docs/20-domain/23-rework-and-lifecycle.md) | Retry, return, reopen, redo, cancel, reset, and historical-attempt boundaries |
| [Built-in presets](docs/20-domain/24-built-in-presets.md) | Purpose, safeguards, and evolution rules for the four built-in procedures |

### Public interfaces

| Document | Purpose |
|---|---|
| [CLI specification](docs/30-interfaces/30-cli-specification.md) | Full command grammar, flags, confirmations, help, and shell completion |
| [JSON contract](docs/30-interfaces/31-json-contract.md) | Stable machine-readable envelopes and result shapes |
| [IPC protocol](docs/30-interfaces/32-ipc-protocol.md) | Unix-domain socket framing, request/response rules, limits, and compatibility |
| [Errors and exit codes](docs/30-interfaces/33-errors-and-exit-codes.md) | Public error catalog, retryability, and process exit mapping |

### Storage and correctness

| Document | Purpose |
|---|---|
| [SQLite model](docs/40-storage/40-sqlite-model.md) | Authoritative relational model, tables, constraints, migrations, and pragmas |
| [Transactions, concurrency, and idempotency](docs/40-storage/41-transactions-concurrency-and-idempotency.md) | Job admission, FIFO execution, compare-and-set rules, and exact-once effects |
| [Recovery, retention, and maintenance](docs/40-storage/42-recovery-retention-and-maintenance.md) | Crash recovery, corruption behavior, pruning, doctor, and reset-all |

### Operations

| Document | Purpose |
|---|---|
| [Security and trust](docs/50-operations/50-security-and-trust.md) | Same-user trust model, protections, explicit limitations, and data handling |
| [Observability](docs/50-operations/51-observability.md) | Logging, daemon status, diagnostics, redaction, and no-telemetry policy |
| [Release and packaging](docs/50-operations/52-release-and-packaging.md) | macOS artifacts, installation, signing, compatibility, and release operations |

### Quality and delivery

| Document | Purpose |
|---|---|
| [Testing and conformance](docs/60-quality/60-testing-and-conformance.md) | Unit, property, crash, concurrency, Git, service, protocol, and preset tests |
| [Product acceptance](docs/60-quality/61-product-acceptance.md) | Complete public-release acceptance criteria |
| [Requirements traceability](docs/60-quality/62-requirements-traceability.md) | Requirement IDs mapped to design sections and conformance tests |
| [Implementation plan](docs/70-delivery/70-implementation-plan.md) | Ordered implementation program and integration gates |
| [Team work breakdown](docs/70-delivery/71-team-work-breakdown.md) | Parallel team streams, ownership, handoffs, and critical path |
| [Risk register](docs/70-delivery/72-risk-register.md) | Principal technical and product risks with mitigations and triggers |
| [Decision record](docs/70-delivery/73-decision-record.md) | Consolidated final decisions and intentionally deferred capabilities |

## Machine-readable and implementation assets

| Path | Purpose |
|---|---|
| [`schemas/workspace-v1.schema.json`](schemas/workspace-v1.schema.json) | Workspace configuration schema |
| [`schemas/procedure-v1.schema.json`](schemas/procedure-v1.schema.json) | Procedure definition schema |
| [`schemas/ipc-request-v1.schema.json`](schemas/ipc-request-v1.schema.json) | IPC request envelope schema |
| [`schemas/output-v1.schema.json`](schemas/output-v1.schema.json) | Success response envelope schema |
| [`schemas/error-v1.schema.json`](schemas/error-v1.schema.json) | Error response envelope schema |
| [`schemas/status-result-v1.schema.json`](schemas/status-result-v1.schema.json) | `status` result schema |
| [`schemas/next-result-v1.schema.json`](schemas/next-result-v1.schema.json) | `next` result schema |
| [`schemas/registry-v1.schema.json`](schemas/registry-v1.schema.json) | Minimal daemon workspace registry schema |
| [`presets/`](presets/) | Complete built-in preset YAML files |
| [`spec/sqlite-v1.sql`](spec/sqlite-v1.sql) | Reference SQLite DDL |
| [`spec/launchagent.plist.template`](spec/launchagent.plist.template) | Reference macOS LaunchAgent template |
| [`spec/error-codes.json`](spec/error-codes.json) | Machine-readable public error catalog |
| [`spec/command-catalog.yaml`](spec/command-catalog.yaml) | Command classification and mutation/read metadata |
| [`spec/state-transition-matrix.csv`](spec/state-transition-matrix.csv) | Compact state-transition reference |
| [`examples/`](examples/) | Workspace config, custom procedure, and complete session walkthrough |
| [`checksums.sha256`](checksums.sha256) | SHA-256 integrity list for all other package files |

## Architecture Decision Records

| ADR | Decision |
|---|---|
| [ADR-0001](adr/0001-current-task-session-focus.md) | Focus on the current task session rather than long-term evidence or audit |
| [ADR-0002](adr/0002-single-active-stage.md) | Permit exactly one active stage attempt per session |
| [ADR-0003](adr/0003-daemon-single-writer.md) | Make `podwayd` the sole normal state writer |
| [ADR-0004](adr/0004-worktree-local-state.md) | Store task state inside the Git worktree |
| [ADR-0005](adr/0005-rust-and-macos-first.md) | Implement in Rust and deliver macOS first |
| [ADR-0006](adr/0006-same-user-local-trust.md) | Use same-user local trust with no workspace access key |
| [ADR-0007](adr/0007-stage-items-not-evidence-ledger.md) | Model required stage items rather than a general evidence system |
| [ADR-0008](adr/0008-relational-state-not-event-sourcing.md) | Use authoritative SQLite relational state rather than event sourcing |
| [ADR-0009](adr/0009-artifact-metadata-only.md) | Store artifact metadata, never artifact bytes |
| [ADR-0010](adr/0010-generic-cli-json-integration.md) | Integrate external tools through generic CLI and JSON only |
| [ADR-0011](adr/0011-local-make-test-release-gate.md) | Use the local `make test` suite as the sole release-readiness gate |

## Implementation handoff

The development team should treat this archive as the design baseline. Before merging implementation work, the team should:

1. assign owners using [the team work breakdown](docs/70-delivery/71-team-work-breakdown.md);
2. freeze the public schemas and error catalog in the first integration milestone;
3. build the pure domain conformance suite before storage or daemon code;
4. validate all built-in presets against the shipped schema through `make test`;
5. require every public command to have text help, JSON golden tests, and error-code tests;
6. reject changes that expand Podway into command execution, Git mutation, remote service, or long-term audit without a new accepted ADR.
