# Podway Web Operations Console

## Status and authority

- Document state: `Candidate`
- Owning roadmap epic: none
- Target product release: undecided
- Repository scope: Podway only
- Related candidate:
  [Podway Graph Engineering Evolution](TODO-podway-graph-engineering-evolution.md)

This document preserves a possible local Web operations console for later
evaluation. It is not an adopted design dossier, accepted architecture decision,
roadmap commitment, or specification of implemented behavior. No command,
endpoint, daemon lifecycle, schema, or user interface described below exists
merely because it appears in this candidate.

The graph-engineering direction must reach a stable outcome before this candidate
is considered for roadmap adoption. The console should be designed against the
workflow graph that Podway actually adopts rather than make the graph model serve
a premature presentation layer.

## 1. Context

Podway currently concentrates on agent and CLI use. Its user-scoped daemon is the
sole normal writer, routes commands to registered worktrees, recovers durable
jobs, and exposes bounded status over a Unix-domain socket. Session authority and
history remain in each worktree's `.podway/runtime/` Store rather than in one
global task database.

A human operator may eventually need one place to inspect all known worktrees and
sessions, review the workflow graph, understand current placements and blockers,
and perform allowed operations without reconstructing state from several CLI
invocations. The intended inventory includes prepared, running, terminal, and
retained inactive sessions even when no worktree-specific foreground process is
already running.

That requirement changes the runtime-topology trade-off. Session-scoped
foreground processes simplify idle resource use and exact-version ownership, but
a complete cross-worktree console would still need a central catalog, offline
state discovery, version-aware routing, and mutation coordination. Adding those
responsibilities to a Web process risks recreating a second daemon instead of
removing complexity.

A neighboring local tool provides a useful comparison to revalidate before
promotion: its CLI core can operate without a daemon, its Web command can start a
loopback daemon when needed, and login-start service installation is optional.
That hybrid demonstrates that Web availability and an always-running Web server
are separable. It does not settle Podway's design because that product uses one
installation-global database while Podway deliberately distributes authority
across worktrees. Machine-local process IDs, versions, paths, and runtime
observations are not durable evidence for this candidate.

## 2. Goal

Determine whether Podway should provide an on-demand, loopback-only Web operations
console that lets a human:

- discover registered worktrees and their current and retained sessions;
- inspect each session's procedure identity, workflow graph, current placement,
  goal, blockers, readiness, queue state, and bounded history;
- compare live, offline, unavailable, and incompatible states honestly; and
- perform only currently allowed session operations through the same identity,
  revision, admission, idempotency, and confirmation contracts as the CLI.

The current preferred direction is to retain `podwayd` as the central catalog,
sole-writer, recovery, and command-routing control plane. A separate `podway web`
process would run only when requested, serve static assets and a loopback HTTP
gateway, and communicate with `podwayd` through a versioned local protocol.
Neither preference is binding until the promotion conditions are satisfied and
an architecture decision is accepted.

## 3. Non-goals

This candidate does not propose:

- implementing the console before the graph-engineering outcome is stable;
- embedding an HTTP server or Web assets directly in `podwayd`;
- allowing the Web gateway or browser to write SQLite directly;
- weakening the daemon's sole-writer, FIFO, fencing, or idempotency guarantees;
- making Podway a project manager, remote collaboration service, CI system, or
  arbitrary workflow executor;
- remote, multi-user, or non-loopback access;
- treating a browser session or local bearer value as a security boundary
  against processes running as the same operating-system user;
- exposing recorded item contents, artifact paths, credentials, or raw logs by
  default; or
- adopting a roadmap epic, release target, public command, endpoint, or schema
  through this candidate alone.

## 4. Rough Scope

### 4.1 Central control plane

Keep one user-scoped `podwayd` responsible for the minimal worktree catalog,
Store admission, version and mode agreement, durable-job recovery, mutation
serialization, and authoritative session operations. Extend its read model only
as needed to return a bounded cross-worktree inventory and session summaries.
The global catalog must remain metadata-only and must not become a second task or
history database.

### 4.2 On-demand Web gateway

Consider a `podway web` command that starts one foreground, loopback-only gateway
and opens or reports the local UI. The gateway would serve static assets, obtain
bounded projections from `podwayd`, and translate user actions into the same
typed operations available to CLI automation. Closing the gateway would not stop
or mutate `podwayd`, a worktree, or a session.

The gateway would not silently install, upgrade, restart, or replace the daemon.
How it behaves when `podwayd` is unavailable or incompatible remains an open
decision.

