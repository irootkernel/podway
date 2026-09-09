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
5. validate the complete retained task state, including one-active-attempt and cursor invariants;
6. transactionally move all `running` jobs to `queued` and record recovery;
7. start schedulers for workspaces with queued jobs;
8. remove registry entries for missing roots.

A worktree with unreadable state is isolated. It does not prevent other worktrees from operating.

## Minimal global registry

Accepted v0.1.0 target location:

```text
<effective-user-home>/.podway/state/workspaces.json
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

The daemon is the sole writer. The implemented registry uses the user-global
location above. Updates use a same-directory temporary write, fsync where
appropriate, and atomic rename.

Workspace UUIDs and exact roots are each unique in newly written registry state.
Bootstrap, refresh, and move reject an exact root owned by another UUID. A legacy
duplicate-root document remains readable only for isolated diagnosis and explicit
reset recovery; ordinary activation does not add another owner or silently choose
one.

The registry contains no task title, procedure, graph node, attempt, item, blocker, artifact, or job payload data.

Each runtime mode has a separate registry namespace. A worktree is admitted only
when config, the Store's schema-v10 mode binding, and that namespace agree. Legacy
stores without a mode column belong to `prod`; a named daemon does not migrate or
open them.

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

1. validate the Git worktree and path containment; when persisted Git directory
   fingerprints are detached, reset still requires an exact stored-root match and
   registry membership for the persisted workspace UUID;
2. acquire the workspace maintenance lock and bind the exact registry-root UUID
   generation to Store identity or an existing reset marker;
3. stop its scheduler and reject new admissions;
4. close all database handles;
5. create `.podway/runtime/reset.marker` atomically; newly published v2 markers contain the operation ID, idempotency key, request digest, predecessor and target workspace UUIDs, submitted time, and original response request ID; v1 markers remain readable only so an upgrade can finish an already-published reset;
6. remove `state.sqlite3`, `-wal`, and `-shm` files;
7. create a new database using the target workspace UUID;
8. insert a terminal workspace-scoped reset job and v3 idempotency receipt into the new database, including the lookup command and full target-workspace response context;
9. compare the registry-root generation again and atomically replace every stale
   exact-root entry with the target UUID;
10. remove the marker;
11. restart the scheduler.

On startup, an existing marker causes the daemon to finish the reset before serving the workspace. A lost client response can be retried with the same idempotency key and is answered from the new database. After the terminal job row is pruned, `job.lookup` and exact reset replay reconstruct the same full terminal output from the retained workspace receipt. The operation is idempotent.

## Retention policy

### Sessions and attempts

- the current completed or cancelled session remains until reset or archive;
- all attempts and item slots for that session remain until reset, archive, or explicit start-time deletion;
- the complete Procedure v2 current-task state remains together, including its immutable
  snapshot, graph placements, trace, workflow memory, and goal history;
- a normal session reset removes that complete current-task state together while
  preserving the initialized workspace identity, schema history, and workspace-scoped receipts;
- archive retains that complete state as one immutable inactive session and frees
  the single current-session slot;
- preserving running work during start atomically cancels and archives it with a
  superseded disposition that records the reason, optional actor, and successor
  session identity;
- at most 32 inactive sessions are retained per worktree; reaching the limit
  blocks archival and terminal eligible replacement without eviction;
- confirmed purge deletes exactly one inactive session; inactive sessions cannot
  be restored or mutated;
- reset removes session-scoped idempotency records.

### Jobs

Default pruning:

- always retain non-terminal jobs;
- retain at least the newest 100 terminal jobs;
- retain terminal jobs for up to 7 days;
- cap terminal jobs at 1,000 per workspace;
- prune oldest eligible rows after successful mutations.

### Idempotency records

- ordinary session mutation jobs and receipts remain until a destructive reset or start-resolution barrier commits;
- the barrier deletes old-session operational payloads after all earlier jobs are terminal;
- reset and replace receipts are workspace-scoped and survive deletion of the old session;
- workspace bootstrap and maintenance records retain the newest 100 or 30 days, whichever is smaller after the minimum set;
- terminal response JSON remains after its job row is pruned;
- receipt v5 retains the v4 command, projections, bounded response context, and
  complete canonical public terminal envelope plus an explicit durable Procedure v2
  execution flavor sealed atomically with terminal state;
- v5 lookup returns the stored public envelope without reapplying the current catalog
  or result renderer; predecessor receipt schemas remain strictly decodable;
- canonical requests, selectors, preconditions, idempotency keys, and submitted
  values are not copied into the retained response context;
- v0-v2 succeeded/failed receipt-only lookup fails closed when a complete envelope
  cannot be reconstructed; v3 remains decodable through its golden-tested legacy
  projection path for compatibility.

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
- Git-to-Store workspace binding revalidation in deep mode.
- active-Store and disposable-snapshot agreement in deep mode, reported as
  `store_snapshot_diverged` when the two coherent reads do not match.
- current-session observation projection from that same coherent Store view in
  deep mode, reported as `session_projection_failed` with a bounded
  `failure_kind` when public session reads cannot reconstruct the stored state.

Doctor is read-only. It may recommend `init --repair`, `workspace repair`, daemon restart, or destructive reset, but does not perform them automatically.

## Maintenance locks

Workspace maintenance operations such as migration and reset-all obtain an exclusive in-memory scheduler lock plus a database or filesystem marker. Normal admission fails with a retryable maintenance error while the operation is active.

Explicit workspace removal uses the same maintenance key and the closed
`podway.workspace-removal-marker/v1` contract. The marker binds an operation ID,
canonical request digest, response request ID, workspace UUID, exact root path and
Git identities, and creation time. Recovery resumes only after revalidating that
identity; it removes the marker last and accepts only a descriptor-verified empty
`.podway` residual as an already-absent replay convergence. Symlinks, mounts,
non-directories, changed roots, competing markers, and unknown non-empty residuals
fail closed.

Workspace mode switching uses the same exclusive maintenance key and the adjacent
`podway.workspace-mode-switch-marker/v1`. The marker binds the plan-token digest,
workspace UUID, source and target modes, original and expected target config
digests, and the ordered
source-close, registry-retirement, runtime-removal, config-update, target-init,
and target-publication steps. An exact retry resumes only the missing suffix and
a completed marker returns `already_applied`; a later independently planned switch
may retire that completed receipt before publishing its own marker. Runtime reset
deletes the complete current and archived session, attempt, queue, receipt, and job
history held under `.podway/runtime/`. A session without queued or running work is
reported by plan but does not make the disposable source busy. The caller must
retain the exact plan token: expiry applies before marker publication, while an
existing marker keeps that token as recovery authority until its completed receipt
is retired. A source daemon restart before marker publication invalidates its
process-local plan record, so the caller must plan again; a restart after marker
publication resumes with the retained token. If config publication completed but
the `config_updated` marker step did not, recovery recognizes the exact target
digest, records the completed step, and continues; any third config digest fails
closed. Runtime reset never deletes
procedures, `.podway/.gitignore`, config
content unrelated to mode, or Git worktree, index, and ref state.

## Reserved account-runtime retirement

The [runtime reset contract](../operations/runtime-reset.md#durable-retirement-and-recovery)
defines a separate offline-capable account record outside worktree state. It
reserves account-runtime deletion while preserving every Store, job and session;
it does not alter workspace reset, removal or mode-switch marker semantics.
Stable singleton anchors, committed admission fencing and exact intent-backed
retry are mandatory before its runtime admission. No SQLite migration is added.

## No backup or export requirement

The public v1 product intentionally has no:

- session export;
- session import;
- global backup command;
- event replay;
- post-mortem archive.

Users who need durable task records should record outcomes in their normal project systems. Podway's responsibility ends at the current worktree session.
