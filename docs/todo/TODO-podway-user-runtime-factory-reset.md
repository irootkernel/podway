# Podway User Runtime Factory Reset

## Status and authority

- Document state: `Candidate`
- Owning roadmap epic: none
- Target product release: undecided
- Repository scope: Podway only
- Planning baseline: September 4, 2026

This document records a candidate for supported cleanup of Podway's current-user
global runtime namespaces. It is not an adopted design dossier, accepted
architecture decision, roadmap commitment, or specification of implemented
behavior. No command or deletion behavior described here exists merely because
it appears in this candidate.

## 1. Context

Podway currently has supported operations for uninstalling the production
LaunchAgent and retiring one registered worktree, but it has no public operation
that retires one complete keyed runtime namespace or all Podway runtime namespaces
for the current user. Production state and named-mode state occupy different
locations and have different service lifecycles. Manual deletion of
`~/.podway/` can therefore race a live daemon, leave service metadata behind, or
remove more state than the user intended.

An upgrade recovery may need a genuine factory reset after each affected
worktree has been handled through its own supported recovery or removal command.
That account-level operation must not be confused with `reset --all`, which
recreates one worktree's runtime database and preserves its Podway configuration.

## 2. Goal

Determine a supported, bounded interface that can:

- stop and delete one explicitly selected production or named-mode runtime
  namespace;
- uninstall the selected mode's managed service when that mode has one;
- compose those mode-specific operations into an explicitly confirmed
  current-user factory reset; and
- leave Git repositories, worktrees, and every worktree-local `.podway/` tree
  outside the selected account-runtime operation.

The design should make selection, confirmation, cleanup results, partial failure,
retry, and post-reset re-registration inspectable through stable machine output.

## 3. Non-goals

This candidate does not propose:

- automatic runtime deletion during upgrade, migration, install, or daemon start;
- deleting any worktree, Git metadata, or worktree-local `.podway/` content;
- accepting arbitrary filesystem paths as cleanup targets;
- making Podway a system-wide package manager or privileged cleanup service;
- silently terminating unrelated processes;
- treating manual directory deletion as the supported implementation; or
- adding a command, release target, or roadmap commitment before promotion.

## 4. Rough scope

The investigation should cover production and every bounded keyed mode defined by
the runtime-mode contract. A mode-specific operation may need to coordinate the
selected endpoint, lock, socket, registry, bounded logs, mode metadata, and any
managed service registration before retiring that namespace. A whole-user
operation should be a visible composition over discovered valid namespaces, not a
recursive deletion of the account root.

The implementation direction should evaluate crash-safe retirement through an
exact, validated rename or marker protocol; deterministic retry after response
loss; refusal while ownership or process identity is ambiguous; and a result that
enumerates each selected mode and resource class. Re-registration of retained
worktrees after cleanup must use public workspace operations rather than direct
registry reconstruction.

## 5. Open decisions

- What CLI grammar distinguishes mode deletion, managed-service uninstall, and
  the all-mode factory reset?
- Should production and managed development services share one lifecycle result
  schema while foreground-only named modes report a narrower result?
- How are valid named namespaces discovered without following symlinks or scanning
  an unbounded filesystem tree?
- Which flags select logs, service metadata, installed binaries, or an entire
  namespace, and which combinations require separate confirmation?
- What happens when a selected daemon is busy, its identity cannot be proven, or
  graceful termination times out?
- Which durable marker or renamed quarantine location makes interruption and
  replay safe without broad deletion?
- How does the operation report partial success across multiple modes, and what
  idempotency scope permits deterministic retry?
- Which compatibility behavior is required when an older CLI, daemon, registry,
  or mode layout is encountered?

## 6. Roadmap promotion conditions

Promote this candidate only after:

- an accepted ADR defines ownership, exact deletion boundaries, mode discovery,
  service lifecycle behavior, crash recovery, and idempotency;
- a threat and filesystem-safety review covers symlinks, mounts, ownership,
  process identity, concurrent recreation, and partial failure;
- the public CLI, JSON, error, registry, service, and compatibility contracts are
  decision-complete;
- an adopted roadmap epic and design dossier own implementation and migration;
  and
- isolated end-to-end tests prove per-mode deletion, applicable service
  uninstall, all-mode composition, interrupted replay, and preservation of every
  worktree-local `.podway/` tree.

## 7. References

- [ADR 0012: Explicit daemon endpoint and canonical per-user Podway home](../architecture-decision-records/0012-explicit-daemon-endpoint-and-canonical-per-user-podway-home.md)
- [ADR 0030: Retire workspaces and prune stale registry entries](../architecture-decision-records/0030-retire-workspaces-and-prune-stale-registry-entries.md)
- [ADR 0031: Keyed runtime modes and managed development service](../architecture-decision-records/0031-keyed-runtime-modes-and-managed-development-service.md)
- [CLI specification](../specs/interfaces/cli-specification.md)
- [Recovery, retention, and maintenance](../specs/storage/recovery-retention-and-maintenance.md)
- [Release and packaging](../specs/operations/release-and-packaging.md)
- [Security and trust](../specs/operations/security-and-trust.md)
