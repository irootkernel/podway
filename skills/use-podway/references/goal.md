# Session Goals and Criteria

Read when choosing, defining, assessing or revising a goal, or operating Podway
alongside an authorized Codex goal. A goal states an accountable outcome.

## Choose and define

- Goal tracking belongs to the base Procedure. `small-change-v3` is untracked.
  `analysis-v3`, `bug-fix-v3` and `sw-dev-v3` track goals. Untracked sessions
  reject goal input with `GOAL_TRACKING_NOT_ENABLED`. A tracked session needs a
  current goal before group activation and a fresh base assessment before finish.
- With explicit intent, supply `--goal <statement> --criterion <id>=<statement>`
  at `begin` to define revision 1 together with the first attempt. If omitted
  at begin, `goal define` can define it once while running. Read current fences
  and exact grammar before invoking it.
- Write one outcome sentence of at most 1,000 characters and 1..16 distinct,
  externally checkable criteria of at most 300 characters each. Use unique
  lowercase kebab-case IDs starting with a letter (at most 64 bytes). Every
  criterion must admit an unsatisfied result. Identify what current recorded
  evidence would substantiate each one.

## Assess and decide

Assess at the active tracked base Decision that declares the session-goal
assessment. Read `result.goal.criteria`, `result.cursor.goal_revision` and
`result.guidance.evidence` in v8 `observe`. On a valid commit-bound first reply,
read the same fields under `result.observation`. Use only declared currently
resolved evidence with a non-null scoped attempt, or current recorded Decision
items. Explain actual findings. Podway validates citation applicability and
shape. The caller owns semantic assessment.
Citations are optional for satisfied and unsatisfied results; a citation-free result is a caller assertion that Podway records but does not substantiate, so cite resolved evidence for every non-retirement status.

Use `goal assess-criteria --stdin` for an atomic batch. Copy the
`goal.assess_criteria` `stdin_template` with its exact `context`, or assemble
`podway.scoped-criterion-assessment-input/v1` using the shared
[stdin fence rules](../SKILL.md#command-execution). Add optional actual `actor`
and 1..16 uniquely named entries in `assessments`. Every entry supplies
`criterion`, `status`, `reason` and `citations`. Status is `satisfied`,
`unsatisfied` or `not_applicable`. Cite up to four distinct sources per entry:

| Citation | Current source |
| --- | --- |
| `{"kind":"local","node":"<node-id>"}` | Resolved declared evidence from a node in the active scope |
| `{"kind":"plan"}` | Resolved declared current plan evidence port |
| `{"kind":"execution_result"}` | Resolved declared output port of the selected completed group |
| `{"kind":"item","item":"<item-id>"}` | Recorded item on this Decision attempt |

Use `citations: []` on `not_applicable`. Single `goal assess-criterion` uses
`--evidence <node-id>`, `--evidence @plan`, `--evidence @execution-result`
and/or `--item <item-id>` for the same supported sources. Supply the current
argv fences and key. This single-criterion command has no `--with-observation`.
A scoped `CRITERION_CITATION_INVALID` detail identifies currently citable
sources. After a refusal, read fresh state and fix the underlying reference.
Do not delete a needed citation to avoid repairing its source.

A current set is decisive only when all criteria are recorded and applicable.
The entire set uses one mode: satisfied/unsatisfied assessment, or all
not-applicable retirement. Every criterion satisfied yields `achieved`, any
unsatisfied criterion yields `not_achieved`, and a complete not-applicable set
yields `superseded`. Read `result.goal.outcome` and
`guidance.allowed_option_ids` from fresh observation. Choose the declared
ordinary `decide` option matching the outcome. Outcome spelling may differ
from the actual option ID. A complete batch from a known first submission
may carry the next fenced `session.decide` in its commit-bound observation.
An exact replay or recovered outcome is historical. Read fresh state before
making another request.

If an optional Decision value is cleared after a result cites it, the edit can
succeed yet make the recorded outcome unavailable. Follow current guidance
and use a fresh assessment attempt when needed. Retained results do not
satisfy a fresh attempt. Duplicate assessment in one current attempt does not
replace its settled assertion.

## Revise or reactivate

Revise when the intended result changes under explicit authority. Restate the
complete goal and criteria with `goal revise`, the declared `--rework-to`
target and `--reason`. Once a group exists, target the base anchor, supply
`--basis-description`, `--basis-digest` and `--feedback`, and settle affected
workers and uncertain external effects before closing the selection. The new
goal revision requires a fresh design and activation. Prior groups and results
remain retained under their original bindings. Goal revision cannot act as a
substitute for a failed replan guard or local correction. Use `--reactivate`
only for a completed current session with separate user intent. Cancelled and
archived sessions cannot reactivate.

## Cooperate with Codex goals

Codex and Podway have independent goal authority and completion rules. An
active Podway session may cover only part of a broader task. Verify external
results against the actual request before completing the broader work.
Continue independent work when a Podway dependency blocks one path. Follow
that dependency's normal recovery rules and preserve the same target.
