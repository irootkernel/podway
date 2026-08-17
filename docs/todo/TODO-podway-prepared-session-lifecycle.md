# Podway Prepared Session Lifecycle

## Status and authority

- Document state: `Adopted`
- Dossier type: adopted design dossier
- Owning roadmap epic: `V2LIF`
- Target product release: v0.2.4
- Repository scope: Podway only
- Adoption baseline: August 17, 2026

This dossier is the decision-complete implementation plan for separating session
allocation from procedure execution and for making session deletion depend on
Podway-owned lifecycle evidence. The active roadmap owns task order and status.
Accepted ADRs, canonical machine assets, and current specifications remain
authoritative for implemented behavior until the owning tasks update them.

## 1. Verified context and evidence

Podway currently persists exactly three session lifecycle states: `running`,
`completed`, and `cancelled`. An absent session is represented by no current
session row and is not a lifecycle state. `podway start` currently creates a
`running` session, its first active attempt, and optional goal revision 1 in one
mutation. Consequently a caller that merely reserves and reviews a session must
delete a session that already looks like work began.

Current `podway reset` treats every current session as destructive history and
requires `--yes` for deletion. It cannot distinguish an untouched allocation
from work in progress or a terminal session whose result was handed off. Git
cleanliness cannot supply the missing distinction: Podway is read-only with
respect to Git, untracked or external work may be material, and a clean worktree
does not prove that procedure history is disposable.

The current canonical SQLite schema is v4. `V2SCL` has reserved
`observation-result/v2`, and `V2AST` has reserved SQLite schema v5. This epic
precedes both, so it owns those next version numbers. `V2SCL` moves its
observation reservation to v3, and `V2AST` moves its storage reservation to v6.

## 2. Goals, non-goals, and scope

The epic will:

- add a persistent `prepared` session lifecycle distinct from absence;
- make `podway start` allocate a prepared session without an active attempt or
  goal revision;
- add an atomic `podway begin` transition that creates the first active attempt
  and optional initial goal revision;
- record bounded caller-asserted terminal handoff disposition against the current
  terminal session revision;
- make default reset and eligible replacement delete only sessions whose
  Podway-owned state proves them disposable; and
- preserve explicit force deletion for running or undisposed terminal sessions
  with a bounded progress summary and destructive confirmation.

The epic will not inspect Git cleanliness, mutate Git, infer semantic completion,
validate external references, archive evidence bytes, add remote coordination,
add another active cursor, or change workspace initialization semantics.
`podway init` remains workspace initialization and does not create a session.

## 3. Accepted design and public interfaces

### 3.1 Lifecycle model

The persistent session lifecycle is the closed set `prepared`, `running`,
`completed`, and `cancelled`. Absence remains the lack of a current session and
is never serialized as a lifecycle value.

A prepared session owns its immutable Procedure snapshot, digest, task title,
session identity, and creation metadata. It has revision 0, trace sequence 0,
no attempts, no active cursor, no goal revisions, no recorded items, and no
blockers. Reconstruction rejects any prepared row that violates that shape.

Only `session.begin`, eligible `session.reset`, eligible replacement, status,
next, observation, and other read-only inspection routes admit a prepared
session. Item mutations, goal mutations, cursor transitions, blocking,
completion, retry, skip, decision, rework, and cancellation fail closed with
`SESSION_NOT_RUNNING`. Prepared sessions cannot be cancelled because no work has
begun; callers delete them through eligible reset.

`session.begin` atomically changes `prepared` to `running`, creates attempt 1 at
the Procedure entry node, and optionally creates and binds goal revision 1. The
goal arguments retain the current Procedure goal-mode validation. Begin is
fenced by workspace UUID, session ID, and session revision and is idempotent
through the durable job machinery. A repeated exact receipt is replayed; a
different or stale begin fails without mutation.

### 3.2 CLI and command routes

`podway start` retains Procedure selection, digest confirmation, task title,
workspace fencing, and dry-run support, but no longer accepts initial goal or
actor arguments. Its successful runtime result is
`podway.session-start-result/v3` and reports `session_state: prepared` with null
active-attempt and goal projections.

The new command is:

