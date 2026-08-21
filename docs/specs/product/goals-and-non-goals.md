# Goals and Non-Goals

Podway prevents omitted steps in one current task without becoming a project
manager. It makes the active graph node, missing recorded items, blockers, goal
state, legal actions, and next commands explicit for humans and automation.

Podway must support finite declarative Procedure v2 graphs with action, decision,
and goal-assessment placements; one authoritative cursor; exactly one active
attempt; bounded typed items; declared evidence references; deterministic retry and
rework; durable idempotent mutations; crash recovery; and worktree-local state.

It must fail closed on stale identities, revisions, attempts, item versions, goal
versions, unsupported procedure schemas, inconsistent persistence, and unknown
mutation outcomes. It must expose stable machine contracts and useful text without
making automation parse prose.

Procedure v2 may derive conditional required items and decision-option
availability from a closed bounded typed predicate vocabulary over current
attempt state and selected fresh evidence. These predicates are deterministic
progression guards, not a general expression or policy engine.

Podway is not a project manager, CI system, command runner, Git mutation layer,
arbitrary workflow engine, plugin host, remote collaboration service, artifact
store, long-term evidence archive, AI runtime, or same-user security boundary. It
does not execute configured commands, access the network, mutate Git, or store
artifact bytes.

It does not support arbitrary expressions, OR or NOT groups, nested predicates,
scripts, environment reads, cross-session conditions, dynamic routes, or
user-defined predicate functions.

Podway may record a structurally bound external check result whose closed fields
identify an operation, caller-supplied input basis, executor, outcome, and output.
This prevents omission of those identities; it does not prove that the external
check ran, that its executor was honest, or that any supplied digest describes the
claimed bytes. Podway never calls the external operation or promotes the recorded
value into an attestation, trusted receipt, or security boundary.
