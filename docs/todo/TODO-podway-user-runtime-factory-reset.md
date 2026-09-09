# Podway User Runtime Factory Reset

## Status and authority

- Document state: `Adopted`
- Owning roadmap epic: V2FRT — User Runtime Factory Reset
- Dependency: completed V2DVC; preserve the subsequent released CLI contracts.
- Target product release: undecided; this epic grants no release or installation
  authority.
- Repository scope: Podway only.
- Detailed SOT: this dossier until epic closeout. The roadmap alone owns task
  status and ordering.

This dossier owns the bounded implementation plan for V2FRT. It does not claim
that the new commands or contracts are implemented. Existing accepted ADRs,
machine contracts, and specifications remain authoritative until their owning
implementation task promotes the change.

Reset authority and the reserved contract surface are now owned by
[ADR-0032](../architecture-decision-records/0032-retire-ordinary-user-runtimes.md)
and the [runtime reset specification](../specs/operations/runtime-reset.md).
That specification fixes the wire encodings, error choices and narrow persistent
control exchange required by the connection-bound reservation. The roadmap
continues to own implementation admission and task status.

## Verified context

Production owns the effective user's .podway/run, state, and logs directories
and the fixed dev.podway.podwayd LaunchAgent in Library/LaunchAgents. Ordinary
non-production modes own `.podway/modes/<mode>/{run,state,logs}`. Runtime identity
includes both root and mode. Aquarium and disposable qualification roots have
separate lifecycle owners.

Current reset --all recreates one workspace Store; workspace remove deletes one
workspace's .podway tree; daemon uninstall removes the production service and
optionally its logs. None composes account-runtime retirement. Existing
daemon.terminate is a development-only control surface; it must not be treated
as a general idle-only reset handshake.

## Goal and boundaries

Provide a supported CLI for inspecting and explicitly retiring either one
ordinary account runtime or all discovered ordinary account runtimes, without
requiring a Git worktree or a healthy registry. The scope includes ordinary
account runtimes only and rejects busy runtimes rather than waiting or forcing.

Delete selected registry and runtime recovery data, sockets after proven daemon
exit, daemon and bootstrap logs and rotations, service metadata, and the
selected production LaunchAgent. Leave the selected runtime stopped and
uninstalled; no automatic replacement daemon or fresh database is created.

Preserve every Git repository, worktree-local .podway tree, current and archived
session, job, configuration, Procedure, installed executable, and non-selected
namespace. Keep persistent lock anchors and a bounded account-reset
recovery/completion record; factory reset means removal of runtime data and
service registration, not removal of the .podway parent directory.

Exclude Aquarium-managed roots and services, arbitrary custom roots,
contributor/qualification fixtures, unrelated files, backups, exports, binary
removal, automatic upgrades, forced process termination, and automatic resetting
of worktree Stores. Disclose external managed-runtime exclusion in both human
and JSON plans; report an encountered managed namespace as excluded rather than
following its root. Do not search arbitrary external directories to discover it.

## Public behavior

Add an offline-capable runtime reset command group:

```text
podway --mode <key> runtime reset plan
podway runtime reset plan --all-modes
podway --mode <key> runtime reset apply --plan-token <token> --yes
podway runtime reset apply --all-modes --plan-token <token> --yes
```

Require an explicit mode selector (including the existing --dev alias) or
--all-modes. Never infer the target from the current worktree or default to
destructive prod selection. Reject selector conflicts, --worktree, --socket, and
custom/managed runtime-root overrides on these commands. Account discovery uses
the existing effective-user home authority.

Plan is read-only, including on an absent account root. It lists selected modes,
fixed resource classes and paths, preserved lock/control residue, external
exclusions, live/busy/offline/unsupported/unsafe states, and exact blocking
reasons. It returns an apply token only for an eligible bounded selection.
Tokens bind caller UID, account-root identity, selection, observed namespace
directory and service/process identities, and approved resource classes; apply
repeats the validation. Mutable log contents, registry bytes, and pre-shutdown
leaf-file enumeration are previews, not an inode-frozen deletion list. An absent
or already-retired selection returns no_change with no apply token and creates
no directories. Lock anchors and completion records alone do not make a runtime
a deletion target. Plan has a ten-minute pre-commit expiry; a durable
in-progress marker keeps the original token valid for exact recovery after
publication.

All-mode selection is a frozen inventory, ordered prod first and then named
modes bytewise. It never expands during apply. A new ordinary mode appearing
before the first destructive step invalidates the plan. Any initially busy,
unsupported, unsafe, or unprovable selected namespace rejects the complete apply
before service removal or data deletion. Excluded externally managed modes are
reported explicitly and are not targets.

Apply requires the token and --yes even on a TTY. No --force option and no
automatic wait for work completion. A durable active task without executing work
is not busy. For a live daemon only, queued/running jobs, other admitted
clients, maintenance, ongoing recovery, missing activity counts, or unresolved
recovery that prevents proving idleness are busy/unsafe. A short bounded
shutdown wait is allowed after a successful idle reservation; it is not a wait
for work to finish.

