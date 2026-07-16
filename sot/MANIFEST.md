# Package Manifest

This manifest lists every design and implementation-contract file in the Podway design package. The package is intended to be copied into the development repository as the initial specification baseline.

`checksums.sha256` covers every regular file in this package except `checksums.sha256` itself. Paths are relative to the package root.

## Root files

| Path | Purpose |
|---|---|
| `README.md` | Master index, source precedence, reading order, and implementation handoff entry point |
| `IMPLEMENTATION_HANDOFF.md` | Immediate kickoff sequence, first vertical slice, correctness milestone, and prohibited shortcuts |
| `IMPLEMENTATION_STATUS.md` | Non-normative G001–G005 implementation and verification checkpoint index |
| `DESIGN_VERSION` | Immutable design-package version identifier |
| `LICENSE` | MIT license text for Podway |
| `MANIFEST.md` | Complete package inventory |
| `VALIDATION_REPORT.md` | Package-time consistency validation results and required implementation revalidation |
| `checksums.sha256` | SHA-256 integrity list for all other files in the package |

## Product documents

| Path | Purpose |
|---|---|
| `docs/00-product/00-product-overview.md` | Product definition, confirmed decisions, operating model, and success criteria |
| `docs/00-product/01-goals-and-non-goals.md` | Ordered product goals and explicit exclusions |
| `docs/00-product/02-terminology-and-invariants.md` | Normative vocabulary and system-wide invariants |
| `docs/00-product/03-user-workflows.md` | Complete interactive, automated, AI-assisted, retry, return, and reset workflows |

## Architecture documents

| Path | Purpose |
|---|---|
| `docs/10-architecture/10-system-architecture.md` | Component model, process boundaries, ownership, dependency rules, and read/write paths |
| `docs/10-architecture/11-daemon-and-write-queue.md` | Durable admission, per-worktree FIFO scheduling, idempotency, cancellation, barriers, and restart recovery |
| `docs/10-architecture/12-git-worktree-and-filesystem.md` | Worktree discovery, identity, state layout, containment, copies, moves, and deletion |
| `docs/10-architecture/13-macos-service.md` | User LaunchAgent lifecycle, socket and log locations, install/update/uninstall behavior |
| `docs/10-architecture/14-rust-codebase.md` | Cargo workspace, crate responsibilities, dependency direction, concurrency model, and implementation constraints |

## Domain documents

| Path | Purpose |
|---|---|
| `docs/20-domain/20-domain-model.md` | Normative entities, identifiers, revisions, derived state, and ownership |
| `docs/20-domain/21-procedure-and-item-specification.md` | Procedure authoring contract, item types, validation, canonicalization, limits, and artifact handling |
| `docs/20-domain/22-state-transitions.md` | Preconditions, effects, revisions, and errors for every mutation |
| `docs/20-domain/23-rework-and-lifecycle.md` | Retry, return, redo, reopen, block, cancel, reset, and attempt-retention semantics |
| `docs/20-domain/24-built-in-presets.md` | Built-in procedure intent, safeguards, evolution rules, and validation expectations |

## Interface documents

| Path | Purpose |
|---|---|
| `docs/30-interfaces/30-cli-specification.md` | Complete CLI grammar, flags, output behavior, confirmation policy, help, and shell completion |
| `docs/30-interfaces/31-json-contract.md` | Stable success, error, status, next, job, and revision response contracts |
| `docs/30-interfaces/32-ipc-protocol.md` | Unix-domain socket framing, negotiation, request handling, limits, timeouts, and compatibility |
| `docs/30-interfaces/33-errors-and-exit-codes.md` | Public structured error behavior and process exit-code mapping |

## Storage and correctness documents

| Path | Purpose |
|---|---|
| `docs/40-storage/40-sqlite-model.md` | Authoritative relational model, table ownership, constraints, pragmas, migrations, and query rules |
| `docs/40-storage/41-transactions-concurrency-and-idempotency.md` | Transaction boundaries, compare-and-set rules, exact-once effects, ordering, and lost-response behavior |
| `docs/40-storage/42-recovery-retention-and-maintenance.md` | Startup recovery, corruption fail-closed behavior, pruning, doctor, destructive reset, and deletion handling |

## Operations documents

| Path | Purpose |
|---|---|
| `docs/50-operations/50-security-and-trust.md` | Same-user local trust model, path protections, data policy, and explicit non-protections |
| `docs/50-operations/51-observability.md` | Structured logs, daemon diagnostics, redaction, retention, and no-telemetry guarantee |
| `docs/50-operations/52-release-and-packaging.md` | macOS build artifacts, LaunchAgent integration, install/update/uninstall, signing, and compatibility |

## Quality and delivery documents

