# Procedure v2 Workflow

This walkthrough exercises the shipped `bug-fix-v2` preset through the normal
daemon endpoint. Run it in the Git worktree whose task Podway should guard. The
canonical preset records a caller's evidence and decisions. Podway enforces the
declared progression rules, but it does not run checks or decide whether recorded
claims are true. The local socket trusts same-user processes; actor labels provide
correlation, not authentication or authorization.

Write Procedure definitions, prompts, goal statements and criteria, narrative
text and list evidence, check-result descriptors and summaries, and mutation or
decision reasons in English. Preserve exact commands, paths, hashes, identifiers,
enumerated values, and product names verbatim. Summarize a non-English log or
source passage in English and record its digest or stable reference instead of
copying it into Podway. This is an authoring policy, not runtime validation:
Podway remains locale-neutral and accepts bounded Unicode.

## Prepare the workspace

Initialize Podway state, then inspect the shipped preset and its pinned digest:

```bash
podway init
podway --json preset show bug-fix-v2
podway --json start \
  --preset bug-fix-v2 \
  --task "fix duplicate session creation" \
  --dry-run
```

The dry run returns `podway.output/v3` with
`result.schema: podway.session-start-result/v3`, `result.dry_run: true`, and the
shipped `result.procedure_digest`; it creates no session. Remove `--dry-run` to
create a prepared session, then begin it with the optional initial goal:

```bash
podway --json begin \
  --goal "Prevent duplicate sessions without regressing login." \
  --criterion reproduced="The original defect is recorded." \
  --criterion verified="Fresh verification supports the correction." \
  --actor developer
```

For a smaller change that does not require a tracked goal, inspect and start the
lightweight preset without `--goal` or `--criterion`:

```bash
podway --json preset explain small-change-v2
podway --json start \
  --preset small-change-v2 \
  --task "update one validation rule" \
  --dry-run
podway --json start \
  --preset small-change-v2 \
  --task "update one validation rule"
podway --json begin
```

`podway init` configures `sw-dev-v2` as the default for later new sessions; it
does not reinterpret an existing session. The three established presets are version
3. A newly started preset session admits its exact current embedded bytes and
digest. A session admitted under an earlier version keeps its immutable snapshot
until it is explicitly replaced; there is no in-place migration.

`small-change-v2`'s complete graph is
`inspect -> implement -> verify -> review -> closeout`.
Review option `changes-requested` returns to `implement`; option `ready` advances
to closeout. Its manual rework targets are `inspect`, `implement`, and `verify`.
The same observation, fence, item-recording, and outcome-reconciliation rules
below apply.

## Author typed conditions

Procedure authors can make a later optional item conditionally required from an
earlier, unconditionally required item in the same definition. Predicates are
AND-combined and use the closed typed vocabulary; this example requires a summary
only when documentation was updated:

```yaml
items:
  - id: documentation-state
    type: choice
    prompt: Record the documentation state.
    required: true
    choices: [not-applicable, updated]
  - id: documentation-summary
    type: text
    prompt: Summarize the durable documentation update in English.
    required: false
    required_when:
      - item: documentation-state
        equals: updated
```

A decision guard can read only an explicitly selected item from a required,
dominating evidence reference. It controls option availability without hiding the
authored option:

```yaml
node_definitions:
  assess-review:
    type: decision
    title: Assess the review
    objective: Decide whether unresolved valid findings remain.
    prompt: Is the review ready to close?
    evidence_guidance:
      - Read the selected unresolved-finding count before deciding.
    options:
      - id: approved
        label: Review approved
        criteria: No unresolved valid finding remains.
        guards:
          - evidence:
              node: review
              item: unresolved-valid-findings
            equals: 0
      - id: changes-required
        label: Changes required
        criteria: At least one unresolved valid finding remains.
        guards:
          - evidence:
              node: review
              item: unresolved-valid-findings
            at_least: 1
    reason:
      required: true
      prompt: Explain the selected review outcome.
graph:
  entry: assess
  nodes:
    - id: assess
      use: assess-review
      evidence_from:
        - node: review
          required: true
          items: [unresolved-valid-findings]
      routes:
        approved:
          to: closeout
          effect: advance
        changes-required:
          to: review
          effect: rework
```

