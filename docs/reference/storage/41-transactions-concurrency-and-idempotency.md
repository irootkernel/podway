# Transactions, Concurrency, and Idempotency

## Concurrency model

Podway has three independent ordering concepts:

1. **Workspace sequence:** total order of admitted mutation jobs in one worktree.
2. **Session revision:** total order of successful session state changes.
3. **Item revision:** conflict boundary for one item in one attempt.

This allows multiple callers to update different active-stage items without silent same-item overwrites, while cursor-changing commands remain strict.

## Admission transaction

Pseudocode:

```text
BEGIN IMMEDIATE
  load workspace_state
  lookup idempotency_key

  if key exists:
    if request_digest differs: error IDEMPOTENCY_KEY_REUSED
    else: return existing job

  reject if queued_count >= max_pending
  sequence = next_workspace_sequence + 1
  update workspace_state.next_workspace_sequence
  insert job(state=queued, sequence, canonical_request)
  insert idempotency_record(key, digest, job_id)
COMMIT
```

The daemon acknowledges admission only after commit.

## Claim transaction

Under the workspace scheduler lock:

```text
BEGIN IMMEDIATE
  select lowest sequence queued job
  if none: COMMIT and sleep
  update selected job to running with claimed_at
COMMIT
```

The scheduler does not claim a later job while a lower executable job is queued.

## Preparation outside the state transaction

Some commands require read-only work that may be slower than a transaction:

- hashing an attached local artifact;
- rehashing required local artifacts before completion;
- canonicalizing and validating a procedure before start.

For the accepted v0.1.0 start target, canonical Procedure bytes and their digest
become part of the durable admitted input before acknowledgement. The job never
depends on re-reading its source file after admission. The normalized Procedure
snapshot is committed in the same SQLite transaction as the queued job and its
idempotency binding, so a pre-commit failure rolls back all three rows and a
post-commit retry observes the exact admitted snapshot.

The worker:

1. reads the command and current identifiers;
2. performs preparation outside an open SQLite transaction;
3. begins the state transaction;
4. revalidates every relevant precondition and item revision;
5. commits only if the prepared result still applies.

This avoids long database locks without weakening correctness.

## Successful execution transaction

```text
BEGIN IMMEDIATE
  load idempotency record and running job
  load workspace and session state
  validate workspace identity
  validate session, attempt, and item preconditions
  validate prepared artifact or procedure data
  invoke pure transition
  persist all changed rows
  increment session revision once if changed
  write terminal public success response
  update idempotency terminal response
  mark job succeeded
  append bounded journal summary
COMMIT
```

No response is sent before commit.

## Domain failure transaction

If validation or the pure transition returns a public domain error:

```text
BEGIN IMMEDIATE
  revalidate running job identity
  store terminal public error response
  update idempotency terminal response
  mark job failed
  append bounded failure summary
COMMIT
```

Session rows are untouched.

## Unexpected failure

If an unexpected failure occurs before a state transaction commits:

- rollback;
- mark the job failed with `INTERNAL_ERROR` when the database remains usable; or
- leave/reset it to queued only for explicitly classified transient infrastructure failures.

If the daemon cannot prove the outcome, it closes the workspace scheduler and requires `doctor`. It MUST NOT guess or apply the mutation again outside idempotency rules.

## Cursor concurrency

Complete, skip, retry, return, block, unblock, and cancel validate:

```text
expected_session_revision
expected_attempt_id
```

A queued command may become stale before execution because an earlier job changed the cursor. It then fails with a conflict and does not adapt itself to the new stage.

This is deliberate. A command intended for one stage must never mutate another.

## Item concurrency

Item commands validate:

```text
expected_attempt_id
expected_item_revision
```

Example:

```text
attempt A active
item x revision 0
item y revision 0

client 1 sets x expecting 0 -> succeeds, x=1, session revision increments
client 2 sets y expecting 0 -> succeeds, y=1, session revision increments
client 3 sets x expecting 0 -> ITEM_REVISION_CONFLICT
```

The session revision may change between unrelated item updates without causing a conflict. Attempt identity prevents updates after complete, retry, or return.

## No-op behavior

A semantic no-op, such as clearing an already-unset item with current preconditions:

- succeeds;
- returns `changed=false`;
- does not increment item or session revision;
- still produces one terminal job response and idempotency record.

No-op behavior must be identical on retry.

## Artifact completion race

For required local artifacts:

1. read item slot revisions and metadata;
2. hash files outside the transaction;
3. begin transaction;
4. verify active attempt and each artifact item revision are unchanged;
5. compare fresh digest and size;
6. fail `ARTIFACT_CHANGED` or commit completion.

A file may change immediately after completion because Podway is not an adversarial security boundary. The check ensures current procedural freshness at the completion instant as far as local observation permits.

## Job cancellation transaction

Queued cancellation executes under the scheduler lock:

```text
BEGIN IMMEDIATE
  load target job
  require state=queued
  update state=cancelled, finished_at, cancellation result
  update idempotency terminal response
COMMIT
```

It may bypass normal FIFO because it does not mutate session state. A running job cannot be cancelled.

## Reset and replace barrier transaction

A destructive reset or replace job is admitted as the current queue tail and blocks later mutation admission. When it executes, all earlier jobs are terminal. Its transaction may safely:

1. delete old session domain rows;
2. delete terminal jobs and idempotency records scoped to the old session;
3. preserve the workspace-scoped barrier job and receipt;
4. create a replacement session when requested;
5. commit the reset or replace result atomically.

If the barrier is cancelled while queued, normal admission resumes.

## Read consistency

A normal read uses one SQLite read transaction and returns a coherent snapshot.

It includes queue indicators so the caller knows whether later admitted mutations are pending. `--after-job` and `--wait-for-idle` wait outside the read transaction, then open a fresh read snapshot.

## Exact-once effect, not exactly-once transport

IPC transport may retry and responses may be lost. Podway guarantees **one logical state effect per idempotency key and canonical request**, not exactly one network exchange.

The accepted target exposes lookup by idempotency key without replaying a request.
It returns terminal receipt data after job-row pruning and makes pre-admission,
admitted-timeout, and transport-outcome-unknown states machine-distinguishable.

## Crash outcomes

| Crash point | Durable outcome |
|---|---|
| Before admission commit | No job exists |
| After admission commit, before response | Queued job exists; retry returns it |
| After claim commit, before state transaction | Running job resets to queued on restart |
| During preparation | Running job resets to queued on restart |
| During uncommitted state transaction | SQLite rollback; job resets to queued or is safely failed |
| After state and terminal result commit, before response | Succeeded job and response exist; retry returns them |
| During failure-result transaction | Session unchanged; job is retried or failed once after restart |

## Busy and lock behavior

The daemon is the only normal writer, so persistent `SQLITE_BUSY` indicates an internal bug, unsupported external writer, or maintenance conflict.

The store uses bounded retry within `busy_timeout`. Exhaustion becomes an internal or workspace-state error and does not spin indefinitely.
