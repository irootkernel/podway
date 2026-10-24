# Session Goals and Criteria

Read when choosing, defining, assessing or revising a goal, or operating Podway
alongside an authorized Codex goal. A goal defines the accountable end state,
not a task title, activity list or progress summary.

## Choose and define

- Goal tracking is declared by the Procedure. `small-change-v2` omits it;
  `analysis-v2`, `bug-fix-v2` and `sw-dev-v2` require fresh assessment before
  terminal progression. An untracked Procedure rejects goal input with
  `GOAL_TRACKING_NOT_ENABLED`.
- Prefer an untracked graph for bounded work whose required items already establish
  completion. Track an outcome that needs assessment across attempts, actors or
  different terminal results. Define/revise only with explicit user intent;
  propose unknown goal wording rather than inferring it from a title.
- When known before begin, provide the initial goal/criteria there; revision 1 is
  created atomically with attempt 1. Otherwise `goal define` is accepted exactly
  once while running; a second definition fails `SESSION_GOAL_ALREADY_DEFINED`.
  Missing goal blocks assessment and terminal progression.
- `goal define` needs workspace/session identity and session revision fences, not a
  goal revision. `goal revise` and `goal assess-criterion` also require the exact
  current `--if-goal-revision`. Consult command help for their complete arguments.

Write one scoped end-state sentence, at most 1,000 characters. Use 1..16 distinct,
externally checkable criteria, each at most 300 characters, with unique lowercase
kebab-case IDs beginning with a letter and at most 64 bytes. `--criterion
<id>=<statement>` separates that ID from its condition. Every criterion must
meaningfully admit an unsatisfied result; effort, overlapping conditions and
unverifiable quality claims do not define completion.

Identify the fresh evidence source or decision-local item that can substantiate
each criterion. A recorded assertion or a structurally valid goal does not prove
external truth; Podway does not require citations to make an unsupported assertion
invalid mechanically.

## Assess and revise

Assess only at the active goal-assessment decision attempt after performing the
cited work. Supply an English reason describing what was observed. Cite only
resolved `guidance.references[]` sources and recorded items of the decision
attempt, each once, with at most four citations per result; `not_applicable`
carries no citations. Templates omit optional citation arguments: append
`--evidence <node>` and/or `--item <item>` for the supported citations, preserving
the template's fences; use `--actor` only for actual attribution. On
`CRITERION_CITATION_INVALID`, re-observe before
assessing again and cite only currently admitted sources. Under
`--contract-generation 2`, its details
(`podway.criterion-citation-invalid-details/v1`) list the active attempt's
`citable_evidence_source_nodes` and `citable_item_ids`; cite only from those
lists, and still re-observe before assessing again. The deprecated
generation-1 default keeps `podway.v2-runtime-error-details/v2`.

The complete current result set derives:

| Results | Goal outcome |
| --- | --- |
| Every criterion satisfied | `achieved` |
| Any criterion unsatisfied | `not_achieved` |
| Every criterion not applicable | `superseded` |

Assess every criterion first or receive `CRITERION_RESULT_MISSING`. Assessment
modes cannot mix (`CRITERION_MODE_MIXED`); not-applicable retires a whole goal
revision, not an inconvenient criterion. The following decision must select an
option matching the derived outcome or receive `GOAL_ASSESSMENT_OUTCOME_NOT_ALLOWED`.

Revise only when the intended outcome changes, not to make a failed assessment
pass. `goal revise` restates the entire goal/criteria with `--rework-to <node>` and
`--reason`, safely reworking to that target. Revision or rework past assessment
requires fresh assessment before terminal completion (`FRESH_GOAL_ASSESSMENT_MISSING`).
Reactivation of a completed session needs explicit intent; cancelled sessions do
not reactivate.

## Cooperate with Codex goals

Codex and Podway goals have independent creation, authorization and completion
rules. Neither creates or revises the other. When both are authorized, align
scope without duplicating the full session history or evolving criteria elsewhere.
A session may cover only part of the broader objective.

Podway success, terminal state and null guidance are not proof of the Codex goal.
Verify the actual requirements and external work; even `achieved` records caller
assertions. Continue independent remaining work before closing a broader goal.

A Podway blocker does not automatically block a Codex goal. Follow that tool's
recurrence/creation rules and continue useful work within scope. Do not poll an
unchanged dependency or reset, cancel, replace or reactivate a session merely to
synchronize statuses.
