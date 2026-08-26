# ADR-0030: Retire Workspaces and Prune Stale Registry Entries

- Status: Accepted
- Date: 2026-08-26
- Extends: [ADR-0003](0003-daemon-single-writer.md)
- Extends: [ADR-0004](0004-worktree-local-state.md)
- Extends: [ADR-0026](0026-retain-inactive-terminal-sessions.md)

## Context

The daemon registry is metadata-only, but production startup currently retains an
entry when its last-known worktree root no longer exists. That contradicts the
recovery specification, inflates failed-worktree readiness totals, and leaves
metadata for deleted temporary and long-retired worktrees indefinitely.

There is also no supported way to stop using Podway in an existing worktree.
Session reset preserves workspace initialization, and the test-only registry
removal helper is neither a production capability nor a safe operator interface.
Removing only registry metadata would leave all worktree-local Podway content in
place and allow the next access to register it again.

Routine inactive-session retention is a different lifecycle. A worktree retains
at most 32 inactive sessions and fails without eviction when full. Workspace
retirement must not turn that bounded, explicit policy into automatic history
deletion during ordinary use.

## Decision

Production daemon startup removes a registry entry only when the exact
last-known root is conclusively absent. The registry writer rechecks the exact
workspace UUID and encoded root under its normal lock, validates absence at the
mutation boundary, and publishes the reduced document through its existing
atomic and durable write path. A changed entry, a reappeared root, ambiguous
identity, permission failure, transient filesystem error, invalid configuration,
unreadable Store, or job-recovery failure is retained and reported rather than
pruned. Successful pruning completes that recovery item and is not a workspace
failure.

The same exact-generation rule applies when pre-mutation revalidation
conclusively observes that a queued mutation's worktree root is absent. The
scheduler stops, the registry writer repeats the UUID/root and absence checks
under its lock, and clients receive `WORKTREE_GONE` where possible. A changed
generation, reappeared root, or inconclusive observation retains the entry.

Podway adds `podway workspace remove --force` for an existing Git worktree. The
command removes the exact global registry entry and the complete `.podway`
subtree, including configuration, ignore rules, custom Procedures, runtime state,
and unknown files below that boundary. It never deletes the Git worktree or any
path outside `.podway`, follows no symlink, and performs no Git index, reference,
commit, or remote mutation.

The daemon opens the `.podway` final component descriptor-relatively from the
validated worktree root without following it. A symlink, non-directory, mount or
descriptor substitution, or changed root fails with `WORKSPACE_PATH_UNSAFE`.

The command is deliberately destructive:

- an interactive caller is shown the resolved absolute worktree root and must
  type that path exactly;
- JSON and non-interactive callers must supply `--yes` and the exact
  `--if-workspace-uuid` fence;
- all callers must supply `--force`;
- the daemon revalidates Git identity, root containment, workspace UUID, and
  registry generation immediately before admission;
- copied, moved, conflicted, changed, or otherwise ambiguous identity fails
  closed without deletion.

Removal is a synchronous daemon-owned maintenance mutation, not a durable
workspace job. The daemon closes admission, quiesces and retires the exact
scheduler generation, and closes Store handles before deleting state. A bounded
canonical removal marker under `.podway/runtime/` records the operation and exact
binding. The registry entry is removed before descriptor-relative subtree
deletion. The daemon removes every other entry and empty directory that does not
contain the marker, removes the marker as the last file, and then removes the
now-empty runtime and `.podway` directories. Startup completes a marked removal
while the entry remains registered; a later request at the worktree completes a
marked removal after the entry has been removed.

Interruption after marker removal may leave only a descriptor-verified empty
`.podway` directory skeleton. When the selected Git worktree still exists and no
registry entry owns its exact root, absence or that safe empty residual is an
`already_absent` success. Automation must still submit `--if-workspace-uuid`, but
the no-content path has no authoritative UUID to compare and therefore does not
produce a mismatch. It may remove the empty residual; any entry, unsafe node, or
ambiguous identity fails closed. This convergence retains no global tombstone or
deleted payload.

The public success result is `podway.workspace-removal-result/v1`. It reports the
resolved worktree root, nullable prior workspace UUID, whether registry metadata
and `.podway` content were removed, and whether the target was already absent.
The UUID is null when no marker, Store, or registry identity remains. Human help
and output state that tracked project content may now appear deleted, daemon logs
remain outside the worktree, and `podway daemon uninstall --purge-logs --yes`
removes those logs explicitly.

## Rejected alternatives

- A public command that removes registry metadata only does not express the
  requested end-of-use lifecycle and silently leaves all local state behind.
- Selecting arbitrary missing roots or UUIDs from another worktree weakens Git,
  root, and Store identity proof. Missing roots are handled automatically.
- Recursively deleting the Git worktree crosses Podway's ownership boundary.
- Treating every recovery error as proof of deletion can discard moved or
  temporarily unavailable workspace metadata.
- Automatically evicting inactive sessions after a lower advisory threshold
  hides irreversible loss and contradicts ADR-0026.

## Consequences

- Stale metadata converges away during ordinary production startup without a
  manual registry editor or development-only switch.
- A user can deliberately remove all Podway-owned content from one verified
  existing worktree through a supported command.
- Workspace removal may delete tracked `.podway` content and all retained
  sessions, so its target confirmation and identity fences are load-bearing.
- The existing 32-session archive limit, explicit per-session purge, and
  fail-without-eviction behavior remain unchanged.
- Registry mutation, removal-marker recovery, CLI and JSON contracts,
  observability, and distribution compatibility require coordinated conformance
  work before the command is admitted in production.
