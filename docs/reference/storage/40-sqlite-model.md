# SQLite Model

## Authoritative state

The worktree-local SQLite database is the authoritative state for:

- workspace identity;
- one current task session;
- immutable procedure snapshot;
- stage progress and attempts;
- item slots and artifact metadata;
- blockers;
- durable mutation jobs;
- idempotency records;
- bounded operational journal.

Podway does not use event sourcing. The journal is diagnostic and cannot be used as the only reconstruction source.

Reference DDL and migrations: [`../../spec/sqlite-v1.sql`](../../spec/sqlite-v1.sql)
followed by [`../../spec/sqlite-v2.sql`](../../spec/sqlite-v2.sql). Schema v2 adds
the bounded admission-time response context used by durable reconciliation.

## Database location

```text
<worktree>/.podway/runtime/state.sqlite3
```

WAL and shared-memory side files remain in the same directory. The directory is untracked and user-private.

## Connection policy

Every daemon connection MUST apply and verify:

```sql
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA busy_timeout = 5000;
PRAGMA trusted_schema = OFF;
```

Additional performance pragmas may be used only when they do not weaken acknowledged-job durability or transaction correctness.

The application MUST use an application-controlled SQLite build that supports STRICT tables, JSON validation, WAL, and the required durability pragmas. It must not rely on an unknown system SQLite version.

Only `podwayd` opens the live database in write mode. Read-only diagnostic tooling must not run concurrently unless explicitly supported by SQLite and the daemon maintenance lock.

## Schema tables

### `schema_migrations`

Records each applied forward migration:

```text
version
name
checksum
applied_at_ms
```

Migration checksums detect modified migration history.

### `workspace_state`

A singleton row containing:

```text
workspace_uuid
git_common_fingerprint
git_worktree_fingerprint
last_validated_root
next_workspace_sequence
created_at_ms
updated_at_ms
```

### `procedure_snapshots`

Stores immutable canonical procedure JSON and digest. V1 normally has zero or one snapshot because only one session exists, but the table remains normalized for clean reset and future migration.

### `task_sessions`

A singleton current-session row containing lifecycle, revision, active cursor, timestamps, and procedure snapshot reference.

A partial or singleton constraint ensures at most one row.

### `stage_progress`

One row per session stage:

```text
stage_id
stage_index
progress_state
latest_attempt_number
latest_attempt_id
```

Progress states are constrained to `pending`, `current`, `done`, `skipped`, `redo`, and `abandoned`.

### `attempts`

One row per stage attempt, unique on session, stage, and attempt number. Lifecycle is constrained to `active`, `completed`, `skipped`, or `abandoned`.

### `item_slots`

One row per item definition in each created attempt. Slots exist even when unset:

```text
attempt_id
item_id
item_type
item_revision
value_json nullable
created_at_ms
updated_at_ms
```

`item_revision` begins at 0. A changed set or clear increments it. Keeping an unset slot after clear prevents stale writers from treating the item as never modified.

### `blockers`

Stores attempt-scoped open or resolved blockers.

### `jobs`

Stores durable admitted requests, queue sequence, lifecycle, canonical request, and terminal result.

### `idempotency_records`

Binds an idempotency key to one request digest and job ID. It retains the terminal response after terminal job pruning.

### `operational_journal`

Stores bounded diagnostic events such as daemon recovery, job claim, transition success, transition failure, migration, and pruning. It MUST NOT store item values or artifact locations by default.

## Constraints

The relational schema enforces as much as practical:

- singleton workspace and session rows;
- unique workspace UUID;
- unique stage index and ID within the session;
- unique attempt number per stage;
- unique item slot per attempt and item;
- unique workspace job sequence;
- unique idempotency key;
- valid enum values;
- non-negative revisions and counts;
- valid JSON where JSON is stored;
- foreign-key cleanup on session reset.

Cross-row invariants such as exactly one active attempt are enforced by transactions and verified by integrity checks. SQLite triggers MAY be used only when their behavior is fully tested and documented; application-level transitions remain the primary source of semantics.

## Procedure snapshot storage

The store persists:

```text
snapshot_id
schema_id
procedure_id
procedure_version
name
digest
canonical_json
source_kind: preset | file
source_label
created_at_ms
```

The digest is checked when loading. A mismatch makes the workspace unreadable until destructive reset because it indicates corruption or unsupported direct modification.

## JSON storage rules

Stored JSON must be canonical for its type:

- procedure snapshots use Podway Canonical JSON v1;
- job requests use canonical request JSON;
- item values use compact typed JSON;
- result and error envelopes use compact public JSON;
- journal details use bounded internal JSON.

The database does not store raw YAML.

## Artifact representation

Artifact metadata is stored in an `artifact` item slot value. No blob table exists. The database never stores file bytes.

## Migration policy

- Migrations are forward-only.
- The initial migration uses the non-file fixture identity `uninitialized-database`: predecessor `schema-0-uninitialized` and result `schema-v1`.
- Initialization and every migration MUST first apply and verify the required connection pragmas: foreign keys ON, WAL journal mode, FULL synchronous, 5000 ms busy timeout, and trusted schema OFF.
- Each migration runs inside an exclusive transaction when SQLite permits.
- The daemon backs no database up globally.
- A migration either commits completely or leaves the prior schema intact.
- Migration code validates invariants before declaring success and MUST preserve user task state, durable mutation and idempotency state, and attempt and lifecycle semantics without duplicating a mutation.
- A database created by a newer unsupported schema fails with `WORKSPACE_SCHEMA_UNSUPPORTED`.
- Downgrade is unsupported.

## Integrity checks

Fast startup checks:

- schema version and migration checksums;
- workspace singleton and UUID;
- snapshot digest;
- session cursor consistency;
- one active attempt invariant;
- queued/running job sanity.

`podway doctor --deep` additionally revalidates the Git-to-Store workspace binding. The store layer's deep SQLite integrity mode is currently exercised by tests only and is not wired into the production doctor command in v0.1.0.

## Database lifecycle

### Creation

Initialization from `uninitialized-database` creates a temporary database in `.podway/runtime/`, applies and verifies the required connection pragmas, and runs `schema-0-uninitialized` to `schema-v1` in a transaction. It writes workspace identity, validates invariants, commits, fsyncs as required, and atomically installs the result as `state.sqlite3`. A failure leaves an existing `state.sqlite3` intact or installs no database; it never leaves a partial installation.

### Normal reset

Session reset runs as a destructive queue barrier. It deletes session domain rows and old session-scoped job/idempotency payloads while preserving workspace schema and the workspace-scoped reset receipt.

### Reset all

Destructive reset closes handles, writes a filesystem marker containing idempotency and target identity, removes database and side files, creates a new schema and workspace UUID, persists the terminal reset receipt in the new database, then removes the marker. Startup completes an interrupted marked reset idempotently.

### Worktree deletion

No special database action occurs. The database disappears with the worktree and the global registry entry is removed when detected.
