# ADR-0021: Separate Session Preparation from Execution

- Status: Accepted
- Date: 2026-08-17
- Extends: [ADR-0001](0001-current-task-session-focus.md)
- Extends: [ADR-0017](0017-single-cursor-convergence.md)

## Context

Podway currently creates a running session and its first active attempt in the
same `start` mutation. That shape cannot distinguish a caller that has only
selected and bound a Procedure from one that has started performing the entry
node. It also makes every reset look equally destructive even when a session has
no attempt, item, blocker, or goal history.

Git cleanliness cannot repair the ambiguity. Podway deliberately does not mutate
Git, a clean worktree does not account for external or untracked work, and Git
does not establish whether terminal procedure results were handed to their
owning project authority. Reset eligibility must come from state Podway owns.

## Decision

Podway persists four session lifecycle states: `prepared`, `running`,
`completed`, and `cancelled`. Absence remains the lack of a current session and
is not a lifecycle state.

`start` creates a prepared session containing the immutable Procedure snapshot,
digest, task title, and session identity. A prepared session has revision zero,
no trace members, no active cursor, and no goal revision. `begin` is the sole
transition from prepared to running. It atomically creates the first entry-node
attempt and may create and bind initial goal revision 1. The single-cursor
invariant begins at that transition: every running session still has exactly one
active attempt.

Prepared sessions admit read-only inspection, `begin`, eligible reset, and
eligible replacement. They reject item, goal, cursor, blocker, completion,
cancellation, retry, decision, and rework mutations. They are removed rather
than cancelled because execution never began.

Default reset deletes only a prepared session or a terminal session whose
current terminal revision has a caller-asserted disposition. A `handed_off`
disposition records a bounded summary and stable reference; `not_required`
records a bounded reason. Podway stores and fences the assertion but does not
verify its semantic truth or dereference it. Reactivating a completed session
makes its previous disposition stale, so a later terminal revision requires a
new disposition.

Running sessions and terminal sessions without a current disposition require an
explicit force reset or force replacement, destructive confirmation, and a
bounded progress summary. Eligible replacement is a separate atomic mode that
deletes only a default-reset-eligible current session before creating a new
prepared session. Eligibility is evaluated in the same sole-writer transaction
as deletion or replacement and never consults Git, roadmap, process, or network
state.

The lifecycle change uses SQLite schema v5. Existing v4 sessions retain their
exact running, completed, or cancelled state; migration never infers prepared
state or disposition. Prepared-aware closed public results receive new schema
versions, including a cursor-free prepared branch for `session.next`.
`session.begin` and terminal disposition receive new command routes.
Existing automation that supplied initial goal data to `start` must use
`start`, observe the prepared session, then issue fenced `begin`.

## Rejected alternatives

- Deriving prepared from a running session with no recorded values fails after
  restart and cannot distinguish external work that produced no Podway item.
- Using Git cleanliness as a reset signal crosses the read-only Git boundary and
  does not prove ownership handoff.
- Automatically deleting every completed or cancelled session loses the explicit
  distinction between terminal procedure state and durable project handoff.
- Keeping initial goal creation in `start` would create goal history before the
  session begins and preserve the allocation/execution conflation.
- Treating absence as another lifecycle value would invent a session identity
  and revision where no session exists.

## Consequences

- Callers can allocate and inspect a session without claiming that work began.
- A running session retains exactly one active attempt and all existing graph
  convergence constraints.
- Common cleanup and replacement of untouched sessions no longer needs a force
  confirmation.
- Terminal cleanup requires an explicit, inspectable ownership assertion.
- Force deletion remains possible but carries an explicit progress-loss summary.
- Clients must handle nullable cursor and goal projections for prepared sessions
  and adopt the versioned result families.
- The added lifecycle and disposition state require a transactional migration,
  replay coverage, and fail-closed reconstruction checks.
