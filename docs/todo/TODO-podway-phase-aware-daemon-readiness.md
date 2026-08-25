# Podway Phase-Aware Daemon Readiness

## Status and authority

- Document state: `Adopted`
- Owning roadmap epic: `V2RDY`
- Target product release: v0.2.6
- Repository scope: Podway only
- Accepted authority: [ADR-0029](../architecture-decision-records/0029-serve-phase-aware-daemon-readiness.md)

This dossier is the decision-complete implementation plan for the unfinished
`V2RDY` epic. The active roadmap owns task order and status. Accepted ADRs,
canonical machine assets, and current specifications remain higher authority.

## 1. Verified context

The production runtime currently acquires the endpoint and then synchronously
loads the registry, opens registered worktrees, recovers their scheduler state,
and drains recovered queues before constructing the accept loop. The CLI install
path polls a v1 daemon probe for 30 seconds, while start reports only the launchd
transition. A valid observed startup took about 47 seconds.

The existing server already handles `daemon.status` before workspace dispatch.
The architectural repair is therefore to start that bounded control surface
earlier and make recovery state explicit, not to raise a blind timeout.

## 2. Goals, non-goals, and scope

Goals:

- make the authenticated control endpoint reachable before registered-worktree
  recovery;
- expose stable starting, recovering, ready, and failed process states;
- reject normal routes before readiness without touching workspace state;
- add one bounded read-only readiness waiter used by install and automation;
- preserve deterministic install retry and all durable service/workspace state;
- distinguish slow startup, contract mismatch, launchctl failure, and genuine
  endpoint failure.

Non-goals:

- parallel active Procedure nodes or multiple writers;
- executing configured commands, Git operations, or network requests;
- changing launchd ownership or the authenticated endpoint contract;
- using the installed production daemon as a test fixture;
- declaring distribution readiness without the separate release gate.

## 3. Accepted interfaces

### 3.1 Readiness state

`podway.daemon-status-result/v2` keeps every v1 field and adds:

- `readiness_state`: `not_running`, `unreachable`, `starting`,
  `recovering`, `ready`, or `failed`;
- `readiness_stage`: `endpoint`, `registry`, `workspaces`, `jobs`,
  `ready`, `failed`, or `null`;
- `readiness_elapsed_ms`: a nonnegative integer or `null`;
- `worktree_recovery`: closed total, completed, and failed counts or `null`.

The direct daemon probe carries live identity and a non-null readiness stage.
The merged service form carries launchd fields even when no verified daemon can
answer. `ready` requires reachable live identity, exact contract verification,
and the daemon-owned ready state.

### 3.2 Startup admission

The early accept loop permits only handshake, `daemon.status`, and coordinated
termination. Other requests receive retryable exit-3 `DAEMON_STARTING` with
`admission: {\"admitted\": false}`, state, stage, elapsed time, progress, and a
read-only wait-ready recipe. The rejection occurs before workspace parsing,
registry lookup, queue admission, or durable mutation.

### 3.3 Readiness waiting

`podway daemon wait-ready [--timeout <duration>]` is read-only. The default is
120 seconds and the maximum is 3,600,000 milliseconds. It succeeds only with a
v2 ready result whose live identity and manifest match the current CLI.

`daemon install` uses the same waiter after service reconciliation.
`daemon start` remains a launchd transition; automation composes start and
wait-ready explicitly.

Timeout produces `DAEMON_READINESS_TIMEOUT` with:

- operation `install`, `start`, or `wait_ready`;
- last state, nullable stage, elapsed and deadline milliseconds;
- installed, loaded, socket-present, reachable, and contract-verified booleans;
- nullable worktree progress;
- `same_command_retry_safe: true`;
- the exact read-only `podway --json daemon wait-ready` recovery recipe.

## 4. Failure handling and compatibility

- v1 daemon status remains a valid older-peer result; v2 does not change or open
  the v1 schema.
- Loaded but unreachable is non-ready and human output must say startup is
  incomplete.
- Contract mismatch remains `DAEMON_CONTRACT_MISMATCH` and never becomes a
  timeout.
- Launchctl and lifecycle-lock failures keep their current service-lifecycle
  error semantics.
- A slow but responsive daemon reports starting or recovering, not unavailable.
- A timeout performs no uninstall, reload, socket deletion, registry rewrite, or
  workspace mutation.
- Partial worktree recovery follows existing quarantine rules and can still
  converge to ready. Fatal registry initialization never opens normal dispatch.

## 5. Roadmap ownership

- `V2RDY-001` adopts and reserves the contract without runtime admission.
- `V2RDY-002` starts the control plane early and gates normal dispatch on the
  readiness state machine.
- `V2RDY-003` implements the waiter, install reuse, status rendering, timeout
  diagnostics, compatibility proof, and epic closeout.

Tasks execute in order. The dossier remains until `V2RDY-003` promotes all
lasting behavior to durable specifications and closes the epic.

## 6. Verification and release acceptance

Contract reservation proves closed schemas, catalog/route parity, output-envelope
binding, recovery-recipe bounds, manifest pins, and runtime non-admission.

Runtime verification uses injected barriers and clocks. It covers more than 30
simulated seconds without sleeping, multiple registered worktrees, slow open and
job recovery, control-only admission, partial and fatal recovery, identity and
contract mismatch, loaded-but-unreachable status, timeout diagnostics, and
idempotent install retry with unchanged workspace/session state.

Each task passes focused checks and `make test`. The exact v0.2.6 release
candidate still requires the separately authorized complete distribution gate.

## 7. References

- [ADR-0029](../architecture-decision-records/0029-serve-phase-aware-daemon-readiness.md)
- [IPC protocol](../specs/interfaces/ipc-protocol.md)
- [CLI specification](../specs/interfaces/cli-specification.md)
- [Observability](../specs/operations/observability.md)
- [Errors and exit codes](../specs/interfaces/errors-and-exit-codes.md)
