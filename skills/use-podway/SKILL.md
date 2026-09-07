---
name: use-podway
description: Use only when the user explicitly asks to use Podway for the current workflow or to inspect, advance, initialize, start, replace, archive, purge, cancel, discard, reset, remove a workspace, diagnose, or recover Podway state. Operate Procedure v2 safely by reading authoritative state, recording supported results, following graph transitions, and reconciling uncertain mutations. Do not use for authoring Procedure source or merely because Podway is installed or configured, a repository contains Podway files, another skill mentions optional Podway integration, or a session already exists.
---

# Use Podway

## Require explicit activation

- Treat Podway as opt-in for the current user request, not for the repository. Activate it only when the request explicitly asks to use or manage Podway, or when the user approves a workflow envelope that explicitly names the Podway operations in scope.
- Treat the binary, daemon, `.podway/config.yaml`, Procedure files, repository guidance about optional integration, and existing session state as availability facts only. None activates Podway by itself.
- Without explicit activation, do not run Podway commands, inspect or attach to an existing session, validate Podway integration, or let Podway state block unrelated work. Continue the non-Podway workflow independently.
- Context restoration or automatic continuation of the same authorized workflow retains its original scope and operation boundaries; do not request activation again. A new workflow requires its own applicable authorization. If prior authorization cannot be established, do not infer it from runtime state or an active Codex goal.
- Treat an explicit request to diagnose or discard the current session as activation for that lifecycle operation only.

## Preserve the boundary

- Treat Podway as a procedure guard, not a task runner, semantic judge, project manager, Git mutation layer, or security boundary.
- Perform the external work before recording its result. Never mark an item or criterion satisfied merely because Podway requests it.
- Use JSON fields and stable error codes for decisions. Use human output only for an interactive explanation.
- Use `podway help <route>` as the current command grammar. Do not invent flags from this skill.
- Do not install Podway or mutate repository guidance merely because this skill is present.
- For creating, revising, reviewing, or hardening custom Procedure source, use `$create-podway-procedure` when it is available. Do not author the Procedure through this runtime skill.
- Support Procedure v2 only. On `LEGACY_PROCEDURE_STATE_UNSUPPORTED`, stop and ask the user to back up the runtime state before authorizing `podway reset --all`.

## Enter a worktree

Enter a worktree only after explicit activation for the current workflow.

1. Confirm that `podway` is available. If it is absent, report that fact and do not install it without an explicit request.
2. Check for `.podway/config.yaml` in the owning Git worktree.
3. If the workspace is initialized, run:

   ```bash
   podway observe --json --wait-for-idle
   ```

4. On success, require `result.schema` to identify `podway.observation-result/v3`. If observation returns `SESSION_NOT_FOUND`, record that the initialized workspace has no current session and continue to the archive advisory. Handle every other error normally.
5. Treat a successful observation as authoritative. Do not rely on chat memory. Read identity and queue facts from `status`, current guidance from `guidance`, item declarations and bounded values from `active_items`, and fenced mutation recipes from `mutation_templates`. Add any CLI-required semantic subcommand or value described by command help; the template deliberately does not invent them. Prepared guidance has no cursor and offers begin, eligible reset, and start with explicit deletion policy. A null `guidance` means the session is completed or cancelled: an undisposed terminal revision offers only terminal disposition, while a disposed terminal revision is eligible for archive, reset, or automatic archival by the next start. Use `status --verbose` only when history is needed and `next` only for compatibility with callers that need its narrower result.
6. After a successful observation or `SESSION_NOT_FOUND`, run `podway archive list --json` once for this workflow entry and require `result.schema` to identify `podway.session-archive-list-result/v1`. The result is newest first. If `count` is greater than 10, tell the user the current count and present exactly these two alternatives:

   - Reinitialize the complete workspace runtime, deleting all current and inactive session state while preserving reviewable `.podway` configuration and Procedures:

     ```bash
     podway reset --all --force --yes
     ```

   - Permanently delete only the oldest inactive session, using its `session_id` and `session_revision` plus `workspace.uuid` from the same archive-list envelope:

     ```bash
     podway archive purge --session-id <oldest-session-id> --if-session-revision <revision> --yes --if-workspace-uuid <workspace-uuid>
     ```

   This is an advisory threshold, not authorization or a product retention limit. Do not run either command or block the current workflow. Before an explicitly authorized operation, read [references/lifecycle.md](references/lifecycle.md) and the applicable command help, then re-read the target state required there. After an authorized purge, re-read `archive list` before suggesting another candidate. If the advisory read fails independently of observation, report that the count was unavailable and continue the requested workflow.
