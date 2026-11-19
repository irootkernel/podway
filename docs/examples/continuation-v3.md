# Procedure v3 continuation walkthrough

The `small-change-v3` preset keeps one base Procedure and selects one group at
its planning anchor. Start with the built-in default group. If the evidence
calls for a different plan, select a replacement that still supplies the same
base results. The examples below use caller-asserted checks. Run those checks
outside Podway and retain their actual outputs.

Use the examples in an owned Git fixture with a verified successor CLI. The
example group requires Procedure v3, Execution Group v1 and a compatible
Store-generation-16 writer. A previous Procedure v2 writer cannot execute it.
Both output presentation generations use the same successor model. The commands
below use generation 2. [CLI specification](../specs/interfaces/cli-specification.md)

## Inspect and author

```bash
podway --json version --identity
podway --contract-generation 2 --json preset show small-change-v3
podway --contract-generation 2 --json procedure check \
  docs/examples/procedures/compatibility-checks-v1.yaml --warnings-as-errors
podway --contract-generation 2 --json procedure preview \
  docs/examples/procedures/compatibility-checks-v1.yaml
```

Run the authoring reads from this repository, or copy the example file into an
owned fixture and use its relative path. Use preview's canonical `digest`
for the supplied group document. The preset show result provides `base_digest`,
`group_digest`, `procedure` and `default_group` for the built-in pair.

When authoring a compatible replacement, copy the anchor's selected plan input
declarations and the base's result declarations in order. Preserve matching
types, required conditions, constraints, help and operation identities. Prompt
wording can differ. The [compatibility group](procedures/compatibility-checks-v1.yaml)
reads `scope-summary` at `prepare-cases`, records the supported and boundary
checks in separate Actions, and supplies `implementation-summary`,
`verification-command` and `verification-exit-status` from `return-results`.
It declares one local correction path to `prepare-cases`, triggered by recorded
failure with a viable correction. The two checks remain independently recorded
even though the base receives a single verification summary.

For a new base Procedure, define one unskippable anchor and its success return.
Use the selected plan ports inside the group and execution-result ports on or
after the base return. An authored replan option needs declared, required
evidence and guards, and its route uses `effect: replan` without a local
target. A group can preserve the same base interface with a different graph.
Run structural checks and preview on each file, then confirm the pair by
activating it in an owned fixture. [Execution Group contracts](../specs/domain/execution-groups.md)

## Activate and finish the normal path

1. In the owned Git fixture, run `init` and prepare the preset with
   `start --preset small-change-v3 --task <title>`. Pass
   `--worktree <fixture-root> --contract-generation 2 --json` on workspace
   commands. Take fresh generation-2
   `observe` output before each mutation. Resolve each advertised command
   template using a new caller-owned idempotency key, the exact observed
   fences, and the real values. Save the resolved request for outcome lookup
   and exact replay if output is lost. The preset is untracked, so use
   `begin` without a goal.
2. At the base `inspect` anchor, record `scope-summary` with the actual
   inspected boundary. Observe again and use the `group.activate` stdin
   template. Keep its current `context.cursor` and `plan`, use
   `source.kind: default` with the preset's current `group_digest` in
   `expected_definition`, keep the first-activation predecessor empty and
   `prior_work` empty, and supply the actual hashed working-copy basis.
   Submit with `group activate --stdin` using the explicit fixture worktree.
   Group stdin requests carry their own key and fences.
   [Activation input schema](../../assets/schemas/group-activate-input-v1.schema.json)
3. At `implement`, record the implemented change and run `complete`. At
   `verify`, run the check, record the real command and integer exit status,
   and run `complete` to enter `assess-review`. The affected Evaluator records
   `success`, `not-scored`, and the required zero placeholder whenever the
   evidence supports success, including on a later entry. Run `complete` to
   enter the `review` Decision. The Coordinator then takes the ordinary
   `ready` option with the current reason using `decide`.
4. At `return-group-result`, record all three results from the current
   evidence. Use the advertised `group return` command at the terminal group
   Action. The base enters `closeout` with the selected group's result. Read
   the declared execution-result evidence using
   `evidence read --source @execution-result --item <item>`, record
   `closeout-note` and complete the base. Record the appropriate terminal
   disposition when its factual prerequisites are met. A tracked custom base
   also needs its current post-return goal assessment before completion.

## Assess actual failures

These are separate examples of decisions for different evidence. An entry
ordinal belongs to the selected group and decision. The affected Evaluator
reads `next_decision_entry` in `observe` while preparing the assessment. The
Coordinator uses the current evidence and advertised ordinary and replan
options at the Decision.

