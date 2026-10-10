# Recovery, Retention, and Maintenance

## Recovery philosophy

Podway manages disposable task state in the owning Git worktree. SQLite rollback
and retained success outcomes recover ordinary command interruption; native
maintenance markers recover multi-step filesystem operations. There is no
resident process, global registry, restart scanner, scheduler or durable job.
Podway does not provide a backup, export, restore or forensic recovery product.

Unreadable, foreign-writer, newer or legacy state fails closed. A recovery recipe
is advice, not permission: destructive reset or removal still requires explicit
authority and confirmation. Podway never discards or converts such state during
an ordinary read or mutation.

## Daemon restart

This heading preserves historical roadmap links. Daemon restart and running-job
requeue belonged to the superseded daemon architecture. Current commands have no
restart recovery service. [ADR-0038](../../architecture-decision-records/0038-remove-the-daemon.md)
owns the cutover, and the sections below describe the replacement behavior.

## Ordinary command interruption

Before COMMIT, a failed or killed mutation has no successful outcome and its
SQLite writes roll back. After COMMIT, state and the full canonical success are
retained together even if stdout was lost. Read-only
`podway outcome lookup --idempotency-key <original-key>` or an exact request retry
returns the original success without another transition or clock read.

Automation retains its original caller-owned key and request fences. Not-found
is scoped to one observed committed snapshot and the retention boundary; it
cannot establish that a pruned older success never existed.
`MUTATION_OUTCOME_UNKNOWN` is limited to a public COMMIT I/O error. Lookup
reconciles that uncertainty; there is no queued/running/admission status to poll.

## Maintenance locks and markers

Runtime coordination is local to `.podway/runtime/`:

- The runtime directory inode: exclusively locked by maintenance before named
  locks may be created, and retained through final removal even after those
  names are unlinked. Empty-residual removal obeys the same directory exclusion.
- `maintenance.lock`: shared for ordinary opens, exclusive for destructive
  maintenance, creation, doctor and declared predecessor migration.
- `maintenance-intent.lock`: exclusive entrants take it before draining shared
  leases, preventing an endless stream of new readers from starving maintenance.
- `maintenance-intent.json`: durable operation identity during publication and
  reset cleanup, distinct from the native intent lock. Removal deletes this intent
  before its final operation marker, which then retains the exact-retry identity.
- `reset-all.marker` and `removal.marker`: the corresponding operation's
  resumable phase and identity.

Maintenance-lock contention uses read-only `session.observe` after the owner
finishes when a Store is present. Store-less contention during final removal or
empty-residual cleanup instead
emits `version --identity`, which works without a Store; wait for the owner and
retry the original request with its key and fences. If removal finds its parent
directory repopulated, it refuses cleanup and directs the caller to preserve the
current contents and inspect the writer and workspace before any new request.

Lock holders revalidate directory, lock and database path/inode identity after
acquisition and before effects; a replaced or unlinked anchor is not authority.
Marker publication is atomic and no-clobber. Replacement/removal requires the
exact previous marker document. Marker identities bind the original key,
canonical request digest, response request ID, writer contract, canonical root
and Git identities, time, command and applicable workspace UUID.
Reset and removal markers also retain a closed `request` object containing the
original asserted `workspace_uuid` (nullable) and semantic `force` value. Reset's
top-level `workspace_uuid` identifies the new generation; it is not the original
request fence. Expected and selector-derived UUID assertions normalize to one
request fence and must agree when both are supplied. The canonical digest binds
that fence and force value; continuation recipes reconstruct those parameters
without adding or dropping a fence or changing removal force. Typed removal
requests without the UUID fence or force required by the JSON CLI retain typed
exact retry and receive a human-terminal removal recipe with the original key
and flags. An unfenced recipe omits `--json` and `--yes`, requiring exact-path
confirmation. It does not add force or a UUID assertion.

The error boundary follows the observed marker kind: an intent-only window
reports `intent_marker`, independently of the requested command. Interrupted
initialization gives its original-key `init` recipe, preserving `--repair`. A
foreign writer or root remains `WORKSPACE_MAINTENANCE`; its nonrecursive
`version --json --identity` recipe identifies the current writer and directs
recovery to the owning writer and worktree. A predecessor intent receives that
identity diagnosis because this writer declares no production predecessor.
Neither case authorizes the current writer to adopt or remove the marker.

An exact supported retry resumes its own published operation. Another request,
key, digest, writer, root, Git identity or UUID cannot adopt it. Ordinary commands
and reads return `WORKSPACE_MAINTENANCE` instead of finishing cleanup. Reads never
remove markers. Native lock contention is bounded by the original invocation
deadline, with one immediate acquisition attempt even if the budget is exhausted.
Acquired locks permit maintenance to finish past that deadline. Connection-gate,
SQLite read-admission and writer contention return `WORKSPACE_BUSY` with their
distinct boundaries. Neither error authorizes
manual marker deletion or editing a database.

