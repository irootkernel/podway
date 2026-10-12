# Workspace and Session Lifecycle

Read this reference only when the user asks to initialize Podway, start or replace a session, archive or purge retained state, cancel or reset state, remove Podway from a workspace, repair a workspace, or inspect retired lifecycle guidance.

## Diagnose first

1. Run `podway version --json --identity` when binary identity is relevant. Require
   Output v4, version-result v2 and contract-manifest v2 for the current single CLI.
   Record its manifest/source/build identity, not just semver. Daemon, job and
   account-runtime commands are retired.
2. Inside an initialized target Git worktree, inspect `.podway/config.yaml` and use `podway observe --json` for current session state. Use `podway workspace show` and `podway doctor` when workspace identity or health needs diagnosis.
3. Use `podway help <route>` before every unfamiliar lifecycle command.
4. Preserve existing state on a diagnostic failure. Do not repair, reset, or remove anything merely to make the diagnostic pass succeed. Carry an established explicit authorization forward within its exact scope.

## Initialize and start

- Run `podway init` only when the user explicitly asks to initialize the target worktree. Do not initialize scratch worktrees that merely perform side work for a session owned elsewhere.
- Before starting, inspect choices with `podway preset list` and `podway preset explain <name>`. This skill supports only the built-in Procedure v2 presets `analysis-v2`, `bug-fix-v2`, `small-change-v2`, and `sw-dev-v2`. Select `analysis-v2` for source-backed research or technical analysis; select any preset only when it matches the user's task, otherwise author and review a bounded custom Procedure v2.
- For a custom file, validate and preview it using `$create-podway-procedure`. Start it with the exact `--expect-procedure-digest` reported by the reviewed v2 document; omitting the digest fails with `DIGEST_CONFIRMATION_REQUIRED`. Presets do not use this option.
- Dry-run a start when its task, Procedure, or replacement effect needs review.
- Plain `start --dry-run` previews only the Procedure when no existing-session policy is supplied. Use `--dry-run --on-existing preserve|delete` to inspect the observed lifecycle and proposed state action. A dry run may become stale immediately, so the real command still revalidates.
- Human TTY `start` interviews only after observing an existing session. Enter or EOF continues the current work, creates nothing, and returns fresh current guidance. Prepared state offers continue or delete; running offers continue, preserve as superseded, or delete; an undisposed terminal session offers continue, handed-off, or not-required. Never supply disposition text, a supersede reason, progress summary, or actor that the user did not provide.
- Automation, JSON, quiet, and non-TTY execution never prompts. Use `--on-existing preserve --supersede-reason <text>` only when the user authorizes retaining the identified running session; add `--actor` only when supplied. Use `--on-existing delete` only for authorized prepared/running deletion. Running deletion additionally requires the user-supplied `--progress-summary <text>` and `--yes`.
- Plain `start` automatically archives a completed or cancelled current session only when its disposition is current. Treat a request to start the next session as authorization for this retention transition. Never evict history automatically.
- A successful `start` or replacement returns `podway.session-start-result/v4` and creates a prepared session at revision 0 without an active attempt or goal. Re-read `podway observe --json`, then use its fenced `session.begin` template to create attempt 1. Supply the optional initial goal, criteria, and actor only to `begin`, and supply a goal only with explicit user intent; when it is omitted, a goal-tracking Procedure still accepts `goal define` once later.

## Manage terminal and destructive operations

- Match the command to the requested deletion boundary. `reset` deletes the current session and its session-scoped history. `archive purge` permanently deletes one selected inactive session. `reset --all` reinitializes all workspace runtime state while preserving reviewable `.podway` configuration and Procedures. `workspace remove` deletes the complete `.podway` tree while preserving the Git worktree. Do not widen one request into another boundary.
- Treat `cancel` as ending the current task, not as a pause.
- Treat `reset` as deletion of session-scoped history. Show or summarize the current session first and require an explicit user request before invoking it. Default reset is eligible only for prepared state or terminal state with a disposition for the exact current revision. Force reset requires a bounded progress summary and explicit confirmation.
- A completed or cancelled session becomes eligible only after `disposition handed-off` or `disposition not-required` records its current ownership outcome. `superseded` is created only by atomic running-session preservation during `start`. Never invent the summary, reference, reason, or actor.
- Prefer `archive` when the user wants to free the current slot while retaining a completed or cancelled session. Re-read the exact terminal identity and revision, use `podway help session.archive`, and preserve every supplied fence. Confirm success only from `podway.session-archive-result/v2` with `activity: inactive`.
- Use `archive list` and `archive show --session-id <uuid>` for retained read-only state. Show includes the immutable terminal disposition, including the reason, actor, and successor ID for superseded work. Add `--verbose --history-before <n>` only when bounded history is needed. Inactive sessions cannot be restored, reactivated, or mutated.
- Treat `archive purge` as permanent deletion. Require an explicit purge request, the exact retained session ID and revision from a fresh list/show, `--yes`, an explicit stable idempotency key, and `podway help session.archive_purge`. Never purge merely to make room. After response loss, follow outcome lookup and identical-request recovery, then re-read the archive. Absence alone does not identify which caller deleted it.
- Treat `reset --all` as workspace-wide destructive reinitialization. Use it only for the exact target and within explicit authorization, with `--yes` and a stable explicit idempotency key. Preserve the workspace UUID fence when available. `--force` is accepted only with `reset --all` and is unnecessary; the command preserves reviewable configuration, Procedures, and paths outside runtime state.
- Use `workspace repair` only for a diagnosed condition and an explicit request. Plain `init` with a new key refuses an already initialized workspace; repair is a separate operation.
- Do not edit `.podway/runtime/`, SQLite files, maintenance markers, or filesystem identities manually to simulate a supported lifecycle action.

