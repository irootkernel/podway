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

Podway is not a project manager, CI system, command runner, Git mutation layer,
arbitrary workflow engine, plugin host, remote collaboration service, artifact
store, long-term evidence archive, AI runtime, or same-user security boundary. It
does not execute configured commands, access the network, mutate Git, or store
artifact bytes.
