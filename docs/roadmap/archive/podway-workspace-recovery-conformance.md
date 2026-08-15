# Podway Workspace Recovery Conformance

## Status and authority

- Document state: `Historical`
- Owning roadmap epic: `V2REC`
- Product version: unchanged
- Repository scope: Podway only

This dossier records the completed workspace-identity and confirmed `reset --all`
recovery design. Accepted ADRs, specifications, and machine contracts retain
their normal precedence.

## Verified context

The registry guarantees unique, sorted workspace UUIDs but historically allowed
two different UUIDs to retain the same exact root. A later `workspace.init` could
therefore create a new Store identity while stale metadata for the prior Store
remained. Confirmed `reset --all` then rejected the root as ambiguous before
admission, even when the local SQLite binding still proved the exact predecessor.

## Goal

- Prevent bootstrap, refresh, and move operations from assigning one root to
  different workspace UUIDs.
- Let explicit confirmed reset select a legacy predecessor from validated local
  binding evidence and atomically converge stale same-root metadata.
- Preserve fail-closed identity, concurrency, crash, and copied-worktree behavior.

## Non-goals

- Do not add a registry-editing command or automatic destructive cleanup.
- Do not treat Procedure v1 task state as supported normal Store state.
- Do not change session identity, history, or one-current-session behavior.
- Do not change the public command, envelope, result-schema, or error-code set.

## Accepted design

The registry writer enforces both sides of the current mapping: one UUID has one
root and one root has one UUID. Bootstrap checks root ownership before creating a
Store. Refresh and move reject a root owned by another UUID.

Legacy duplicate-root documents remain readable only so the affected root can be
diagnosed and explicitly reset without isolating unrelated workspaces. Reset uses
a read-only identity inspection that verifies the predecessor schema and exact
workspace-root binding without accepting or opening legacy task state. Persisted
Git fingerprints remain mandatory for ordinary activation; explicit destructive
reset may recover when those fingerprints are detached only when the stored exact
root and UUID membership still bind the predecessor.

The selected root's complete sorted UUID set becomes a compare-and-swap generation.
Under the maintenance lease, registry replacement succeeds only if that generation
is unchanged, removes every stale entry for the exact root, and publishes only the
reset target. Missing, unregistered, or root-conflicting identity evidence remains
fail-closed; unreadable legacy task state is eligible only when reset-specific
binding inspection proves the predecessor.

## Roadmap and acceptance

`V2REC-001` owns implementation, specification synchronization, focused regression
coverage, and the complete development gate. Acceptance requires:

- same-root insert, bootstrap, and move rejection before a second Store identity
  is created;
- successful confirmed reset from a legacy Store plus duplicate-root registry;
- atomic registry convergence without changing unrelated entries;
- preserved reset replay, crash recovery, move, and copied-worktree conflict
  behavior; and
- passing `make test` on the integrated change.

Release, publication, installation, and production runtime activation are separate
operator actions and are not authorized by this task.
