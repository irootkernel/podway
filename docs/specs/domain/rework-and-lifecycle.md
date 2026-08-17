# Rework and Lifecycle

## Session states

A Procedure v2 session is `prepared`, `running`, `completed`, or `cancelled`.
Absence means that the workspace has no current session and is not a lifecycle
state.

A prepared session binds the immutable Procedure snapshot, digest, task title,
session identity, and creation metadata. It has revision 0, trace sequence 0, no
attempts, no active cursor, no goal revisions, no item values, and no blockers.
Reconstruction rejects any other prepared shape. Prepared sessions permit
read-only inspection, `begin`, eligible reset, and eligible replacement. Every
item, goal, blocker, cursor, completion, retry, skip, decision, rework, and
cancellation mutation requires a running or terminal shape as defined below and
fails closed for prepared state.

`begin` atomically moves prepared to running, creates attempt number 1 and trace
sequence 1 at the immutable Procedure entry placement, and optionally creates
and binds initial goal revision 1. A running session has exactly one active
graph-node attempt, and the active attempt is the last trace member. Completion
or a decision follows only an edge declared by the immutable Procedure. Reaching
a terminal placement completes the session after its node-specific gates pass.

Completed and cancelled sessions have no active attempt. The last valid attempt
of a completed session is completed or skipped. The last attempt of a cancelled
session is abandoned and invalid. Cancel is legal only for a running session and
terminally abandons its active attempt; a prepared session is deleted rather than
cancelled because it contains no execution.

## Progression and rework

`retry` abandons the current action attempt and creates a clean attempt on the
same node. `rework --to <node>` targets only a declared manual-rework placement
with a valid attempt on the current trace. It creates a new target attempt and
applies the Procedure's explicit evidence invalidation rules; node identity and
trace membership remain authoritative throughout the transition.

Action completion is forbidden while required items are missing, blockers are
open, referenced evidence is unresolved, or required goal conditions are
unsatisfied. Skipping is allowed only when the action declares it and its reason
policy is met. Decisions accept only declared options and route effects. Goal
revision follows its own revision fence and may reactivate a completed
goal-tracked session only through the explicitly authorized command path.

Reactivating a completed session creates a fresh running attempt and advances the
session revision. Any disposition recorded for the earlier terminal revision
becomes historical and is not current reset evidence. Cancelled sessions never
reactivate.

## Terminal disposition and reset ownership

A terminal disposition is an immutable caller assertion bound to one completed
or cancelled session revision:

- `handed_off` contains a non-blank summary and stable reference;
- `not_required` contains a non-blank reason.

Each text field is bounded to 4,000 Unicode scalars. The record also carries its
kind, session identity, terminal revision, optional caller actor label, and
recorded time. Podway validates shape, bounds, identity, lifecycle, revision, and
idempotency. It does not verify the summary or reason, dereference the reference,
inspect Git, or decide whether the external handoff is semantically true. One
terminal revision admits one disposition; exact idempotent replay returns its
original result.

Default `reset` is an eligible reset. It atomically removes the session and all
session-scoped history only when the session is prepared or the current terminal
revision has a disposition. It preserves workspace initialization. A running
session or a terminal session without a current disposition fails with
`SESSION_RESET_NOT_ELIGIBLE` and no deletion.

Force reset requires explicit destructive confirmation (`--yes` for JSON or
non-terminal callers, or an interactive prompt), a non-blank progress summary
bounded to 4,000 Unicode scalars, and the same workspace, session, and revision fences. The
summary is retained only in the bounded durable request and terminal receipt
that outlive session-row deletion under normal retention rules. It is not a new
evidence archive.

Eligible replacement applies the same eligibility predicate and atomically
deletes the eligible current session before creating a new prepared session.
Force replacement retains explicit confirmation and the progress-summary rule.
Eligibility is derived only from authoritative Podway lifecycle and disposition
state in the sole-writer transaction; Git cleanliness, roadmap status, process
state, and external reference reachability never participate.

`reset --all` recreates runtime state through the guarded filesystem protocol;
it remains the supported recovery after the user has backed up state rejected as
`LEGACY_PROCEDURE_STATE_UNSUPPORTED`.
