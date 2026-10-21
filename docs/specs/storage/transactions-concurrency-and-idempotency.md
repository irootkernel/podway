# Transactions, Concurrency, and Idempotency

## Concurrency model

Every command runs in its invoking process. `podway-engine` prepares and executes
commands; `podway-store` arbitrates process concurrency through native maintenance
leases and SQLite. There is no admission queue, durable job, scheduler, or FIFO
fairness guarantee. At most one SQLite writer commits in a worktree at a time.

Three identities have different meanings:

1. The committed workspace sequence orders successful mutation outcomes. It is
   allocated inside the success transaction, never at submission.
2. The session revision fences cursor and session changes.
3. The item revision fences a particular item on a particular attempt.

A read observes committed state. No acknowledged mutation remains pending.

## Preparation outside the state transaction

Procedure parsing, semantic validation and canonicalization, artifact hashing,
and graph-dependent preparation run without holding the write transaction.
Preparation is side-effect-free. Start commits the canonical Procedure snapshot
with the prepared session; the explicit `begin` operation later enters the graph.
A custom Procedure start requires its canonical digest confirmation.

The Engine revalidates the relevant session, attempt, goal and item identities,
Procedure digest, graph preparation and artifact metadata at the transaction
boundary. A changed workspace sequence alone is not a caller conflict. The Store
reuses an unchanged prepared graph or loads transaction-current state, then the
Engine evaluates the command's own fences and start policy against that state.
Unrelated item commits preserve valid item fences; a stale fence reports the
current revision. A plain start that loses a race reports the current session's
start decision instead of an internal error. Changed artifact or Procedure inputs
still fail closed, and native resolution failures retain their typed errors.
Preparation does not reserve a queue position or promise an eventual execution.
An exact successful start replay can recover its original pinned
source digest from the stored outcome; it does not require the original file to
remain available.

## Successful execution transaction

The ordinary mutation path is:

```text
classify Store without writes
prepare side-effect-free inputs
acquire shared maintenance lease and validate current paths/inodes
validate writer, schema and Git binding in a read-only snapshot
BEGIN IMMEDIATE within the invocation deadline
  validate writer, schema, binding and file identities again
  resolve the canonical request identity
  lookup idempotency key
  if success exists:
    reject a different digest, otherwise return the exact stored success
  validate command fences and prepared inputs
  read the injected clock once
  apply the pure domain transition
  persist only changed relational state
  allocate the committed workspace sequence
  persist canonical success, journal and retention pruning
  persist any supported moved-root rebind
COMMIT
print the validated success envelope
```

State, `mutation_outcomes`, sequence, journal, pruning and root rebind are one
atomic `BEGIN IMMEDIATE` transaction. Persisting a successor must be bounded to
changed rows, while preserving complete domain validation. New-session creation
and destructive maintenance have the separately guarded publication protocols
in [Recovery](recovery-retention-and-maintenance.md).

Post-commit checkpointing is best effort and does not change a committed result.
A passive checkpoint never waits. An oversized WAL gets a bounded truncation
attempt only after the passive result reports every frame copied; a pinned older
reader otherwise would make repeated truncation waits contend with writers.

An existing matching outcome is returned before the transition callback or
clock runs. It preserves the original `request_id`, timestamps, workspace
sequence, result and complete `podway.output/v4` envelope. Replaying a committed
request uses its original fences and key; replacing them changes the canonical
request. The caller's expected workspace UUID is still checked against the
actual Store before replay, so a wrong workspace cannot recover another Store's
success.

## Rejections and no-ops

A malformed request, stale fence, domain rejection, failed prepared-input check,
or pre-COMMIT infrastructure failure rolls back. It records no successful
outcome, allocates no committed sequence and leaves no partial item, session,
receipt or journal change. No failed job or persistent rejected-request receipt
exists. A rejected request may be reconsidered later, but had no prior effect.

A semantic no-op succeeds with `changed=false`; item and session revisions remain
unchanged. It still commits one successful outcome and workspace sequence, so
replay returns the identical response without another effect.

## Cursor concurrency

Complete, skip, retry, block, unblock, decide, rework and cancel use the command's
session-revision and active-attempt fences. A caller cannot adapt a request meant
for one placement to another placement after contention. Goal-sensitive commands
also validate the expected goal revision. Refresh observation and choose a new
request only after reconciling the original key when its outcome is uncertain.

## Item concurrency

