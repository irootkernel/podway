# ADR-0003: Make the Daemon the Sole Normal Writer

- Status: Accepted
- Date: 2026-07-13

## Context

Humans, scripts, and AI agents may issue Podway commands concurrently. Direct CLI writes would require complex file locking and still make durable admission, retries, and service-level observation inconsistent.

## Decision

`podwayd` is the sole normal writer of every live worktree database. The CLI submits mutations as durable jobs. The daemon maintains one FIFO queue per worktree and processes independent worktrees concurrently.

Read queries also go through the daemon so responses can include pending queue state and consistent protocol behavior.

## Consequences

Positive:

- deterministic write ordering;
- durable admission and restart recovery;
- centralized path, schema, and precondition validation;
- one place for idempotency and diagnostics.

Negative:

- daemon installation and lifecycle are required;
- IPC and service integration increase implementation scope;
- daemon unavailability temporarily blocks workspace operation.
