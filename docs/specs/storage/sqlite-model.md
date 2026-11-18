# SQLite Model

## Current generation

The sole production baseline is
[`sqlite-v15.sql`](../../../assets/specifications/sqlite-v15.sql), owned by
[ADR-0038](../../architecture-decision-records/0038-remove-the-daemon.md).
`podway_store::direct_store::DirectStoreV1` creates and opens this generation.
The DDL is a contract-manifest asset: changing it changes writer identity.
Generation 15 adds a retained runtime-directory exclusion anchor through removal
and accepts exact recovery from a lone matching removal marker. Generation 14
was the unreleased direct baseline; this change declares no predecessor or
automatic migration. Generation 14 is refused unchanged through the older-state
`LEGACY_RUNTIME_STATE_UNSUPPORTED` contract with an explicitly confirmed reset
recipe. Public older- and newer-generation bounds are checked against the
Store constant and instantiated DDL by the standard contract gate.
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

## Execution-group candidate Store

[Generation 16](../../../assets/specifications/sqlite-v16.sql) is the canonical
unreleased execution-group layout. The test-support constructor creates fresh
fixture files with an exact DDL-bound writer identity. The development manifest
continues to select generation 15 for the public Procedure v2 runtime. The
candidate declares no predecessor. Its fixture wrapper records a workspace
UUID and owning root separately from successor domain state. The workspace
sequence, operational journal and mutation outcome tables use the Direct
transaction layout. Command outcomes carry the exact session scope.

An immutable canonical definition is keyed by its digest. Each session retains
its base, ordered activations, scoped attempt trace, and successive goal
revisions. A group activation carries its exact design attempt, plan item
bindings, predecessor, prior-work references, outcome and selection closure.
Attempt rows retain final item slots and revisions, blockers, decisions,
timestamps, and scoped delegation metadata. A guarded replan retains the
completed Decision with the selected option and exact source attempt and
item bindings. Its selection closure names that Decision as the source, and
the next attempt opens at the base anchor. Completed and partial attempts
remain in the trace. Repeated activation can reuse definition bytes while
keeping separate execution IDs and attempt histories.

SQLite enforces composite session and activation relationships. On cold read,
the Store verifies schema and writer identity, canonical bytes, row checksums,
scope relationships, and item value digests. The definition-admitter callback
must fully validate the supplied canonical bytes and return the corresponding
typed definition. The candidate integration harness supplies Config's successor
parser and validator. Core reconstructs the cursor and decision counts using
the original transitions and final item states. The retained trace must match
the recovered outcomes, closures, guards, and historical references. Blocker
IDs are unique across the trace and each attempt orders its blockers by
creation time and ID.

Cold admission checks the stored session revision against the cursor
transitions and the minimum possible item writes in the retained trace. One
atomic batch can increment several distinct slots in one attempt within one
session revision. The highest slot revision in each attempt therefore supplies
that attempt's write lower bound. Final slots preserve exact item revisions
without retaining the membership of each historical batch.

### Candidate group lineage reads

The candidate Store selects an exact current or archived session and admits its
state inside a single read transaction. The Engine lists groups in activation
order with their definition, design attempt, predecessor, outcome and closure
cause. Group detail returns the bound plan and working-copy basis, terminal
attempt, closure feedback and a page of Decision entries in trace order. Each
entry identifies its scoped attempt, ordinal, recorded option and reason,
applicable goal assessment and exact guard bindings. The Decision retains
the scoped attempt for each declared evidence reference at selection time,
including references used by an unguarded option. Cold replay verifies the
source list. History reads combine that list with the retained definition and
immutable item slots, so later rework cannot switch an earlier Decision's
assessment to a different source. Choice, integer and confirmation values are
readable alongside their item bindings. Counts include open Decision entries
and entries abandoned before an option was selected.

Goal-revision history is a separate page in goal-revision order. It exposes the
exact predecessor group when one exists, closure source and successor
attempts, cause, timestamp and feedback. It preserves changes made without an
active selection, including a revision after replan or reactivation after
completion. The original group's closure remains visible in group detail.

