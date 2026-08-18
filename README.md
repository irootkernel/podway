# Podway

Podway is a local procedure guard for one task in one Git worktree. It keeps the current task on an explicit sequence, shows what is missing, and makes retry and rework visible instead of leaving them in shell history or chat context.

Podway does not run your build commands, edit Git state, contact a remote service, or store artifact contents. You do the work; Podway keeps the process explicit.

Podway ships two matching binaries:

- `podway`, the command-line interface;
- `podwayd`, a user-scoped daemon that serializes task-state changes.

## When to use Podway

Use Podway when a task has explicit graph nodes that should not be skipped,
especially when work may be retried, handed between people or agents, or sent
through a declared rework path. Three built-in procedures cover common work:

| Preset | Best for |
|---|---|
| `sw-dev-v2` | Graph-based implementation work with decisions, evidence read-back, rework, and goal assessment |
| `bug-fix-v2` | Graph-based defect repair with decision rework and goal closeout |
| `small-change-v2` | Short inspect, implement, verify, review, and closeout path without goal tracking |

Podway admits only Procedure v2 through the normal runtime and emits successful
responses through `podway.output/v3`.

Podway is not a project manager, CI system, command runner, Git automation layer, artifact store, or multi-user access-control service.

### For AI agents and harnesses

Scripts and AI agents use the same public contract as humans. The single
`podway observe --json --wait-for-idle` command returns one authoritative,
self-contained task observation, so an agent re-derives what happens next from
the worktree instead of from its conversation history. Sessions survive context
loss and transfer between actors mid-task, and mutations accept explicit
preconditions and idempotency keys so retried or concurrent callers cannot
corrupt state. See the [Procedure v2 workflow](docs/examples/v2-workflow.md).

## Requirements

Podway supports native Apple Silicon macOS only. Intel macOS, Rosetta execution, universal binaries, Linux, and Windows are not supported release targets.

The current public package is unsigned and not notarized. Verify the published checksum before installing it. macOS may require you to authorize the downloaded binaries in System Settings before their first run.

## Install a release

