# ADR-0004: Store Task State Inside the Worktree

- Status: Accepted
- Date: 2026-07-13

## Context

A global state directory would survive worktree deletion and support history, but it would weaken the intended ownership model and create cleanup, mapping, and privacy complexity.

## Decision

All task, queue, snapshot, attempt, item, blocker, and receipt state is stored in `.podway/runtime/state.sqlite3` inside the Git worktree. Deleting the worktree deletes the task state.

The daemon may keep only a minimal UUID, last path, and last-seen registry for queued-job recovery.

## Consequences

Positive:

- intuitive ownership and deletion;
- no global task database;
- worktree isolation;
- simple reset and cleanup.

Negative:

- deleting a worktree loses acknowledged queued jobs and current task state;
- moved worktrees need registry repair or rediscovery;
- runtime files must be safely ignored by Git.
