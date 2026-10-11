# Product Overview

## Goal

Podway is a local Procedure v2 guard for one task in one Git worktree. It keeps
the active graph node, required inputs, goal criteria, decisions, and declared
rework paths explicit without executing the work itself.

## Core lifecycle

```text
initialize a worktree
  -> start one cursor-free prepared Procedure v2 session
  -> begin it at the unique graph entry, optionally defining the initial goal
  -> define or revise the goal and criteria when the Procedure enables them
  -> satisfy the active action node
  -> complete, retry, decide, or follow declared rework
  -> reach a terminal node and close the goal
  -> record terminal ownership, then archive or start the next task
```

Podway answers which Procedure governs the task, which node is active, which
formal conditions remain unsatisfied, which transitions are valid, and which
declared rework path applies.

## Product composition

Podway ships one `podway` CLI. Its in-process Direct Engine reads one committed
snapshot or commits a fenced mutation and its validated success outcome in the
owning worktree. Native maintenance leases and SQLite serialize compatible
writers; there is no durable admission queue or FIFO guarantee. Authoritative
task state and maintenance locks live under `.podway/runtime/`. Podway creates
no daemon, socket, LaunchAgent, account state, or global workspace registry.

The public product accepts only `podway.procedure/v2`, emits successful results
through `podway.output/v4`, and ships exactly `analysis-v2`, `bug-fix-v2`,
`small-change-v2`, and `sw-dev-v2`.
Failures use `podway.error/v2`. Version identity and the contract manifest use
their v2 families, and every Procedure uses `podway.observation-result/v5`.
Workspace configuration and other result families retain their own versions;
Procedure support is identified by `podway.procedure/v2`.

## Principles

1. The current task and its formal next action come first.
2. Procedure definitions are bounded data, never executable code.
3. Retry, decisions, and declared rework are explicit graph transitions.
4. Successful mutations commit atomically with fenced, idempotent outcomes;
   rejected mutations have no outcome and are re-evaluated on retry.
5. Automation consumes versioned JSON and stable error codes.
6. Worktree-local state is not copied into a global task ledger.
7. Same-user local trust prevents accidents; it is not a security boundary.

## Non-goals

Podway is not a workflow server, project manager, CI system, command runner, Git
mutation layer, AI runtime, evidence archive, artifact store, remote collaboration
service, or multi-user access-control system. It performs no network I/O and
stores artifact metadata rather than artifact bytes.

## Supported boundary

The supported release target is native Apple Silicon macOS. Direct SQLite
generation 15 is the canonical Store. It declares no predecessor: daemon-era
generations 1–13 and the unreleased direct generation 14 are refused unchanged
with `LEGACY_RUNTIME_STATE_UNSUPPORTED`. Explicitly confirmed reset discards
their runtime history and creates a fresh workspace without conversion or backup.
Newer generations are refused even by destructive commands and require a
compatible writer. The [SQLite model](../storage/sqlite-model.md) owns writer
compatibility, and [release and packaging](../operations/release-and-packaging.md)
owns separately authorized consumer adoption and legacy-writer retirement.

Continue with [goals and non-goals](goals-and-non-goals.md), [terminology and
invariants](terminology-and-invariants.md), and [user workflows](user-workflows.md).