7. If no active session exists, continue the user's work without creating one unless the user explicitly asks to start or manage a Podway session.

`--json` is a global flag on every command. For a non-default invocation, the global endpoint options are `--worktree <path>`, `--socket <absolute-path>`, and `--timeout <duration>`.

For initialization, session creation or replacement, archive or purge, daemon control, reset, cancel, workspace removal, or workspace repair, read [references/lifecycle.md](references/lifecycle.md) before acting.

When the user explicitly asks to start a session, prefer `small-change-v2` for a bounded change that needs inspection, implementation, verification, review, and closeout but no tracked goal. Prefer `analysis-v2` for source-backed research or technical analysis. Use the other fuller goal-tracked presets when their assessment and evidence requirements match the task.

## Resume after context loss

- Treat recovered notes and historical observations as background. Before deriving the next Podway action, run `podway observe --json --wait-for-idle` in the authorized owning worktree and compare its workspace UUID and session ID with the resumed workflow's recorded identities. If either identity differs and existing authorization does not cover the change, report the mismatch and do not mutate the observed session until its authorization is established. Within the same authorized workspace and session, use the currently observed active attempt and revisions; their changes alone do not require renewed activation. Do not attach to a different session because a note names it.
- Derive new mutations from current observation, not remembered templates or fences. For an uncertain submitted mutation, retain its original idempotency key and canonical request, or their exact recovery location, and follow [references/recovery.md](references/recovery.md) before issuing another mutation. Identical-request recovery is distinct from deriving a new mutation.
- Keep agent-authored notes focused on the authorized objective and scope, decisions and reasons, unresolved dependencies, and source or evidence locations with relevant identities. Treat recorded session identifiers and stages as historical lookup hints. Avoid duplicating complete observations, graph history, or artifact contents; preserve unresolved-request recovery information.
- Context restoration alone does not invalidate an external check. Reuse results only when their target and relevant inputs still match and the current Procedure permits it. Preserve required evidence readback and freshness checks; notes and previews never replace them.
- This supplements ordinary observation and recovery rules even without Codex context management. It adds no observation before unrelated reads or external work, and resuming the same workflow does not repeat its entry archive advisory.

## Advance an active session

1. Perform only the work required by the active graph node. Side work may run concurrently, but Podway retains one authoritative active attempt.
2. Inspect `missing_required_items` and `suggestions[].argv`. Fill placeholders only with results supported by the work just performed.
3. Before a mutation, take the applicable workspace, session, attempt, goal, and item revisions from the latest JSON state. Use explicit precondition flags and a unique, stable idempotency key.
4. Record each result with the correct item command. Do not substitute a confirmation for evidence or collapse multiple actors into an unsupported claim.
   - The seven item types map to their commands: `confirm` uses `check` and `uncheck`; `text`, `choice`, and `integer` use `set`, with `--stdin` reading a text value; `list` uses `add` and `remove`; `artifact` uses `attach`; `check_result` uses `record --stdin` with the bound operation identity; `clear` removes any recorded value.
   - Keep distinct actors distinct with `--actor`, accepted by goal-bearing `begin`, terminal disposition, `decide`, `rework`, and the `goal` commands.
   - A required local artifact path is re-verified when completing the active action. A file changed after `attach` fails `complete` with `ARTIFACT_CHANGED` and must be attached again.
   - When one observation supports several item values, prefer one atomic `podway record --stdin`. Build a closed `podway.item-record-many-input/v1` document from that same observation: include its workspace UUID, session ID and revision, active attempt ID, a stable unique idempotency key, and 1..128 unique operations with each observed item revision. Use exactly one typed `record` value or `clear: true` per operation. Do not also pass identity, revision, or idempotency flags. Confirm success only from `podway.item-record-many-result/v1`; its outcomes are item-ID ordered. Any rejected operation leaves every selected item unchanged.
