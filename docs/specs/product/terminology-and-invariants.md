# Terminology and Invariants

- **Workspace:** one valid non-bare Git worktree with a stable Podway identity.
- **Procedure:** one admitted immutable `podway.procedure/v2` graph snapshot.
- **Placement:** a graph node using an action, decision, or goal-assessment definition.
- **Attempt:** one execution of a placement; exactly one is active while running.
- **Recorded item:** a typed assertion attached to an action attempt.
- **Evidence reference:** a bounded reference to a prior valid terminal attempt.
- **Rework:** an explicit transition to a declared manual-rework target.
- **Goal revision:** an immutable version of the optional session goal and criteria.
- **Writer admission:** acquisition of the native maintenance lease and SQLite
  write lock, followed by current identity and fence validation; no job is stored.
- **Success outcome:** the validated original response committed with its effect,
  canonical request digest, idempotency key, scope and workspace sequence.

The implementation continuously enforces:

- one task session per workspace and one active attempt per running session;
- no active attempt for completed or cancelled sessions;
- an immutable procedure snapshot and declared graph movement only;
- completion only after required items, blocker, evidence, and goal gates pass;
- old-attempt values never satisfy a fresh attempt automatically;
- exactly one session revision increment per successful state-changing mutation;
- serialized compatible mutations per worktree, without promised FIFO order;
- atomic, idempotent, fail-closed writes under explicit identity fences;
- authoritative task state only under the owning worktree's `.podway/runtime/`;
- bounded input, output, collections, paths, journal and admission waits;
- no Git mutation, network request, configured-command execution, or artifact bytes.

Public result-family versions are independent of Procedure support. Current
Output v4, Error v2, manifest v2 and observation v5 describe the direct product;
IPC and daemon/job contracts are retired. Only successful mutations retain
outcomes. Rejected fences, unsupported identities and illegal transitions change
neither state nor outcome records. One absolute invocation deadline bounds waits,
permits an immediate attempt even at zero remaining budget, and does not abort
work after the required locks are held.