A cold hot-WAL/no-SHM read also temporarily drains managed readers under an
exclusive lease to build a write-free heap WAL index. It is not destructive
maintenance and publishes no operation marker. See [SQLite Model](sqlite-model.md).

## Initialization and writer migration

Init validates Git and path containment, takes an exclusive lease and publishes
its durable intent. It creates a complete private generation, records the
successful init result, checkpoints and syncs it, then publishes without
clobbering another generation. Recovery recognizes only the matching private or
published generation and original outcome; it removes intent after verification.

A writer mismatch is diagnosed without writing. A future release declaring a
predecessor must migrate it under an exclusive lease with matching durable
recovery authority. Before reporting completion, it checkpoints the successor's
schema, writer identity and generation into the main file and syncs it under the
same exclusive lease and database inode witness. The current production registry is empty: Engine refuses
other writer digests, and only the Store framework is exercised with isolated
test declarations. This is not an active public migration route and does not
authorize daemon-era conversion, feature-work declarations or read-side migration.

## Destructive reset-all protocol

`podway --json reset --all --yes --idempotency-key <caller-owned-key>` explicitly
replaces current runtime state. It requires no `--force`, makes no backup, and
performs no task-state conversion. A newer unsupported Store is refused.

1. Resolve the exact Git worktree and validate safe contained filesystem state.
2. Acquire and revalidate exclusive intent/maintenance leases and reject a
   competing removal or mismatched published operation.
3. For a valid current Store, validate any expected workspace UUID before effects
   or recognize the same already-committed reset. An unfenced confirmed request
   can recover current-generation writer, schema or binding admission failures,
   including malformed or unreadable rows and pages encountered during qualification
   or the maintenance read of current state and outcomes,
   without adopting its old UUID or using unvalidated old outcomes as recovery
   authority. Fenced requests
   preserve the original admission failure. Unsupported legacy state supplies no
   inferred current-workspace identity for a fenced reset.
4. Publish durable intent and the reset phase marker with the target UUID and
   original response identity. Persist them before destructive work.
5. Clear authorized runtime contents while preserving recovery anchors; retire
   legacy configuration markers and ensure the supported workspace layout.
6. Publish the complete new Store and its workspace-scoped reset success, or
   verify that exact already-published generation and result during recovery.
7. Remove the phase marker, then the durable intent after success is verified.

A crash after phase-marker removal still leaves intent and the new success for
exact retry. Reset preserves tracked Procedure files, `.podway/.gitignore`,
configuration unrelated to legacy mode, and Git worktree/index/ref state. All
current and inactive runtime sessions, attempts and old outcomes are discarded.
It creates a new workspace UUID rather than adopting unsupported old state.
Recovery of invalid current state retains all path, descriptor, exclusive lease
and exact-marker checks, including database inode revalidation when admission
fails. Contention, I/O failures, changed ancestry and foreign maintenance markers
do not qualify as invalid-state recovery authority.

## Workspace removal

Explicit removal binds the same root, Git identity, writer, key and workspace
fences under exclusive maintenance authority. A running session requires the
explicit force policy in addition to removal confirmation. Runtime and
configuration deletion proceed through the recorded phases; the full response
is prepared before namespace deletion destroys retention.
An unfenced confirmed removal can discard invalid current-generation state,
including copied or rebound runtime, under the same safety checks. It does not
adopt the invalid Store's UUID, graph or outcomes; fenced requests refuse it.
A present invalid Store that is removed reports `already_absent=false` and
`podway_directory_removed=true`, with `workspace_uuid=null` when its prior identity
could not be validated. A later request after complete removal reports
`already_absent=true`; absence is independent of whether a UUID was readable.
The non-interactive CLI still requires its explicit UUID fence; unfenced CLI
removal uses the interactive exact-path confirmation.

Descriptor-relative traversal rejects symlinks, mounts, changed ancestry,
foreign ownership and unknown residual contents. Removal clears the intent and
named locks before its final operation marker. The live runtime directory lease
excludes init, reset, duplicate removal and empty-residual contenders across both
marker-unlink windows. A crash can leave only the matching removal marker; an
exact validated retry restores its intent under the directory lease and converges
through the remaining suffix without changing the original response or clock.
Ordinary reads fail closed until the last marker is gone; exclusive entrants
still respect the retained directory lease. A fresh runtime published after the old runtime directory is removed is
preserved; old-parent cleanup can fail closed if that parent is repopulated.
Creation uses held parent descriptors and retries disappearing directories within
its original wait budget. Cold classification refuses changed directory witnesses
before admitting initialization; the caller can retry the same request after
removal finishes. An old UUID cannot authorize deletion of the fresh workspace. A
fully removed workspace has no outcome Store; a later already-absent removal is
idempotent convergence, not retained original-result lookup. An empty residual
may be removed only after descriptor validation, never by broad recursive
cleanup based on path text alone.

## Legacy state and installed writers

