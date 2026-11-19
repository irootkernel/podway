# Author Procedure v3 and Execution Group v1

Follow repository authority for naming, placement, purpose and checks. The
usual base location is `.podway/procedures/<procedure-id>.yaml` under
configured `procedure_paths`. Keep group definitions at their own canonical
paths. Use `podway help procedure.<operation>`, `podway preset show sw-dev-v3`,
and the exact binary's manifest-bound schemas. Human views alone do not define
the grammar. On `WORKSPACE_CONFIG_INVALID` or an excluded startup target,
leave runtime repair to `$use-podway` under separate authorization.

## Design the pair

- One base `execution_group` boundary names the unskippable Action `anchor`,
  `success_return`, selected `plan_inputs`, returned `result_items` and any
  eligible base-review `replan_decisions`. Anchor `next` equals `success_return`.
  Anchor and return dominate every real session terminal. A tracked base owns
  the final assessment at or after the return. A group owns its entry, local
  decisions, local correction targets and one actual successful terminal Action.
- The group's `plan_inputs` must match the base's selected anchor declarations
  in order. Its terminal items must supply the base's `result_items` in order.
  Compare canonically admitted IDs, types, constraints, help, conditional rules,
  choices and typed check operation contracts. `prompt` alone is ignored for the
  interface, and it still contributes to each document digest. Select any
  conditional plan controller earlier in the interface and keep it unconditionally
  required. Neither definition gains an arbitrary typed-output schema.
- Declare `evidence_from` using `scope: local` with the node, `scope: plan` for
  group access to plan inputs, or `scope: execution_result` for base consumption
  of selected completed-group outputs at or after return. Select the actual
  items needed. Required local evidence must dominate its consumer and be
  available even on a correction path. Keep genuinely branch-specific sources
  optional. Old executions and unselected values do not satisfy current gates.
- Put item prompts, help and acceptance bounds with the typed declarations.
  A `check_result` binds an authored operation and outcome contract. It is a
  caller-supplied result. Keep logs and transcripts outside Podway and reference
  actual artifacts or bounded conclusions.

## Author work, assessment and correction

Give every Action a purpose, accountable role, input basis, output and a way to
report actual negative or incomplete work. If a required result cannot be
produced, preserve the blocker and missing facts. For each Decision specify the
current evidence, available positive and negative choices, correction destination
and reason to keep. Use explicit not-applicable decisions when evidence must still
be accounted for. Session-goal criteria and option criteria serve different roles.

Use the existing mechanically decidable predicates for guards. Select the
required reference and exact item in the current scope. Keep numeric default
score inputs unconditional and use a conditional explanation for lightweight
or scored assessments. Cover the reachable cases, including legitimate failure.
Normal advancement should need no artificial attempt. Separate ordinary decision
rework from `manual_rework.allowed_targets`. Correction must target a declared
owner in the same valid scope. A Decision must never use local routing to return
to the anchor or cross its boundary. The anchor cannot be skipped or completed
as a substitute for activation.

A replan option is a distinct `effect: replan` route without `to`. Guard it with
fresh selected evidence of actual failure and declare a base Decision in
`replan_decisions` when applicable. Session-goal assessment Decisions cannot
replan. Local corrections need a viable change. Real dependencies need a
blocker, an owner and a condition for resumption. Author explicit preserve and
replan reasoning where a high score admits both routes. The native guard checks
recorded conditions. Workflow owners and affected Evaluators assess whether the
facts justify closing selection. Reuse a prior assessment only when its scope,
inputs and evidence still apply. Retain the current applicability rationale.

### Default failure policy

Success advances as soon as current requirements are satisfied. First-entry
failure uses viable local correction, second-entry failure uses lightweight
screening, and third-or-later failure uses current scored assessment. A concrete
serious structural defect can justify early scoring. Read the current
per-execution, per-Decision ordinal. Apply that progression schedule as workflow
policy. For default groups the native gates check the declared stage, verdict
and score guards. Custom guards use their own predicates and may omit the
default score and threshold.

Use the shipped score rubric in increments of ten and keep component reasons:

| Component | Supported reasons |
| --- | --- |
| Need to change DAG structure (0–50) | 0 no structural defect, 10 or 20 preference or inefficiency, 30 unsuitable boundaries, 40 missing required dependency or verification route, 50 required delivery is impossible |
| Difficulty of local correction (0–30) | 0 a viable correction, 10 alternatives remain uninvestigated, 20 attempted fixes failed but alternatives remain, 30 no viable local correction |
| Lack of progress (0–20) | 0 no repeat or meaningful improvement, 10 repeated cause with partial progress, 20 three or more failures without meaningful progress |

Use `not-scored` or `lightweight` with the unscored placeholder `0`. Record
`scored` with real component reasons and an evidence reference or reuse basis.
Below 80, retain the plan and correct viable local work or remain blocked. At
or above 80, the Coordinator may continue with a viable local correction,
choose a declared replan, or retain a blocker with the missing facts. Record
the reason for the choice. Never introduce a defect, inflate a score, repeat
a Decision to reach an ordinal, or force a full review when a scoped applicable
report exists.
Read the [worked continuation walkthrough](../../../docs/examples/continuation-v3.md)
from the exact compatible source commit or release tag for scenario paths. In
an installed skill, find `docs/examples/continuation-v3.md` in that revision.

## Bounds, findings and handoff

Choose meaningful item, list, artifact and evidence bounds. Preserve stable IDs
and version canonical semantic changes. Existing executions retain admitted
snapshots and exact definitions. Review reachability, correction dominance,
terminal bypass, cross-scope evidence, and the interface in the canonical diff.

Inspect canonical output before `procedure format <file> --write`. Repeat
`procedure check <file> --warnings-as-errors` and preview on both files.
`check` treats format drift as an error. `--warnings-as-errors` changes the
advisory exit status while retaining warning severity. Preview can still return
a start suggestion for an admissible base with advisory warnings. Fix the owning
declaration or record a justified disposition for a finding.

Copy the base's exact preview start suggestion and both canonical digest facts.
Hand off the group source/document and expected digest for the current activation
request. The caller owns scope, plan bindings, working-copy basis and any
prior-work dispositions. Keep live operation outside the authoring handoff.