`podway procedure check <path>` validates controller order, predicate typing,
evidence selection, dominance, and graph rules before a session can start. At
runtime, observation exposes authored `required`, derived `required_now`, and a
bounded condition status. `podway next --json` keeps all authored options and
publishes authoritative `allowed_option_ids` plus bounded guard statuses when any
guard exists. An unavailable selection fails with
`OPTION_GUARD_UNSATISFIED`; stale required evidence retains
`EVIDENCE_REFERENCE_STALE` precedence.

Text and list guards use `empty` or `non_empty` instead of a comparison value.
Keep the option set total by authoring complementary routes over the same selected
required source:

```yaml
options:
  - id: clean
    label: No findings
    criteria: The selected findings list is empty.
    guards:
      - evidence: {node: review, item: findings}
        empty: true
  - id: rework
    label: Findings remain
    criteria: The selected findings list is non-empty.
    guards:
      - evidence: {node: review, item: findings}
        non_empty: true
```

The source must still be a dominating `required: true` evidence reference with
`items: [findings]`; guards never grant access to an undeclared or stale value.

## Read stable state before each mutation

```bash
podway --json observe --wait-for-idle
```

Automation uses the JSON contract, never human-readable text. The observation
returns `podway.observation-result/v3`; read these stable fields:

| JSON field | CLI precondition or use |
|---|---|
| `workspace.uuid` | `--if-workspace-uuid` |
| `result.status.session.id` | `--if-session-id` |
| `result.status.session.revision` | `--if-session-revision` |
| `result.status.current.attempt.attempt_id` | `--if-attempt` |
| `result.status.goal_revision` | `--if-goal-revision` when present |
| `result.active_items[].item_id` and `.revision` | select the matching `--if-item-revision` |
| `result.guidance.allowed_actions[]` | legal current mutations |
| `result.guidance.allowed_manual_rework_targets[]` | legal `rework --to` values |
| `result.status.queue.pending_mutations` | false after the requested queue barrier |

`result.guidance.readback[].items[]` carries each selected evidence item's identity,
digest, and total size. Observation guidance is metadata only: it never carries a
`preview` or a `next_page_token`, which appear only in `podway next`. Read the value
itself through the paged query and follow `next_page_token` until `truncated` is `false`:

```bash
podway --json evidence read --source review --item review-findings
podway --json evidence read --source review --item review-findings \
  --page-token <next-page-token>
```

That selected multi-page item belongs to `sw-dev-v2`; `bug-fix-v2` and
`small-change-v2` deliberately keep selected lists within one page.

`podway.evidence-read-result/v1` reports `total_size` with its `size_unit`, so a caller
knows how much remains before requesting the next page. `EVIDENCE_PAGE_TOKEN_STALE` means
the recorded value changed: re-read observe and restart that item from its first page.

`result.mutation_templates[]` supplies the applicable optimistic-concurrency
fences and states whether explicit authorization is required. Templates still
contain semantic and idempotency placeholders; callers must fill them from
performed work and their own stable key. Human-readable wording is not an
automation key.

Pass all applicable fences for a direct mutation. The examples below abbreviate
UUIDs and revisions only for readability:

```bash
podway --json set reproduction-status reproduced \
  --if-workspace-uuid <workspace-uuid> \
  --if-session-id <session-id> \
  --if-attempt <attempt-id> \
  --if-item-revision 0 \
  --idempotency-key bug-42-reproduction-status

podway --json set observed-behavior \
  "Concurrent callbacks create two sessions." \
  --if-workspace-uuid <workspace-uuid> \
  --if-session-id <session-id> \
  --if-attempt <attempt-id> \
  --if-item-revision 0 \
  --idempotency-key bug-42-observed
```

