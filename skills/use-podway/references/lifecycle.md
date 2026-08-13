# Workspace and Session Lifecycle

Read this reference only when the user asks to initialize Podway, start or replace a session, control the daemon, cancel or reset state, or repair a workspace.

## Diagnose first

1. Run `podway version --json` and `podway daemon status` when installation or daemon health is relevant.
2. Inside the target Git worktree, inspect `.podway/config.yaml`, then use `podway workspace show`, `podway doctor`, and `podway status --json` as applicable.
3. Use `podway help <route>` before every unfamiliar lifecycle command.
4. Do not install, uninstall, start, stop, restart, or replace a daemon solely to make a diagnostic pass succeed.

## Initialize and start

- Run `podway init` only when the user explicitly asks to initialize the target worktree. Do not initialize scratch worktrees that merely perform side work for a session owned elsewhere.
- Before starting, inspect choices with `podway preset list` and `podway preset explain <name>`. This skill supports only the built-in Procedure v2 presets `sw-dev-v2` and `bug-fix-v2`. Select one only when it matches the user's task; otherwise author and review a bounded custom Procedure v2.
- For a custom file, validate and preview it using the Procedure-authoring workflow. Start it with the exact `--expect-procedure-digest` reported by the reviewed v2 document; omitting the digest fails with `DIGEST_CONFIRMATION_REQUIRED`. Presets do not use this option.
- Dry-run a start when its task, goal, criteria, actor label, Procedure, or replacement effect needs review.
- Use `--dry-run` for `reset` or `start --replace` when the corresponding command help exposes it. A dry run may become stale immediately, so the real command still revalidates.
- Never use `start --replace` unless the user explicitly authorizes replacing the identified current session after seeing its latest status.

## Manage terminal and destructive operations

- Treat `cancel` as ending the current task, not as a pause.
- Treat `reset` as deletion of session-scoped history. Show or summarize the current session first and require an explicit user request before invoking it.
- Treat `reset --all` as workspace-wide destructive reinitialization. Use it only for the exact target and only after explicit authorization.
- Use `workspace repair` or daemon uninstall only for a diagnosed condition and an explicit request. Preserve the current installed binary and endpoint identities unless replacement is authorized.
- Daemon replacement is not a subcommand: replacing the managed daemon means re-running `podway daemon install` with a new binary, and it remains explicit-request-only.
- Do not edit `.podway/runtime/`, SQLite files, registry metadata, sockets, or LaunchAgent files manually to simulate a supported lifecycle action.

After every lifecycle mutation, re-read `podway status --json` or the relevant daemon/workspace status and report the resulting state. Do not imply that a session mutation installed, committed, pushed, or executed project work.