Pages return exact totals, truncation and a continuation when rows remain. A
page holds up to 64 rows within the direct response byte and depth limits.
Continuations bind the query kind, workspace, exact session and location,
session revision, group execution and definition when selected, and offset.
A different binding returns a stale-token error. Read admission and paging
use the stored definition and trace, independent of outcome and journal rows.

The isolated fixture may archive a terminal session at its expected revision.
Archival marks the stored session without rewriting its retained execution data.
Current snapshots and fresh mutations reject archived sessions. Exact mutation
replay remains available through the existing outcome path. Public archive
cutover and destructive lifecycle qualification have separate roadmap owners.

### Candidate DAG, attempt and item history

An exact base definition digest or group execution ID with its definition
digest selects a historical DAG. The Config projection uses the admitted
bytes retained in SQLite. Canonical DAG pages reconstruct the entire
projection and its digest, including graphs larger than the unpaged
authoring response limit. Two executions of one definition remain distinct
selectors.

The same scope selects attempt summaries in original trace order. Each row
includes its absolute trace sequence, lifecycle and validity, timestamps,
retained decision or outcome facts, declared item identities and revisions,
and the retained blocker count. Blocker pages select the exact scoped attempt
and give each record's identity, state, reason and timestamps in retained order.
Resolved records remain pageable when the total exceeds the open-blocker limit.
Item reads additionally require the exact scoped attempt and item.
They expose the retained item revision and value digest, with a declared
unrecorded slot distinguishable from a recorded empty value.

Text and canonical DAG pages advance by Unicode scalars; list pages advance
by whole entries. Other typed values return as one complete record. Each
page reports its logical offset, size, exact total and continuation. Encoded
data fits within 256 KiB and the complete candidate response within 320 KiB.
Continuation identity includes the query, owning session and revision, and
the selected scope and exact definition, projection or value identifiers.

An artifact page returns its original location, digest, byte length and
media type. Local availability uses a no-follow metadata observation beneath
the fixture root. A missing path is distinguishable from an unverified path.
Unsafe, inaccessible and opaque external locations are unverified. This
observation reads metadata only and does not certify content. Missing bytes
never invalidate the retained reference.

### Candidate prior-work history

A prior-work query selects a retained activation by exact group execution and
definition digest, in the requested current or archived session. Every page
identifies that activation's scoped design attempt, immutable plan binding,
goal revision when tracked, and caller-supplied working-copy basis. Its latest
predecessor is independent of the disposition sources, which may point to an
earlier execution.

The first activation has an empty page. Later pages preserve the admitted
disposition order, choice and rationale. Each row identifies its earlier group
execution and definition, and adds a scoped attempt or exact item revision and
value digest when cited. Existing decision, attempt and item queries resolve
those links. Pages use the shared history row and response bounds.
Continuations bind the query kind, workspace, exact session ID, session
location and revision, target group execution, definition digest and offset.

The generation-16 record encodings are private Store codecs. The successor
component schemas describe the separately owned wire contracts. The candidate
checks retained row ordinals and sizes before decoding record payloads. Its
2 MiB activation-row limit accounts for the escaped bytes of up to 128
prior-work rationales with 2,000 scalars each, their retained references, plan
bindings and closure metadata. The group activation issuer still applies
the existing 1 MiB direct-input bound to canonical request content. The private
retained-record cap supports SQLite persistence independently of the request
and response bounds. A row checksum provides an integrity check within
the existing same-user trust boundary.

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

The `bug-fix-v2` version `4` reproduction-prompt update changes the manifest even
though Store generation and layout remain unchanged. The candidate therefore
refuses Stores carrying the preceding manifest on normal reads, writes and
original-request replays with `WORKSPACE_CONTRACT_MISMATCH`. This applies to
sessions using unchanged presets as well. Their snapshots remain immutable;
that does not promise transparent continuation with a new manifest. Verify the
candidate in freshly initialized isolated fixtures. No migration, predecessor
declaration, forced reset or installed-runtime update is part of this change.

