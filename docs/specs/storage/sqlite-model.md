# SQLite Model

## Current generation

The sole production baseline is
[`sqlite-v14.sql`](../../../assets/specifications/sqlite-v14.sql), owned by
[ADR-0038](../../architecture-decision-records/0038-remove-the-daemon.md).
`podway_store::direct_store::DirectStoreV1` creates and opens this generation.
The DDL is a contract-manifest asset: changing it changes writer identity.
There is no production daemon Store, legacy migration chain, runtime-mode
column, job table, job-backed receipt, linear Procedure v1 state, or global
workspace registry.

Daemon-era generations 1 through 13 and their markers fail closed without
writes. Their [frozen rejected-input fixture](../../../tests/fixtures/storage/daemon-era-v2dfc/generation-1-to-13-ddl.json)
and populated generation-13 fixture are test input, not supported predecessors.
Explicitly confirmed reset creates a new Store without conversion or backup.
Historical migrations and their rationale remain in accepted ADRs; this baseline
does not execute them.

## Relational state

The baseline keeps the complete Procedure v2 model:

- Immutable canonical Procedure snapshots and ordered graph placements.
- One current prepared/running/completed/cancelled session, with any retained
  immutable inactive sessions and unique archive slots in `1..=4294967295`.
- One active attempt and one valid attempt per placement, trace and attempt
  counters, item slots, blockers and ordered resolved evidence references.
- Decision and rework records, workflow memory through retained attempt state,
  goal revisions, criteria, results, citations and immutable assessments.
- Terminal dispositions tied to an exact terminal session revision, including
  handed-off, not-required and superseded successor identity.
- One immutable delegation receipt per attempt, with a 45,000-byte JSON bound
  and composite session/attempt foreign key.

A prepared session has revision zero, no trace, active cursor, goal, attempts or
dependent execution records. Begin creates its first attempt and any goal in
one transaction. Running sessions have exactly one active attempt. Terminal
sessions have no active cursor. Inactive sessions are terminal and have archive
metadata; SQL partial unique indexes enforce the current-session and active
attempt uniqueness boundaries. Archive allocation and lookup use bounded
SQL-side selection rather than loading every retained session.

Strict tables, foreign keys, discriminator checks, immutable receipt triggers
and domain reconstruction retain these invariants. Ordered evidence references
use `(attempt_id, reference_ordinal)` so separate references may select different
items from the same source placement. Typed conditions remain immutable snapshot
declarations; no materialized condition evaluator or mutable default is added.

## Workspace and writer identity

`workspace_state` holds the worktree-local UUID, role-tagged canonical Git common
and administration path fingerprints, last validated root, committed mutation
sequence and timestamps. Canonical-path identity follows
[ADR-0036](../../architecture-decision-records/0036-use-canonical-git-paths-for-workspace-identity.md).
A supported linked-worktree root move preserves those Git identities and rebinds
the root only in a successful write transaction. UUID or canonical Git identity
mismatch fails closed. No global registry can arbitrate another copy's UUID.

The singleton `writer_contract` holds the creating or last-migrating manifest
digest, generation identity and recorded time. Normal reads and writes require
the current binary's manifest digest. A mismatch returns
`WORKSPACE_CONTRACT_MISMATCH` unless the compiled release-owned predecessor
registry explicitly declares a reviewed forward migration. The current registry
is empty; the cutover declares no daemon-era predecessor. Only a release or its
following open-development commit may add a predecessor. Feature work and a
reduced metadata-only patch gate cannot do so.

A declared migration verifies the complete predecessor schema and identity
before opening a writer, holds the exclusive native lease, applies the reviewed
migration and writer update atomically, then validates the successor. The
framework's isolated tests do not establish a released predecessor or automatic
daemon-era migration.

## Success outcomes and journal

`mutation_outcomes` stores only successful canonical `podway.output/v4`
envelopes, keyed by idempotency key, with request digest, command name, unique
committed workspace sequence, retention scope and recorded time. Its response
bound is 1,048,576 bytes. It stores no rejected-request or queued/running state.
Engine owns command-specific public result validation; Store validates retained
success headers, identity, digest and sequence consistency.

A session outcome has no cascading foreign key to archived session rows. It
survives archive purge until the relevant session reset or worktree reset.
Workspace outcomes retain at most 100 entries and no entries older than 30 days,
pruned during successful mutations. The journal retains committed sequence,
bounded event name/summary and optional bounded details without job identity.
Its pruning preserves the newest 200 rows, otherwise removing rows older than
seven days or outside the newest 10,000. State, success, journal, sequence,
pruning and root rebind commit together.

## Classification and write-free reads

Classification reads the SQLite header and checksum-valid committed WAL page one
through bounded no-follow handles, before any read-write SQLite open. A partial
or uncommitted WAL tail cannot promote generation. Legacy markers, generations
1–13, unsupported newer state, unknown writer, schema drift and foreign Git
identity refuse ordinary reads and writes without modifying legacy bytes or
creating SHM.

Every ordinary open acquires and revalidates its native maintenance lease and
current runtime-directory, lock and database identities. A normal read uses one
deferred snapshot with a read-only WAL index. If hot WAL exists without SHM, the
Store drains managed readers under an exclusive native lease and uses SQLite's
read-only `unix-flock` VFS in exclusive locking mode to build a heap WAL index.
This changes no database, WAL, SHM or marker file. The exclusive lease prevents
managed writers from racing that different SQLite locking protocol. A bounded
process-local connection gate prevents a read-only connection from inheriting a
managed writer's writable SHM mapping.

These paths verify pragmas, binding, schema-object inventory and writer identity.
They do not run `quick_check`, `foreign_key_check`, full graph reconstruction or
all-receipt scans on every invocation. Explicit doctor and maintenance perform
those stronger checks. Ordinary reads never resume a maintenance marker.

## Mutations and maintenance

Before opening a writer, the mutation path verifies the current Store through a
read-only snapshot. Its `BEGIN IMMEDIATE` repeats writer/schema/binding/inode
validation, checks exact replay before callback or clock, and revalidates prepared
inputs. Relational persistence writes changed state rather than rewriting the
entire successor. Success retains the original response UUID for replay.

SQLite uses WAL and `synchronous=FULL`, enables foreign keys, disables trusted
schema and verifies schema objects. Ordinary admission does not verify page
size or application ID. Maintenance, connection-gate and SQLite admission waits
use one invocation deadline. An exhausted budget still permits one immediate
attempt at each boundary; retries and sleeps consume the remaining budget.
Successful acquisition does not impose a timer on snapshot callbacks, integrity
scans, bootstrap or COMMIT. Read admission and connection-gate exhaustion have
distinct `WORKSPACE_BUSY` boundaries; a read never reports a SQLite write lock.
Only an I/O
failure at the public mutation's COMMIT is an unknown outcome. Earlier failures
roll back; post-commit checkpoint failure does not alter acknowledged success.

Bootstrap constructs, checkpoints and syncs a complete private generation before
no-clobber publication under the exclusive lease. Its private transaction failure
has no public unknown outcome. Native intent and lock identities fence init,
reset-all, removal, doctor and declared-predecessor maintenance. Engine owns published init/reset/removal operation identity and matching
recovery; Store owns safe filesystem and transaction primitives. The empty
predecessor registry exposes no current Engine migration route; a future declared
release must supply the corresponding exclusive recovery path. See [Recovery](recovery-retention-and-maintenance.md)
and [Transactions](transactions-concurrency-and-idempotency.md).