```text
podway begin [--goal <text> --criterion <id>=<statement>...] [--actor <text>] \
  --if-workspace-uuid <uuid> --if-session-id <uuid> \
  --if-session-revision <n>
```

It uses route `session.begin` and returns
`podway.session-begin-result/v1`. Goal, criterion, and actor have the same bounds
and caller-attribution semantics as the former start arguments.

Terminal disposition uses route `session.terminal_disposition` and these closed
CLI forms:

```text
podway disposition handed-off --summary <text> --reference <text> <fences>
podway disposition not-required --reason <text> <fences>
```

Summary, reference, and reason are non-blank and bounded to 4,000 Unicode
scalars each. The record also stores disposition kind, actor when supplied,
recorded time, session ID, and the exact terminal session revision. It is a
caller assertion, not a Podway verification of the referenced handoff.

`podway reset` without `--yes` performs an eligible reset. It atomically deletes
the current session only when it is prepared or has a current terminal
disposition. `--dry-run` reports lifecycle, eligibility, disposition state, and
the exact required mode without mutation. `podway reset --yes` is the force
mode for a running session or a terminal session without a current disposition;
it requires `--progress-summary <text>` and every normal identity and revision
fence. The summary is non-blank and bounded to 4,000 Unicode scalars and exists
only in the durable request and terminal receipt that deletion leaves behind.

`podway start --replace-eligible` atomically deletes an eligible current session
and creates a new prepared session. It rejects an ineligible current session
without deleting either side. Existing `--replace --yes` remains the explicit
force replacement path and requires `--progress-summary` when the replaced
session is running or lacks a current terminal disposition. Replacement remains
fully fenced and dry-runnable.

### 3.3 Terminal ownership and staleness

A completed or cancelled session is eligible for default deletion only when its
current terminal revision has exactly one disposition:

- `handed_off` requires a bounded summary and stable caller-supplied reference;
- `not_required` requires a bounded reason.

Only terminal sessions accept a disposition. Recording replaces no prior record;
one current terminal revision admits one immutable disposition, and exact
idempotent replay returns the same receipt. A completed session reactivated by
rework or goal revision retains historical disposition rows for audit but makes
them stale immediately because the session revision and lifecycle no longer
match. A later terminal revision requires a new disposition. Cancelled sessions
do not reactivate.

The eligibility decision uses only authoritative Podway state in the same write
transaction as reset or replacement. Git state, roadmap state, process state,
file mtimes, and external reference reachability are not safety signals.

### 3.4 Read models and compatibility-sensitive inventory

Prepared-aware public shapes are versioned instead of widening released closed
schemas in place. This epic reserves and then admits:

- `session-start-result/v3`;
- `session-begin-result/v1`;
- `terminal-disposition-result/v1`;
- `session-reset-result/v1`;
- `status-result/v3` and `compact-status-result/v3`;
- `prepared-next-result/v1` for the cursor-free `session.next` branch;
- `observation-result/v2`;
- shared prepared-aware result components v2; and
- the corresponding closed `output-v3`, job result, lookup, command-route,
  public-error, manifest, and compatibility-fixture branches.

Prepared status has null cursor, attempt, and goal projections.
`prepared-next-result/v1` offers only begin, eligible reset, and eligible
replacement guidance. Prepared observation contains no active items and adds the
corresponding fenced mutation templates. Existing v2
result schemas remain available for decoding previously released payloads, but a
v0.2.4 daemon emits only the new family for affected routes.

The canonical SQLite schema becomes v5. It admits the prepared lifecycle shape
and adds terminal-disposition storage bound to terminal session revisions. The
v4-to-v5 migration preserves every existing session exactly as running,
completed, or cancelled; it never infers prepared state or terminal disposition.
Downgrade remains fail-closed. `V2AST` therefore owns SQLite schema v6.

Because this epic publishes `observation-result/v2`, `V2SCL` owns
`observation-result/v3`; its later changes retain prepared lifecycle and template
semantics. `V2SCL` continues to own `next-result/v3` because this epic does not
materially change the running next projection.

## 4. Failure handling and compatibility boundaries

