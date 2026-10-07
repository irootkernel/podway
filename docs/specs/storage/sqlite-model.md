# SQLite Model

The current canonical DDL is [`sqlite-v14.sql`](../../../assets/specifications/sqlite-v14.sql).
New databases use that complete baseline. Production does not migrate generations
1 through 13; their [frozen rejected-input fixture](../../../tests/fixtures/storage/daemon-era-v2dfc/generation-1-to-13-ddl.json) remains test-only.

## Previous daemon generations

The following history describes the retired generations. The v5 contract is
normative under ADR-0021; `V2LIF-002` reserved its exact canonical DDL and
`V2LIF-003` admitted it in runtime. ADR-0025 owns schema v6 for external check
results; `V2AST-002` reserved its canonical DDL and named migration, and
`V2AST-004` admitted it in runtime.

Schema v7 is the retained inactive-session model under ADR-0026. It replaces the
singleton session ownership column with an explicit `activity` discriminator,
an optional archive slot, archive time, and archived workspace revision.
`v2_workspace_state` points to the sole current session; when no current session
exists its row is absent while inactive session rows and their dependent state
remain. A partial unique index permits at most one current session, and v7
archive slots originally constrained inactive retention to 32. Migration preserves
the predecessor session as current and infers no archive.

Schema v11, under ADR-0034, rebuilds `v2_task_sessions` so `archive_slot` stays
unique and in `1..=4294967295` without the 1-through-32 check. Existing inactive rows copy
unchanged. Archival assigns the first unused positive slot with a SQL-side gap
query. Archive list, keyed summary lookup, and open-time integrity use `COUNT`,
`LIMIT`, and aggregate checks instead of loading every inactive row.

Schema v12 adds a strict side table for one immutable delegation receipt per
attempt. Its composite foreign key ties each receipt to the owning session and
attempt, with cascade on session reset or archive purge. The receipt JSON has a
bounded byte length and cannot be updated. The migration creates only this table
and trigger; existing snapshot, item, goal, trace, and terminal envelope bytes
remain in place.
Open-time integrity checks scan all retained receipts and match each one to its
owning attempt, session, procedure snapshot, and placement declaration.

Schema v13 marks the canonical-path workspace identity generation under
ADR-0036. It adds no table or public field. During a supported v12 open, the
daemon first proves that the stored workspace UUID is unchanged and that the
stored `last_validated_root` exactly equals the freshly resolved canonical root.
The migration then rewrites the persisted common-directory and
worktree-administration hashes from legacy filesystem-object values to
role-tagged canonical-path values and records migration 13 atomically. A v13
identity mismatch is not rewritten.

Schema v8 extends terminal dispositions for ADR-0027. It adds the closed
`superseded` kind and a successor session ID. The migration preserves v7
`handed_off` and `not_required` rows byte-for-byte in their existing fields and
sets the new successor field to null. A superseded row requires reason and a
distinct successor ID; other kinds forbid that field.

Schema v9 corrects the evidence-reference key to match the Procedure v2 contract.
References remain ordered by their bounded declaration ordinal, and two references
from one consumer attempt may select different items from the same source node.
The migration rebuilds only `v2_resolved_evidence_references`, preserves every v8
row and foreign key, and replaces the source-based primary key with
`(attempt_id, reference_ordinal)`.

Schema v10 binds the workspace singleton to one bounded runtime mode. New stores
persist the daemon's exact effective mode. Versions 1 through 9 have no named-mode
identity and therefore migrate only as `prod`; a named daemon rejects them without
mutation. Every normal inspection and open verifies the persisted value before
task state is read or changed. Changing workspace config alone never transfers
ownership between daemon namespaces.

ADR-0028 itself adds no materialized condition state. Typed conditions are immutable fields
of the canonical Procedure snapshot already stored with each session, and their
evaluation uses existing attempt-local item values and selected evidence. Older
snapshots reconstruct unchanged because omitted condition fields have no
materialized defaults. Schema v9 independently repairs a pre-existing mismatch
between legal duplicate evidence references and their relational key.

Schema v6 rebuilds `v2_item_slots` so its closed item discriminator additionally
admits `check_result`. The v5-to-v6 migration preserves every existing
`value_json` byte-for-byte and does not reinterpret any existing declaration,
slot, value, attempt, or session. It applies the canonical rebuild and foreign-key
check atomically, advances `user_version` and the migration ledger together, and
rolls both back on any invalid predecessor row. Opening a database newer than the
implemented current version remains an unsupported downgrade and performs no
mutation.

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
Opening a schema newer than v13 remains an unsupported downgrade and performs no
mutation.