Classification detects Store generations 1–13 and retired `reset.marker`,
`workspace-removal.marker`, `development-v2.marker` and `.podway/mode-switch.json`
without writes, migration or SHM creation. It returns
`LEGACY_RUNTIME_STATE_UNSUPPORTED` and an explicitly authorized reset recipe with
a fresh caller-owned key. Before an actual upgrade/reset, retire the installed
old daemon and Aquarium writer under their original controllers and separate
authority: an old open handle can keep writing unlinked files after deletion.
Follow the complete [upgrade prerequisite](../operations/release-and-packaging.md#upgrade).
A disposable frozen legacy fixture proves byte-identical refusal and explicit
reset, not retirement of an installed writer.

## Moved worktrees

Identity is worktree-local and based on canonical Git common/administration paths
under [ADR-0036](../../architecture-decision-records/0036-use-canonical-git-paths-for-workspace-identity.md).
A linked-worktree root move can preserve those identities; reads report the new
root without persisting it, and a successful mutation or workspace repair rebinds
it transactionally. Moving a main/common Git root, changing UUID or copying
state with different canonical identities fails closed. No registry rediscovery,
global UUID-uniqueness check or queued-work drain exists.
Confirmed unfenced reset or interactive removal can discard that invalid current
runtime at the selected canonical worktree; reset creates a fresh identity.

## Newer Store generations

Every workspace Store admission, including init, doctor, reset and removal,
refuses a newer committed main-file or WAL generation before acquiring native
leases or creating SHM. This refusal also takes precedence over maintenance or
legacy marker names and invalid workspace configuration. Existing bytes and
file identities remain unchanged; no new lock or marker is created.
`WORKSPACE_SCHEMA_UNSUPPORTED` reports the actual and supported generations and
guidance to use a Podway version that supports the newer Store. It provides no
doctor/reset recipe: this older binary cannot recover or discard that generation.
Marker diagnosis remains available when the bounded generation inspection is
inconclusive. It does not authorize deletion: reset and removal independently
inspect generation before acquiring a lease, again under the exclusive lease,
and at runtime destruction. A checksum-valid committed newer WAL prefix refuses
even with an oversized tail or a missing main file. An unresolved valid stream
beyond the scan budget, or an unreadable committed page-one generation, refuses
destructive maintenance even when a marker exists. An invalid WAL header or tail
cannot authorize later frames; supported main-file recovery retains exact-marker
checks. These probes create no SQLite sidecars and revalidate file identities.

A maintenance lock or marker protocol change requires a Store generation bump
so an older destructive writer refuses it. The existing protocol is unchanged
by invalid-current recovery and newer-generation guidance.

## Worktree deletion and corrupt state

Deleting a worktree deletes its task state; Podway stores no copy elsewhere.
Safe path resolution or inode revalidation refuses a vanished or replaced root.
Corrupt SQLite, schema objects, writer identity, Procedure snapshots, graph
invariants, receipts or outcomes fail closed through the canonical workspace
errors. Doctor may diagnose supported readable state but does not automatically
repair it. Destructive reset remains a separately authorized owner decision.

## Retention policy

- The current terminal session remains until explicit reset, archive or eligible
  replacement. Its snapshot, attempts, items, trace, workflow memory and goals
  remain together.
- Archive preserves complete immutable inactive state and frees the current
  slot. Preserving running work atomically cancels/archives it with superseded
  disposition and successor identity. Archive never evicts older sessions.
- Inactive sessions remain until confirmed purge or worktree reset, cannot be
  restored or mutated, and use the bounded archive-slot identity space.
- Session-scoped success outcomes remain until reset of that session or worktree
  reset. They survive archive purge and have no cascading archive foreign key.
- Workspace-scoped outcomes retain at most 100 entries and no entries older than
  30 days, pruned during successful mutations. Session reset preserves them.
- The journal keeps at most the newest 10,000 rows with a seven-day age limit,
  except that the newest 200 rows are always retained. Successful mutations prune
  it atomically. Normal summaries contain no item values or artifact paths.

There is no job retention policy, legacy terminal-receipt decoder or separate
account-runtime retirement record in the current product. Changes to bounded
retention require adopted contract work, not unbounded configuration.

## Doctor checks

`podway doctor` holds an exclusive native lease and verifies supported Store
binding, schema-object and writer identities, `quick_check`, foreign keys, current
and archived graph reconstruction, retained outcomes and public status
projection. `--deep` additionally revalidates current required local artifacts;
otherwise that check is skipped. Checks are returned in the bounded result object of the
`workspace.doctor` command inside `podway.output/v4`; this command has no separate
named result-schema family.

Readable binding can support an unhealthy integrity report without reconstructing
invalid domain state. Unsupported writer/generation or unreadable admission may
instead return the canonical error envelope. Doctor creates no runtime migration,
row update, marker completion or automatic reset. It checks no daemon, socket,
registry, account mode or service installation. Supported repair and destructive
recovery remain explicit commands under their own authority.

## No backup or export requirement

No session export/import, global backup, event replay or post-mortem archive is
required. Users record durable outcomes in their project systems. Immutable
inactive sessions are local runtime retention, not an evidence archive or a
restorable execution product.