When one observation supplies several independent results, record them in one
atomic mutation. Put all fences in the closed stdin document; do not repeat
identity, revision, or idempotency flags on the command line:

```bash
podway --json record --stdin <<'JSON'
{
  "schema": "podway.item-record-many-input/v1",
  "workspace_uuid": "<workspace-uuid>",
  "session_id": "<session-id>",
  "session_revision": 12,
  "attempt_id": "<attempt-id>",
  "idempotency_key": "bug-42-record-reproduction",
  "operations": [
    {"item_id":"reproduction-status","expected_item_revision":0,"record":{"type":"choice","value":"reproduced"}},
    {"item_id":"observed-behavior","expected_item_revision":0,"record":{"type":"text","value":"Concurrent callbacks create two sessions."}}
  ]
}
JSON
```

The result schema is `podway.item-record-many-result/v1`. Its `items` array is
ordered by item ID and reports each item revision; any invalid or stale operation
leaves the whole set unchanged.

At the `bug-fix-v2` `verify` node, record the structurally bound external result
through the same atomic route. The operation ID and digest must match the preset;
the input and output digests bind caller-supplied data but do not prove that the
operation ran:

```json
{
  "schema": "podway.item-record-many-input/v1",
  "workspace_uuid": "<workspace-uuid>",
  "session_id": "<session-id>",
  "session_revision": 12,
  "attempt_id": "<attempt-id>",
  "idempotency_key": "bug-42-record-verification",
  "operations": [{
    "item_id": "verification-result",
    "expected_item_revision": 0,
    "record": {
      "type": "check_result",
      "operation_id": "bug-fix-verification",
      "operation_digest": "sha256:01122b6057efcfa3f22c453aa48fb647ccba9f7db437dd44473a9f32a854a979",
      "input_basis": {
        "descriptor": "Current bug-fix candidate and declared regression check",
        "digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      },
      "executor": {"name": "repository verification", "version": "1"},
      "outcome": "pass",
      "summary": "The declared regression check and surrounding verification passed.",
      "output_digest": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    }
  }]
}
```

Save this document and pass it to `podway --json record --stdin`; JSON shown in a
documentation block is not shell input by itself.

Re-read observe after every successful mutation and use the returned revisions.
Do not increment a revision locally or reuse a stale attempt ID.

## Advance, retry, and decide

After recording all required items for an action node, complete it with the
current fences:

```bash
podway --json complete \
  --if-workspace-uuid <workspace-uuid> \
  --if-session-id <session-id> \
  --if-session-revision <session-revision> \
  --if-attempt <attempt-id> \
  --idempotency-key bug-42-complete-reproduce
```

If an action or decision attempt must be discarded and repeated, use retry. A
successful retry creates a fresh attempt. Before recording any new item, re-run
`podway observe --json --wait-for-idle`:

```bash
podway --json retry \
  --reason "verification used the wrong feature flags" \
  --if-workspace-uuid <workspace-uuid> \
  --if-session-id <session-id> \
  --if-session-revision <session-revision> \
  --if-attempt <attempt-id> \
  --idempotency-key bug-42-retry-verify
```

At `evaluate-verification`, select only an ID from
`result.allowed_option_ids[]`. The reason is the caller's recorded judgment, not
a truth determination made by Podway:

```bash
podway --json decide \
  --option passed \
  --reason "The structurally bound result records a passing outcome for the current input basis." \
  --actor verifier \
  --if-workspace-uuid <workspace-uuid> \
  --if-session-id <session-id> \
  --if-session-revision <session-revision> \
  --if-attempt <attempt-id> \
  --idempotency-key bug-42-decide-verification
```

The success result exposes `result.effect`, `result.target_graph_node_id`,
`result.target_attempt_id`, and `result.revision`. Do not infer later state from
these fields alone; instead, re-run `podway observe --json --wait-for-idle`.

