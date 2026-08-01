# Roadmap

This roadmap records both the completed implementation baseline and the remaining
automation-readiness work required before Podway v0.1.0 can be released.

## Sequence policy

Epics run from top to bottom. Tasks within an epic run in numeric order. A task may
start only when every earlier task in its epic is `Completed`, and an epic may start
only when every task in every preceding epic is `Completed`. Valid states are
`Planned`, `In Progress`, `Blocked`, and `Completed`. The first ten epics preserve
the completed historical baseline. `AUTOM` records the accepted planning change;
`RPATH` through `DOLGI` record the completed automation-readiness implementation,
and the remaining `REL10` tasks are release-blocking.

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
| `FOUND004` | Lock executable contracts | Completed | Record canonical imports, command routes, crate adjacency, contract manifests, and validation sentinels. | [Workspace map](structure.md#workspace-map), [Tests](structure.md#tests) |
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

## AUTOM — Automation Contract Baseline

| id | title | status | goal | references |
|---|---|---|---|---|
| `AUTOM001` | Audit the existing automation boundary | Completed | Identify implementation and documentation gaps affecting generic local automation clients. | [Contract status](reference/interfaces/34-automation-client-contract.md#1-status-and-target-release) |
| `AUTOM002` | Record the runtime endpoint decision | Completed | Accept the canonical Podway home, explicit socket, PATH invocation, and absolute daemon execution. | [ADR-0012](adr/0012-explicit-daemon-endpoint-and-canonical-per-user-podway-home.md) |
| `AUTOM003` | Define the automation client contract | Completed | Establish stable normative requirements without claiming that planned behavior exists. | [Automation client contract](reference/interfaces/34-automation-client-contract.md) |
| `AUTOM004` | Establish acceptance traceability | Completed | Map every automation requirement to implementation tasks and planned executable evidence. | [Traceability](reference/interfaces/34-automation-client-contract.md#25-requirements-to-roadmap-traceability) |

## RPATH — Runtime Paths and Explicit Socket

| id | title | status | goal | references |
|---|---|---|---|---|
| `RPATH001` | Implement the PodwayHome abstraction | Completed | Resolve the effective user's canonical Podway home without ambient environment variables. | [AUT-HOME-001](reference/interfaces/34-automation-client-contract.md#8-podway-user-global-home-and-layout-aut-home-001003) |
| `RPATH002` | Implement the user-global file layout | Completed | Move service-global paths under the canonical per-user root while preserving worktree-local state. | [AUT-HOME-002–004](reference/interfaces/34-automation-client-contract.md#8-podway-user-global-home-and-layout-aut-home-001003), [Worktree boundary](reference/interfaces/34-automation-client-contract.md#9-worktree-local-state-boundary-aut-home-004) |
| `RPATH003` | Add the global explicit socket option | Completed | Support an absolute endpoint for all daemon-backed commands with strict no-fallback behavior. | [AUT-SOCK-001–004](reference/interfaces/34-automation-client-contract.md#10-explicit-socket-resolution-aut-sock-001004) |
| `RPATH004` | Persist the installed endpoint | Completed | Record the canonical socket and actual absolute daemon path in service metadata and the LaunchAgent. | [AUT-DAEMON-001–003](reference/interfaces/34-automation-client-contract.md#7-daemon-discovery-and-launchagent-execution-aut-daemon-001003), [AUT-HOME-003](reference/interfaces/34-automation-client-contract.md#8-podway-user-global-home-and-layout-aut-home-001003) |
| `RPATH005` | Preserve singleton and socket safety | Completed | Enforce one daemon per effective user, private permissions, peer validation, and safe stale-socket recovery. | [AUT-SEC-001–004](reference/interfaces/34-automation-client-contract.md#11-socket-and-directory-security-aut-sec-001004), [AUT-SOCK-005](reference/interfaces/34-automation-client-contract.md#12-one-daemon-per-user-invariant-aut-sock-005) |
| `RPATH006` | Verify PATH and sanitized-environment operation | Completed | Prove the runtime-path slice of CLI and daemon-backed operation from arbitrary directories without HOME or TMPDIR; leave the installed-service integration scenario to DOLGI001. | [AUT-PATH-001–003](reference/interfaces/34-automation-client-contract.md#6-path-based-cli-invocation-aut-path-001003), [AUT-T-PATH](reference/interfaces/34-automation-client-contract.md#24-acceptance-matrix) |

## CONID — CLI, Daemon, and Contract Identity

| id | title | status | goal | references |
|---|---|---|---|---|
| `CONID001` | Define the contract manifest artifact | Completed | Add the manifest format and deterministic digest over integration-critical contracts. | [AUT-CONTRACT-001](reference/interfaces/34-automation-client-contract.md#13-cli-and-daemon-contract-identity-aut-contract-001005) |
| `CONID002` | Add machine-readable version identity | Completed | Expose build, target, protocol, and contract identity through JSON version output. | [AUT-CONTRACT-002](reference/interfaces/34-automation-client-contract.md#13-cli-and-daemon-contract-identity-aut-contract-001005) |
| `CONID003` | Embed matching identity in both binaries | Completed | Embed the same contract manifest digest in `podway` and `podwayd`. | [AUT-CONTRACT-003](reference/interfaces/34-automation-client-contract.md#13-cli-and-daemon-contract-identity-aut-contract-001005) |
| `CONID004` | Enforce the daemon contract handshake | Completed | Reject product or manifest mismatch before command execution or durable admission. | [AUT-CONTRACT-004](reference/interfaces/34-automation-client-contract.md#13-cli-and-daemon-contract-identity-aut-contract-001005), [AUT-ERR-002](reference/interfaces/34-automation-client-contract.md#22-error-and-exit-code-requirements-aut-err-001002) |
| `CONID005` | Expose daemon process identity | Completed | Report daemon version, executable, process instance, endpoint, and manifest digest. | [AUT-CONTRACT-005](reference/interfaces/34-automation-client-contract.md#13-cli-and-daemon-contract-identity-aut-contract-001005) |
| `CONID006` | Test mixed-installation failures | Completed | Cover mismatched manifests, stale daemons, replaced executables, and upgrade refresh. | [AUT-T-CONTRACT](reference/interfaces/34-automation-client-contract.md#24-acceptance-matrix) |

## CASID — Workspace and Session Identity Fences

| id | title | status | goal | references |
|---|---|---|---|---|
| `CASID001` | Add public identity precondition flags | Completed | Add workspace and session identity flags and define command applicability. | [AUT-ID-001–006](reference/interfaces/34-automation-client-contract.md#14-workspace-and-session-identity-preconditions-aut-id-001007) |
| `CASID002` | Propagate identity through IPC | Completed | Carry explicit identities in requests and canonical request identity. | [AUT-ID-001–006](reference/interfaces/34-automation-client-contract.md#14-workspace-and-session-identity-preconditions-aut-id-001007) |
| `CASID003` | Enforce identity before transitions | Completed | Reject reads or mutations targeting a replaced workspace or session. | [AUT-ID-002–006](reference/interfaces/34-automation-client-contract.md#14-workspace-and-session-identity-preconditions-aut-id-001007) |
| `CASID004` | Define closed identity-conflict errors | Completed | Add typed codes, closed details, catalog entries, and exit behavior. | [AUT-ID-007](reference/interfaces/34-automation-client-contract.md#14-workspace-and-session-identity-preconditions-aut-id-001007), [AUT-ERR-001–002](reference/interfaces/34-automation-client-contract.md#22-error-and-exit-code-requirements-aut-err-001002) |
| `CASID005` | Test stale-identity races | Completed | Cover workspace reset, session replacement, matching revision on another session, reopen, and item mutations. | [AUT-T-ID](reference/interfaces/34-automation-client-contract.md#24-acceptance-matrix) |

## PSTRT — Procedure Start Integrity

| id | title | status | goal | references |
|---|---|---|---|---|
| `PSTRT001` | Add the expected Procedure digest option | Completed | Reject a mismatching canonical Procedure digest without creating a session. | [AUT-START-001](reference/interfaces/34-automation-client-contract.md#15-procedure-start-integrity-aut-start-001004) |
| `PSTRT002` | Make the admitted Procedure immutable | Completed | Persist the canonical snapshot before reporting durable admission. | [AUT-START-002–003](reference/interfaces/34-automation-client-contract.md#15-procedure-start-integrity-aut-start-001004) |
| `PSTRT003` | Bind start idempotency to the Procedure | Completed | Include the canonical digest and relevant start preconditions in request identity. | [AUT-START-004](reference/interfaces/34-automation-client-contract.md#15-procedure-start-integrity-aut-start-001004) |
| `PSTRT004` | Return the admitted Procedure identity | Completed | Expose the exact digest in start and later session observations. | [AUT-START-004](reference/interfaces/34-automation-client-contract.md#15-procedure-start-integrity-aut-start-001004) |
| `PSTRT005` | Test source-file race conditions | Completed | Cover source drift, symlink changes, daemon delay, restart, and idempotent retries. | [AUT-T-START](reference/interfaces/34-automation-client-contract.md#24-acceptance-matrix) |

## RECON — Durable Mutation Reconciliation

| id | title | status | goal | references |
|---|---|---|---|---|
| `RECON001` | Add job lookup by idempotency key | Completed | Implement the read-only worktree-scoped lookup query. | [AUT-RECON-001–002](reference/interfaces/34-automation-client-contract.md#18-job-lookup-by-idempotency-key-aut-recon-001004) |
| `RECON002` | Expose durable admission metadata | Completed | Make admitted and non-admitted outcomes distinguishable and preserve job identity on timeout. | [AUT-ADMIT-001–002](reference/interfaces/34-automation-client-contract.md#16-durable-mutation-admission-aut-admit-001002) |
| `RECON003` | Preserve receipt-only lookup | Completed | Reproduce the complete immutable original terminal output/error envelope after terminal job pruning and restart. | [AUT-RECON-003](reference/interfaces/34-automation-client-contract.md#18-job-lookup-by-idempotency-key-aut-recon-001004) |
| `RECON004` | Define unknown-outcome handling | Completed | Encode timeout and disconnect states without treating client termination as cancellation. | [AUT-ADMIT-003](reference/interfaces/34-automation-client-contract.md#17-timeout-disconnect-and-unknown-outcome-aut-admit-003), [AUT-RECON-004](reference/interfaces/34-automation-client-contract.md#18-job-lookup-by-idempotency-key-aut-recon-001004) |
| `RECON005` | Test response-loss recovery | Completed | Cover response loss before and after mutation dispatch, wait timeout, every job state, pre-init missing lookup, reset-marker recovery, pruning, and idempotent reconciliation. | [AUT-T-RECON](reference/interfaces/34-automation-client-contract.md#24-acceptance-matrix) |

## MCONT — Closed Machine Contracts

| id | title | status | goal | references |
|---|---|---|---|---|
| `MCONT001` | Add command-specific result schemas | Completed | Replace integration-critical generic result objects with versioned closed schemas. | [AUT-JSON-001](reference/interfaces/34-automation-client-contract.md#21-command-specific-json-schemas-aut-json-001004) |
| `MCONT002` | Add error-detail schemas | Completed | Define closed details for endpoint, identity, digest, conflict, idempotency, and timeout errors. | [AUT-JSON-002](reference/interfaces/34-automation-client-contract.md#21-command-specific-json-schemas-aut-json-001004), [AUT-ERR-001](reference/interfaces/34-automation-client-contract.md#22-error-and-exit-code-requirements-aut-err-001002) |
| `MCONT003` | Add result and details discriminators | Completed | Make every automation result and error detail shape unambiguous. | [AUT-JSON-003–004](reference/interfaces/34-automation-client-contract.md#21-command-specific-json-schemas-aut-json-001004) |
| `MCONT004` | Add the compact status contract | Completed | Implement a closed, bounded, quiescent state view for automation decisions. | [AUT-OBS-001](reference/interfaces/34-automation-client-contract.md#19-quiescent-observation-aut-obs-001), [AUT-OBS-002–004](reference/interfaces/34-automation-client-contract.md#20-compact-status-contract-aut-obs-002004) |
| `MCONT005` | Synchronize catalogs and contract assets | Completed | Update schemas, catalogs, transition references, generated mirrors, manifest, and packaging. | [AUT-CONTRACT-001](reference/interfaces/34-automation-client-contract.md#13-cli-and-daemon-contract-identity-aut-contract-001005), [AUT-JSON-001–003](reference/interfaces/34-automation-client-contract.md#21-command-specific-json-schemas-aut-json-001004) |
| `MCONT006` | Add contract fixtures and drift tests | Completed | Fail the release gate on field, type, canonicalization, size, or manifest drift. | [AUT-T-JSON](reference/interfaces/34-automation-client-contract.md#24-acceptance-matrix), [AUT-OBS-004](reference/interfaces/34-automation-client-contract.md#20-compact-status-contract-aut-obs-002004) |

## DOLGI — Dolgorae Integration Conformance

| id | title | status | goal | references |
|---|---|---|---|---|
| `DOLGI001` | Build the controlled-PATH integration harness | Completed | Complete AUT-T-PATH by installing through a controlled PATH from a sanitized arbitrary directory and verifying sibling, explicit, and PATH daemon resolution plus the canonical absolute plist path. | [AUT-PATH-001–003](reference/interfaces/34-automation-client-contract.md#6-path-based-cli-invocation-aut-path-001003), [AUT-DAEMON-001–003](reference/interfaces/34-automation-client-contract.md#7-daemon-discovery-and-launchagent-execution-aut-daemon-001003), [AUT-T-PATH](reference/interfaces/34-automation-client-contract.md#24-acceptance-matrix) |
| `DOLGI002` | Verify service and quiescent observation | Completed | Install the daemon, connect through an explicit socket, initialize a worktree, and obtain compact idle status. | [AUT-SOCK-001–004](reference/interfaces/34-automation-client-contract.md#10-explicit-socket-resolution-aut-sock-001004), [AUT-SOCK-005](reference/interfaces/34-automation-client-contract.md#12-one-daemon-per-user-invariant-aut-sock-005), [AUT-OBS-001](reference/interfaces/34-automation-client-contract.md#19-quiescent-observation-aut-obs-001), [AUT-OBS-002–004](reference/interfaces/34-automation-client-contract.md#20-compact-status-contract-aut-obs-002004) |
| `DOLGI003` | Verify session and item operations | Completed | Exercise the full lifecycle with explicit identity fences. | [AUT-ID-001–007](reference/interfaces/34-automation-client-contract.md#14-workspace-and-session-identity-preconditions-aut-id-001007), [AUT-START-001–004](reference/interfaces/34-automation-client-contract.md#15-procedure-start-integrity-aut-start-001004) |
| `DOLGI004` | Verify conflict and reconciliation paths | Completed | Exercise stale identity, digest and daemon mismatch, timeout, response loss, and lookup. | [AUT-T-ID](reference/interfaces/34-automation-client-contract.md#24-acceptance-matrix), [AUT-T-RECON](reference/interfaces/34-automation-client-contract.md#24-acceptance-matrix) |
| `DOLGI005` | Verify the packaged test-fixture archive | Completed | Run the complete Dolgorae consumer conformance suite using a native arm64 archive built from debug binaries, require explicit test-fixture provenance, and fail closed unless both packaged binaries expose debug-only isolation. | [AUT-REL-001–003](reference/interfaces/34-automation-client-contract.md#23-release-artifact-and-installation-aut-rel-001004), [AUT-T-DIST](reference/interfaces/34-automation-client-contract.md#24-acceptance-matrix) |

`DOLGI005` supersedes the earlier unsafe requirement to exercise release-profile
binaries through debug-only account and launchctl overrides. `REL10003` repeats
packaged conformance through the release binaries' isolated foreground dev mode;
the test-fixture result alone is not release-profile qualification.

## REL10 — Podway v0.1.0 Release

| id | title | status | goal | references |
|---|---|---|---|---|
| `REL10001` | Freeze the v0.1.0 contract set | Completed | Freeze schemas, specs, canonicalization fixtures, toolchain, source revision, and manifest. | [AUT-CONTRACT-001](reference/interfaces/34-automation-client-contract.md#13-cli-and-daemon-contract-identity-aut-contract-001005), [AUT-REL-002–003](reference/interfaces/34-automation-client-contract.md#23-release-artifact-and-installation-aut-rel-001004) |
| `REL10002` | Pass the complete local release gate | Completed | Run the authoritative clean-tree `make test` with every new conformance test included. | [AUT-REL-004](reference/interfaces/34-automation-client-contract.md#23-release-artifact-and-installation-aut-rel-001004) |
| `REL10003` | Build and qualify the deterministic distribution | Completed | Build the native arm64 release archive, checksum, and provenance, then run packaged conformance through the isolated foreground dev daemon mode. | [AUT-REL-001–003](reference/interfaces/34-automation-client-contract.md#23-release-artifact-and-installation-aut-rel-001004) |
| `REL10004` | Produce the Dolgorae compatibility handoff | Planned | Publish the binary, contract, source, tree, and toolchain identities required for pinning. | [AUT-REL-002–004](reference/interfaces/34-automation-client-contract.md#23-release-artifact-and-installation-aut-rel-001004) |
| `REL10005` | Tag and release Podway v0.1.0 | Planned | Create the immutable release only after every preceding task is complete. | [AUT-REL-004](reference/interfaces/34-automation-client-contract.md#23-release-artifact-and-installation-aut-rel-001004) |
