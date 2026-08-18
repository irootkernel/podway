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
  -> reset the worktree for the next task
```

Podway answers which Procedure governs the task, which node is active, which
formal conditions remain unsatisfied, which transitions are valid, and which
declared rework path applies.

## Product composition

Podway consists of the `podway` CLI and the user-scoped `podwayd` daemon. The
daemon is the sole normal writer and executes at most one mutation per worktree;
independent worktrees may progress concurrently. Authoritative task state lives
under `.podway/runtime/` in the owning worktree.

The public product accepts only `podway.procedure/v2`, emits successful results
through `podway.output/v3`, and ships exactly `bug-fix-v2`, `small-change-v2`,
and `sw-dev-v2`.
Procedure-independent contracts such as `podway.ipc/v1`, `podway.error/v1`, the
workspace configuration, and the contract manifest retain their own versions.

## Principles

1. The current task and its formal next action come first.
2. Procedure definitions are bounded data, never executable code.
3. Retry, decisions, and declared rework are explicit graph transitions.
4. Mutations are durable, serialized, fenced, atomic, and idempotent.
5. Automation consumes versioned JSON and stable error codes.
6. Worktree-local state is not copied into a global task ledger.
7. Same-user local trust prevents accidents; it is not a security boundary.

## Non-goals

Podway is not a workflow server, project manager, CI system, command runner, Git
mutation layer, AI runtime, evidence archive, artifact store, remote collaboration
service, or multi-user access-control system. It performs no network I/O and
stores artifact metadata rather than artifact bytes.

## Supported boundary

The supported release target is native Apple Silicon macOS. SQLite schema-v5 is
the canonical store. Empty predecessors and v2-only schema-v3 or schema-v4
stores migrate transactionally; any predecessor containing Procedure v1 domain
state fails closed with `LEGACY_PROCEDURE_STATE_UNSUPPORTED` and is never
converted or deleted automatically.

Continue with [goals and non-goals](goals-and-non-goals.md), [terminology and
invariants](terminology-and-invariants.md), and [user workflows](user-workflows.md).
