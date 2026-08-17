# State Transitions

The canonical command matrix is
[`assets/specifications/state-transition-matrix.csv`](../../../assets/specifications/state-transition-matrix.csv).
The command catalog binds each route to its public result family.

All session mutations require the owning workspace identity and current session
revision. Running-node mutations additionally fence the active attempt; item
mutations fence the item revision; goal mutations fence the goal revision where
applicable. Single-item mutations retain their item-local concurrency exception
and do not require an unchanged session revision. An atomic multi-item mutation
fences the observed session and active attempt plus every selected item revision.
Durable mutations also carry an idempotency key.

The evaluator first validates identity and preconditions, then domain legality,
then writes the complete transition atomically. A changed session advances its
revision exactly once. A rejected transition does not write domain state. Retrying
the same admitted idempotency key returns the original receipt or terminal result;
using the key with a different canonical request fails.

Supported lifecycle transitions are start, start-replace, begin, complete, skip,
retry, block, unblock, cancel, terminal disposition, reset, decide, rework, goal
define/revise/assess, and typed item mutations. Graph movement is always derived
from the admitted Procedure v2 snapshot; callers cannot supply an undeclared edge
or arbitrary destination.

Start validates and durably snapshots the Procedure, creates a prepared session
at revision 0, and does not create an attempt or goal revision. Begin is the only
prepared-to-running transition. It creates the entry-node attempt and optional
initial goal revision atomically, advances the session revision once, and is
fenced by workspace UUID, session ID, and session revision. Every other
cursor-bearing or goal-bearing mutation rejects prepared state.

Terminal disposition is legal only for a completed or cancelled session and is
bound to its exact current terminal revision. Completed-session reactivation
advances the revision and makes the earlier disposition non-current. Default
reset and eligible start-replace evaluate one closed eligibility predicate in the
same transaction as deletion: prepared is eligible, and terminal is eligible only
with a disposition for the current terminal revision. Running and undisposed
terminal sessions require the explicit force mode and bounded progress summary.
Rejected eligibility, missing force data, or a stale fence writes no domain state.

An atomic item mutation contains 1 to 64 unique current-attempt item operations.
The domain rejects duplicates before sorting operations by item ID, validates the
complete set against one input state, and returns either one successor or no
successor. Changed item revisions advance independently, while the workspace and
session revisions advance exactly once when at least one item changes. A semantic
all-item no-op advances no revision. Item mutations never move the graph cursor.