Ordinary item commands validate active attempt and expected item revision.
Different item updates may succeed across intervening session-revision changes;
a stale update to the same item fails with `ITEM_REVISION_CONFLICT`.

```text
attempt A active; x=revision0; y=revision0
set x expecting0 -> x=revision1 and session revision advances
set y expecting0 -> y=revision1 and session revision advances
set x expecting0 -> ITEM_REVISION_CONFLICT, no effect
```

### Atomic multi-item concurrency

`item.record_many` fences the observed session revision and active attempt, and
its 1 to 128 unique operations each fence their item revision. The Engine
normalizes operations into item-ID order. All types, constraints, prepared
artifacts and revisions must validate against one current state. One invalid
operation leaves every target unchanged.

The changed rows, item-ID-ordered result, success outcome and journal commit
together. Typed in-process calls and JSON stdin both enforce the named aggregate
input bound of 1,048,576 bytes; avoiding IPC does not bypass that budget. A
post-state/pre-outcome failure rolls the transaction back. An exact retry returns
one stored success.

## Supervised delegation

Acceptance verifies declaration, current package binding, trusted expected
package digest, current item-state digest, supplied execution facts and ordinary
action gates before storing one immutable attempt receipt. Engine supplies the
package projection and trusted digest; it neither invokes an executor nor judges
external semantic truth. Completion verifies that receipt inside the same
transaction that advances the cursor and stores success.

A receipt is current only for its workspace, session, attempt, placement,
Procedure and declaration binding. Actual item changes, including optional items
or edits later reverted, stale the accepted item-state digest. Blocker-only and
session-revision-only changes do not. Missing or stale receipts leave the cursor
and outcome unchanged. See [Supervised Delegation](../interfaces/supervised-delegation.md).

## Artifact completion race

For attach, record, complete and decide, resolve and hash local artifacts before
writer admission. Capture each file's device/inode identity, size and nanosecond
modification/change times before streaming, and require the same stamp afterward.
A change while hashing returns `ARTIFACT_CHANGED`.

Inside the write transaction, read no artifact bytes. Reobserve each prepared
stamp through the same no-follow descriptor chain and validate the active attempt,
item revisions, recorded size/digest, required-artifact set and workspace/Git
identity against preparation. A mismatch returns `ARTIFACT_CHANGED` without
an effect. Caller fences retain their ordinary typed rejection ordering. Consume
captured preparation failures only after authoritative successful-outcome replay
and caller fences, so an exact replay does not depend on today's file.

The filesystem may change afterward; Podway is not a security boundary against
same-user processes and retains metadata rather than artifact bytes. The separate
deep Doctor check retains its exclusive integrity-lease verification.

## Session reset, archive and replacement

