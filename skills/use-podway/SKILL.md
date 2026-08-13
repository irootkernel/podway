---
name: use-podway
description: Operate Podway safely in a Git worktree by reading and advancing an existing task session, recording supported results, handling retry or rework, and recovering uncertain mutations. Also use when the user explicitly asks to initialize Podway, manage a session or daemon, select or author a Procedure, or diagnose Podway state.
---

# Use Podway

## Preserve the boundary

- Treat Podway as a procedure guard, not a task runner, semantic judge, project manager, Git mutation layer, or security boundary.
- Perform the external work before recording its result. Never mark an item or criterion satisfied merely because Podway requests it.
- Use JSON fields and stable error codes for decisions. Use human output only for an interactive explanation.
- Use `podway help <route>` as the current command grammar. Do not invent flags from this skill.
- Do not install Podway or mutate repository guidance merely because this skill is present.

## Enter a worktree

1. Confirm that `podway` is available. If it is absent, report that fact and do not install it without an explicit request.
2. Check for `.podway/config.yaml` in the owning Git worktree.
3. If the workspace is initialized, run:

   ```bash
   podway status --json
   podway next --json
   ```

4. Treat the returned session, current attempt, readiness, missing items, allowed actions, and suggestions as authoritative. Do not rely on chat memory. When taking a task over, anchor on `task.title` and `task.procedure`, `current` for the active node or stage and attempt, `stages` or the graph state, `items` with their values and revisions, and `blockers`.
5. If no active session exists, continue the user's work without creating one unless the user explicitly asks to start or manage a Podway session.

`--json` is a global flag on every command. For a non-default invocation, the global endpoint options are `--worktree <path>`, `--socket <absolute-path>`, and `--timeout <duration>`.

For initialization, session creation or replacement, daemon control, reset, cancel, or workspace repair, read [references/lifecycle.md](references/lifecycle.md) before acting.

## Advance an active session

1. Perform only the work required by the active action or stage. Side work may run concurrently, but Podway retains one authoritative active attempt.
2. Inspect `missing_required_items` and `suggestions[].argv`. Fill placeholders only with results supported by the work just performed.
3. Before a mutation, take the applicable workspace, session, attempt, goal, and item revisions from the latest JSON state. Use explicit precondition flags and a unique, stable idempotency key.
4. Record each result with the correct item command. Do not substitute a confirmation for evidence or collapse multiple actors into an unsupported claim.
   - The six item types map to their commands: `confirm` uses `check` and `uncheck`; `text`, `choice`, and `integer` use `set`, with `--stdin` reading a text value; `list` uses `add` and `remove`; `artifact` uses `attach`; `clear` removes any recorded value.
   - Keep distinct actors distinct with `--actor`, accepted by `start` with a goal, `decide`, `rework`, and the `goal` commands.
   - A local artifact path is re-verified at session completion. A file changed after `attach` fails the completion with `ARTIFACT_CHANGED` and must be attached again.
5. Re-read `podway status --json` and `podway next --json` after every mutation. Never issue a batch of mutations from one stale snapshot. When queued work may still be pending, re-read with `--wait-for-idle` so the snapshot follows the queue barrier and reports `pending_mutations=false`.
6. Invoke `complete`, `skip`, `retry`, `decide`, `rework`, `block`, `unblock`, or the v1-only `return` and `reopen` only when the current work justifies the transition and the latest allowed actions for the session's Procedure version permit it.
   - Procedure v1 reports `allowed_actions` as the five-field object `complete`, `skip`, `retry`, `return_to`, and `cancel`. Procedure v2 reports `allowed_actions[]` as a command enumeration that never contains `session.return` or `session.reopen`, so neither verb exists for a v2 session.
   - Skip is a distinct disposition, not a quiet completion. It is legal only where the active placement's declared skip policy allows it; required items and open blockers do not gate it; it atomically clears the attempt's recorded item values; and a terminal skip applies the same goal and fresh-assessment readiness gates as terminal completion.
7. Stop and report a real external dependency with `podway block` only when the active task cannot progress; do not use a blocker to represent ordinary incomplete work.

Active-session item updates and justified progression do not require a separate confirmation. Session creation, replacement, cancellation, reset, workspace-wide reset, daemon lifecycle changes, repair, and reactivating a completed session — v1 `reopen`, or v2 `rework` or `goal revise` with `--reactivate` — require an explicit user request.

## Respect Procedure versions

- Derive the active Procedure version from JSON state.
- For Procedure v1, use the linear stage lifecycle. Use `return` for earlier-stage rework and `reopen` for a completed session.
- For Procedure v2, follow the graph cursor and use `decide` for a declared decision and `rework` for an allowed trace target. The `decide` reason is mandatory and non-blank, and the option must come from the reported `allowed_option_ids`.
- A v2 `rework` or `goal revise` on a completed session reactivates it, and the result reports `reactivated: true`. Cancelled sessions never reactivate.
- The goal commands are `goal define`, `goal revise`, and `goal assess-criterion`, and they require the Procedure to opt into goal tracking. `define` is accepted exactly once for the session and needs no revision precondition; `revise` and `assess-criterion` require the exact current goal revision through `--if-goal-revision`.
- Define or revise the session goal only with explicit user intent. Assess a criterion only on the active goal-assessment decision attempt, and only after performing the cited work.
- Only evidence presented as `resolved` satisfies readiness and decision preconditions. Read `references[].state` and `readback[].state` together with the `readiness` fields `items_satisfied`, `unblocked`, `goal_ready`, and `can_advance`. Stale or unresolved evidence cannot satisfy them, so rework the source instead of repeating a failing transition.
- Never treat v1 `return` or `reopen` as aliases for v2 `rework`.
- Re-read state after retry or rework. Historical attempts remain inspectable but do not satisfy the fresh active attempt.

For creating or reviewing a custom Procedure, read [references/authoring.md](references/authoring.md) before acting.

## Recover failures

- On a stale revision, attempt, identity, or item precondition, do not weaken the fence. Re-read status and next, then derive a fresh action.
- On an uncertain mutation outcome, do not blindly retry or change the idempotency key.
- On daemon, storage, job, or state-recovery problems, read [references/recovery.md](references/recovery.md) before acting.
