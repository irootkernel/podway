# Terminology and Invariants

- **Workspace:** one valid non-bare Git worktree with a stable Podway identity.
- **Procedure:** one admitted immutable `podway.procedure/v2` graph snapshot.
- **Placement:** a graph node using an action, decision, or goal-assessment definition.
- **Attempt:** one execution of a placement; exactly one is active while running.
- **Recorded item:** a typed assertion attached to an action attempt.
- **Evidence reference:** a bounded reference to a prior valid terminal attempt.
- **Rework:** an explicit transition to a declared manual-rework target.
- **Goal revision:** an immutable version of the optional session goal and criteria.
- **Admission:** the durable acceptance of a mutation job before execution.

The implementation continuously enforces:

- one task session per workspace and one active attempt per running session;
- no active attempt for completed or cancelled sessions;
- an immutable procedure snapshot and declared graph movement only;
- completion only after required items, blocker, evidence, and goal gates pass;
- old-attempt values never satisfy a fresh attempt automatically;
- exactly one session revision increment per successful state-changing mutation;
- one executing mutation per worktree and ordered durable admission;
- atomic, idempotent, fail-closed writes under explicit identity fences;
- authoritative task state only under the owning worktree's `.podway/runtime/`;
- bounded input, frames, queues, collections, paths, logs, and timeouts;
- no Git mutation, network request, configured-command execution, or artifact bytes.

A `/v1` suffix on IPC, error, workspace, manifest, or first-version result families
versions that contract only. Procedure support is identified exclusively by
`podway.procedure/v2`.