An unreachable daemon is not proof of absence. Offline cleanup requires proof
that the service is not loaded, the namespace singleton lock is exclusively held
by the reset operation, and endpoint/process ownership is unambiguous. Offline
eligibility does not open any worktree Store and does not require live activity
counts. Durable queued jobs or stale running rows in preserved Stores remain
untouched; they may resume through normal recovery after an explicitly started
daemon re-registers that worktree. Corrupt registry bytes do not block cleanup
of a known owned regular file once those conditions hold. Unknown live/older
daemon contracts fail closed; the user stops the old service through its
existing supported lifecycle and obtains a new offline plan. No migration of
workspace SQLite or change to Procedure v2 is required.

Use the existing public output/error envelopes, with versioned runtime-reset
plan/result schemas, command catalog entries, and stable failure branches.
Results distinguish complete, already-applied, and incomplete recovery with
per-mode/resource outcomes, preserved/excluded resources, and a bounded retry
recipe. A partial destructive result is always non-success; never advertise
rollback of removed data. Existing reset, uninstall, workspace remove,
daemon.terminate, and Aquarium controller contracts retain their meanings.

## Coordination, deletion, and recovery

podway-cli owns grammar, confirmations, and rendering and calls an offline
service-layer coordinator. podway-service owns account discovery, safe path
operations, platform service actions, locking, and the durable reset record.
podway-daemon owns atomic idle reservation and admission fencing; protocol owns
the new bounded control and result contracts. Do not add store access to CLI or
reverse crate dependencies.

Keep owner-only, no-follow validated control files at
.podway/maintenance/runtime-reset.lock, runtime-start.lock,
runtime-reset-commit.lock, and runtime-reset.json. These paths are outside reset
payload; the JSON record has an in-progress or completed state. Use one account
reset mutex and stable per-namespace singleton anchors. Keep existing
run/podwayd.lock paths and their containing directories across resets so startup
cannot lock a replacement inode while reset owns the original. Coordinate
production service install/start/stop/uninstall through the existing service
lifecycle lock; freeze namespace creation/startup while a selected reset is
committing. Lock order is account reset mutex, production service lifecycle lock
when applicable, account topology gate, commit interlock, then namespace
singleton locks in target order. Ordinary namespace creation and daemon startup
take the topology gate while acquiring their singleton and checking reset state.
Existing daemon requests use the admission reservation gate instead. Never hold
a singleton while asking its live owner to terminate. Restrict the new fencing
to ordinary account runtimes; do not alter Aquarium's external controller
protocol.

Before the first destructive side effect, reserve every live selected daemon's
idleness atomically with admission closure. A reservation is not a user job and
does not count itself as busy. If any reservation fails, release earlier
reservations and return without stopping services or deleting data. Reserve
requests are fenced to exact process/namespace identity and remain tied to the
reset coordinator connection. Reservation release and marker commit share
runtime-reset-commit.lock: the coordinator holds this interlock while
revalidating every reservation generation via read-only control snapshots and
fsyncing the commit marker. A disconnect/release handler acquires the same
interlock before its admission-state mutex; it releases admission only when no
matching committed marker exists, otherwise it keeps admission fenced for
recovery. Read-only reservation revalidation does not reacquire the interlock.
If a connection or generation was lost before revalidation, commit is refused
and all uncommitted reservations are released. The topology gate prevents
replacement daemons during this transition. After commit, markers prevent normal
admission and daemon restart from entering selected namespaces until explicit
retry converges. Recovery reattaches only to matching recorded participants; a
conflicting process generation is an explicit non-success.

After all reservations and identity checks succeed under the commit interlock,
publish and fsync one bounded account reset marker outside selected state/log
directories. Before each shutdown, journal the exact stop-service/stop-process
intent, including expected endpoint-guard socket cleanup and production
uninstall metadata/plist removal. Stop the production service via its exact
native service owner and request orderly shutdown for reserved foreground
daemons. Wait at most the existing service shutdown timeout, without escalating
to a kill. Obtain namespace singleton locks after verified process exit. At that
point enumerate and persist a final per-resource deletion inventory within the
originally approved resource classes and namespace directories. Normal shutdown
log append/rotation is allowed before this final inventory; no data writers
remain afterward. No newly discovered namespace, resource class, or external
path may enter it. Runtime deletion begins only from that final inventory.

Deletion is descriptor-relative, no-follow, owner-checked, identity-revalidated,
and contained in the named namespace. Reject symlinked ancestry, mounted/foreign
resource roots, multiply linked sensitive files, malformed owned-layout
identities, and unexpected deletion-tree entries. Never recursively remove
.podway or follow registry paths into worktrees. A resource already absent
before final inventory is not scheduled for deletion; expected endpoint/service
teardown is verified against its pre-recorded stop intent. After final inventory
publication, each removal has durable intended and completed states. An absent
resource with a matching durable removal intent and proven exclusive ownership
satisfies the exact postcondition even if interruption prevented writing
completed. Absence without an applicable intent, or a conflicting recreated
inode, fails closed.

