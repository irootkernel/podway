# Roadmap

This roadmap reorganizes the work completed for Podway 0.1.0 into sequential
capability epics. It is a historical implementation record, not a future backlog.

## Sequence policy

Epics run from top to bottom. Tasks within an epic run in numeric order. A task may
start only when every earlier task in its epic is `Completed`, and an epic may start
only when every task in every preceding epic is `Completed`. All entries below are
complete in the current v0.1.0 implementation.

## DESGN — Design Baseline

| id | title | status | goal | references |
|---|---|---|---|---|
| `DESGN001` | Reconcile product goals | Completed | Define the current-task problem, success model, and explicit non-goals. | [Goal](project.md#goal), [Non-goals](project.md#non-goals) |
| `DESGN002` | Freeze v1 contracts | Completed | Fix the initial procedure, workspace, JSON, IPC, error, and storage contracts. | [Supported boundary](project.md#supported-boundary), [Canonical assets](README.md#canonical-assets) |
| `DESGN003` | Record architecture decisions | Completed | Make the single-writer, worktree-local, same-user, and non-execution boundaries explicit. | [Principles](project.md#principles), [ADRs](adr/) |
| `DESGN004` | Validate the design baseline | Completed | Remove contradictions and establish an implementation-ready reading order and precedence. | [Precedence](README.md#precedence), [Detailed reference](README.md#detailed-reference) |

## FOUND — Repository Foundation

| id | title | status | goal | references |
|---|---|---|---|---|
| `FOUND001` | Bootstrap the Rust workspace | Completed | Establish the pinned Rust workspace, application lockfile, binaries, and crate skeletons. | [Workspace map](structure.md#workspace-map), [Prerequisites](contributing.md#prerequisites) |
| `FOUND002` | Define dependency boundaries | Completed | Enforce pure-core and acyclic infrastructure dependency direction. | [Dependency direction](structure.md#dependency-direction) |
| `FOUND003` | Establish canonical assets | Completed | Make schemas, specifications, and presets reviewable and reproducibly mirrored. | [Canonical and generated assets](structure.md#canonical-and-generated-assets) |
| `FOUND004` | Lock executable contracts | Completed | Record command routes, crate adjacency, handoffs, fixtures, and validation sentinels. | [Workspace map](structure.md#workspace-map), [Tests](structure.md#tests) |
| `FOUND005` | Establish local verification | Completed | Provide ordered preparation, unit, integration, fuzz, and end-to-end entry points. | [Verification layers](contributing.md#verification-layers) |

## COREX — Domain and Procedure Engine

| id | title | status | goal | references |
|---|---|---|---|---|
| `COREX001` | Define domain state | Completed | Model workspaces, sessions, stages, attempts, items, blockers, artifacts, and jobs. | [Domain model](reference/domain/20-domain-model.md) |
| `COREX002` | Parse and validate procedures | Completed | Reject invalid YAML, unknown fields, unsafe paths, and violated semantic limits. | [Procedure specification](reference/domain/21-procedure-and-item-specification.md) |
| `COREX003` | Canonicalize procedure snapshots | Completed | Produce deterministic immutable snapshots and SHA-256 digests for running sessions. | [Snapshot behavior](reference/domain/21-procedure-and-item-specification.md#snapshot-behavior) |
| `COREX004` | Implement lifecycle transitions | Completed | Apply item mutations, completion, skip, retry, return, reopen, block, cancel, and reset without invalid states. | [State transitions](reference/domain/22-state-transitions.md), [Lifecycle](reference/domain/23-rework-and-lifecycle.md) |
| `COREX005` | Derive status and next actions | Completed | Compute coherent state summaries and actionable missing-item suggestions. | [Domain boundary](architecture.md#domain-boundary), [CLI status and next](reference/interfaces/30-cli-specification.md) |
| `COREX006` | Validate built-in presets | Completed | Parse the four shipped presets through the same public procedure rules. | [Product composition](project.md#product-composition), [Built-in presets](reference/domain/24-built-in-presets.md) |

## STORE — Persistence and Durable Queue

| id | title | status | goal | references |
|---|---|---|---|---|
| `STORE001` | Implement schema-v1 migration | Completed | Initialize or transactionally migrate schema-0 state to canonical schema-v1. | [SQLite model](reference/storage/40-sqlite-model.md) |
| `STORE002` | Persist domain state | Completed | Store workspaces, snapshots, sessions, stages, attempts, items, blockers, and artifacts coherently. | [SQLite model](reference/storage/40-sqlite-model.md), [State ownership](architecture.md#state-ownership-and-concurrency) |
| `STORE003` | Implement durable FIFO jobs | Completed | Admit idempotent mutations, claim work in order, and retain terminal results. | [Transactions](reference/storage/41-transactions-concurrency-and-idempotency.md) |
| `STORE004` | Enforce concurrency rules | Completed | Apply revisions, cursor preconditions, single-writer transactions, and exactly-once effects. | [State ownership](architecture.md#state-ownership-and-concurrency), [Transactions](reference/storage/41-transactions-concurrency-and-idempotency.md) |
| `STORE005` | Implement maintenance and recovery | Completed | Recover interrupted ownership, prune bounded data, check integrity, and reset safely. | [Recovery and maintenance](reference/storage/42-recovery-retention-and-maintenance.md) |

## GITFS — Git Worktree Boundary

| id | title | status | goal | references |
|---|---|---|---|---|
| `GITFS001` | Discover Git worktrees | Completed | Resolve main and linked non-bare worktrees without mutating Git. | [Worktree boundary](architecture.md#worktree-boundary), [Git model](reference/architecture/12-git-worktree-and-filesystem.md) |
| `GITFS002` | Resolve workspace identity | Completed | Bind workspace UUIDs to stable Git identity fingerprints. | [Git model](reference/architecture/12-git-worktree-and-filesystem.md) |
| `GITFS003` | Initialize local layout | Completed | Create configuration, runtime state, and ignore rules inside the owning worktree. | [Product composition](project.md#product-composition), [Git model](reference/architecture/12-git-worktree-and-filesystem.md) |
| `GITFS004` | Enforce path safety | Completed | Reject escapes, symlink traversal, unsafe procedure paths, and changed required artifacts. | [Worktree boundary](architecture.md#worktree-boundary), [Implementation tips](contributing.md#implementation-tips) |
| `GITFS005` | Handle worktree lifecycle | Completed | Repair moves, reject copied identities, and keep deletion behavior worktree-local. | [Git model](reference/architecture/12-git-worktree-and-filesystem.md) |

## DAEMN — Daemon and IPC Runtime

| id | title | status | goal | references |
|---|---|---|---|---|
| `DAEMN001` | Implement IPC v1 framing | Completed | Encode, decode, bound, and negotiate versioned local protocol frames. | [IPC and compatibility](architecture.md#ipc-and-compatibility), [IPC protocol](reference/interfaces/32-ipc-protocol.md) |
| `DAEMN002` | Implement local server lifecycle | Completed | Own the Unix socket and singleton lock, validate peers, and shut down gracefully. | [Request flow](architecture.md#request-flow), [Daemon and queue](reference/architecture/11-daemon-and-write-queue.md) |
| `DAEMN003` | Schedule per worktree | Completed | Maintain the workspace registry and serialize one worker per workspace identity. | [State ownership](architecture.md#state-ownership-and-concurrency), [Daemon and queue](reference/architecture/11-daemon-and-write-queue.md) |
| `DAEMN004` | Dispatch reads and mutations | Completed | Route public operations to durable admission, pure transitions, and coherent read views. | [Request flow](architecture.md#request-flow), [System architecture](reference/architecture/10-system-architecture.md) |
| `DAEMN005` | Implement waits and recovery | Completed | Support idle waits, job control, cancellation, restart recovery, and bounded blocking work. | [Daemon and queue](reference/architecture/11-daemon-and-write-queue.md), [Recovery](reference/storage/42-recovery-retention-and-maintenance.md) |
| `DAEMN006` | Add production observability | Completed | Provide bounded structured logs and exercise the real CLI-daemon-store vertical path. | [macOS service and observability](architecture.md#macos-service-and-observability), [Observability](reference/operations/51-observability.md) |

## CLINT — CLI and JSON Interface

| id | title | status | goal | references |
|---|---|---|---|---|
| `CLINT001` | Implement command grammar | Completed | Provide complete commands, global flags, offline help, preset inspection, and procedure validation. | [CLI specification](reference/interfaces/30-cli-specification.md) |
| `CLINT002` | Implement the daemon client | Completed | Connect with bounded timeouts and send idempotency keys, preconditions, waits, and detached requests. | [Request flow](architecture.md#request-flow), [IPC protocol](reference/interfaces/32-ipc-protocol.md) |
| `CLINT003` | Implement workflow operations | Completed | Expose workspace, session, stage, item, blocker, reset, and job behavior. | [Core lifecycle](project.md#core-lifecycle), [CLI specification](reference/interfaces/30-cli-specification.md) |
| `CLINT004` | Implement public output | Completed | Keep human text and versioned JSON consistent with stable errors and exit codes. | [JSON contract](reference/interfaces/31-json-contract.md), [Errors and exit codes](reference/interfaces/33-errors-and-exit-codes.md) |
| `CLINT005` | Complete shell UX coverage | Completed | Generate bash, zsh, and fish completion and verify public real-binary workflows. | [Tests](structure.md#tests), [CLI specification](reference/interfaces/30-cli-specification.md) |

## MACOS — Native macOS Service

| id | title | status | goal | references |
|---|---|---|---|---|
| `MACOS001` | Define service paths | Completed | Calculate per-user runtime paths and generate the LaunchAgent contract. | [macOS service](reference/architecture/13-macos-service.md) |
| `MACOS002` | Implement service lifecycle | Completed | Install, start, stop, restart, inspect, log, and uninstall the daemon safely. | [macOS service and observability](architecture.md#macos-service-and-observability), [macOS service](reference/architecture/13-macos-service.md) |
| `MACOS003` | Validate native service health | Completed | Reject incompatible binaries and verify the native Apple Silicon daemon lifecycle. | [Supported boundary](project.md#supported-boundary), [Release packaging](reference/operations/52-release-and-packaging.md) |
| `MACOS004` | Integrate service packaging | Completed | Ship matching binaries, LaunchAgent behavior, presets, schemas, and completions. | [Release artifacts](contributing.md#release-artifacts), [Release packaging](reference/operations/52-release-and-packaging.md) |

## DOGFD — Preset Dogfooding

| id | title | status | goal | references |
|---|---|---|---|---|
| `DOGFD001` | Exercise `sw-dev` | Completed | Complete a non-trivial feature procedure including a return to earlier work. | [Built-in presets](reference/domain/24-built-in-presets.md), [Core lifecycle](project.md#core-lifecycle) |
| `DOGFD002` | Exercise `bug-fix` | Completed | Complete reproduction, diagnosis, retry, return, repair, and regression coverage. | [Built-in presets](reference/domain/24-built-in-presets.md), [Lifecycle](reference/domain/23-rework-and-lifecycle.md) |
| `DOGFD003` | Exercise `docs-only` | Completed | Complete a documentation procedure with source and validation checks. | [Built-in presets](reference/domain/24-built-in-presets.md) |
| `DOGFD004` | Exercise `analysis` | Completed | Complete an investigation procedure ending in supported conclusions. | [Built-in presets](reference/domain/24-built-in-presets.md) |
| `DOGFD005` | Refine preset tooling and UX | Completed | Correct unclear procedure text and provide validated contributor authoring tools. | [Canonical assets and presets](contributing.md#canonical-assets-and-presets) |

## HARDN — Hardening and Release Readiness

| id | title | status | goal | references |
|---|---|---|---|---|
| `HARDN001` | Compose local test layers | Completed | Run preparation, unit, and integration checks through deterministic repository entry points. | [Verification layers](contributing.md#verification-layers) |
| `HARDN002` | Add destructive-path coverage | Completed | Exercise bounded fuzzing, concurrency, crash windows, recovery, and reset behavior. | [Quality reference](reference/quality/60-testing-and-conformance.md), [Tests](structure.md#tests) |
| `HARDN003` | Bind acceptance evidence | Completed | Map every product acceptance criterion and mandatory requirement to executable evidence. | [Product acceptance](reference/quality/61-product-acceptance.md), [Traceability](reference/quality/62-requirements-traceability.md) |
| `HARDN004` | Build deterministic distribution | Completed | Produce thin arm64 binaries, a reproducible archive, checksum, and provenance document. | [Release artifacts](contributing.md#release-artifacts), [Release packaging](reference/operations/52-release-and-packaging.md) |
| `HARDN005` | Align release policy | Completed | Make the local gate, platform limit, signing status, and distribution boundary explicit. | [Supported boundary](project.md#supported-boundary), [Release readiness](release-readiness.md) |
| `HARDN006` | Verify the complete release gate | Completed | Make repository-root `make test` the sole source-revision readiness decision. | [Verification layers](contributing.md#verification-layers), [Release readiness](release-readiness.md) |