### 4.3 Human review and operations

The first useful console would cover:

- an inventory filtered by worktree and lifecycle;
- a session detail view with the canonical graph projection and active state;
- bounded trace, decision, rework, goal, blocker, item-status, and job views;
- explicit offline, missing-worktree, recovery, and compatibility states; and
- allowed operations derived from current daemon guidance, with fresh fences,
  idempotency keys, destructive-action confirmation, and post-mutation refresh.

The UI must not infer an allowed operation, semantic truth, or successful result
from display state. Machine fields from the daemon remain authoritative.

## 5. Open Decisions

1. Whether `podwayd` remains a login-start LaunchAgent, becomes an on-demand
   service with bounded idle exit, or supports both as explicit installation
   policies.
2. Whether daemon startup is ever initiated by `podway web`, by the CLI, by an
   agent host, or only through an explicit lifecycle command.
3. How one daemon and gateway generation handle older and newer worktree Stores,
   CLI binaries, contract manifests, and graph generations without silently
   migrating or partially rendering unsupported state.
4. Which metadata the global catalog needs to enumerate offline and retained
   sessions without copying worktree-local task state into a global ledger.
5. How missing, moved, removed, busy, recovering, and unreadable worktrees appear
   and which of those states permit an operation.
6. Which graph projection the UI consumes after the graph-engineering work, and
   how it represents ready frontiers, joins, claims, rework, and stale history if
   those concepts are adopted.
7. Whether the gateway uses new daemon inventory and event-stream routes, invokes
   existing request envelopes, or needs a separately versioned local adapter
   protocol.
8. The loopback threat model, including Host and Origin validation, one-time
   browser session establishment, CSRF resistance, content-security policy,
   secret handling, and log redaction.
9. Which read and mutation capabilities ship first, and which destructive or
   lifecycle operations remain CLI-only.
10. Pagination, refresh, event coalescing, and resource budgets for many
    worktrees, sessions, graph nodes, and retained history entries.
11. Whether daemon lifecycle simplification and the Web console belong to one
    adopted epic or to separate dependent epics.

## 6. Roadmap Promotion Conditions

This candidate may be promoted only when all of the following are true:

1. The graph-engineering candidate has reached an implemented or explicitly
   closed outcome that fixes the graph and runtime concepts the console must
   present.
2. Representative human workflows prove a need for cross-worktree active,
   terminal, offline, and retained-session inspection and identify the minimum
   safe operation set.
3. An accepted ADR decides the central daemon, Web gateway, lifecycle, discovery,
   version, and compatibility boundaries and explicitly reconciles Podway's
   current no-network product invariant with a loopback HTTP listener.
4. The metadata-only global catalog boundary and every worktree-local authority
   field are specified without creating a global task database.
5. The HTTP and browser-session threat model, permission model, bounded response
   model, and sensitive-data exclusions are decision-complete.
6. The behavior for unavailable, incompatible, moved, removed, recovering, and
   partially readable worktrees is fail-closed and testable.
7. The daemon lifecycle and upgrade design demonstrates that the console does not
   worsen idle operation or multi-version development and states whether those
   changes are independently adoptable.
8. A bounded prototype or contract exercise proves that the chosen inventory and
   graph projections remain usable at the repository's declared scale limits.
9. One decision-complete adopted dossier assigns the selected work to one or more
   roadmap epics with explicit dependencies, ownership, migration, verification,
   and release acceptance.

## 7. References

- [ADR-0003: Make the Daemon the Sole Normal Writer](../architecture-decision-records/0003-daemon-single-writer.md)
- [ADR-0004: Store Task State Inside the Worktree](../architecture-decision-records/0004-worktree-local-state.md)
- [ADR-0031: Keyed Runtime Modes and Managed Development Service](../architecture-decision-records/0031-keyed-runtime-modes-and-managed-development-service.md)
- [Podway Graph Engineering Evolution](TODO-podway-graph-engineering-evolution.md)
- [Daemon and Write Queue](../architecture/daemon-and-write-queue.md)
- [macOS Service Specification](../architecture/macos-service.md)
- [CLI Specification](../specs/interfaces/cli-specification.md)
- [IPC Protocol](../specs/interfaces/ipc-protocol.md)
- [Recovery, Retention, and Maintenance](../specs/storage/recovery-retention-and-maintenance.md)
- [Observability](../specs/operations/observability.md)
- [Security and Trust](../specs/operations/security-and-trust.md)