| Scenario | Assumed observation | Assessment and supported decision |
| --- | --- | --- |
| Normal completion | The proposed default work already meets the recorded acceptance check. | Record success, `not-scored`, and a zero placeholder. Advance on the first entry. |
| One viable correction | The check finds one wrong exit status. The implementation can correct it without changing the graph. | On the first failed entry, record failure, `not-scored`, and zero. Choose `changes-requested` with the concrete correction. If the fresh check then passes, record success and advance on entry two. |
| Second-entry screening | After a first correction fixes the wrong status for accepted input, the fresh boundary check still exposes the same status error for rejected input. An identified branch can be corrected locally. | Record failure, `lightweight`, zero, and `screening` with the common cause, observed progress and viable local alternative. Take that correction if justified. |
| Third or later failure, below 80 | A required compatibility check still lacks an independent result route. Two corrections failed, but they isolated the failing input and a bounded local correction remains under investigation. | The missing route, failed corrections with an alternative, and partial progress support 40 + 20 + 10 = 70. Preserve the plan. Proceed only on a supported viable correction, or block with the missing facts. |
| At least 80, preserve the plan | The default graph lacks the independent check route. The current report records two tried corrections, alternatives still under examination, and three failed results without meaningful progress. | At 40 + 20 + 20 = 80, replan is eligible. Preserve the plan and block while the existing runner's independent evidence capability is established. If fresh facts establish a viable in-plan correction and the failure verdict still applies, resolve the blocker and record the changed facts in the reason for `changes-requested`. |
| At least 80, choose replan | Acceptance requires separately recorded results from two verification steps. Three failures show no meaningful progress, and the available local corrections cannot add the missing route. A replacement with separate placements is viable. | Record 40 for the missing required verification route, 30 for having no viable local correction, and 20 for no meaningful progress, totaling 90. After settling active work, replan to add the required route. |
| Early serious structural failure | On the first inspection of recorded verification, acceptance requires two independently gated outcomes from different runners. The single verification route cannot satisfy the requirement and no local correction within that graph can supply the gates. | A first-entry `scored` assessment is appropriate. With facts supporting 50 + 30 + 0 = 80, the Coordinator can justify early replan. |

For each third or later failed entry, or an earlier serious structural
failure, record `failure`, `scored`, the applicable total and `scored-basis`.
Use the 50/30/20 rubric in the [default group](../../assets/presets/small-change-default-v3.yaml):
need for structural change, difficulty of local correction, and lack of
progress. Each component uses its specified ten-point increments. A score of
at least 80 permits a Coordinator decision to replan. Every choice still
requires current support. A blocker preserves the missing facts and can be
resolved only when those facts change. When a required assessment cannot be
produced, block before deciding. If the changed facts require a new check and
assessment, use the declared manual rework target `verify` when observation
admits it. Run the new check there and complete `verify` to reach
`assess-review` with fresh results. Use actual failed evidence and identified
corrections. Never create a failure, inflate a score, re-enter a decision to
raise its ordinal, or replan to exercise a route.

## Select a justified replacement

For a concrete replacement, use the 90-point case in which the default
verification placement cannot supply the required separate results.
The compatible replacement adds a separate placement, output, and evidence
reference per case. Where acceptance requires authenticated external results
or separately enforced ownership, author the matching requirements explicitly.
The supplied compatibility group records caller assertions with distinct
evidence slots.

1. At `review`, confirm that the current observation advertises `replan` in
   `replan_option_ids`. Settle work that can still modify the shared working
   copy and resolve uncertain external operations first. Use the advertised
   `group.replan` stdin template with the exact current cursor, chosen option,
   factual reason, working-copy basis and feedback naming the independent
   verification route to add and the work to retain. Submit with
   `group replan --stdin`. Ordinary `decide` cannot take this route.
   [Replan input schema](../../assets/schemas/group-replan-input-v1.schema.json)
2. Keep the original execution and your working copy. The replan closes that
   selection and returns to a fresh `inspect` attempt. Record the revised
   scope at the anchor. Use `group list --session <session>` to find the
   old group and its definition, then
   `history attempts --session <session> --definition-digest <digest>
   --group-execution <execution>` to find the historical implementation
   attempt. These reads grant no current evidence eligibility.
3. Observe and take the fresh activation template. Replace the default
   source with `{"kind":"document","format":"yaml","source":"<file contents>"}`
   and set `expected_definition` to the preview digest for that exact
   document. Preserve the new plan binding and current predecessor from
   observation. Supply the actual working-copy basis and the applicable
   `prior_work` entries. For example, identify the historical
   implementation attempt with `group_execution_id`, `attempt`,
   `choice: revalidate` and a `rationale` explaining why the current plan
   keeps the implementation and reruns both case checks. Submit the
   document through `group activate --stdin` and retain the new
   execution identity. [Prior-work contract](../specs/domain/execution-groups.md#identity-and-retained-assertions)
4. At `prepare-cases`, record the actual implementation and the two expected
   outcomes from the new plan, then run `complete`. Run and record each case
   in its own Action, using `complete` after `supported-case` and after
   `boundary-case`. At `assess-cases`, record the conclusion and basis, then
   run `complete` to enter the `route-cases` Decision. If both cases meet their
   expected outcomes, choose `ready` with `decide` and record the three required
   outputs at `return-results`. Use `group return` and finish the base
   `closeout` as above. Use the correction route only for an observed defect
   with an in-scope remedy.

The source guides cover [operating a current
session](../../skills/use-podway/SKILL.md) and [authoring successor
documents](../../skills/create-podway-procedure/SKILL.md).