Journal intent before each irreversible action and recognize its exact
postcondition after interruption. One in-progress operation per account
serializes single-mode and all-mode reset. Retry with the same token resumes
only its frozen selection; a fresh plan exposes the recovery operation and
cannot supersede unfinished cleanup. A failure after stopping a service or
deleting bytes preserves progress and returns a non-success recovery result. Do
not automatically restart a stopped service or restore deleted data.

Keep at most one completed receipt until the next reset begins. Exact replay
returns already-applied without deleting a newly created runtime. Starting a
later operation retires the previous receipt; its old token becomes stale. Newly
started runtimes after completion are preserved by old-token replay. The
completion record contains only bounded identities, selected resources, and
outcomes, never registry contents, job payloads, or logs.

Plan enumerates resource classes, while final-inventory entries are bounded
before deletion. The normal teardown step and final-inventory publication each
have their own durable intent and recognizable postconditions so shutdown-time
rotations and socket cleanup cannot strand an operation between incompatible
snapshots.

Use fixed admission bounds: at most 64 ordinary namespaces per operation
(including prod); scan at most 256 immediate mode-directory entries; at most
4096 candidate resource entries across a reset; maximum depth 8; maximum
marker/result 512 KiB; maximum token 256 KiB. Exceeding a bound returns an
explicit failure before destructive work; no truncation or caller-adjustable
unbounded override. These limits keep public envelopes below the existing 1 MiB
frame contract.

## Ordered ownership and acceptance

### V2FRT-001: Adopt reset authority and reserve contracts

Promote this bounded design through the next accepted ADR and corresponding
canonical CLI, IPC, service, recovery, error, schema, and compatibility assets.
Specify exact closed encodings and error-catalog choices before runtime
admission. Preserve all existing command meanings and classify retained
coordination residue explicitly. Verify schema, catalog, manifest, architecture
and compatibility fixtures.

### V2FRT-002: Add bounded read-only reset planning

Implement effective-user inventory, selected/all-mode grammar, token binding,
absent-root behavior, managed exclusion, ownership checks and bounds. Verify no
filesystem creation, corrupt offline registry handling, selector rejection,
malicious layouts, and stale/oversized plans.

### V2FRT-003: Add atomic idle reservation and startup fencing

Implement daemon admission reservation/release, process identity fencing,
service/startup serialization and stable singleton ownership. Verify reset
versus queued/running jobs, new clients, maintenance, recovery, daemon start,
service start/install, competing resets, reservation loss and stale processes.
Barrier-controlled tests must interleave disconnect, reservation release,
new-work admission and marker commit and prove that no live work can be admitted
into a committed reset. An active idle task alone must remain eligible.

### V2FRT-004: Retire one ordinary runtime with durable recovery

Implement offline and live apply, production uninstall, named orderly shutdown,
fixed resource removal, marker/replay, bounded waits and preserved residue.
Fault-inject each irreversible boundary, including unlink-before-completed,
socket removal during shutdown, log rotation during shutdown, final-inventory
publication, and retry after each. Prove offline corrupt-registry cleanup
without Store inspection, preservation of offline durable jobs, no ambiguous
process termination, preserved worktrees/binaries/other modes, and unchanged
legacy command behavior.

### V2FRT-005: Compose all-mode reset and exact retry

Reserve/preflight the complete frozen set before deletion; process in
deterministic order, report partial failure honestly, resume only unfinished
original resources, reject competing/newly appeared targets, and prove old-token
replay preserves recreated namespaces.

### V2FRT-006: Close conformance and operator guidance

Run isolated real CLI/daemon E2E and macOS service integration fixtures,
compatibility and crash/race scenarios; prove retained workspaces re-register
through existing public commands after an explicitly requested fresh daemon
start. Promote lasting guidance and acceptance to canonical owners, remove all
links to this temporary dossier and delete it only at epic closeout. Pass make
test through Gaori full; distinguish development acceptance from release
readiness. Leave epic acceptance for its independent validator.

Task order and status are owned only by the active roadmap. Every
behavior-changing development revision requires the repository make test gate.
Focused registered int_suite tests precede the full gate; use repository Make
and Gaori routing. Tests must use established isolated roots and process/service
helpers, never this worktree's active runtime, the user's actual namespace,
production LaunchAgent, or Aquarium development root.

## Delivery boundary

Implementation starts only through the repository's explicitly requested task or
epic workflow. V2FRT owns development acceptance, not release selection,
distribution, installation, or activation. Completion promotes lasting
contracts, replaces the roadmap's Detailed SOT link with Canonical Outcomes, and
retires this dossier and its TODO index entry together.

## References

- [Contributor documentation](../README.md)
- [TODO lifecycle](README.md)
- [Active roadmap](../roadmap/README.md)
- [Runtime-mode
  authority](../architecture-decision-records/0031-keyed-runtime-modes-and-managed-development-service.md)
- [Workspace retirement
  authority](../architecture-decision-records/0030-retire-workspaces-and-prune-stale-registry-entries.md)
- [macOS service architecture](../architecture/macos-service.md)
- [CLI specification](../specs/interfaces/cli-specification.md)
- [Recovery
  specification](../specs/storage/recovery-retention-and-maintenance.md)
- [Aquarium service
  ownership](../specs/operations/aquarium-development-service.md)