| Path | Purpose |
|---|---|
| `docs/60-quality/60-testing-and-conformance.md` | Unit, property, crash, concurrency, Git, service, protocol, JSON, preset, and performance test requirements |
| `docs/60-quality/61-product-acceptance.md` | Normative acceptance criteria for the complete product release |
| `docs/60-quality/62-requirements-traceability.md` | Requirement identifiers mapped to implementation areas and conformance evidence |
| `docs/70-delivery/70-implementation-plan.md` | Ordered integration phases, exit gates, critical path, and change-control rules |
| `docs/70-delivery/71-team-work-breakdown.md` | Parallel work streams, ownership, interfaces, handoffs, and integration cadence |
| `docs/70-delivery/72-risk-register.md` | Product and technical risks, symptoms, mitigations, owners, and triggers |
| `docs/70-delivery/73-decision-record.md` | Consolidated final product decisions and deliberately deferred capabilities |

## Architecture Decision Records

| Path | Decision |
|---|---|
| `adr/0001-current-task-session-focus.md` | Podway manages the current task session, not a long-term evidence or audit archive |
| `adr/0002-single-active-stage.md` | Exactly one active stage attempt exists in a running session |
| `adr/0003-daemon-single-writer.md` | `podwayd` is the sole normal writer and mutations use durable per-worktree queues |
| `adr/0004-worktree-local-state.md` | Runtime task state resides inside the Git worktree and disappears with it |
| `adr/0005-rust-and-macos-first.md` | Implementation uses Rust and the first supported platform is macOS |
| `adr/0006-same-user-local-trust.md` | Podway trusts processes running as the same OS user and does not use a workspace access key |
| `adr/0007-stage-items-not-evidence-ledger.md` | Procedure completion uses typed stage items rather than a general evidence ledger |
| `adr/0008-relational-state-not-event-sourcing.md` | SQLite relational state is authoritative and the operational journal is bounded |
| `adr/0009-artifact-metadata-only.md` | Podway stores artifact reference metadata and digests, never artifact bytes |
| `adr/0010-generic-cli-json-integration.md` | External systems integrate through generic CLI and versioned JSON contracts only |

## JSON Schemas

| Path | Contract |
|---|---|
| `schemas/README.md` | Schema scope, identifiers, validation policy, and compatibility rules |
| `schemas/workspace-v1.schema.json` | `.podway/config.yaml` workspace configuration |
| `schemas/procedure-v1.schema.json` | Procedure definition and all supported item types |
| `schemas/registry-v1.schema.json` | Minimal daemon workspace registry representation |
| `schemas/ipc-request-v1.schema.json` | IPC request envelope |
| `schemas/output-v1.schema.json` | Successful public response envelope |
| `schemas/error-v1.schema.json` | Structured public error envelope |
| `schemas/status-result-v1.schema.json` | `podway status --json` result payload |
| `schemas/next-result-v1.schema.json` | `podway next --json` result payload |

## Built-in presets

| Path | Procedure |
|---|---|
| `presets/README.md` | Preset loading, validation, versioning, and customization rules |
| `presets/sw-dev.yaml` | General software-development task procedure |
| `presets/bug-fix.yaml` | Reproduction-first defect correction procedure |
| `presets/docs-only.yaml` | Source-grounded documentation procedure |
| `presets/analysis.yaml` | Question, source, analysis, challenge, and synthesis procedure |

## Normative implementation assets

| Path | Purpose |
|---|---|
| `spec/README.md` | Authority and synchronization rules for machine-readable implementation assets |
| `spec/sqlite-v1.sql` | Reference SQLite schema version 1 |
| `spec/launchagent.plist.template` | Reference macOS user LaunchAgent property-list template |
| `spec/error-codes.json` | Complete machine-readable public error catalog |
| `spec/command-catalog.yaml` | Public command classification, mutation policy, preconditions, and idempotency scope |
| `spec/state-transition-matrix.csv` | Compact transition matrix for implementation and test generation |

## Examples

| Path | Purpose |
|---|---|
| `examples/README.md` | Example scope and rules against treating examples as higher-precedence contracts |
| `examples/.podway/config.yaml` | Complete workspace configuration example |
| `examples/.podway/procedures/custom-bug-fix.yaml` | Complete custom procedure example |
| `examples/example-session.md` | End-to-end current-task session including retry and return |
| `examples/json/status-result.json` | Valid `status` result payload |
| `examples/json/next-result.json` | Valid `next` result payload |
| `examples/json/output-complete.json` | Valid successful completion response envelope |
| `examples/json/error-required-items.json` | Valid structured error for missing required items |
| `examples/json/ipc-complete-request.json` | Valid IPC mutation request |
| `examples/json/registry.json` | Valid minimal daemon registry example |

## Package validation expectations

Before the package is accepted into the implementation repository, CI SHOULD verify:

1. every Markdown relative link resolves;
2. every JSON and YAML file parses;
3. every JSON Schema is valid Draft 2020-12;
4. all four presets and the custom example validate against `procedure-v1.schema.json`;
5. all JSON examples validate against their declared schemas;
6. `spec/sqlite-v1.sql` creates a fresh SQLite database and sets `user_version = 1`;
7. every public error code referenced in Markdown or machine-readable assets exists in `spec/error-codes.json`;
8. `checksums.sha256` matches the package contents.
