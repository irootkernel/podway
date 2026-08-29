# Workspace and Session Lifecycle

Read this reference only when the user asks to initialize Podway, start or replace a session, archive or purge retained state, control the daemon, cancel or reset state, remove Podway from a workspace, or repair a workspace.

## Diagnose first

1. Run `podway version --json` and `podway daemon status` when installation or daemon health is relevant.
2. Inside the target Git worktree, inspect `.podway/config.yaml`, then use `podway workspace show`, `podway doctor`, and `podway status --json` as applicable.
3. Use `podway help <route>` before every unfamiliar lifecycle command.
4. Do not install, uninstall, start, stop, restart, or replace a daemon solely to make a diagnostic pass succeed.

## Initialize and start

- Run `podway init` only when the user explicitly asks to initialize the target worktree. Do not initialize scratch worktrees that merely perform side work for a session owned elsewhere.
- Before starting, inspect choices with `podway preset list` and `podway preset explain <name>`. This skill supports only the built-in Procedure v2 presets `analysis-v2`, `bug-fix-v2`, `small-change-v2`, and `sw-dev-v2`. Select `analysis-v2` for source-backed research or technical analysis; select any preset only when it matches the user's task, otherwise author and review a bounded custom Procedure v2.
- For a custom file, validate and preview it using `$create-podway-procedure`. Start it with the exact `--expect-procedure-digest` reported by the reviewed v2 document; omitting the digest fails with `DIGEST_CONFIRMATION_REQUIRED`. Presets do not use this option.
- Dry-run a start when its task, Procedure, or replacement effect needs review.
- Plain `start --dry-run` previews only the Procedure when no existing-session policy is supplied. Use `--dry-run --on-existing preserve|delete` to ask the daemon for the observed lifecycle and proposed state action. A dry run may become stale immediately, so the real command still revalidates.
- Human TTY `start` interviews only after observing an existing session. Enter or EOF continues the current work, creates nothing, and returns fresh current guidance. Prepared state offers continue or delete; running offers continue, preserve as superseded, or delete; an undisposed terminal session offers continue, handed-off, or not-required. Never supply disposition text, a supersede reason, progress summary, or actor that the user did not provide.
- Automation, JSON, quiet, detached, and non-TTY execution never prompts. Use `--on-existing preserve --supersede-reason <text>` only when the user authorizes retaining the identified running session; add `--actor` only when supplied. Use `--on-existing delete` only for authorized prepared/running deletion. Running deletion additionally requires the user-supplied `--progress-summary <text>` and `--yes`.
- Plain `start` automatically archives a completed or cancelled current session only when its disposition is current. Treat a request to start the next session as authorization for this bounded retention transition. If the 32-session inactive limit is full, stop and report the exact archive/purge choices; never evict history automatically.
- A successful `start` or replacement creates a prepared session at revision 0 without an active attempt or goal. Re-read `podway observe --json --wait-for-idle`, then use its fenced `session.begin` template to create attempt 1. Supply the optional initial goal, criteria, and actor only to `begin`, and supply a goal only with explicit user intent; when it is omitted, a goal-tracking Procedure still accepts `goal define` once later.

## Manage terminal and destructive operations

- Match the command to the requested deletion boundary. `reset` deletes the current session and its session-scoped history. `archive purge` permanently deletes one selected inactive session. `reset --all` reinitializes all workspace runtime state while preserving reviewable `.podway` configuration and Procedures. `workspace remove` deletes the complete `.podway` tree while preserving the Git worktree. Do not widen one request into another boundary.
- Treat `cancel` as ending the current task, not as a pause.
- Treat `reset` as deletion of session-scoped history. Show or summarize the current session first and require an explicit user request before invoking it. Default reset is eligible only for prepared state or terminal state with a disposition for the exact current revision. Force reset requires a bounded progress summary and explicit confirmation.
- A completed or cancelled session becomes eligible only after `disposition handed-off` or `disposition not-required` records its current ownership outcome. `superseded` is created only by atomic running-session preservation during `start`. Never invent the summary, reference, reason, or actor.
- Prefer `archive` when the user wants to free the current slot while retaining a completed or cancelled session. Re-read the exact terminal identity and revision, use `podway help session.archive`, and preserve every supplied fence. Confirm success only from `podway.session-archive-result/v1` with `activity: inactive`.
- Use `archive list` and `archive show --session-id <uuid>` for retained read-only state. Show includes the immutable terminal disposition, including the reason, actor, and successor ID for superseded work. Add `--verbose --history-before <n>` only when bounded history is needed. Inactive sessions cannot be restored, reactivated, or mutated.
- Treat `archive purge` as permanent deletion. Require an explicit purge request, the exact retained session ID and revision from a fresh list/show, `--yes`, and `podway help session.archive_purge`. Never purge merely to make room; report `SESSION_ARCHIVE_LIMIT_REACHED` and let the user choose the exact target. Purge is not a durable job: after response loss, re-run `archive list`; absence confirms the deletion outcome but not which caller completed it.
- Treat `reset --all` as workspace-wide destructive reinitialization. Use it only for the exact target and only after explicit authorization.
- Use `workspace repair` or daemon uninstall only for a diagnosed condition and an explicit request. Preserve the current installed binary and endpoint identities unless replacement is authorized.
- Daemon replacement is not a subcommand: replacing the managed daemon means re-running `podway daemon install` with a new binary, and it remains explicit-request-only.
- Do not edit `.podway/runtime/`, SQLite files, registry metadata, sockets, or LaunchAgent files manually to simulate a supported lifecycle action.

