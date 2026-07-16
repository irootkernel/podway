# Terminology and Invariants

## Terminology

### Workspace

A **Workspace** is one valid, non-bare Git worktree initialized for Podway. Its root contains `.podway/`. Workspace identity combines Git worktree identity with a generated non-secret UUID stored in the local database.

### Procedure

A **Procedure** is versioned data describing an ordered list of stages, their instructions, required and optional items, skip policy, and allowed return destinations. A procedure contains no executable code.

### Procedure snapshot

A **Procedure Snapshot** is the immutable, canonicalized procedure stored when a task session begins. Editing the source YAML does not change the running session.

### Task session

A **Task Session** is the single current task managed in one workspace. Its lifecycle is `running`, `completed`, or `cancelled`.

### Stage

A **Stage** is one ordered unit of procedure progress. Stages are addressed by stable kebab-case IDs and zero-based internal indexes.

### Attempt

An **Attempt** is one execution of one stage. Retry and return create new attempts. Attempt lifecycle is `active`, `completed`, `skipped`, or `abandoned`.

### Item

An **Item** is a typed value associated with the active attempt. Supported types are `confirm`, `text`, `choice`, `integer`, `list`, and `artifact`.

### Artifact reference

An **Artifact Reference** contains only:

```text
location_type: path | reference
location
sha256_digest
size_bytes
media_type
```

No artifact bytes are stored.

### Blocker

A **Blocker** is an unresolved reason the active attempt cannot complete. Blockers belong only to the attempt on which they were created.

### Mutation job

A **Mutation Job** is a write request durably admitted by `podwayd` to a worktree-local FIFO queue.

### Session revision

The **Session Revision** is a monotonic integer incremented exactly once by every successful state-changing session mutation. Job-only maintenance does not change it.

### Item revision

The **Item Revision** is a monotonic integer scoped to one item value in one attempt. It prevents same-item lost updates without making unrelated item updates conflict.

### Workspace sequence

The **Workspace Sequence** is a monotonic integer assigned to admitted mutation jobs. It defines FIFO order within the worktree.

### Redo

`redo` is a derived stage status meaning that a stage was previously reached but must be performed again after return or reopen. It is an instruction for the current task, not an audit-validity concept.

## System-wide invariants

The implementation MUST continuously enforce the following invariants.

### Workspace invariants

- **INV-W01:** A workspace is a valid, non-bare Git worktree.
- **INV-W02:** `.podway/runtime/` resolves inside the worktree and is not a symlink escape.
- **INV-W03:** One live workspace UUID cannot be accepted at two live roots.
- **INV-W04:** The authoritative task database exists only inside the worktree.
- **INV-W05:** The daemon global registry contains no task title, procedure, stage, item, blocker, artifact, or attempt data.

### Session invariants

- **INV-S01:** A workspace has at most one task session.
- **INV-S02:** A running session has exactly one active attempt.
- **INV-S03:** A completed or cancelled session has no active attempt.
- **INV-S04:** A session follows exactly one immutable procedure snapshot.
- **INV-S05:** Stage order is linear and immutable for the session.
- **INV-S06:** The active attempt belongs to the stage marked `current`.
- **INV-S07:** A stage can complete only when every required item is satisfied and no blocker is open.
- **INV-S08:** Skip is allowed only when declared by the procedure.
- **INV-S09:** Old attempt items never satisfy a new attempt automatically.
- **INV-S10:** Every successful state-changing session mutation increments the session revision exactly once.

### Rework invariants

- **INV-R01:** Retry abandons the active attempt before creating the next attempt of the same stage.
- **INV-R02:** Return targets only an allowed earlier stage.
- **INV-R03:** Return abandons the active attempt before activating the destination.
- **INV-R04:** Return marks every reached stage after the destination as `redo`; never-reached stages remain `pending`.
- **INV-R05:** Return creates a fresh destination attempt with empty items and no blockers.
- **INV-R06:** Reopen follows the same redo semantics as return from a completed session.
- **INV-R07:** Cancelled sessions cannot be reopened.

### Queue and transaction invariants

- **INV-Q01:** Only `podwayd` opens a live workspace database in write mode during normal operation.
- **INV-Q02:** At most one mutation job executes per worktree.
- **INV-Q03:** Jobs execute in ascending workspace sequence, excluding jobs cancelled before claim.
- **INV-Q04:** Different worktrees may execute one mutation each concurrently.
- **INV-Q05:** A mutation is acknowledged as admitted only after its job row commits.
- **INV-Q06:** A failed or cancelled job changes no session state.
- **INV-Q07:** A successful state transition and its terminal job result commit atomically.
- **INV-Q08:** The same idempotency key and canonical request produce one logical mutation.
- **INV-Q09:** Reusing an idempotency key for a different request fails.
- **INV-Q10:** A stale attempt or item revision fails instead of overwriting current state.

### Interface and safety invariants

- **INV-I01:** Read commands do not mutate Podway or external state.
- **INV-I02:** No command executes arbitrary user code.
- **INV-I03:** No command performs Git mutation.
- **INV-I04:** No daemon component opens a network listener or performs network I/O.
- **INV-I05:** Every public JSON object declares its schema version.
- **INV-I06:** Unknown additive JSON fields do not break conforming v1 clients.
- **INV-I07:** Artifact bytes are never copied into Podway storage.
- **INV-I08:** Worktree deletion is sufficient to delete the task state.

## Derived stage-status rules

For a running session:

1. the stage owning the active attempt is `blocked` when it has open blockers, otherwise `current`;
2. a reached stage before the current stage is `done` or `skipped` according to its latest applicable attempt;
3. a reached stage after a return destination is `redo` until reactivated and completed or skipped again;
4. a stage never reached is `pending`;
5. cancellation changes the formerly current stage to `abandoned`.

For a completed session, all stages are `done` or `skipped`. For a cancelled session, the stage active at cancellation is `abandoned`, previously terminal stages remain visible, and no stage is `current`.

## Identity and time conventions

- Public IDs are opaque lowercase UUID strings.
- Database timestamps are signed 64-bit Unix epoch milliseconds in UTC.
- JSON timestamps are RFC 3339 UTC strings with millisecond precision.
- Procedure and request digests use lowercase SHA-256 with the `sha256:` prefix.
- Stage and item IDs use lowercase kebab-case and are stable within a procedure version.