## Retired account-runtime and service commands

Daemon, job, runtime-reset, socket, mode, and detach controls return
`COMMAND_RETIRED`; they do not manage a direct workspace. Report the structured
retirement guidance. Preserve legacy user-global state and installed services.
If the user separately authorizes legacy service retirement, use the original
generation's supported owner and exact target; do not reinterpret that request
as workspace reset or removal. Never install, stop, replace, or delete a legacy
service solely to make the new CLI usable.

For an explicitly authorized daemon-era cutover, first finish or abandon old
sessions, retire the ordinary LaunchAgent through the old binary, and retire any
Aquarium managed writer through its original controller under that owner's separate
authority. Establish that all old writers are gone BEFORE installing the new CLI
or resetting shared legacy worktrees. Only then install the new CLI, clean old
account state under a separate grant, and explicitly reset each identified legacy
worktree. No automatic conversion or backup occurs. The current CLI does not own
legacy account/service cleanup.

## Remove Podway from a workspace

Use this flow only when the user explicitly asks to stop using Podway in one exact existing Git worktree and accepts deletion of its complete `.podway` tree. A generic cleanup request, a missing session, or an old registry entry is not workspace-removal authorization.

1. Resolve and state the exact absolute Git worktree root. Run `podway help workspace.remove`, then inspect the current target with `podway workspace show --json` and `podway doctor` as applicable. Obtain the current workspace UUID from supported output; never invent a missing UUID.
2. Explain that removal deletes configuration, ignore rules, custom Procedures, current and inactive sessions, runtime state, and unknown content below `.podway`. It preserves the Git worktree and every path outside `.podway` and does not mutate Git. Inspect `git status --short -- .podway` and `git ls-files -- .podway` so tracked or modified project content that will appear deleted is visible before authorization.
3. Require the request to identify the exact worktree and accept complete `.podway` deletion. Approval to reset or purge a session, retire a legacy service, or delete a Git worktree is a different boundary.
4. Immediately before mutation, re-read the workspace through supported commands and require the same absolute root and workspace UUID. For JSON or non-TTY execution, invoke `podway --worktree <absolute-root> workspace remove --force --if-workspace-uuid <workspace-uuid> --yes --idempotency-key <original-key>`, retaining the exact request for recovery. Unfenced human TTY removal requires typing the exact resolved absolute root even with `--yes`. A UUID-fenced TTY request may use `--yes` instead of that prompt.
5. Confirm success only from `podway.workspace-removal-result/v2`. Report `worktree_root`, the prior `workspace_uuid`, `podway_directory_removed`, and `already_absent`. Verify through supported read-only workspace status and filesystem inspection that the selected `.podway` tree is absent while the Git worktree remains.
6. After response loss or an interrupted removal, preserve the original request, key, and UUID. Reinspect the exact root with supported workspace diagnostics and follow its marker-backed maintenance recovery recipe. Resume only the original canonical operation; do not invent a new key, manually delete residual state, or weaken the UUID fence. Stop if the root, UUID, filesystem identity, or deletion boundary is ambiguous.

Do not combine workspace removal with legacy service retirement, Git cleanup, worktree deletion, commit, or publication unless each additional operation is explicitly requested.

## Discard the current session

Use this flow only when the user explicitly asks to remove, discard, clear, or reset the current session. Do not decide that a session is stale merely because it is old, incomplete, or unrelated to the current task.

1. Run `podway observe --json` and require `podway.observation-result/v5`. Summarize the Procedure ID and purpose, lifecycle, current node and attempt when present, session ID and revision, current terminal disposition when present, and recorded progress.
2. If observation returns `SESSION_NOT_FOUND`, report that no current session exists and stop successfully. Do not escalate to `reset --all`.
3. Explain the requested disposition. `cancel` terminally abandons a running task but preserves the current session and its history. `archive` frees the current slot while retaining an eligible terminal session as inactive. `reset` deletes the current session and all session-scoped history while preserving workspace initialization. When deletion is the stated goal, use `reset` directly; when preservation is the goal, use `archive`. Do not cancel first solely to enable either operation.
4. Read `podway help session.reset`. Preview the exact observed target with `podway reset --dry-run`, passing the latest workspace UUID, session ID, and session revision fences. Do not pass `--yes`, an idempotency key, or a progress summary to the dry run.
5. Read the structured eligibility result. A prepared session is eligible immediately. A terminal session with `required_action: record_disposition` needs a caller-supplied current disposition before default reset. A running or otherwise ineligible session requires force mode; show the irreversible history loss and obtain a bounded progress summary plus explicit authorization. A dry run is not authorization and may become stale immediately.
6. After authorization and any required disposition, re-run `podway observe --json`. Use only the fresh `session.reset` mutation template, substitute a unique stable idempotency key, and preserve every supplied fence. Invoke eligible reset without `--yes`; invoke force reset with the authorized `--progress-summary <text> --yes`.
7. On an uncertain mutation outcome, retain the original canonical request and idempotency key and follow outcome-lookup recovery. Do not blindly retry or weaken a fence.
8. After reset success, run `podway observe --json` again and require `podway.error/v2` with `SESSION_NOT_FOUND`. Report that workspace initialization remains and that only the session-scoped state was deleted.

After every other lifecycle mutation, re-read `podway observe --json` or the relevant workspace status and report the resulting state. Do not imply that a session mutation installed, committed, pushed, or executed project work.