5. Re-read `podway observe --json --wait-for-idle` after every mutation. Never issue multiple mutations from one stale observation. Templates classify explicit-authorization requirements, but they never supply semantic values or idempotency keys; substitute supported values and a unique stable key before invocation.
6. Invoke `complete`, `skip`, `retry`, `decide`, `rework`, `block`, or `unblock` only when the current work justifies the transition and the latest v2 `allowed_actions[]` permits it.
   - Skip is a distinct disposition, not a quiet completion. It is legal only where the active placement's declared skip policy allows it; required items and open blockers do not gate it; it atomically clears the attempt's recorded item values; and a terminal skip applies the same goal and fresh-assessment readiness gates as terminal completion.
7. Stop and report a real external dependency with `podway block` only when the active task cannot progress; do not use a blocker to represent ordinary incomplete work.

Active-session item updates and justified progression do not require a separate confirmation. Session creation, start-time preservation or deletion, archival, purge, cancellation, reset, workspace-wide reset, workspace removal, daemon lifecycle changes, repair, and reactivating a completed session through `rework` or `goal revise --reactivate` require an explicit user request. Starting a new session authorizes the built-in archival of a disposed terminal session, but does not authorize deleting current work, purging inactive history, or removing the workspace's Podway state.

## Follow the Procedure v2 graph

- Follow the graph cursor and use `decide` for a declared decision and `rework` for an allowed trace target. The `decide` reason is mandatory and non-blank, and the option must come from the reported `allowed_option_ids`.
- A `rework` or `goal revise` on a completed session reactivates it, and the result reports `reactivated: true`. Cancelled sessions never reactivate.
- The goal commands are `goal define`, `goal revise`, and `goal assess-criterion`, and they require the Procedure to opt into goal tracking. `define` is accepted exactly once and requires the applicable workspace, session identity, and session revision fences, but no goal revision fence. `revise` and `assess-criterion` additionally require the exact current goal revision through `--if-goal-revision`.
- Define or revise the session goal only with explicit user intent. Assess a criterion only on the active goal-assessment decision attempt, and only after performing the cited work.
- Read `references[].state` and `readback[].state` together with the `readiness` fields `items_satisfied`, `unblocked`, `goal_ready`, and `can_advance`. Required evidence must be resolved and fresh. An unresolved optional reference is normal and never blocks readiness, `complete`, or `decide`; do not rework solely because it is unresolved. On `EVIDENCE_REFERENCE_STALE`, re-read state and rework the source only when the current graph requires it.
- Treat every `readback[].items[].preview` as a bounded excerpt, never the complete value. Read the whole value with `podway evidence read --json --source <graph-node-id> --item <item-id>`, then follow `next_page_token` until `truncated` is `false`. A page read is a query: it creates no job and no revision. On `EVIDENCE_PAGE_TOKEN_STALE` the snapshot moved, so re-read observe and restart the item from its first page; never repair a token by hand.
- Re-read state after retry or rework. Historical attempts remain inspectable but do not satisfy the fresh active attempt.

For deciding whether to track a session goal, writing its statement and criteria, assessing a criterion, revising the goal, or operating an authorized Podway workflow alongside a Codex goal, read [references/goal.md](references/goal.md) before acting.

## Recover failures

- When `details.recovery` is present, require its closed five-field shape and use only its structured `argv`. It may name only `observe`, `job lookup`, `job wait`, `daemon status`, or `doctor`; reject any recipe that recommends a mutation, lifecycle change, fence weakening, or `requires_explicit_authorization=true`. A recipe is diagnostic guidance, never authorization.
- On a stale revision, attempt, identity, or item precondition, do not weaken the fence. Re-read observe, then derive a fresh action.
- On an uncertain mutation outcome, do not blindly retry or change the idempotency key.
- On daemon, storage, job, or state-recovery problems, read [references/recovery.md](references/recovery.md) before acting.