## Remove Podway from a workspace

Use this flow only when the user explicitly asks to stop using Podway in one exact existing Git worktree and accepts deletion of its complete `.podway` tree. A generic cleanup request, a missing session, or an old registry entry is not workspace-removal authorization.

1. Resolve and state the exact absolute Git worktree root. Run `podway help workspace.remove`, then inspect the current target with `podway workspace show --json` and `podway doctor` as applicable. Obtain the current workspace UUID from supported output; never read or edit the registry directly and never invent a missing UUID.
2. Explain that removal deletes configuration, ignore rules, custom Procedures, current and inactive sessions, runtime state, and unknown content below `.podway`. It preserves the Git worktree and every path outside `.podway`, does not mutate Git, and leaves daemon logs outside the worktree. Inspect `git status --short -- .podway` and `git ls-files -- .podway` so tracked or modified project content that will appear deleted is visible before authorization.
3. Require the request to identify the exact worktree and accept complete `.podway` deletion. Do not treat approval to reset or purge a session, clean stale registry metadata, uninstall the daemon, or delete a Git worktree as equivalent authorization.
4. Immediately before mutation, re-read the workspace through supported commands and require the same absolute root and workspace UUID. For JSON or non-TTY execution, invoke the exact target with `podway --worktree <absolute-root> workspace remove --force --if-workspace-uuid <workspace-uuid> --yes`. For direct human TTY use, `--yes` may be omitted only so Podway can require the user to type the resolved absolute root exactly.
5. Confirm success only from `podway.workspace-removal-result/v1`. Report its prior workspace UUID, registry and content removal fields, and `already_absent` state. Then verify through supported read-only workspace status and filesystem inspection that the selected `.podway` tree is absent while the Git worktree remains.
6. Workspace removal is a synchronous maintenance mutation, not a durable job. After response loss or an interrupted removal, do not use job lookup, manually delete residual state, weaken the UUID fence, or assume either success or failure. Reinspect the exact root with supported workspace diagnostics; marker-backed recovery or an identical revalidated removal may converge the operation. Stop if the root, UUID, registry generation, filesystem identity, or deletion boundary is ambiguous.

Do not combine workspace removal with daemon uninstall, daemon log purging, Git cleanup, worktree deletion, commit, or publication unless each additional operation is explicitly requested.

## Discard the current session

Use this flow only when the user explicitly asks to remove, discard, clear, or reset the current session. Do not decide that a session is stale merely because it is old, incomplete, or unrelated to the current task.

1. Run `podway observe --json --wait-for-idle` and require `podway.observation-result/v3`. Summarize the Procedure ID and purpose, lifecycle, current node and attempt when present, session ID and revision, current terminal disposition when present, recorded progress, and queue state.
2. If observation returns `SESSION_NOT_FOUND`, report that no current session exists and stop successfully. Do not escalate to `reset --all`.
3. Explain the requested disposition. `cancel` terminally abandons a running task but preserves the current session and its history. `archive` frees the current slot while retaining an eligible terminal session as inactive. `reset` deletes the current session and all session-scoped history while preserving workspace initialization. When deletion is the stated goal, use `reset` directly; when preservation is the goal, use `archive`. Do not cancel first solely to enable either operation.
4. Read `podway help session.reset`. Preview the exact observed target with `podway reset --dry-run`, passing the latest workspace UUID, session ID, and session revision fences. Do not pass `--yes`, `--detach`, an idempotency key, or a progress summary to the dry run.
5. Read the structured eligibility result. A prepared session is eligible immediately. A terminal session with `required_action: record_disposition` needs a caller-supplied current disposition before default reset. A running or otherwise ineligible session requires force mode; show the irreversible history loss and obtain a bounded progress summary plus explicit authorization. A dry run is not authorization and may become stale immediately.
6. After authorization and any required disposition, re-run `podway observe --json --wait-for-idle`. Use only the fresh `session.reset` mutation template, substitute a unique stable idempotency key, and preserve every supplied fence. Invoke eligible reset without `--yes`; invoke force reset with the authorized `--progress-summary <text> --yes`.
7. On an uncertain mutation outcome, keep the same canonical request and idempotency key and follow the job-lookup recovery procedure. Do not blindly retry or weaken a fence.
8. After reset success, run `podway observe --json --wait-for-idle` again and require `podway.error/v1` with `SESSION_NOT_FOUND`. Report that workspace initialization remains and that only the session-scoped state was deleted.

After every other lifecycle mutation, re-read `podway observe --json --wait-for-idle` or the relevant daemon/workspace status and report the resulting state. Do not imply that a session mutation installed, committed, pushed, or executed project work.