The daemon is the sole normal writer. Foreign keys, strict tables, application ID,
page-size and journal pragmas, migration checksums, and reconstructed Procedure v2
graph invariants are verified on open. A mismatch fails closed before serving the
workspace. Paths, JSON blobs, collections, terminal receipts, and journal retention
remain bounded by their executable contracts.

## Daemon-free Store beside the shipped daemon Store

[`sqlite-v14.sql`](../../../assets/specifications/sqlite-v14.sql) is the complete
baseline under ADR-0038, reserved in `V2DFC-003` and implemented by
`podway_store::direct_store::DirectStoreV1` in `V2DFC-005`. It is not an additional
step in the shipped v1–v13 migration chain. The shipped daemon and CLI still use
the legacy Store until `V2DFC-007`; the direct Store is independently exercised
in isolated fixtures. Daemon-era databases are rejected without writes
and require explicitly confirmed reset-all; no automatic conversion is promised.

The baseline preserves all current Procedure v2 snapshot, graph, session,
attempt, item, blocker, evidence, decision, rework, goal, criterion, assessment,
terminal-disposition and delegation-receipt constraints. Workspace identity and
its committed mutation sequence remain worktree-local. It omits jobs,
job-backed idempotency records, runtime modes, linear Procedure v1 state and the
legacy migration ledger. Journal rows retain the committed workspace sequence
without job identity. Journal pruning remains part of each mutation transaction
under the existing 10,000-row, seven-day and newest-200 bounds.

`writer_contract` records the creating or last-migrating manifest digest and
Store generation identity. A different digest is rejected unless an explicit
release-owned predecessor declaration permits forward migration. Version
`v0.2.13` declares no predecessor. `mutation_outcomes` stores only successful
`podway.output/v4` responses with a unique committed sequence, request digest,
idempotency key, retention scope and recorded time. Domain state, outcome,
journal and pruning commit atomically. Session-scoped retention ends only on
session reset or worktree reset; it has no cascading foreign key to archived
session rows. Workspace-scoped retention is at most 100 entries or 30 days.
Rejections leave no outcome. The outcome response byte bound is 1,048,576.

### Admission and write-free reads

Classification reads the database header and checksum-valid committed WAL page
one through bounded, no-follow file handles before SQLite admission. A partial
or uncommitted WAL tail cannot promote the generation. Legacy marker presence,
generations 1–13, an unknown writer, a different Git identity and a mismatched
schema fail closed before a read-write SQLite connection is opened. The frozen
generation-13 text recipe under `tests/fixtures/storage/daemon-era-v2dfc/`
preserves an actual legacy job, delegation receipt and hot-WAL replay recipe.

Ordinary operations first acquire a native shared maintenance lease, checking
the runtime directories, lock inode and all maintenance markers again under
the lease. Read-only SQLite opens use one deferred snapshot and a read-only WAL
index. When SHM is absent, the Store drains readers under an exclusive native
lease and uses SQLite's read-only `unix-flock` VFS in exclusive locking mode to
build a heap WAL index. That cold path changes no database, WAL, SHM or marker
file. Its exclusive lease prevents managed writers from racing the different
SQLite lock protocol. A bounded process-local connection gate prevents a
read-only connection from inheriting a managed writer's writable SHM mapping.

The exclusive intent lock blocks new shared entrants while existing readers
drain. Maintenance leases retain and revalidate directory and lock identities;
marker publication is no-clobber and replacement or removal requires the exact
prior document. Ordinary reads never resume a marker. Engine-owned init,
reset-all, removal and migration resumption remain `V2DFC-006` work.

### Success transactions and maintenance primitives

A direct mutation validates writer, schema and binding in a read-only snapshot
before opening its writer connection. It then uses `BEGIN IMMEDIATE`, repeating
the validation and inode checks before effects. A successful transaction reads
its injected clock once and commits relational state, canonical success,
sequence, journal, pruning and any moved-root rebind together. Original-key
replay returns the stored response before the callback or clock, including its
original response UUID. Rejections and stale prepared graph snapshots roll back
without an outcome. The Store validates persisted success headers and sequence
bounds; command-specific result validation belongs to the direct engine.

Busy waits derive from one absolute invocation deadline and use short bounded
jitter. Only an I/O error returned by the public mutation's COMMIT is classified
as an unknown mutation outcome. Earlier errors are ordinary failures. A
checkpoint attempted after COMMIT cannot change an acknowledged result.

Bootstrap constructs and checkpoints a complete private generation before
no-clobber publication. Its private transaction failure has no public unknown
outcome. The predecessor framework checks a declared predecessor's complete
schema and identity before opening a writer, applies its reviewed migration in
one exclusive transaction, and records the new writer. The production registry
is empty; isolated unit tests exercise the framework without adding a released
predecessor. Full integrity, foreign-key, graph, receipt and outcome checks run
only through explicit doctor/maintenance verification.