Download the latest Apple Silicon archive and its `.sha256` file from [GitHub Releases](https://github.com/irootkernel/podway/releases). Then verify and extract it:

```bash
shasum -a 256 -c podway-<version>-aarch64-apple-darwin.tar.gz.sha256
tar -xzf podway-<version>-aarch64-apple-darwin.tar.gz
```

Install both matching binaries on your `PATH`:

```bash
mkdir -p "$HOME/.local/bin"
install -m 755 podway-<version>-aarch64-apple-darwin/bin/podway \
  "$HOME/.local/bin/podway"
install -m 755 podway-<version>-aarch64-apple-darwin/bin/podwayd \
  "$HOME/.local/bin/podwayd"
```

Add this line to `~/.zprofile` if `~/.local/bin` is not already on your login shell path, then open a new terminal:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

Verify the CLI:

```bash
podway version --json
```

## Start the daemon at login

Install `podwayd` as a per-user macOS LaunchAgent:

```bash
podway daemon install --daemon-path "$HOME/.local/bin/podwayd"
podway daemon status
```

This creates `~/Library/LaunchAgents/dev.podway.podwayd.plist`, starts the daemon for the current login session, and starts it again after future GUI logins. It does not run before login and does not require `sudo`. The LaunchAgent records the daemon's absolute path, so reinstall it after moving or replacing the binaries.

Useful lifecycle commands are:

```bash
podway daemon status
podway daemon start
podway daemon stop
podway daemon restart
podway daemon logs --lines 100
podway daemon logs --follow
```

## Optional: configure an AI coding agent

Podway does not modify a project's `AGENTS.md` or install an agent skill. Both integrations below are optional. They may be used independently or together, and omitting both does not affect the CLI.

For a small project-wide policy, add this template to the project's `AGENTS.md`:

```markdown
## Podway

- Podway supports Procedure v2 only. Treat `LEGACY_PROCEDURE_STATE_UNSUPPORTED` as a backup-and-reset boundary; do not edit the database or attempt conversion.
- When `.podway/config.yaml` exists, read `podway observe --json --wait-for-idle` before task work. Require `podway.observation-result/v2`, and re-read it after every mutation.
- Treat the active Podway graph node and attempt as the current work boundary. Perform the work before recording an item. Side work may run outside Podway, but record only conclusions supported by current evidence on the active attempt.
- Use JSON fields, stable error codes, explicit preconditions, and idempotency keys for mutations. Never parse human-readable output as an API.
- You may update items and advance an existing active v2 session when the work supports it. Do not run `podway init`, start or replace a session, cancel or reset state, control the daemon, or reactivate a completed session through `rework` or `goal revise --reactivate` unless the user explicitly requests it.
- Podway records assertions; it does not run the work or prove semantic truth.
```

For fuller operational guidance, install the complete [`use-podway` skill](https://github.com/irootkernel/podway/tree/main/skills/use-podway) directory under `~/agents/skills/`:

```bash
podway_skill_dir=~/agents/skills/use-podway
mkdir -p "$podway_skill_dir/references"

curl -fsSLo "$podway_skill_dir/SKILL.md" \
  https://raw.githubusercontent.com/irootkernel/podway/main/skills/use-podway/SKILL.md

for reference in lifecycle authoring recovery; do
  curl -fsSLo "$podway_skill_dir/references/$reference.md" \
    "https://raw.githubusercontent.com/irootkernel/podway/main/skills/use-podway/references/$reference.md"
done
```

The skill covers the active-session loop and loads separate references only for less frequent lifecycle, Procedure-authoring, or recovery work. Consult the agent's documentation for its skill reload requirements. The commands require network access and `curl`, and overwrite existing files with the same names. Replace `main` with a release tag or commit SHA when a reproducible, pinned skill version is required.

## Build from source

Source builds require native Apple Silicon macOS and the Rust version pinned in `rust-toolchain.toml`:

```bash
git clone https://github.com/irootkernel/podway.git
cd podway
cargo build --release --locked \
  -p podway-cli --bin podway \
  -p podway-daemon --bin podwayd

mkdir -p "$HOME/.local/bin"
install -m 755 target/release/podway "$HOME/.local/bin/podway"
install -m 755 target/release/podwayd "$HOME/.local/bin/podwayd"
podway daemon install --daemon-path "$HOME/.local/bin/podwayd"
```

Contributors should use the complete verification commands documented under [Contributor Documentation](docs/README.md), not treat a successful build as the release-readiness gate.

## Quick start

Run Podway from anywhere inside a Git worktree, including the repository's main checkout:

```bash
podway init
podway preset list
podway start \
  --preset sw-dev-v2 \
  --task "add bounded retry backoff"
podway begin \
  --goal "Retry transient writes with a bounded exponential delay." \
  --criterion verified="Fresh verification supports the change." \
  --actor developer
podway next
podway observe --json --wait-for-idle
```

`podway next` describes the active graph node, its required items, allowed actions,
and command suggestions. Perform the work outside Podway, record the requested
items with the returned item and attempt identities, then use the suggested
Procedure v2 action with the current revision fences:

```bash
podway --json status
podway --json next
podway --json observe --wait-for-idle
```

The complete fenced mutation sequence is shown in the
[Procedure v2 workflow](docs/examples/v2-workflow.md).

Inspect the current session at any time:

```bash
podway status
podway status --verbose
```

Use `podway help <command>` for the exact grammar of a command.

## Procedures and rework

Inspect a built-in preset before starting it:

```bash
podway preset explain bug-fix-v2
podway preset show bug-fix-v2
```

For a bounded verified change that does not need a tracked goal:

```bash
podway start --preset small-change-v2 --task "update one validation rule"
podway begin
```

You can also use a worktree-local YAML procedure:

```bash
podway procedure validate .podway/procedures/custom.yaml
podway start --procedure .podway/procedures/custom.yaml --task "review queue behavior"
podway begin
```

Rework is part of the normal lifecycle:

- `podway retry --reason "..."` starts a clean attempt of the current graph node.
- `podway rework --to <node> --reason "..."` reactivates an allowed graph node and applies declared evidence invalidation.
- `podway block --reason "..."` prevents completion until `podway unblock --all` resolves the blocker.
- `podway goal revise --reactivate ...` reactivates a completed goal-tracked session when explicitly authorized.
- `--dry-run` previews destructive transitions that support it.

`start` creates a disposable prepared session. `begin` creates the first active
attempt. A prepared session can be reset immediately. A completed or cancelled
session must first record its current terminal disposition before eligible reset:

```bash
podway disposition not-required --reason "No external handoff is required."
podway reset
```

Resetting running work requires explicit confirmation and a bounded progress
summary, for example `podway reset --progress-summary "Preserved the current diff." --yes`.

## Automation

Add `--json` to receive one versioned JSON object instead of human-readable text:

```bash
podway status --json
podway next --json
podway observe --json --wait-for-idle
```

Automation must use JSON fields and stable error codes, never parse human output. Mutations support idempotency keys, revision preconditions, detached admission, job lookup, and durable outcome reconciliation.
`podway record --stdin` accepts one closed, at-most-1-MiB JSON document to
record or clear 1..64 uniquely identified active-attempt items atomically. The
document carries every workspace, session, attempt, item-revision, and
idempotency fence; see the [Procedure v2 walkthrough](docs/examples/v2-workflow.md).
Common stale-state, uncertain-outcome, daemon, and workspace failures include a
closed `details.recovery` recipe containing only a bounded read-only command.
Automation must validate that recipe and must never treat it as authorization
for a lifecycle action or mutation.

Podway v0.2.5 implements an evidence-gated, goal-directed workflow memory:
Procedure v2 documents are declarative graphs with recorded decisions, selected
evidence read-back, explicit rework, goal revision, and goal assessment, still with
exactly one active attempt. Static authoring and inspection are available in the
normal CLI. The release admits Procedure v2 through the normal runtime while
preserving the managed disposable boundary for development builds.
See the [Procedure v2 walkthrough](docs/examples/v2-workflow.md)
and the [roadmap](docs/roadmap/README.md).

## Local data and trust

Task state lives under `.podway/runtime/` in the owning worktree. Deleting the worktree deletes that state. Podway keeps only user-global service metadata, a minimal workspace registry, its socket, and bounded logs under `~/.podway/`.

Podway trusts processes running as the same operating-system user. It is not a security boundary against malicious same-user processes. It performs no network I/O and exposes no arbitrary command-execution or Git-mutation API.

## Upgrade and uninstall

Install both binaries from the same new release, then refresh and verify the LaunchAgent:

```bash
podway daemon install --daemon-path "$HOME/.local/bin/podwayd"
podway daemon status
```

To uninstall:

```bash
podway daemon uninstall --yes
```

Then remove both binaries. Uninstalling the service preserves `.podway/` data in existing worktrees and preserves daemon logs unless `--purge-logs` is requested.

## Contributing and license

Start with the [contributor documentation](docs/README.md) for architecture, decisions, specifications, implementation guidance, open work, and verification. Podway is available under the [MIT License](LICENSE).
