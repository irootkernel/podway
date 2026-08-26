# Podway Workspace Retirement and Registry Hygiene

## Status and authority

- Document state: `Adopted`
- Owning roadmap epic: `V2RET`
- Target product release: v0.2.7
- Repository scope: Podway only
- Accepted authority: [ADR-0030](../architecture-decision-records/0030-retire-workspaces-and-prune-stale-registry-entries.md)

This dossier is the decision-complete implementation plan for the unfinished
`V2RET` epic. The active roadmap owns task order and status. Accepted ADRs,
canonical machine assets, executable contracts, and current specifications
remain higher authority for shipped behavior.

## 1. Verified context

The recovery specification requires a grace-free validation pass to remove a
registry entry whose last-known root is missing. Production startup instead maps
ordinary Git resolution failures to `WorktreeGone`, retains the entry, and counts
it as unavailable. The registry writer already owns bounded loading, exclusive
locking, validation, canonical ordering, private permissions, atomic replacement,
and directory durability, but its removal method is compiled only for tests and
checks UUID without an exact-root generation fence.

Session archives are separate worktree-local state. The Store retains at most 32
inactive sessions, lists them newest first, and permanently purges only one exact
session and revision after explicit authorization. Nothing automatically evicts
old sessions.

## 2. Goals, non-goals, and scope

Goals:

- make missing-root registry cleanup a normal production startup behavior;
- distinguish conclusive absence from every retained recovery failure;
- remove only an exact UUID/root registry generation through the sole writer;
- add one supported, explicitly destructive end-of-use command for an existing
  worktree;
- delete the complete `.podway` subtree without escaping that boundary or
  deleting the Git worktree;
- make interruption and response loss converge through a bounded marker and
  exact replay;
- preserve honest readiness accounting and bounded observability.

Non-goals:

- automatic inactive-session eviction or a change to the 32-session limit;
- a registry browser, arbitrary-root forget command, or cross-worktree cleanup;
- deleting a Git worktree or mutating Git metadata, commits, references, remotes,
  or the index;
- retaining deleted session payloads, artifact bytes, or a global removal
  history;
- using installed production state as a disposable qualification target.

## 3. Accepted interfaces

### 3.1 Registry pruning

Startup classifies a root as conclusively missing only from a fresh
descriptor-safe filesystem observation that returns the platform's absent-path
condition. It does not infer absence from the existing broad `WorktreeGone`
mapping. Under the registry lock, removal requires the same workspace UUID and
encoded last-known root and repeats the absence check immediately before the
canonical write. Any changed or inconclusive observation returns a retained
outcome.

When pre-mutation revalidation conclusively observes the same absent-path
condition, the scheduler stops and the registry writer applies the same locked
UUID/root generation comparison and repeated absence check before removal.
Clients receive `WORKTREE_GONE` where possible. A changed generation, reappeared
root, or inconclusive observation retains the entry and reports the existing
failure instead of weakening the fence.

The readiness total remains the number of loaded registry entries. A successfully
pruned entry completes one worktree-recovery item and never increments the failed
count. Observability emits a bounded cleanup operation and outcome with workspace
UUID when available, but no root, configuration, session, item, artifact, or
unknown `.podway` content.

The closed daemon-log operation is `registry_entry_prune`; its outcomes are
`succeeded`, `rejected`, and `failed`. Startup and pre-mutation pruning share
those values and never place a root path or removed content in a record.

### 3.2 Explicit workspace removal

The public grammar is:

```text
podway [--worktree <path>] workspace remove --force \
  [--if-workspace-uuid <uuid>] [--yes]
```

Only the selected existing Git worktree is eligible. Human TTY mode may omit
`--yes`; it prints the resolved absolute worktree root and requires that exact
path as confirmation. JSON and non-TTY mode require both `--yes` and
`--if-workspace-uuid`. Cancellation performs no mutation.

The synchronous control route returns
`podway.workspace-removal-result/v1` with:

- `schema`;
- `worktree_root`;
- nullable `workspace_uuid`;
- `registry_entry_removed`;
- `podway_directory_removed`;
- `already_absent`.

`already_absent` is true when the selected Git worktree exists, no registry entry
owns that exact root, and `.podway` is absent or contains only a
descriptor-verified empty directory residual from final cleanup. Automation must
still submit `--if-workspace-uuid`, but this no-content path has no authoritative
UUID to compare and does not return `WORKSPACE_UUID_MISMATCH`. The result reports
a null `workspace_uuid`, `registry_entry_removed: false`, and whether the current
request removed an empty residual through `podway_directory_removed`. Any file,
symlink, mount, non-directory, unsafe node, or other ambiguous target is not
collapsed into this result.

### 3.3 Removal transaction and recovery

