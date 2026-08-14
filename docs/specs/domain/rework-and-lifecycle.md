# Rework and Lifecycle

A Procedure v2 session is running, completed, cancelled, or absent. Running means
exactly one graph-node attempt is active. Completion or a decision follows only an
edge declared by the immutable procedure. Reaching a terminal placement completes
the session after its node-specific gates pass.

`retry` abandons the current action attempt and creates a clean attempt on the same
node. `rework --to <node>` targets only a declared manual-rework placement with a
valid attempt on the current trace. It creates a new target attempt and applies the
procedure's explicit evidence invalidation rules; node identity and trace membership
remain authoritative throughout the transition.

Action completion is forbidden while required items are missing, blockers are open,
referenced evidence is unresolved, or required goal conditions are unsatisfied.
Skipping is allowed only when the action declares it and its reason policy is met.
Decisions accept only declared options and route effects. Goal revision follows its
own revision fence and may reactivate a completed goal-tracked session only through
the explicitly authorized command path.

`cancel` terminally abandons the active attempt. `reset` removes the session while
preserving workspace initialization. `reset --all` recreates runtime state through
the guarded filesystem protocol; it is the supported recovery after the user has
backed up state rejected as `LEGACY_PROCEDURE_STATE_UNSUPPORTED`.
