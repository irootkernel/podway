# Domain Model

Podway owns one current Procedure v3 session per worktree. It binds an immutable
base snapshot, task title, optional goal and one active attempt while running.

The base snapshot contains the canonical `podway.procedure/v3` document,
digest, source identity, graph entry, node definitions, placements, evidence
references, manual-rework policy, and admission time. Runtime state records the
active graph node, attempt history, item values, blockers, decisions, rework
effects, goal revisions, criterion assessments, and terminal outcome. Activation
binds the declared plan to one immutable Execution Group v1 definition and a
fresh execution identity. The [group model](execution-groups.md) defines scope,
return, replan and retained applicability.

An action node owns instructions, typed recorded items, optional skip policy, and
one advance edge. A decision node owns closed options and one declared route per
option. A goal-assessment node records criterion results (individually via
`goal.assess_criterion` or atomically in batches of 1 to 16 unique criteria via
`goal.assess_criteria`) and then records the session goal outcome before
following its declared route. The group's terminal action returns declared
outputs to the base. The base terminal ends the session.

Every mutation is evaluated against the immutable snapshot and explicit session,
attempt, goal, and item revision fences. A successful mutation produces one new
revision or a terminal deletion; stale or inconsistent input fails without partial
state. Podway records assertions and transition evidence but does not execute work
or decide whether an assertion is semantically true.