Session reset and start replacement are ordinary single-transaction mutations,
with explicit lifecycle eligibility and required confirmation. They atomically
remove or archive the old current state and create the prepared successor when
requested. No queue barrier is needed. Workspace-scoped outcomes survive session
reset. Session-scoped outcomes are removed by reset of that session or worktree
reset; they survive archive and archive purge. Archive rows cannot be restored
or mutated. See [Retention](recovery-retention-and-maintenance.md#retention-policy).

## Read consistency

A normal read uses one deferred SQLite WAL transaction and returns a coherent
committed snapshot with its committed workspace sequence. It performs no row,
root-binding, migration, marker or WAL/SHM writes, and never drains queued work.
Normal reads do not resume interrupted maintenance. Maintenance exclusion can
require bounded lock waiting; cold hot-WAL/no-SHM reads use an exclusive native
lease and a heap WAL index to preserve write-free reads. There are no
`--after-job` or `--wait-for-idle` read modifiers.

## Exact-once effect, not exactly-once output

The guarantee is one logical effect per retained idempotency key and canonical
request. Printing stdout is outside the SQLite transaction; a process can die
after commit and before output. Automation must retain a caller-owned
`--idempotency-key`; a generated key is unavailable after such a crash.

`podway outcome lookup --idempotency-key <key>` is read-only. It returns the exact
stored success or `found=false`, never replays a request. Not-found proves no
retained committed effect in that observed snapshot within the retention
boundary. It does not prove that a pruned older outcome never existed or prevent
another caller from subsequently committing. Session reset and worktree reset
end the applicable retention boundary.

`MUTATION_OUTCOME_UNKNOWN` is limited to an I/O error returned by the public
mutation's COMMIT. Reconcile the exact key through outcome lookup before retry.
Pre-COMMIT failures have no effect; a checkpoint attempted after COMMIT cannot
turn an acknowledged success into failure or unknown outcome.

## Crash outcomes

| Boundary | Durable state and recovery |
|---|---|
| Before COMMIT, including preparation or changed-row writes | No success outcome; SQLite rolls back; retry with the same request can execute |
| After COMMIT, before stdout | State and complete success retained; lookup or exact retry returns original success |
| Public COMMIT returns an I/O error | Outcome may be unknown; read-only lookup reconciles the caller's exact key |
| Interrupted init/reset-all/removal | Durable maintenance identity refuses ordinary commands; only its supported matching recovery proceeds |

## Busy and lock behavior

`--timeout` supplies one absolute deadline for maintenance, process-local
connection exclusion and SQLite admission waits; the default is five seconds.
Preflight, preparation and human confirmation consume this same budget. Each
boundary makes one non-blocking attempt even with no remaining budget. Only
retries and sleeps check the deadline; callbacks, doctor, bootstrap and COMMIT
continue after successful acquisition. SQLite contention uses short bounded
waits with jitter. `WORKSPACE_BUSY` identifies `connection_gate`,
`sqlite_read_admission` or `sqlite_write_lock`; reads never report a writer lock.
Maintenance lease contention or a published marker returns
`WORKSPACE_MAINTENANCE`. Failed admission leaves no partial state or outcome. Contention is an
ordinary consequence of independent processes and does not establish a disk
fault or an internal daemon defect. There is no automatic replay after an
ambiguous COMMIT.

### Request-owned prepared value reuse

A prepared graph ordinarily belongs to one request and its Store/root/writer
identity. An opaque Engine invocation context may retain at most one immutable
Store-prepared graph from successful full Standard preflight for one later
eligible mutation in the same CLI invocation. Retention uses only the already
qualified generation, binding, admitted graph, committed sequence and Store-owned
root/manifest. It adds no SQL, decoding or validation stage and holds no live
guard. Missing retention metadata leaves successful facts unchanged. The context
is populated only after the preflight read's final witness succeeds; a new
preflight clears previous state, and an eligible mutation clears any unconsumed
hint on every exit. Dry-run and read-only calls neither consume nor produce hints.

The mutation consumes the hint only inside its existing qualified preparation
read. Store compares root, manifest digest, generation, workspace UUID, committed
sequence, session ID and revision, workspace revision, Procedure snapshot
identifier and digest. A complete match permits graph-body reuse with a new
preparation-stage stamp. Any optional comparison mismatch or read/decode failure
discards the hint once and runs ordinary cold preparation in the same snapshot.
Mandatory admission, file witnesses and cold-path failures retain their existing
errors and order; no authoritative failure is caught or retried by hint handling.
Stateless callers retain cold preparation. No lease spans mutation artifact work,
prompts or the competing-writer gap.

Matching stamps do not prove unchanged row bytes following out-of-protocol
database modification, which is an [explicit limitation](../operations/security-and-trust.md#explicit-limitations).
Matching-hint reuse may defer detection until a later cold read or integrity
check. File witnesses establish object identity, not unchanged row content.
Writer replay, identity refusal and reload remain unchanged and use the value
stamped at preparation, preserving the preparation-to-writer identity window.
Receipt readiness keeps its independent acquisition.

When another compatible writer advances the sequence, transaction admission
reloads the current cursor, revisions, attempt metadata, blockers, evidence and
goals. It may reuse immutable parsed Procedure data only for the same session,
snapshot identifier and digest. It may reuse a decoded item value only when its
item identity, type, revision and presence match the current row. Compatible
writers keep creation metadata immutable and advance the item revision on every
value or update-time change; those unchanged slot timestamps belong to the same
revision. Changed or new rows are decoded again; normal cold reads and integrity
checks decode persistent values. The cache does not become durable authority.

Item values are immutable and shared between typed state copies, so transitions
do not copy completed-attempt payload bytes. They reuse the parsed memory
declarations belonging to the immutable Procedure snapshot. A successful value
admission is reusable only for the exact immutable value and item declaration.
Cold reads decode slot values in their metadata query; a stale prepared graph reloads only values
whose authoritative slot stamps changed. Reset, archive and replacement use
the transaction's already selected current graph; they keep revision checks and
avoid a second full reload under the writer lock.
