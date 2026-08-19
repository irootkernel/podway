# Session Goals and Criteria

Read this reference when deciding whether to track a session goal, writing its statement and criteria, assessing a criterion, or revising the goal.

## Decide whether to track a goal

- Treat the session goal as the outcome this session is held accountable to across retry, rework, and actor handoff. It is not a task title, a plan, or a summary of the work performed.
- Goal tracking exists only when the Procedure opts into it, so the decision happens when choosing the Procedure for `start`. `small-change-v2` deliberately omits it, while `bug-fix-v2` and `sw-dev-v2` require a fresh assessment before any terminal node.
- Prefer an untracked Procedure when the work is bounded and mechanical and the graph's own required items already prove completion. Choosing a goal-tracked Procedure for such work produces ceremony, not assurance, and a Procedure without goal tracking rejects goal input with `GOAL_TRACKING_NOT_ENABLED`.
- Track a goal when the session may outlive its current actor, span several attempts or reworks, or end in a completion claim that someone without the conversation must be able to check.
- Define or revise a goal only with explicit user intent, and propose wording for approval instead of recording an inferred goal. Do not derive a goal from the task title.
- Settle the goal and criteria before `begin` when the outcome is already known, so goal revision 1 is created atomically with attempt 1. Otherwise `goal define` is accepted exactly once while the session is running, and a second definition fails with `SESSION_GOAL_ALREADY_DEFINED`.
- Define early rather than late. A goal-tracked graph expects the goal clarified within its first nodes, and no criterion can be assessed and no terminal node can complete while the goal is missing.
- Do not confuse a session goal criterion with the authored `criteria:` field of a decision option, which states when that option applies and belongs to the Procedure-authoring workflow.

## Write the goal statement

- State one desired end state in one sentence, scoped to what this session can deliver. Name what will be true when the session ends, not the activity that gets there. The bound reaches 1,000 characters, but a statement that long is a plan and belongs in items.
- Name both the change and the constraint it must not break, so one reading fixes the scope.
- Avoid effort verbs such as investigate, try, or work on, and avoid open-ended quality words that no observation ever closes.
- Write for an actor who has lost all context. The statement and its criteria must be enough to know what to verify without the conversation that produced them.

## Write the criteria

- Treat the criteria set as the definition of done. Podway derives the goal outcome from it, so a condition that is not a criterion falls outside done, and a criterion that is not a condition of done does not belong.
- Give each criterion exactly one externally checkable condition, and write it so that both a satisfied and an unsatisfied answer are honestly possible.
- Reject any criterion that no honest observation could ever record as `unsatisfied`. It asserts nothing, and its assessment can only ever be a rubber stamp.
- Keep criteria distinct. Overlapping criteria record one failure twice and hide which check actually failed.
- Use the `--criterion <id>=<statement>` split deliberately. The identifier names the check so a later reader can tell which check failed, and the statement states the condition that decides it.
- Keep identifiers unique, lowercase kebab-case, starting with a letter, and within 64 bytes. Keep each statement a complete English sentence within 300 characters.
- Reject the recurring failure modes: a one-word label that names a topic instead of a condition, placeholder text left from drafting, a restatement of the goal under a different name, an aspiration that no external actor could substantiate, and a duplicate that overlaps a criterion already present.
- Treat acceptance as silence, not approval. Every bound above is a shape check, and a criteria set can clear all of them while defining nothing.
- Use the fewest criteria that define done. The bound is one to 16, but a set the assessor cannot hold at once has become a checklist of steps.
- Decide before defining how each criterion will be substantiated: which resolved evidence source node or decision-local item its assessment will cite. Podway accepts an assessment with no citations, so a criterion that nothing in the graph can substantiate fails honesty, not validation.

## Accept the derived outcome

- Podway derives the outcome from the complete result set: every criterion satisfied gives `achieved`, any unsatisfied gives `not_achieved`, and every criterion not applicable gives `superseded`. Assess every criterion of the current goal revision first, or the decision fails with `CRITERION_RESULT_MISSING`.
- The following `decide` must choose the option whose declared outcome matches that derivation, and a disagreeing option is rejected with `GOAL_ASSESSMENT_OUTCOME_NOT_ALLOWED`. A criterion you cannot honestly satisfy therefore forces `not_achieved`, so add a criterion as a condition of done and never as an intention or a stretch target.
- Record `not_achieved` when the work does not support the goal. It is a supported result with its own route, and softening a statement or re-reading a criterion until it passes converts a real outcome into a false record.
- Assessment modes never mix within one assessment, and mixing fails with `CRITERION_MODE_MIXED`, so `not_applicable` cannot retire one stale criterion beside satisfied ones. Use it only when the whole goal revision stopped being the desired outcome, and use `goal revise` for anything smaller.

## Assess and revise honestly

- Assess a criterion only on the active goal-assessment decision attempt, and only after performing the cited work; the graph reaches that decision near its terminals so the goal is judged once the work is done. Podway records caller assertions and does not judge their semantic truth, so a satisfied criterion is your claim and never Podway's finding.
- Supply a reason for every status in English, reporting what was observed rather than restating that the criterion was met.
- Cite at most four resolved evidence sources or decision-local items per criterion result. A citation identifies fresh Podway state and does not validate the external truth of the claim, and a `not_applicable` result carries none.
- Revise as soon as the desired outcome itself changes, and never because a criterion became inconvenient; do not carry a goal you know is stale into its assessment. `goal revise` records a full restatement of the goal and the complete criteria set rather than a diff together with `--rework-to <graph-node-id>` and `--reason`, and it always reworks the session to that revision-safe target so a goal assessment runs again; it never edits the goal in place.
- Expect a revision, or a rework past the assessment, to invalidate freshness and to require assessing every criterion again before a terminal node, reported as `FRESH_GOAL_ASSESSMENT_MISSING`. That cost is deliberate; budget for it instead of avoiding a needed revision.
- Read `podway help goal.define`, `podway help goal.revise`, and `podway help goal.assess_criterion` for the current grammar of each command.
