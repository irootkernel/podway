# Recovery, Retention, and Maintenance

## Recovery philosophy

Podway manages disposable current-task state. It provides strong crash consistency and predictable daemon restart recovery, but it does not build a forensic recovery, archive, or backup product.

When state is corrupt beyond safe transactional repair, Podway fails closed and offers an explicit destructive reinitialization.

## Daemon restart

On startup:

1. acquire singleton lock and socket;
2. load the minimal workspace registry;
3. validate each registered root;
4. open supported databases;
5. transactionally move all `running` jobs to `queued` and record recovery;
6. validate one-active-attempt and cursor invariants;
7. start schedulers for workspaces with queued jobs;
8. remove registry entries for missing roots.

A worktree with unreadable state is isolated. It does not prevent other worktrees from operating.

## Minimal global registry

Location on macOS:

```text
~/Library/Application Support/Podway/workspaces.json
```

Shape:

```json
{
  "schema": "podway.registry/v1",
  "workspaces": [
    {
      "workspace_uuid": "...",
      "last_known_root": "/Users/example/src/project-wt",
      "last_seen_at": "2026-07-13T03:00:00.000Z"
    }
  ]
}
```

The daemon is the sole writer. Updates use temporary-file write, fsync where appropriate, and atomic rename.

The registry contains no task title, procedure, stage, attempt, item, blocker, artifact, or job payload data.

## Moved worktrees

If a registered path is missing, the entry is removed after a grace-free validation pass. A moved worktree is rediscovered when a CLI request arrives at its new path. Matching Git identity and workspace UUID allow the registry to be rebuilt.

Acknowledged queued jobs at a moved but not rediscovered path remain in the worktree database and resume after rediscovery.

## Worktree deletion

Deletion is final by design. Podway does not copy queued jobs elsewhere. Waiting clients may receive `WORKTREE_GONE` or a connection error depending on timing.

## Corrupt state

Symptoms include:

- SQLite open or integrity failure;
- invalid migration checksum;
- procedure snapshot digest mismatch;
- impossible cursor or active-attempt state;
- malformed canonical JSON;
- unsupported newer schema.

Behavior:

- reject mutations and normal state reads;
- return `WORKSPACE_STATE_UNREADABLE` or `WORKSPACE_SCHEMA_UNSUPPORTED`;
- allow `podway doctor` to report diagnostics;
- allow explicit `podway reset --all --force --yes`.

Podway does not automatically discard state.

## Destructive reset-all protocol

Because the database may be unreadable, reset-all is a daemon maintenance operation guarded by a filesystem marker.

1. validate the Git worktree and path containment;
2. acquire the workspace maintenance lock;
3. stop its scheduler and reject new admissions;
4. close all database handles;
5. create `.podway/runtime/reset.marker` atomically; the marker contains operation ID, idempotency key, request digest, target new workspace UUID, and submitted time;
6. remove `state.sqlite3`, `-wal`, and `-shm` files;
7. create a new database using the target workspace UUID;
8. insert a terminal workspace-scoped reset job and idempotency receipt into the new database;
9. update the global registry;
10. remove the marker;
11. restart the scheduler.

On startup, an existing marker causes the daemon to finish the reset before serving the workspace. A lost client response can be retried with the same idempotency key and is answered from the new database. The operation is idempotent.

## Retention policy

### Sessions and attempts

- the current completed or cancelled session remains until reset;
- all attempts and item slots for that session remain until reset;
- no session archive is created;
- reset removes session-scoped idempotency records.

### Jobs

Default pruning:

- always retain non-terminal jobs;
- retain at least the newest 100 terminal jobs;
- retain terminal jobs for up to 7 days;
- cap terminal jobs at 1,000 per workspace;
- prune oldest eligible rows after successful mutations and at daemon idle time.

### Idempotency records

- ordinary session mutation jobs and receipts remain until a destructive reset or replace barrier commits;
- the barrier deletes old-session operational payloads after all earlier jobs are terminal;
- reset and replace receipts are workspace-scoped and survive deletion of the old session;
- workspace bootstrap and maintenance records retain the newest 100 or 30 days, whichever is smaller after the minimum set;
- terminal response JSON remains after its job row is pruned.

### Operational journal

- maximum 10,000 rows per workspace;
- maximum age 7 days;
- always retain the newest 200 rows;
- no item values or artifact paths in normal entries.

Retention constants are internal v1 defaults. Future configuration must not allow unbounded growth.

## Doctor checks

`podway doctor` reports each check as pass, warning, fail, or skipped.

Checks include:

- daemon install, reachability, and version;
- socket permissions and ownership;
- Git worktree discovery and non-bare state;
- `.podway` containment and symlink safety;
- tracked config validation;
- runtime ignore rule;
- runtime file permissions;
- database schema, migration checksums, and fast integrity;
- one-session and one-active-attempt invariants;
- procedure snapshot digest;
- queue sequence and running-job recovery state;
- global registry agreement;
- local required artifact existence in deep mode.

Doctor is read-only. It may recommend `init --repair`, `workspace repair`, daemon restart, or destructive reset, but does not perform them automatically.

## Maintenance locks

Workspace maintenance operations such as migration and reset-all obtain an exclusive in-memory scheduler lock plus a database or filesystem marker. Normal admission fails with a retryable maintenance error while the operation is active.

## No backup or export requirement

The public v1 product intentionally has no:

- session export;
- session import;
- global backup command;
- event replay;
- post-mortem archive.

Users who need durable task records should record outcomes in their normal project systems. Podway's responsibility ends at the current worktree session.
