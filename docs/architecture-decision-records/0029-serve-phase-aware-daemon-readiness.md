# ADR-0029: Serve Phase-Aware Daemon Readiness

- Status: Accepted
- Date: 2026-08-25
- Extends: [ADR-0012](0012-explicit-daemon-endpoint-and-canonical-per-user-podway-home.md)
- Preserves: [ADR-0003](0003-daemon-single-writer.md)

## Context

The daemon currently opens its public socket only after registry loading,
registered-worktree recovery, and recovered-job draining. Service installation
waits a fixed 30 seconds for a verified response. A valid startup with several
registered worktrees has taken about 47 seconds, so installation can report
`DAEMON_UNAVAILABLE` even though launchd loaded the unchanged service and the
daemon later became healthy.

Launchd load state, socket reachability, contract verification, and readiness to
serve workspace commands are distinct facts. The v1 status result preserves the
first three only indirectly, and its `status: running` value can be mistaken for
full readiness.

Increasing one timeout would hide the current symptom without giving callers a
stable startup state or addressing startup work that grows with registered
worktrees.

## Decision

The daemon binds its authenticated endpoint and starts a bounded control plane
before registry and worktree recovery. During startup the control plane serves
only the contract handshake, `daemon.status`, and coordinated termination.
Every other route fails before workspace discovery, durable admission, or
dispatch with retryable `DAEMON_STARTING`.

One process-local readiness state machine owns these states:

- `starting`: endpoint and core process identity are available;
- `recovering`: registry, worktrees, or recovered jobs are being reconciled;
- `ready`: recovery is complete and normal dispatch may begin;
- `failed`: startup encountered a fatal error and normal dispatch remains
  closed.

The state machine exposes one current stage from `endpoint`, `registry`,
`workspaces`, `jobs`, `ready`, or `failed`, monotonic elapsed
milliseconds, and bounded registered-worktree progress. Individual unreadable or
gone worktrees retain their existing quarantined recovery behavior and do not
prevent other worktrees from recovering. A fatal registry or process-composition
failure transitions to `failed` and follows the existing exact endpoint cleanup
path; it never opens normal dispatch.

`podway.daemon-status-result/v2` preserves every v1 service, reachability, and
identity field while adding typed readiness fields. The v1 schema remains valid
for older peers. A v2 result is `ready` only after the live process identity and
contract manifest have been verified and the daemon reports its own ready state.
Loaded but unreachable, starting, recovering, and failed states are never rendered
as ready.

Automation receives a dedicated read-only `podway daemon wait-ready` operation.
It does not start, install, restart, repair, or mutate the service. Its default
deadline is 120 seconds and the existing one-hour wait ceiling remains the hard
maximum. `daemon install` reuses the same waiter after its idempotent service
reconciliation; `daemon start` retains its launchd transition behavior, and
callers use `daemon wait-ready` when verified readiness is required.

Deadline expiry returns retryable `DAEMON_READINESS_TIMEOUT`, not generic
unavailability. Its closed details include the lifecycle operation, last
readiness state and stage, elapsed and deadline milliseconds, installed, loaded,
socket-present, reachable, and contract-verified observations, worktree progress,
`same_command_retry_safe: true`, and a read-only `daemon wait-ready` recovery
recipe. A timeout preserves the LaunchAgent, authenticated service metadata,
socket ownership, registry, workspace databases, and active sessions.

## Consequences

Positive:

- startup cost no longer prevents a verified control response;
- callers can distinguish launchd state, reachability, contract verification, and
  readiness without parsing prose;
- one bounded waiter serves install recovery and explicit automation;
- retrying install after a timeout reconciles the same durable service state.

Negative:

- the daemon has a small control-only interval before normal dispatch;
- status gains a new result family and consumers must opt into the v2 readiness
  fields;
- startup state transitions and progress require concurrency-safe process-local
  coordination and deterministic test clocks.

The decision does not authorize background command execution, network access,
Git mutation, workspace database rewriting, or a second daemon writer.