Admission uses the existing worktree maintenance key, stops new admissions,
waits within the established bounded maintenance deadline, retires the exact
scheduler generation, and closes Store handles. It then publishes a canonical
`podway.workspace-removal-marker/v1` containing the operation ID, request digest,
response request ID, workspace UUID, exact root identity, and creation time.

The daemon opens the `.podway` final component descriptor-relatively from the
validated worktree root without following it. A symlink, non-directory, mount or
descriptor substitution, or changed root fails with `WORKSPACE_PATH_UNSAFE`.
After compare-and-removing the exact registry UUID/root generation, it deletes
entries below `.podway` descriptor-relatively without following symlinks. It
removes every other entry and empty directory that does not contain the marker,
removes the marker as the last file, and then removes the empty runtime and
`.podway` directories. Unknown files and custom Procedures below `.podway` are
intentionally in scope. A path escape, root change, competing marker, registry
generation change, or unsafe file type fails closed.

If interruption occurs before registry removal, registered startup recovery sees
the marker and resumes it. If interruption occurs after registry removal, the
next Podway request at that Git worktree detects the marker before initialization
or activation and resumes it. Interruption after marker removal leaves at most the
safe empty residual accepted by `already_absent`. Response loss is reconciled by
exact replay; no global tombstone or deleted payload is retained.

The closed daemon-log operation is `workspace_remove`; its outcomes are
`succeeded`, `rejected`, and `failed`. Records may carry the bounded workspace UUID
when still known and the explicit request ID, but never the root or removed
content. An `already_absent` convergence is `succeeded`.

## 4. Failure handling and compatibility

- Existing registries remain schema-compatible; pruning changes behavior, not the
  registry document shape.
- Invalid configuration, unreadable or unsupported Store state, queued-job
  recovery failure, permission denial, identity conflict, and transient I/O are
  retained during automatic startup cleanup.
- Explicit removal may delete unreadable Store state only after exact Git/root,
  registry or Store identity, force, and confirmation requirements are satisfied.
- A live mutation that cannot quiesce within the maintenance deadline fails before
  marker publication.
- Marker publication outcome-unknown states use the existing fail-closed
  admission pattern; callers inspect or replay rather than weaken a fence.
- Older peers reject the new command through ordinary unsupported-route behavior.
- Human help and results state that all `.podway` content is removed and tracked
  deletions may remain in the worktree. They also state that daemon logs remain
  outside the worktree and that `podway daemon uninstall --purge-logs --yes`
  removes those logs explicitly.

## 5. Roadmap ownership

- `V2RET-001` adopts ADR-0030 and this decision-complete dossier.
- `V2RET-002` reserves the command, result, error, marker, manifest, replay,
  observability, and compatibility contracts without runtime admission.
- `V2RET-003` adds exact-generation registry removal for production startup and
  pre-mutation missing-root handling with readiness and observability integration.
- `V2RET-004` implements confirmed workspace removal, marker recovery, complete
  `.podway` deletion, empty-residual convergence, observability, rendering, help,
  and completion.
- `V2RET-005` closes race, crash, compatibility, E2E, documentation, and complete
  development-gate conformance, then removes this dossier.

Tasks execute in order. The dossier remains until `V2RET-005` promotes every
lasting behavior to its durable authority and closes the epic.

## 6. Verification and release acceptance

Registry coverage proves one and many startup roots and one pre-mutation root are
removed only after conclusive absence, live entries are preserved, a changed
UUID/root or reappeared root prevents removal, moved-worktree rediscovery still
works, retained failures stay visible, and a second start has no stale-root
failures. Registry locking, ordering, bounds, permissions, atomic publication,
durability, closed observability values, and failpoint recovery remain intact.

Workspace-removal coverage proves exact interactive and automation confirmation,
force and UUID fencing, active-scheduler quiescence, full `.podway` deletion,
unknown-file and custom-Procedure deletion, symlink and path-escape safety,
registry consistency, every marker crash boundary including markerless empty
residuals, response-loss replay without a remaining UUID, already-absent
convergence, bounded observability and log-retention messaging, and preservation
of the Git worktree and all paths outside `.podway`. A markerless non-empty or
unsafe target fails closed. A removed worktree can be initialized again as a new
workspace.

Each implementation task runs focused unit and integration checks. The final
candidate passes `make test`. Distribution readiness, installation, daemon
activation, and cleanup of any real user registry remain separately authorized
release or operational work.

## 7. References

- [ADR-0030](../architecture-decision-records/0030-retire-workspaces-and-prune-stale-registry-entries.md)
- [Recovery, retention, and maintenance](../specs/storage/recovery-retention-and-maintenance.md)
- [CLI specification](../specs/interfaces/cli-specification.md)
- [Errors and exit codes](../specs/interfaces/errors-and-exit-codes.md)
- [Observability](../specs/operations/observability.md)
- [Repository structure](../architecture/repository-structure.md)
- [Git worktree and filesystem](../architecture/git-worktree-and-filesystem.md)
