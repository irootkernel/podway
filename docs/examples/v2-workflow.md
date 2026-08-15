# Procedure v2 Workflow

This walkthrough exercises the shipped `bug-fix-v2` preset through the normal
daemon endpoint. Run it in the Git worktree whose task Podway should guard. The
canonical preset records a caller's evidence and decisions. Podway enforces the
declared progression rules, but it does not run checks or decide whether recorded
claims are true. The local socket trusts same-user processes; actor labels provide
correlation, not authentication or authorization.

## Prepare the workspace

Initialize Podway state, then inspect the shipped preset and its pinned digest:

```bash
podway init
podway --json preset show bug-fix-v2
podway --json start \
  --preset bug-fix-v2 \
  --task "fix duplicate session creation" \
  --goal "Prevent duplicate sessions without regressing login." \
  --criterion reproduced="The original defect is recorded." \
  --criterion verified="Fresh verification supports the correction." \
  --actor developer \
  --dry-run
```

The dry run returns `podway.output/v3` with
`result.schema: podway.session-start-result/v2`, `result.dry_run: true`, and the
shipped `result.procedure_digest`; it creates no session. Remove `--dry-run` to
start the session.

## Read stable state before each mutation

```bash
podway --json observe --wait-for-idle
```

Automation uses the JSON contract, never human-readable text. The observation
returns `podway.observation-result/v1`; read these stable fields:

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
successful retry creates a fresh attempt; re-read status before recording any
new item:

```bash
podway --json retry \
  --reason "verification used the wrong feature flags" \
  --if-workspace-uuid <workspace-uuid> \
  --if-session-id <session-id> \
  --if-session-revision <session-revision> \
  --if-attempt <attempt-id> \
  --idempotency-key bug-42-retry-verify
```

At `decide-verification`, select only an ID from
`result.allowed_option_ids[]`. The reason is the caller's recorded judgment, not
a truth determination made by Podway:

```bash
podway --json decide \
  --option passed \
  --reason "The recorded regression and surrounding checks exited successfully." \
  --actor verifier \
  --if-workspace-uuid <workspace-uuid> \
  --if-session-id <session-id> \
  --if-session-revision <session-revision> \
  --if-attempt <attempt-id> \
  --idempotency-key bug-42-decide-verification
```

The success result exposes `result.effect`, `result.target_graph_node_id`,
`result.target_attempt_id`, and `result.revision`. Re-read status instead of
inferring later state from these fields alone.

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

At `assess-session-goal`, record one assessment per criterion. Evidence and item
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
