# Workspace and Session Lifecycle

Read this reference only when the user asks to initialize Podway, start or replace a session, control the daemon, cancel or reset state, or repair a workspace.

## Diagnose first

1. Run `podway version --json` and `podway daemon status` when installation or daemon health is relevant.
2. Inside the target Git worktree, inspect `.podway/config.yaml`, then use `podway workspace show`, `podway doctor`, and `podway status --json` as applicable.
3. Use `podway help <route>` before every unfamiliar lifecycle command.
4. Do not install, uninstall, start, stop, restart, or replace a daemon solely to make a diagnostic pass succeed.

## Initialize and start

- Run `podway init` only when the user explicitly asks to initialize the target worktree. Do not initialize scratch worktrees that merely perform side work for a session owned elsewhere.
- Before starting, inspect choices with `podway preset list` and `podway preset explain <name>`. This skill supports only the built-in Procedure v2 presets `bug-fix-v2`, `small-change-v2`, and `sw-dev-v2`. Select one only when it matches the user's task; otherwise author and review a bounded custom Procedure v2.
- For a custom file, validate and preview it using the Procedure-authoring workflow. Start it with the exact `--expect-procedure-digest` reported by the reviewed v2 document; omitting the digest fails with `DIGEST_CONFIRMATION_REQUIRED`. Presets do not use this option.
- Dry-run a start when its task, Procedure, or replacement effect needs review.
- Use `--dry-run` for `reset` or `start --replace-eligible` when the corresponding command help exposes it. A dry run may become stale immediately, so the real command still revalidates.
- Never use `start --replace` unless the user explicitly authorizes replacing the identified current session after seeing its latest status.
- A successful `start` or replacement creates a prepared session at revision 0 without an active attempt or goal. Re-read `podway observe --json --wait-for-idle`, then use its fenced `session.begin` template to create attempt 1. Supply the optional initial goal, criteria, and actor only to `begin`.

## Manage terminal and destructive operations

- Treat `cancel` as ending the current task, not as a pause.
- Treat `reset` as deletion of session-scoped history. Show or summarize the current session first and require an explicit user request before invoking it. Default reset is eligible only for prepared state or terminal state with a disposition for the exact current revision. Force reset requires a bounded progress summary and explicit confirmation.
- A completed or cancelled session becomes eligible only after `disposition handed-off` or `disposition not-required` records its current ownership outcome. Never invent the summary, reference, reason, or actor.
- Treat `reset --all` as workspace-wide destructive reinitialization. Use it only for the exact target and only after explicit authorization.
- Use `workspace repair` or daemon uninstall only for a diagnosed condition and an explicit request. Preserve the current installed binary and endpoint identities unless replacement is authorized.
- Daemon replacement is not a subcommand: replacing the managed daemon means re-running `podway daemon install` with a new binary, and it remains explicit-request-only.
- Do not edit `.podway/runtime/`, SQLite files, registry metadata, sockets, or LaunchAgent files manually to simulate a supported lifecycle action.

## Discard the current session

Use this flow only when the user explicitly asks to remove, discard, clear, or reset the current session. Do not decide that a session is stale merely because it is old, incomplete, or unrelated to the current task.

1. Run `podway observe --json --wait-for-idle` and require `podway.observation-result/v2`. Summarize the Procedure ID and purpose, lifecycle, current node and attempt when present, session ID and revision, current terminal disposition when present, recorded progress, and queue state.
2. If observation returns `SESSION_NOT_FOUND`, report that no current session exists and stop successfully. Do not escalate to `reset --all`.
3. Explain the requested disposition. `cancel` terminally abandons a running task but preserves the current session and its history. `reset` deletes the current session and all session-scoped history while preserving workspace initialization. When deletion or freeing the worktree's session slot is the stated goal, use `reset` directly; do not cancel first because the following reset would erase that cancellation record.
4. Read `podway help session.reset`. Preview the exact observed target with `podway reset --dry-run`, passing the latest workspace UUID, session ID, and session revision fences. Do not pass `--yes`, `--detach`, an idempotency key, or a progress summary to the dry run.
5. Read the structured eligibility result. A prepared session is eligible immediately. A terminal session with `required_action: record_disposition` needs a caller-supplied current disposition before default reset. A running or otherwise ineligible session requires force mode; show the irreversible history loss and obtain a bounded progress summary plus explicit authorization. A dry run is not authorization and may become stale immediately.
6. After authorization and any required disposition, re-run `podway observe --json --wait-for-idle`. Use only the fresh `session.reset` mutation template, substitute a unique stable idempotency key, and preserve every supplied fence. Invoke eligible reset without `--yes`; invoke force reset with the authorized `--progress-summary <text> --yes`.
7. On an uncertain mutation outcome, keep the same canonical request and idempotency key and follow the job-lookup recovery procedure. Do not blindly retry or weaken a fence.
8. After reset success, run `podway observe --json --wait-for-idle` again and require `podway.error/v1` with `SESSION_NOT_FOUND`. Report that workspace initialization remains and that only the session-scoped state was deleted.

After every other lifecycle mutation, re-read `podway observe --json --wait-for-idle` or the relevant daemon/workspace status and report the resulting state. Do not imply that a session mutation installed, committed, pushed, or executed project work.
