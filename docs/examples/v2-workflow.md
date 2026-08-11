# Procedure v2 Workflow

This walkthrough exercises the shipped `bug-fix-v2` preset through the managed
development runtime. Procedure v2 runtime admission is still development-gated;
do not use an installed daemon, an active worktree, or production state for this
workflow. The canonical preset records a caller's evidence and decisions. Podway
enforces the declared progression rules, but it does not run checks or decide
whether recorded claims are true.

## Prepare the disposable runtime

Start the isolated daemon in one terminal:

```bash
python3 tools/dev_runtime.py daemon
```

Initialize its disposable Git sandbox in another terminal, then inspect the
shipped preset and its pinned digest:

```bash
python3 tools/dev_runtime.py init
python3 tools/dev_runtime.py run -- --json preset show bug-fix-v2
python3 tools/dev_runtime.py run -- --json start \
  --preset bug-fix-v2 \
  --task "fix duplicate session creation" \
  --goal "Prevent duplicate sessions without regressing login." \
  --criterion reproduced="The original defect is recorded." \
  --criterion verified="Fresh verification supports the correction." \
  --actor developer \
  --dry-run
```

The dry run returns `podway.output/v2` with
`result.schema: podway.session-start-result/v2`, `result.dry_run: true`, and the
shipped `result.procedure_digest`; it creates no session. Remove `--dry-run` to
start the disposable session.

## Read stable state before each mutation

```bash
python3 tools/dev_runtime.py run -- --json status
python3 tools/dev_runtime.py run -- --json next
```

Automation uses the JSON contract, never human-readable text. From the status
envelope, read these stable fields:

| JSON field | CLI precondition or use |
|---|---|
| `workspace.uuid` | `--if-workspace-uuid` |
| `result.session.id` | `--if-session-id` |
| `result.session.revision` | `--if-session-revision` |
| `result.current.attempt.attempt_id` | `--if-attempt` |
| `result.goal_revision` | `--if-goal-revision` when present |
| `result.items[].item_id` and `result.items[].revision` | select the matching `--if-item-revision` |
| `result.allowed_option_ids[]` | legal `decide --option` values |
| `result.allowed_manual_rework_targets[]` | legal `rework --to` values |
| `result.queue.pending_mutations` | re-read after queued work settles |

The next envelope uses `result.node.graph_node_id`,
`result.attempt.attempt_id`, `result.revision`, `result.allowed_actions[]`, and
`result.suggestions[].argv`. These are machine fields; `title`, `intent`,
`prompt`, and other human-readable wording are not automation keys.

Pass all applicable fences for a direct mutation. The examples below abbreviate
UUIDs and revisions only for readability:

```bash
python3 tools/dev_runtime.py run -- --json set reproduction-status reproduced \
  --if-workspace-uuid <workspace-uuid> \
  --if-session-id <session-id> \
  --if-attempt <attempt-id> \
  --if-item-revision 0 \
  --idempotency-key bug-42-reproduction-status

python3 tools/dev_runtime.py run -- --json set observed-behavior \
  "Concurrent callbacks create two sessions." \
  --if-workspace-uuid <workspace-uuid> \
  --if-session-id <session-id> \
  --if-attempt <attempt-id> \
  --if-item-revision 0 \
  --idempotency-key bug-42-observed
```

Re-read status after every successful mutation and use the returned revisions.
Do not increment a revision locally or reuse a stale attempt ID.

## Advance, retry, and decide

After recording all required items for an action node, complete it with the
current fences:

```bash
python3 tools/dev_runtime.py run -- --json complete \
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
python3 tools/dev_runtime.py run -- --json retry \
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
python3 tools/dev_runtime.py run -- --json decide \
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
python3 tools/dev_runtime.py run -- --json rework \
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
python3 tools/dev_runtime.py run -- --json goal revise \
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
python3 tools/dev_runtime.py run -- --json goal assess-criterion verified \
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

When finished, stop the foreground daemon and remove only the managed runtime:

```bash
python3 tools/dev_runtime.py clean --yes
```

The [CLI specification](../specs/interfaces/cli-specification.md) owns command
behavior. Public schemas under [`assets/schemas/`](../../assets/schemas/) own JSON
shape. This walkthrough is operational guidance, not a second semantic authority.
