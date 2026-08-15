# User Workflows

Initialize one Git worktree with `podway init`, inspect `podway preset list`, and
start `sw-dev-v2`, `bug-fix-v2`, `small-change-v2`, or a validated worktree-local
Procedure v2 file. Use `small-change-v2` for a bounded change that still requires
explicit inspection, verification, review, and closeout but no tracked goal. Use
`--dry-run` before a destructive replacement or other supported preview.

During a task, read `podway observe`, perform the actual work
outside Podway, then record only supported evidence on the active attempt. Use typed
item commands and `complete` to follow an action's declared edge. Use `decide` on a
decision node and the goal commands for goal-tracked sessions.

If the current attempt must be repeated, use `retry --reason`. If prior work must be
revisited, use `rework --to <node> --reason` only for a declared manual-rework
target. Podway creates a fresh attempt and applies the procedure's evidence
invalidation rules. Blockers prevent advancement until explicitly resolved.

Automation uses `--json`, carries the latest workspace/session/attempt/item/goal
preconditions, and assigns an idempotency key to every mutation. On response loss it
uses `job lookup --idempotency-key` before retrying. A detached mutation is followed
through `job status` or `job wait`.

Actors may hand off within a worktree by reading one recorded v2 observation
result; chat history is not authority. Parallel tasks belong in separate Git
worktrees and therefore separate Podway workspaces. Podway does not transfer a
session between worktrees.

Cancel a task that will not continue. Reset a completed or cancelled session when
its current-task history is no longer needed. If opening old runtime state returns
`LEGACY_PROCEDURE_STATE_UNSUPPORTED`, back up `.podway/runtime/` first and use
`podway reset --all` only with explicit authorization.
