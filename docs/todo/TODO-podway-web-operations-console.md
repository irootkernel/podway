# Podway Web Operations Console

## Status and authority

- Document state: `Candidate`
- Owning roadmap epic: none
- Target product release: undecided
- Repository scope: Podway only
- Related candidate:
  [Podway Graph Engineering Evolution](TODO-podway-graph-engineering-evolution.md)

[ADR-0038](../architecture-decision-records/0038-remove-the-daemon.md) and the
[System Architecture](../architecture/system.md) replace the historical
`podwayd` control-plane premise. This candidate now explores a foreground-only
console over the same typed Direct Engine. It adopts no command, endpoint,
listener, browser protocol, inventory persistence or product feature.

The graph-engineering direction must reach an implemented or explicitly closed
outcome before promotion, so presentation does not force a premature graph model.
A foreground Web process requires its own accepted decision reconciling the
current no-network invariant. Resident or login-start operation requires a new ADR
and is outside this candidate's preferred direction.

## 1. Context

Podway's current single CLI executes every command in-process. Each worktree owns
its Store, success outcomes and journal. The Engine resolves explicit native Git
identity, derives bounded read projections and serializes fenced mutations through
SQLite with shared/exclusive maintenance admission. There is no global workspace
registry, daemon, FIFO admission, job recovery or account runtime.

A human may later need a convenient view of selected worktrees, their current and
retained sessions, graph placements, blockers and history. No persistent global
catalog exists from which a console could infer all known worktrees. Its input
inventory and lifecycle must therefore be chosen explicitly rather than reviving
removed state discovery through a new host.

A neighboring tool's historical on-demand Web design is only comparative context.
It does not establish Podway's need or compatibility: Podway distributes authority
across worktrees and binds Store writers to exact manifest identity. Revalidate
comparisons before promotion; host-specific observations are not durable evidence.

## 2. Goal

Determine whether an on-demand, loopback-only foreground console would help a
human:

- inspect an explicitly selected bounded set of worktrees and current/inactive
  sessions without copying task authority;
- view graph identity, placements, Goal, blockers, readiness and bounded history;
- distinguish missing, busy, unreadable, legacy and incompatible Stores honestly;
- invoke only currently allowed operations through the same Engine, fences,
  success-only idempotency and explicit destructive confirmation as the CLI.

The preferred direction is a `podway web` foreground process active only while
requested, with explicit worktree input and no login-start policy or resident
broker. Illustrative command names are not reserved grammar. No preference becomes
binding before the promotion conditions and an accepted ADR.

## 3. Non-goals

- Implementing this candidate or changing the current single-cursor graph.
- Reintroducing `podwayd`, global workspace discovery, runtime modes, IPC framing,
  durable jobs, FIFO promises or execution after the caller exits.
- Giving the browser or Web host a second SQLite implementation or mutation path.
- Installing a service, automatically launching a background host, or migrating
  unsupported writer/legacy state to make the UI available.
- Project management, remote/multi-user access, workflow execution, Git mutation,
  or a security boundary against same-user processes.
- Exposing full recorded items, artifact paths, credentials or history by default.
- Adopting a roadmap, release target, endpoint or schema through this document.

## 4. Rough Scope

### 4.1 Explicit bounded worktree inventory

Evaluate caller-supplied exact roots or a session-local list selected by the human.
Validate each through the native workspace boundary; do not scan home directories,
reuse a removed registry or create a global task ledger. Keep the selected
inventory session-local; this candidate adopts no global or persistent catalog.

### 4.2 Foreground Web host

A possible `podway web` process would serve static assets and a loopback gateway
only while in use, route operations through the typed Direct Engine and exit on
explicit shutdown. A CLI/JSON wrapper is an alternative to evaluate; neither may
implement independent Store transactions or a compatibility shim. Closing the host
must not cancel a committed mutation or mutate any session. There is no admission
promise for work not committed before process termination.

