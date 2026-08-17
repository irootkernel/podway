# SQLite Model

The canonical DDL history is `sqlite-v1.sql` through `sqlite-v5.sql` under
`assets/specifications/`. New databases apply the ordered history in one
transaction and finish at schema version 5. The v5 contract is normative under
ADR-0021; `V2LIF-002` reserves its exact canonical DDL and `V2LIF-003` admits it
in runtime.

Schema v4 is the Procedure v2-only model. It preserves shared operational tables
for workspace identity, durable jobs, idempotency receipts, and the bounded journal.
It preserves all `v2_*` procedure snapshot, graph placement, session, attempt,
item, blocker, evidence, decision, rework, goal, criterion, and assessment tables.
It removes the former linear-procedure tables.

Schema v5 is the prepared-session lifecycle model. It rebuilds
`v2_task_sessions` so the lifecycle discriminator admits `prepared` and the
session revision admits zero only for that state. A prepared row has revision 0,
latest trace sequence 0, null active cursor, null current goal revision, and null
terminal timestamps and reason. No attempt, item, blocker, goal, decision,
rework, assessment, or disposition row may exist for a prepared session.

Schema v5 adds `v2_terminal_dispositions`, keyed by session ID and positive
terminal session revision. The row stores `handed_off` with non-null summary and
reference or `not_required` with non-null reason, plus optional actor and recorded
time. The opposite-kind fields are null. The session foreign key cascades on
reset, so disposition history remains session-scoped. A disposition is current
only when its revision equals the current completed or cancelled session
revision; reactivation retains the older row but makes it non-current.

Opening a schema 1, 2, or 3 database first verifies the predecessor migration
history and integrity. If any legacy procedure snapshot, session, stage, attempt,
item, blocker, or session-scoped legacy job exists, migration rolls back without
changing `user_version`, migration rows, or task data and returns
`LEGACY_PROCEDURE_STATE_UNSUPPORTED`. Podway never converts or deletes that state.
After the owner backs it up, `podway reset --all` is the supported clean recovery.
Reset may inspect only the validated predecessor workspace binding from such a
database to disambiguate stale same-root registry metadata. This read-only identity
inspection does not migrate, accept, open, convert, or delete the legacy task state.

An empty predecessor or one containing only Procedure v2 and shared operational
state first migrates atomically to v4. The migration drops only obsolete tables
and keeps v2 snapshots, execution history, jobs, idempotency receipts, and
journal rows.
Retained Procedure v2 receipts carry a durable execution flavor. Migration also
recognizes the released pre-v4 shape of an unclaimed `session.start` or
`session.start --replace` cancellation and promotes it to that explicit identity;
an otherwise ambiguous orphan receipt still fails closed as legacy state.

The v4-to-v5 migration is transactional. It preserves every existing v2 session
as running, completed, or cancelled with identical identity, revision, cursor,
trace, goal, terminal timestamps, attempts, and dependent records. It creates no
prepared session and infers no terminal disposition. Rebuilding the constrained
session table preserves referenced bytes and foreign-key relationships before
the old table is removed. Any row that fails v4 reconstruction or v5 constraints
rolls the migration back without advancing `user_version` or its migration row.
The migration driver disables foreign-key enforcement before opening that
transaction, applies the canonical v5 DDL with legacy rename behavior, runs
`foreign_key_check` before commit, and restores enforcement after commit or
rollback. The DDL does not toggle `foreign_keys` because SQLite ignores that
pragma inside an open transaction.
Opening a schema newer than v5 remains an unsupported downgrade and performs no
mutation.

The daemon is the sole normal writer. Foreign keys, strict tables, application ID,
page-size and journal pragmas, migration checksums, and reconstructed Procedure v2
graph invariants are verified on open. A mismatch fails closed before serving the
workspace. Paths, JSON blobs, collections, terminal receipts, and journal retention
remain bounded by their executable contracts.
