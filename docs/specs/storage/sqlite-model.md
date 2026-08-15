# SQLite Model

The canonical DDL history is `sqlite-v1.sql` through `sqlite-v4.sql` under
[`assets/specifications/`](../../../assets/specifications/). New databases apply
the ordered history in one transaction and finish at schema version 4.

Schema v4 is the Procedure v2-only model. It preserves shared operational tables
for workspace identity, durable jobs, idempotency receipts, and the bounded journal.
It preserves all `v2_*` procedure snapshot, graph placement, session, attempt,
item, blocker, evidence, decision, rework, goal, criterion, and assessment tables.
It removes the former linear-procedure tables.

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
state migrates atomically to v4. The migration drops only obsolete tables and keeps
v2 snapshots, execution history, jobs, idempotency receipts, and journal rows.
Retained Procedure v2 receipts carry a durable execution flavor. Migration also
recognizes the released pre-v4 shape of an unclaimed `session.start` or
`session.start --replace` cancellation and promotes it to that explicit identity;
an otherwise ambiguous orphan receipt still fails closed as legacy state.

The daemon is the sole normal writer. Foreign keys, strict tables, application ID,
page-size and journal pragmas, migration checksums, and reconstructed Procedure v2
graph invariants are verified on open. A mismatch fails closed before serving the
workspace. Paths, JSON blobs, collections, terminal receipts, and journal retention
remain bounded by their executable contracts.