Do not silently install, replace or stop an old generation. Unsupported writer
identity, legacy Store or configuration remains a structured error. Any reset is a
separate explicitly confirmed lifecycle action, after every old writer is retired.

### 4.3 Human review and operations

The first useful scope would include a bounded inventory, graph/session detail,
trace/decision/rework/Goal/blocker/item metadata, retained-session views and clear
unavailable states. Mutation controls derive from fresh Engine guidance, obtain
current workspace/session/attempt/item/Goal fences, retain caller-owned keys and
refresh after each success. Response loss uses outcome lookup and exact replay.
A displayed control is never authorization or proof of semantic truth.

Use current evidence pagination rather than exposing arbitrarily large values.
No event stream, polling policy or notification contract exists today; any future
refresh scheme needs a bounded budget and fail-closed snapshot semantics.

## 5. Open Decisions

1. What representative human workflows require a console beyond existing CLI/JSON?
2. How are exact worktrees supplied, scoped and bounded without a global registry?
3. Does the host invoke the typed Engine directly or wrap exact public CLI/JSON,
   and how is executable/manifest provenance preserved?
4. How are legacy, newer, undeclared-writer, copied/moved, busy, missing and invalid
   configuration states represented without automatic migration or repair?
5. Which graph projection is appropriate after graph-engineering has settled, and
   which future work-unit concepts must remain absent until independently adopted?
6. How does foreground shutdown handle a running command and possible lost output,
   with no daemon admission or execution-after-exit promise?
7. How are maintenance markers, outcome retention and exact retry guidance shown
   without granting destructive authority or replacing original keys?
8. Which commands stay CLI-only, and how are exact-root destructive confirmation,
   user intent and all current fences preserved in browser interactions?
9. What loopback threat model covers Host/Origin checks, CSRF, session establishment,
   content-security policy, secret exclusions and redaction under same-user trust?
10. What paging/refresh/resource limits keep many worktrees and histories bounded?
11. Does a prototype prove value without persistent catalogs, background processes
    or changes to the current no-network invariant before adoption?

## 6. Roadmap Promotion Conditions

Promote only after:

1. Graph-engineering has an implemented or explicitly closed outcome and real
   human workflows identify the smallest useful console scope.
2. An accepted ADR decides foreground process/listener, explicit inventory,
   Engine/CLI boundary, lifecycle and trust semantics, reconciling no-network
   authority. A resident/login-start proposal needs a separate new ADR.
3. Worktree-local state remains sole task authority; no global registry, alternate
   Store writer, daemon compatibility route or implicit legacy conversion appears.
4. Closed browser/read/mutation contracts preserve exact manifest identity,
   fences, success-only outcomes, bounded evidence and destructive confirmation.
5. Missing, incompatible, maintenance, response-loss and shutdown behavior is
   decision-complete and independently testable.
6. A bounded isolated prototype demonstrates useful inventory/projection behavior
   at declared scale and proves its children/temporary roots are cleaned up.
7. One adopted dossier assigns ownership, dependencies, compatibility, verification
   and release acceptance. This candidate alone authorizes none of that work.

## 7. References

- [ADR-0038: Remove the Daemon](../architecture-decision-records/0038-remove-the-daemon.md)
- [ADR-0004: Store Task State Inside the Worktree](../architecture-decision-records/0004-worktree-local-state.md)
- [Podway Graph Engineering Evolution](TODO-podway-graph-engineering-evolution.md)
- [System Architecture](../architecture/system.md)
- [CLI Specification](../specs/interfaces/cli-specification.md)
- [JSON Contract](../specs/interfaces/json-contract.md)
- [Transactions, Concurrency, and Idempotency](../specs/storage/transactions-concurrency-and-idempotency.md)
- [Recovery, Retention, and Maintenance](../specs/storage/recovery-retention-and-maintenance.md)
- [Observability](../specs/operations/observability.md)
- [Security and Trust](../specs/operations/security-and-trust.md)
