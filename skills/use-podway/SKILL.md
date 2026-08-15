---
name: use-podway
description: Operate a Podway Procedure v2 session safely in a Git worktree by reading authoritative state, recording supported results, advancing graph nodes, handling decisions or rework, and recovering uncertain mutations. Also use when the user explicitly asks to initialize Podway, manage a v2 session or daemon, select or author a v2 Procedure, or diagnose Podway state.
---

# Use Podway

## Preserve the boundary

- Treat Podway as a procedure guard, not a task runner, semantic judge, project manager, Git mutation layer, or security boundary.
- Perform the external work before recording its result. Never mark an item or criterion satisfied merely because Podway requests it.
- Use JSON fields and stable error codes for decisions. Use human output only for an interactive explanation.
- Use `podway help <route>` as the current command grammar. Do not invent flags from this skill.
- Do not install Podway or mutate repository guidance merely because this skill is present.
- Support Procedure v2 only. On `LEGACY_PROCEDURE_STATE_UNSUPPORTED`, stop and ask the user to back up the runtime state before authorizing `podway reset --all`.

## Enter a worktree

1. Confirm that `podway` is available. If it is absent, report that fact and do not install it without an explicit request.
2. Check for `.podway/config.yaml` in the owning Git worktree.
3. If the workspace is initialized, run:

   ```bash
   podway observe --json --wait-for-idle
   ```

4. Require `result.schema` to identify `podway.observation-result/v1`.
5. Treat the returned observation as authoritative. Do not rely on chat memory. Read identity and queue facts from `status`, current guidance from `guidance`, item declarations and bounded values from `active_items`, and copyable fenced mutations from `mutation_templates`. A null `guidance` means the session is completed or cancelled. Use `status --verbose` only when history is needed and `next` only for compatibility with callers that need its narrower result.
6. If no active session exists, continue the user's work without creating one unless the user explicitly asks to start or manage a Podway session.

`--json` is a global flag on every command. For a non-default invocation, the global endpoint options are `--worktree <path>`, `--socket <absolute-path>`, and `--timeout <duration>`.

For initialization, session creation or replacement, daemon control, reset, cancel, or workspace repair, read [references/lifecycle.md](references/lifecycle.md) before acting.

## Advance an active session

1. Perform only the work required by the active graph node. Side work may run concurrently, but Podway retains one authoritative active attempt.
2. Inspect `missing_required_items` and `suggestions[].argv`. Fill placeholders only with results supported by the work just performed.
3. Before a mutation, take the applicable workspace, session, attempt, goal, and item revisions from the latest JSON state. Use explicit precondition flags and a unique, stable idempotency key.
4. Record each result with the correct item command. Do not substitute a confirmation for evidence or collapse multiple actors into an unsupported claim.
   - The six item types map to their commands: `confirm` uses `check` and `uncheck`; `text`, `choice`, and `integer` use `set`, with `--stdin` reading a text value; `list` uses `add` and `remove`; `artifact` uses `attach`; `clear` removes any recorded value.
   - Keep distinct actors distinct with `--actor`, accepted by `start` with a goal, `decide`, `rework`, and the `goal` commands.
   - A required local artifact path is re-verified when completing the active action. A file changed after `attach` fails `complete` with `ARTIFACT_CHANGED` and must be attached again.
5. Re-read `podway observe --json --wait-for-idle` after every mutation. Never issue multiple mutations from one stale observation. Templates classify explicit-authorization requirements, but they never supply semantic values or idempotency keys; substitute supported values and a unique stable key before invocation.
6. Invoke `complete`, `skip`, `retry`, `decide`, `rework`, `block`, or `unblock` only when the current work justifies the transition and the latest v2 `allowed_actions[]` permits it.
   - Skip is a distinct disposition, not a quiet completion. It is legal only where the active placement's declared skip policy allows it; required items and open blockers do not gate it; it atomically clears the attempt's recorded item values; and a terminal skip applies the same goal and fresh-assessment readiness gates as terminal completion.
7. Stop and report a real external dependency with `podway block` only when the active task cannot progress; do not use a blocker to represent ordinary incomplete work.

Active-session item updates and justified progression do not require a separate confirmation. Session creation, replacement, cancellation, reset, workspace-wide reset, daemon lifecycle changes, repair, and reactivating a completed session through `rework` or `goal revise --reactivate` require an explicit user request.

## Follow the Procedure v2 graph

- Follow the graph cursor and use `decide` for a declared decision and `rework` for an allowed trace target. The `decide` reason is mandatory and non-blank, and the option must come from the reported `allowed_option_ids`.
- A `rework` or `goal revise` on a completed session reactivates it, and the result reports `reactivated: true`. Cancelled sessions never reactivate.
- The goal commands are `goal define`, `goal revise`, and `goal assess-criterion`, and they require the Procedure to opt into goal tracking. `define` is accepted exactly once and requires the applicable workspace, session identity, and session revision fences, but no goal revision fence. `revise` and `assess-criterion` additionally require the exact current goal revision through `--if-goal-revision`.
- Define or revise the session goal only with explicit user intent. Assess a criterion only on the active goal-assessment decision attempt, and only after performing the cited work.
- Read `references[].state` and `readback[].state` together with the `readiness` fields `items_satisfied`, `unblocked`, `goal_ready`, and `can_advance`. Required evidence must be resolved and fresh. An unresolved optional reference is normal and never blocks readiness, `complete`, or `decide`; do not rework solely because it is unresolved. On `EVIDENCE_REFERENCE_STALE`, re-read state and rework the source only when the current graph requires it.
- Re-read state after retry or rework. Historical attempts remain inspectable but do not satisfy the fresh active attempt.

For creating or reviewing a custom Procedure, read [references/authoring.md](references/authoring.md) before acting.

## Recover failures

- On a stale revision, attempt, identity, or item precondition, do not weaken the fence. Re-read observe, then derive a fresh action.
- On an uncertain mutation outcome, do not blindly retry or change the idempotency key.
- On daemon, storage, job, or state-recovery problems, read [references/recovery.md](references/recovery.md) before acting.