Integration settings do not participate in writer, Procedure snapshot, session
or receipt identity. Matching-manifest Direct operations remain HOME-independent
with either valid project permission; an undeclared previous manifest still
refuses normal reads, writes and retained replay. Only the explicit Git/config
settings query bypasses Store/maintenance admission, including beside a refused
writer. Configuration setup cannot manufacture compatibility or a predecessor.

A declared migration verifies the complete predecessor schema and identity
before opening a writer, holds the exclusive native lease, applies the reviewed
migration and writer update atomically, then validates the successor. The
exclusive lease and exact database witness remain held through a bounded
TRUNCATE checkpoint and sync, so the main file carries the successor schema,
writer identity and `user_version` before completion. The
framework's isolated tests do not establish a released predecessor or automatic
daemon-era migration.

## Success outcomes and journal

`mutation_outcomes` stores only successful canonical `podway.output/v4`
envelopes (including opt-in `podway.mutation-observation-result/v1` envelopes on
the 6 supported `--with-observation` routes), keyed by idempotency key, with
request digest, command name, unique committed workspace sequence, retention
scope and recorded time. Its response bound is 1,048,576 bytes, and every
`podway.mutation-observation-result/v1` envelope is additionally checked before
commit for worst-case `outcome.lookup` headroom (`<= 1,048,576` outer bytes and
stored envelope depth `<= 61` so the outer `outcome.lookup` envelope stays at
depth `<= 64`). It stores no rejected-request or queued/running state.
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
or uncommitted WAL tail cannot promote generation. A validated committed newer
page-one prefix refuses even with a missing main file or an oversized tail; a
later downgrade does not grant an older writer destructive authority. The bounded
scan refuses an unresolved valid stream rather than falling back to an older
main generation. Marker diagnosis never substitutes for destructive generation
admission, which independently revalidates the main/WAL file identities.
Legacy markers, generations
1–13, unsupported newer state, unknown writer, schema drift and foreign Git
identity refuse ordinary reads and writes without modifying legacy bytes or
creating SHM.
Newer committed generations take precedence over legacy/maintenance markers and
invalid configuration, including for reset and removal. They refuse before any
lock creation, with actual/supported generation and compatible-Podway guidance.
Changing a maintenance lock or marker protocol requires a Store generation bump;
an unchanged protocol does not.

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

A successful classification and a failed probe remain distinct in Engine
configuration diagnostics. Both callers prefer an observed newer generation or
maintenance classification over invalid configuration. Init propagates probe
errors; existing-workspace resolution retains its configuration error on any
probe failure, including a changed-witness maintenance error. This precedence
shares the original bounded native observation without a SQLite open or new
lease, sidecar or marker.

Unknown committed page-one generation and an unresolved valid WAL stream at the
scan boundary remain destructive-admission failures even when a marker is
present. Supported main-file generation, force, confirmation and writer-version
diagnosis do not establish the WAL generation. Existing invalid-current recovery
advice applies only after generation admission; indeterminate input stays
preserved without a deletion recipe or an inferred UUID.

## Mutations and maintenance

Before opening a writer, the mutation path verifies the current Store through a
read-only snapshot. Its `BEGIN IMMEDIATE` repeats writer/schema/binding/inode
validation, checks exact replay before callback or clock, and revalidates prepared
inputs. Relational persistence writes changed state rather than rewriting the
entire successor: for `goal.assess_criteria`, all 1 to 16 newly recorded
`v2_criterion_assessment_results` rows and their ordered `v2_criterion_citations` rows on
the active `goal_assessment` attempt commit atomically in that single write
transaction without a schema generation bump, and any validation or successor
admission failure rolls back the complete batch. Success retains the original
response UUID for replay.

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

[Integrated maintenance qualification](../../implementation-tips/testing.md#maint-integrated-qualification)
covers native generation/witness refusal, checkpoint/sync durability and matching-
writer normal-flow continuity through the existing owners. A rejected or mismatched
Store is not reset or migrated to make that continuity check pass.
