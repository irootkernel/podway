# Daemon and Write Queue

## Purpose

`podwayd` provides one trusted write path, deterministic ordering, durable admission, idempotent retries, and crash recovery. The queue does not create parallel stages. It permits independent callers to submit updates safely and permits different worktrees to progress concurrently.

## Daemon singleton

There is one daemon per OS user.

Singleton enforcement uses:

1. a user-private runtime directory;
2. an exclusive process lock file;
3. exclusive ownership of the Unix-domain socket;
4. peer-user checks on accepted connections.

A second daemon instance MUST exit with a clear diagnostic and MUST NOT remove a live instance's socket.

## Queue topology

```text
podwayd
  + workspace A FIFO: 101 -> 102 -> 103
  + workspace B FIFO:  41 ->  42
  + workspace C FIFO:   9
```

Rules:

- each workspace has a monotonic `workspace_sequence`;
- one job per workspace may be `running`;
- the next executable job is the lowest `queued` sequence;
- cancelled queued jobs are skipped;
- separate workspaces may have one `running` job each;
- execution fairness SHOULD prevent a busy worktree from starving others;
- database transitions are short and MUST NOT perform network I/O or arbitrary external work.

## Durable admission

A mutation is admitted only after this transaction commits:

1. validate workspace database and queue capacity;
2. canonicalize the request and compute its digest;
3. look up the idempotency key;
4. if new, increment `next_workspace_sequence`;
5. insert the `queued` job;
6. persist the idempotency binding;
7. commit;
8. acknowledge admission.

If the daemon exits before commit, the client has no admitted job. If the daemon exits after commit but before acknowledgement, retrying the same idempotency key returns the existing job.

## Job model

A job contains:

```text
id
workspace_uuid
workspace_sequence
idempotency_key
request_digest
command_name
canonical_request_json
state: queued | running | succeeded | failed | cancelled
submitted_at
claimed_at
finished_at
result_json or error_json
```

A mutation job's request is immutable after admission.

## Job lifecycle

```text
queued -> running -> succeeded
                  -> failed
queued -> cancelled
running --daemon crash--> queued
```

A domain validation error produces a terminal `failed` job and no session mutation. Internal transient failures MAY return the job to `queued` only when the implementation can prove that no domain transaction committed.

## Claim and execution

The scheduler claims a job by transactionally changing the lowest queued sequence to `running`. The executing worker then:

1. loads the canonical request;
2. validates idempotency and target workspace identity;
3. loads current session state;
4. validates cursor or item preconditions;
5. invokes the pure domain transition;
6. persists all relational changes;
7. increments session revision exactly once when applicable;
8. stores the terminal response and command receipt;
9. marks the job `succeeded`;
10. commits atomically.

For a domain error, a separate short transaction stores the error and marks the job `failed` without modifying session rows.

## Idempotency

Every mutation has an idempotency key.

- The CLI generates a random key by default.
- Automation MAY provide `--idempotency-key`.
- Same key plus same canonical request returns the same job and final response.
- Same key plus different canonical request fails with `IDEMPOTENCY_KEY_REUSED`.
- Idempotency records for session mutations remain until session reset.
- Workspace-maintenance receipts are retained according to the bounded retention policy.

Canonical request identity excludes transport-only fields such as wait timeout but includes command, workspace UUID, target session or attempt, all preconditions, and payload.

## Concurrency preconditions

### Cursor-changing commands

The following commands require an expected session revision and active attempt ID:

- complete;
- skip;
- retry;
- return;
- block and unblock;
- cancel;
- reset of a running session.

The normal CLI reads current state and supplies the observed values automatically. Automation can specify them explicitly.

A mismatch returns `SESSION_REVISION_CONFLICT` or `ATTEMPT_NOT_CURRENT`.

### Item mutations

Item mutations require:

- expected active attempt ID;
- expected item revision when replacing, clearing, or removing an existing value;
- an unset precondition for the first write when applicable.

Different items may update concurrently because they do not require an unchanged session revision. Each successful item mutation still increments the session revision for global observation ordering.

Same-item conflicts return `ITEM_REVISION_CONFLICT`.

## Synchronous and detached behavior

By default, the CLI waits for terminal state:

```bash
podway complete
```

With `--detach`, the daemon returns after durable admission:

```bash
podway complete --detach
```

The admission response includes the job ID, workspace sequence, and state `queued` or, rarely, a terminal state if it completed immediately.

A client waiting synchronously may disconnect. The job continues. Retrying with the same idempotency key returns the existing result.

The accepted automation target also provides read-only worktree-scoped
`job lookup --idempotency-key`. It can recover a queued or running job and the
original terminal envelope from a retained receipt after job-row pruning. Response
loss after possible admission is an outcome-unknown state; it is not cancellation.
Admission-aware envelopes and lookup remain planned under the `RECON` epic.

## Cancellation

`podway job cancel <job-id>` is a daemon control operation, not an ordinary queued session mutation.

- If the target is `queued`, the daemon atomically marks it `cancelled` under the workspace scheduler lock.
- If the target is `running` or terminal, cancellation fails with `JOB_NOT_CANCELLABLE`.
- Running database transitions are not interrupted.
- Cancelling a job does not increment session revision.
- A cancelled job remains visible until normal job pruning.

## Destructive queue barriers

`reset`, `reset --all`, and `start --replace` are destructive barriers. Once admitted, the daemon rejects later workspace mutation admissions with `WORKSPACE_MAINTENANCE` until the barrier is cancelled or reaches terminal state. Reads remain available.

This guarantees that reset can remove old session jobs, requests, and idempotency data without invalidating a later acknowledged mutation. Jobs admitted before the barrier execute first in FIFO order. The barrier job uses a workspace-scoped idempotency record.

## Queue limits and backpressure

Workspace configuration defines `job_queue.max_pending`, default 256, minimum 1, maximum 4096.

The pending count includes `queued` jobs and excludes `running` and terminal jobs. Admission beyond the limit fails with `WORKSPACE_QUEUE_FULL`. Read commands remain available.

## Restart recovery

On daemon startup:

1. load the minimal workspace registry;
2. discard entries whose roots no longer exist;
3. validate each remaining worktree and database;
4. change any `running` jobs to `queued` because an uncommitted transaction could not survive process death;
5. start a scheduler for workspaces with queued jobs;
6. process jobs in sequence order.

If a worktree moved while the daemon was stopped, queued work resumes when the CLI accesses the new path and identity is repaired. The daemon does not search the entire filesystem.

## Graceful shutdown

On requested shutdown:

- stop accepting new connections;
- stop claiming new jobs;
- allow currently executing short database transactions to finish;
- flush responses where possible;
- close database handles;
- close the listener, remove the ownership-token-guarded socket while still holding the singleton lock, then release the lock.

A forced termination remains safe because SQLite transactions and restart recovery define the outcome.

## Read consistency

Read responses include:

```text
latest_workspace_sequence
session_revision
queued_job_count
running_job_id
pending_mutations
```

Options:

- `--wait-for-idle`: wait until no queued or running jobs remain;
- `--after-job <id>`: wait until the named job is terminal, then read;
- `--timeout <duration>`: bound either wait.

Reads never wait by default. Callers must inspect `pending_mutations` when they require a quiescent view.
