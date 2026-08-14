# Domain Model

Podway owns one Procedure v2 session per worktree. A session binds an immutable
procedure snapshot, task title, optional goal, graph execution state, and exactly
one active attempt while running.

The procedure snapshot contains the canonical `podway.procedure/v2` document,
digest, source identity, graph entry, node definitions, placements, evidence
references, manual-rework policy, and admission time. Runtime state records the
active graph node, attempt history, item values, blockers, decisions, rework
effects, goal revisions, criterion assessments, and terminal outcome.

An action node owns instructions, typed recorded items, optional skip policy, and
one advance edge. A decision node owns closed options and one declared route per
option. A goal-assessment node records the session goal outcome and criterion
results before following its declared route. Terminal placements have no outgoing
edge.

Every mutation is evaluated against the immutable snapshot and explicit session,
attempt, goal, and item revision fences. A successful mutation produces one new
revision or a terminal deletion; stale or inconsistent input fails without partial
state. Podway records assertions and transition evidence but does not execute work
or decide whether an assertion is semantically true.