## Rework and revise the goal

Manual graph rework is a Procedure v2 command. It is not the v1 `return` or
`reopen` command:

```bash
podway --json rework \
  --to implement \
  --reason "Independent review found an unresolved cancellation path." \
  --actor reviewer \
  --if-workspace-uuid <workspace-uuid> \
  --if-session-id <session-id> \
  --if-session-revision <session-revision> \
  --if-attempt <attempt-id> \
  --idempotency-key bug-42-review-rework
```

Manual rework is allowed only by `manual_rework.allowed_targets`. A decision
route with `effect: rework` is different: its selected option owns a specific
target and may be guarded by recorded evidence. Both create a fresh target
attempt and make the replaced suffix stale; neither executes corrective work.
In `sw-dev-v2`, review option `implementation-changes` returns to `implement`,
while `documentation-changes` returns to `document`.

If the desired outcome itself changes, create a new immutable goal revision and
declare its rework target:

```bash
podway --json goal revise \
  --goal "Prevent duplicate and leaked login sessions." \
  --criterion reproduced="The original defect is recorded." \
  --criterion verified="Fresh verification covers duplication and cleanup." \
  --rework-to implement \
  --reason "Cancellation cleanup is now in scope." \
  --actor owner \
  --if-workspace-uuid <workspace-uuid> \
  --if-session-id <session-id> \
  --if-session-revision <session-revision> \
  --if-attempt <attempt-id> \
  --if-goal-revision <goal-revision> \
  --idempotency-key bug-42-revise-goal
```

## Assess criteria and close out

At `assess-goal`, record one assessment per criterion. Evidence and item
citations identify recorded Podway state; they do not validate an external test:

```bash
podway --json goal assess-criterion verified \
  --status satisfied \
  --reason "The fresh regression and cleanup checks passed." \
  --evidence verify \
  --actor verifier \
  --if-workspace-uuid <workspace-uuid> \
  --if-session-id <session-id> \
  --if-session-revision <session-revision> \
  --if-attempt <attempt-id> \
  --if-goal-revision <goal-revision> \
  --idempotency-key bug-42-assess-verified
```

The criterion result reports `result.goal_revision`, `result.result.status`,
`result.complete`, and, once every criterion is assessed,
`result.determined_outcome`. Select the matching goal-assessment decision option,
record the outcome and closeout items, and continue using `status --json` and
`next --json` until the terminal action completes.

## Record terminal ownership and reset

A completed or cancelled session is not eligible for default deletion until its
exact current terminal revision has a disposition. Record either a handoff or the
fact that no handoff is required, then use the fresh observation's reset template:

```bash
podway --json disposition not-required \
  --reason "No external handoff is required." \
  --if-workspace-uuid <workspace-uuid> \
  --if-session-id <session-id> \
  --if-session-revision <session-revision> \
  --idempotency-key bug-42-disposition

podway --json reset \
  --if-workspace-uuid <workspace-uuid> \
  --if-session-id <session-id> \
  --if-session-revision <disposed-session-revision> \
  --idempotency-key bug-42-reset
```

A prepared session is already eligible. Running work is not: force reset requires
`--progress-summary <text> --yes` and a fresh identity/revision fence.

## Optional contributor isolation

Contributors who need a disposable daemon and sandbox can run the same commands
through the development helper. Start `python3 tools/dev_runtime.py daemon` in one
terminal, run `python3 tools/dev_runtime.py init` in another, and replace each
`podway --json` invocation above with
`python3 tools/dev_runtime.py run -- --json`. When finished, stop the foreground
daemon and remove only that managed runtime:

```bash
python3 tools/dev_runtime.py clean --yes
```

The [CLI specification](../specs/interfaces/cli-specification.md) owns command
behavior. Public schemas under [`assets/schemas/`](../../assets/schemas/) own JSON
shape. This walkthrough is operational guidance, not a second semantic authority.
