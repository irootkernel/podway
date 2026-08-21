# ADR-0026: Retain Inactive Terminal Sessions

- Status: Accepted
- Date: 2026-08-21
- Supersedes in part: [ADR-0001](0001-current-task-session-focus.md)
- Extends: [ADR-0008](0008-relational-state-not-event-sourcing.md)
- Extends: [ADR-0021](0021-separate-session-preparation-from-execution.md)
- Superseded in part by: [ADR-0027](0027-state-aware-session-start.md)

## Context

The single-current-session rule prevents two tasks from progressing in one
worktree, but eligible reset and replacement erase a completed or cancelled
session after its ownership disposition is recorded. That removes the most
trustworthy terminal state when the next task starts.

Podway still is not an evidence archive or project manager. Retention therefore
needs a small, bounded relational history that never becomes active again.

## Decision

Each worktree has at most one current session and at most 32 inactive sessions.
Archiving atomically changes a completed or cancelled current session with a
disposition for its exact terminal revision into immutable inactive state. Its
Procedure snapshot, trace, attempts, item values, references, decisions, rework,
goal history, terminal disposition, and task metadata remain readable.

`podway archive` performs that transition explicitly. Plain `podway start` and
eligible `start --replace-eligible` archive an eligible terminal session before
creating the next prepared session; eligible replacement of a prepared session
still deletes it. Running sessions and terminal sessions without a current
disposition still require force replacement and remain destructive.

Inactive sessions have list and read-only show operations. They cannot be
restored, reactivated, or mutated. `archive purge` permanently deletes exactly
one inactive session under its session identity and revision fence plus explicit
confirmation. When all 32 slots are occupied, archive and terminal eligible
replacement fail without eviction or partial mutation.

`reset` remains the deliberate destructive operation. It never archives
implicitly. SQLite schema v7 separates the current-session pointer from retained
session rows while preserving sole-writer, atomicity, and one-current-session
constraints.

## Rejected alternatives

- Automatic oldest-session eviction hides irreversible data loss.
- Treating every finished session as current blocks the next task unnecessarily.
- Restoring an inactive session expands Podway into historical task management.
- Exporting an event stream or artifact bytes crosses the relational current-state
  and metadata-only boundaries.

## Consequences

- Normal task succession preserves disposed terminal state for later reference.
- Retention is bounded and explicit deletion remains available.
- Callers distinguish current from inactive sessions and select inactive sessions
  by ID for reads or purge.
- The schema, public commands, results, documentation, and migration form one
  coordinated compatibility change.
