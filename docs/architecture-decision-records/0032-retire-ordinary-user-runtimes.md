# ADR-0032: Retire Ordinary User Runtimes

- Status: Accepted
- Date: 2026-09-09
- Extends: [ADR-0003](0003-daemon-single-writer.md),
  [ADR-0006](0006-same-user-local-trust.md),
  [ADR-0012](0012-explicit-daemon-endpoint-and-canonical-per-user-podway-home.md),
  [ADR-0030](0030-retire-workspaces-and-prune-stale-registry-entries.md), and
  [ADR-0031](0031-keyed-runtime-modes-and-managed-development-service.md)

## Context

Workspace reset recreates a Store, workspace removal deletes a worktree's Podway
tree, and service uninstall removes the production service. None retires ordinary
account-runtime data without touching worktrees. A corrupt registry must not
force users to enumerate or edit worktree Stores to retire a stopped runtime.

Deletion based on an earlier status probe races with new work, daemon startup,
service restart, log rotation, and a lost coordinator connection. Removing a
locked file also permits a new process to lock a replacement inode. Runtime
retirement therefore requires its own bounded coordination and recovery contract.

## Decision

Add explicit single-mode and all-mode runtime reset planning and application as
specified by the [runtime reset contract](../specs/operations/runtime-reset.md).
No mode is inferred from a worktree or defaults to destructive production
selection. Plan writes nothing; apply requires the state-bound token and `--yes`.
Retirement removes only selected ordinary runtime data and the selected
production service. It preserves worktrees, all worktree-local Podway content,
installed binaries, external managed runtimes, and unselected namespaces.

The CLI owns grammar, confirmation, protocol adaptation, and presentation. The
service crate owns the offline coordinator, effective-user discovery, safe
filesystem access, platform service actions, locks, and one bounded account reset
record. It receives an injected reset-control interface implemented by the CLI;
service gains no dependency on protocol, daemon, Git, or Store. The daemon owns
atomic idleness reservation and admission closure. Protocol owns its bounded
wire contracts. Core retains pure domain values only.

Live reset rejects executing or queued work, other admitted clients,
maintenance, incomplete recovery, and unknown activity. A durable idle session
alone is eligible. Offline reset requires an unloaded service, exclusive
ownership of the existing singleton anchor, and unambiguous endpoint/process
ownership. It never opens a workspace Store. Unknown or older live reset
contracts fail closed; the operator uses the existing supported service lifecycle
to stop that daemon before obtaining a new offline plan.

Ordinary runtime creation and startup participate in an account topology gate.
Reset retains the existing `run/podwayd.lock` inode and containing directories.
The lock order is account reset mutex, existing production service lifecycle
lock when applicable, topology gate, commit interlock, then singleton locks in
target order. Managed external roots keep their existing lifecycle owners.

All selected live daemons reserve idleness before any destructive operation.
The reserve connection stays open through the bounded reset-control exchange;
ordinary IPC remains one request/response per connection. Reservation release
and durable marker publication share the commit interlock. A release handler
takes it before admission state and reopens admission only without a matching
committed marker. Read-only reservation snapshots never retake that interlock.
The coordinator revalidates every participant and fsyncs the marker while holding
it, so loss before validation prevents commit and loss after commit cannot admit
new work. Startup and admission remain fenced until explicit recovery completes.

After commit, journal exact service/process stop intent, stop without forced
termination, verify exit, and acquire singleton ownership. Then durably publish
the final deletion inventory within approved classes. Journal each removal before
unlinking it and recognize its exact postcondition on retry. Descriptor-relative,
no-follow, ownership and identity validation applies at every filesystem boundary.
Absent resources satisfy recovery only through their matching durable intent;
conflicting recreated identities fail closed. Partial deletion is never success
or a claim of rollback.

All-mode selection is frozen, with production first and named modes in byte order.
One in-progress operation serializes the account. At most one completion receipt
is retained; exact replay returns `already_applied` without inspecting or deleting
a newly created runtime. A later committed reset retires the previous receipt.
The fixed bounds and closed encodings are owned by the runtime reset specification
and canonical schemas. They are not caller-adjustable.

## Rejected alternatives

- Reusing workspace reset or removal destroys the worktree state this operation
  must preserve.
- Reusing `daemon.terminate` changes its dev-only semantics and does not reserve
  idleness atomically.
- A status probe followed by stop permits work admission between those actions.
- Deleting singleton anchors allows competing processes to lock different inodes.
- Expanding an all-mode selection during apply deletes namespaces never approved.
- Freezing log leaf identities before shutdown makes normal rotation incompatible
  with recovery; approved classes are frozen first, final leaves after exit.
- Automatic restart, forced termination, backup, and arbitrary-root cleanup exceed
  the explicit retirement boundary.

## Consequences

The new contracts are reserved before runtime admission. They add no workspace
SQLite migration or Procedure change and preserve existing command meanings.
The public manifest changes so incompatible peers reject the new contract set.
Acceptance requires isolated real CLI/daemon tests, service adapter integration,
barrier-controlled concurrency tests, and interruption tests at every irreversible
boundary; successful scaffolding or mocked shutdown alone is insufficient.