- Begin from any state other than prepared fails without mutation.
- A prepared session with attempts, a cursor, a goal, items, or blockers is an
  integrity failure on reconstruction.
- Default reset and `--replace-eligible` fail with
  `SESSION_RESET_NOT_ELIGIBLE` when authoritative state is not disposable and
  return the required force or disposition action in structured details.
- Terminal disposition on prepared or running state fails with
  `SESSION_NOT_TERMINAL`; a stale revision fails through the existing conflict
  model.
- Missing, blank, oversized, or shape-inconsistent disposition and progress
  fields fail before enqueue or mutation.
- Uncertain begin, disposition, reset, and replacement outcomes use existing job
  lookup recovery. Retrying never weakens identity or revision fences.
- Existing v4 databases migrate atomically and retain their lifecycle and trace.
  No old session becomes newly eligible without an explicit disposition.
- Existing automation that supplied a goal to `start` must call `start` followed
  by fenced `begin`; the migration is intentionally visible through new result
  schemas and help text.

## 5. Roadmap ownership and dependencies

`V2LIF` depends on completed `V2REC` and precedes `V2SCL`.

- `V2LIF-001` adopts the ADR and normative lifecycle, ownership, reset, and
  compatibility specifications.
- `V2LIF-002` reserves the public routes, schemas, errors, fixtures, manifest
  branches, and canonical SQLite v5 contract without runtime admission.
- `V2LIF-003` implements prepared lifecycle values, invariants, reconstruction,
  migration, and persistence while preserving existing v4 sessions.
- `V2LIF-004` implements start, begin, terminal disposition, smart reset,
  eligible replacement, force summaries, daemon dispatch, and CLI surfaces.
- `V2LIF-005` closes migration, restart, replay, stale-fence, lifecycle, help,
  completion, and end-to-end conformance; promotes durable documentation;
  removes this dossier; and passes the development gate.

Before `V2LIF` is marked complete, every lasting lifecycle, ownership, storage,
compatibility, and operating decision must exist in the affected ADR, machine
contracts, specifications, architecture, implementation tips, examples, and
roadmap evidence. The final task replaces roadmap references to this dossier,
removes its TODO index entry, and deletes this file. The dossier is not moved to
roadmap archive.

## 6. Verification and acceptance

Acceptance requires focused core, protocol, store, daemon, CLI, migration, and
end-to-end tests plus the complete `make test` development gate. Coverage must
include:

- valid and invalid prepared reconstruction;
- start without cursor or goal followed by atomic begin with and without a goal;
- forbidden prepared mutations and cancellation;
- v4-to-v5 migration with running, completed, and cancelled sessions unchanged;
- restart and durable replay for begin, disposition, reset, and both replacement
  modes;
- disposition currentness across terminal revisions and completed reactivation;
- eligible prepared and disposed-terminal deletion;
- rejection of undisposed terminal and running default deletion;
- explicit force deletion and replacement with required progress summary;
- stale workspace, session, revision, attempt, and idempotency conflicts;
- prepared status, next, observation, human output, help, and completions;
- closed schema validation, manifest identity, compatibility fixtures, response
  bounds, and downgrade rejection; and
- one real isolated CLI/daemon persistence flow proving start, restart, begin,
  complete or cancel, disposition, and reset.

The epic is development-complete only after every task has its isolated task-ID
commit, the final integrated revision passes the development gate, the complete
epic Mulgae target has zero unresolved valid findings, and the roadmap and durable
authorities no longer depend on this dossier. Release, distribution, installation,
plugin updates, daemon replacement, push, and publication are separate work.

## 7. References

- [Current lifecycle specification](../specs/domain/rework-and-lifecycle.md)
- [Current state-transition specification](../specs/domain/state-transitions.md)
- [Current SQLite model](../specs/storage/sqlite-model.md)
- [Automation client contract](../specs/interfaces/automation-client-contract.md)
- [Active roadmap](../roadmap/README.md)
- [ADR-0017: single-cursor convergence](../architecture-decision-records/0017-single-cursor-convergence.md)
- [ADR-0019: Procedure v2-only product](../architecture-decision-records/0019-procedure-v2-only-product.md)
