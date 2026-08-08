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

For Procedure v2, the same database additionally owns the immutable graph snapshot, single
cursor and append-only execution trace, workflow-memory records, and optional goal history.

Podway does not use event sourcing. The journal is diagnostic and cannot be used as the only reconstruction source.

Reference DDL and migrations: [`../../../assets/specifications/sqlite-v1.sql`](../../../assets/specifications/sqlite-v1.sql)
followed by [`../../../assets/specifications/sqlite-v2.sql`](../../../assets/specifications/sqlite-v2.sql)
and [`../../../assets/specifications/sqlite-v3.sql`](../../../assets/specifications/sqlite-v3.sql).
Schema v2 adds the bounded admission-time response context used by durable reconciliation. Schema
v3 adds parallel `v2_` tables without altering or reinterpreting retained v1 session rows.

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

### Procedure v2 parallel state

Schema v3 keeps Procedure v2 state separate from the released v1 domain tables:

- `v2_workspace_state`, `v2_procedure_snapshots`, and `v2_graph_nodes` own workspace revision,
  canonical snapshot identity, and immutable placement metadata;
- `v2_task_sessions`, `v2_graph_node_counters`, and `v2_attempts` own the single cursor, trace
  assignment state, attempt counts, lifecycle, validity, and goal-revision binding;
- `v2_item_slots`, `v2_blockers`, and `v2_resolved_evidence_references` own attempt-local values
  and exact evidence-resolution snapshots;
- `v2_decision_records` and `v2_rework_records` preserve immutable workflow history;
- `v2_goal_revisions`, `v2_goal_criteria`, `v2_criterion_assessment_results`,
  `v2_criterion_citations`, and `v2_goal_assessments` preserve optional goal history.

The existing `jobs`, `idempotency_records`, and `operational_journal` remain shared infrastructure.
Schema v2 already made terminal envelopes version-neutral, so schema v3 does not duplicate their
queue, receipt, or idempotency state.

The additive Procedure v2 store boundary creates a complete graph session or replaces its current
state under exact workspace and session revision preconditions. Each write is one immediate
transaction over the immutable snapshot, derived graph placements, singleton cursor, append-only
attempt trace, and per-node counters. Replacement advances both revisions exactly once, preserves
snapshot and trace identities, terminalizes the prior active attempt, and may append at most one
fresh active attempt. Loading reconstructs the domain trace and verifies its cursor, lifecycle,
validity, per-node numbering, trace sequencing, timestamps, snapshot digest, placement metadata,
and counters. Snapshot reconstruction also verifies the canonical bytes against the canonical
Procedure v2 JSON Schema; configuration remains responsible for source parsing, closed-reference
validation, and graph vetting before admission. The same reconstruction runs during fast startup
integrity checks, so inconsistent Procedure v2 graph state fails closed after reopen.

Workflow memory is part of that same complete-state transaction. Every attempt materializes one
item slot per item in the referenced node definition, in definition order, together with bounded
blockers and the placement's evidence-resolution snapshot in declaration order. A cursor-stable
replacement may advance item revisions or resolve and append blockers without changing the active
attempt. Recorded values must satisfy their immutable item declarations, a completed attempt must
contain every required item and no open blocker, and a skipped attempt contains no recorded value.
References resolve exactly at consumer activation: an optional reference remains unresolved only
when no earlier valid terminal source exists. Cursor-moving replacements keep terminal item,
blocker, and evidence rows immutable;
decision and rework records are append-only and bound to the exact successor revision and fresh
target attempt, while conservative invalidation changes only attempt validity. Rework requires a
prior valid attempt of its target node. Completed-session reactivation requires an appended
manual-rework record that identifies the fresh target attempt and carries `reactivated: true`, or an
appended goal revision whose binding trace identifies that target and carries `reactivated: true`.

`items_digest` is SHA-256 over Podway Canonical JSON v1 for an array in item-definition order. The
array contains only recorded slots, with each member shaped as `{"id": <item-id>, "value":
<compact-typed-value>}`. Selectors never alter this complete-source digest; they only restrict the
recorded values returned by selected read-back. Loading reorders relational slots from the immutable
definition, requires byte-canonical typed values and selector JSON, recomputes every resolved or
skipped reference digest, and rejects valid consumers whose resolved source attempt is stale.
Graph-session selected read-back derives its `stale` marker from the bound consumer and source
attempt validity without rewriting the immutable reference snapshot.

`v2_goal_assessments.record_digest` is SHA-256 over Podway Canonical JSON v1 for one complete
assessment object. Its fields are `session_id`, `goal_revision`, `outcome`, `mode`,
`selected_option_id`, `route_effect`, `route_target_graph_node_id`, `decision_attempt_id`,
`decision_graph_node_id`, `decision_trace_sequence`, `actor`, `recorded_at_ms`,
`criterion_results`, and `evidence`. `criterion_results` remains in goal-definition order; each
member contains `criterion_id`, `status`, `reason`, and its citation-order `citations` array.
`evidence` remains in declaration order and contains the exact slim resolved, skipped, or unresolved
reference fields recorded by the decision. The digest binds the assessment to its ordinary decision
and complete immutable evidence snapshot without depending on relational query order.

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
- at most one active Procedure v2 attempt per session;
- at most one valid Procedure v2 attempt per graph node;
- unique Procedure v2 trace sequence and per-node attempt number;
- closed Procedure v2 lifecycle, validity, node, reference, assessment, and route values.

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
- The first migration uses the non-file fixture identity `uninitialized-database`: predecessor
  `schema-0-uninitialized` and result `schema-v1`. A new current database applies the canonical
  schema-v1, schema-v2, and schema-v3 migrations together before publication.
- Initialization and every migration MUST first apply and verify the required connection pragmas: foreign keys ON, WAL journal mode, FULL synchronous, 5000 ms busy timeout, and trusted schema OFF.
- Each migration runs inside an immediate transaction. Upgrading schema v1 directly to the current
  schema applies both later migrations and their checksum rows in one transaction.
- The same transaction verifies the predecessor version, exact schema objects, migration checksums,
  quick check, and foreign keys before applying any forward DDL.
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

Initialization from `uninitialized-database` creates a temporary database in `.podway/runtime/`,
applies and verifies the required connection pragmas, and runs the canonical schema-v1,
schema-v2, and schema-v3 migrations in one transaction. It writes workspace identity, validates
invariants, commits, fsyncs as required, and atomically installs the schema-v3 result as
`state.sqlite3`. A failure leaves an existing `state.sqlite3` intact or installs no database; it
never leaves a partial installation.

### Normal reset

Session reset runs as a destructive queue barrier. The store's revision-fenced Procedure v2 clear
helper validates and removes the complete relational current-task state; the daemon composes that
helper with old session-scoped job/idempotency cleanup and the workspace-scoped reset receipt in
the barrier transaction. Workspace identity and schema history remain intact.

### Reset all

Destructive reset closes handles, writes a filesystem marker containing idempotency and target identity, removes database and side files, creates a new schema and workspace UUID, persists the terminal reset receipt in the new database, then removes the marker. Startup completes an interrupted marked reset idempotently.

### Worktree deletion

No special database action occurs. The database disappears with the worktree and the global registry entry is removed when detected.
