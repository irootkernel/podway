# Product Acceptance Criteria

Podway is ready for public macOS release only when every mandatory criterion below is demonstrated by an automated test, release artifact inspection, or documented manual release check.

## Product purpose

- A user can initialize a valid Git worktree without starting a task.
- A user can start one task from a preset or custom procedure.
- `status` makes the active stage, attempt, item state, blockers, and pending jobs clear.
- `next` lists every missing required item and a structured command suggestion.
- A stage cannot complete with missing required items, changed required local artifacts, or open blockers.
- The common interface emphasizes current task execution rather than historical evidence.

## Procedure semantics

- Custom ordered procedures validate and run without source changes.
- Procedure snapshots are immutable for a running session.
- All six item types and constraints behave as specified.
- Skip works only when explicitly permitted.
- A running session has exactly one active attempt.
- Retry creates a clean attempt of the same stage.
- Return creates a clean destination attempt and marks reached downstream stages `redo`.
- Reopen applies the same redo semantics to a completed session.
- Cancelled sessions cannot reopen.
- Reset removes the task session and its session-local history.

## Daemon and queue

- `podwayd` is the sole normal writer.
- Mutations are durable before admission acknowledgement.
- FIFO ordering is proven within one worktree.
- independent worktrees can execute mutations concurrently.
- same-item concurrent updates cannot silently overwrite.
- cursor commands cannot apply to a different active stage.
- client retry after lost response does not duplicate a mutation.
- queued jobs survive daemon restart while the worktree remains discoverable.
- job cancellation works only before claim.

## Crash safety

- Every defined crash-injection point has a deterministic valid recovery outcome.
- A state transition and terminal job result commit atomically.
- A failed job changes no session state.
- Running jobs recover to queued after daemon restart when no terminal commit exists.
- Reset-all recovers idempotently from an interrupted marker state.

## Git and filesystem boundary

- Workspace commands fail outside a valid non-bare Git worktree.
- Main and linked worktrees are supported.
- Path and symlink escapes are rejected.
- Copied live workspace UUID conflicts are detected.
- A moved worktree can repair its registry path after identity validation.
- Runtime state remains inside `.podway/runtime/` and is ignored by Git.
- Deleting the worktree deletes task state and queued jobs.

## Persistence

- SQLite schema, foreign keys, WAL, and synchronous durability are configured as specified.
- Migration diagnostics identify predecessor `schema-0-uninitialized` and result `schema-v1` for the non-file `uninitialized-database` initialization fixture.
- Deterministic schema-0 to v1 conformance proves required pragmas, transactional initialization, no user task-state loss, no duplicated mutation, and no partial installation.
- Migrations are transactional and checksummed.
- Unsupported newer state fails closed.
- Corrupt state fails closed and is diagnosable.
- Destructive reset-all recreates a usable workspace.
- Terminal jobs, idempotency data, and journal remain bounded according to policy.
- No global task-state copy exists.

## Interfaces

- Every public command has complete help.
- Every public command emits valid versioned JSON.
- Public error codes and exit codes match the catalog.
- Text and JSON never disagree on successful state.
- zsh, bash, and fish completion are shipped and tested.
- IPC rejects unsupported versions and oversized or malformed frames safely.
- Automation can pass explicit revision, attempt, item revision, and idempotency values.

## macOS operation

- Both binaries install and run on the supported macOS release matrix.
- LaunchAgent install is idempotent.
- Daemon starts at user login.
- Explicit stop, start, restart, status, logs, and uninstall work.
- Socket and runtime permissions are correct.
- Stale sockets and unexpected daemon exits recover.
- Service uninstall preserves all worktree state.

## Safety and trust

- No command executes arbitrary procedure-defined code.
- No command performs Git mutation.
- No daemon component listens on a network socket or performs network I/O.
- Artifact bytes are never stored.
- Logs exclude item values and task content by default.
- Same-user trust limitations are documented in help and release documentation.
- No workspace access key or misleading authentication claim exists.

## Built-in presets

- `sw-dev`, `bug-fix`, `docs-only`, and `analysis` validate against the shipped schema.
- Each preset has clear help and stage descriptions.
- Each preset passes a complete end-to-end scenario.
- Each preset passes at least one retry and one return scenario across the conformance suite.

## Distribution

- Rust lockfile is committed.
- Apple Silicon and Intel artifacts are built and checksummed.
- Release archive contains both binaries, completions, schemas, presets, README, and MIT License.
- Public artifacts have documented signing and notarization status.
- Upgrade from the previous supported database schema is tested.
- Phase 8 release evidence at `release/migration-evidence-v1.json` records the deterministic schema-0 to v1 migration conformance result.
- Release notes document contract versions and any migration.

## Final acceptance rule

No criterion may be waived silently. A release exception requires:

1. an issue with owner and rationale;
2. a user-visible release-note entry;
3. a time-bounded remediation plan;
4. confirmation that the exception does not violate core invariants or create possible duplicate state mutation.
