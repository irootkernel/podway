# Podway

Podway is a local procedure guard for one task in one Git worktree. It keeps the
current task on an explicit sequence, shows what is missing, and makes retry and
rework visible instead of leaving them in shell history or chat context.

Podway does not run your build commands, edit Git state, contact a remote service,
or store artifact contents. You do the work; Podway keeps the process explicit.

Podway ships two matching binaries:

- `podway`, the command-line interface;
- `podwayd`, a user-scoped daemon that serializes task-state changes.

## When to use Podway

Use Podway when a task has steps that should not be skipped, especially when work
may be retried, handed between people or agents, or sent back to an earlier stage.
Four built-in procedures cover common work:

| Preset | Best for |
|---|---|
| `sw-dev` | Feature and implementation work |
| `bug-fix` | Reproduction, diagnosis, repair, and regression coverage |
| `docs-only` | Documentation changes with source and link validation |
| `analysis` | Investigations that end in supported conclusions |

Podway is not a project manager, CI system, command runner, Git automation layer,
artifact store, or multi-user access-control service.

## Requirements

Podway supports native Apple Silicon macOS only. Intel macOS, Rosetta execution,
universal binaries, Linux, and Windows are not supported release targets.

The current public package is unsigned and not notarized. Verify the published
checksum before installing it. macOS may require you to authorize the downloaded
binaries in System Settings before their first run.

## Install a release

Download the latest Apple Silicon archive and its `.sha256` file from
[GitHub Releases](https://github.com/irootkernel/podway/releases). Then verify and
extract it:

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

Add this line to `~/.zprofile` if `~/.local/bin` is not already on your login
shell path, then open a new terminal:

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

This creates `~/Library/LaunchAgents/dev.podway.podwayd.plist`, starts the daemon
for the current login session, and starts it again after future GUI logins. It does
not run before login and does not require `sudo`. The LaunchAgent records the
daemon's absolute path, so reinstall it after moving or replacing the binaries.

Useful lifecycle commands are:

```bash
podway daemon status
podway daemon start
podway daemon stop
podway daemon restart
podway daemon logs --lines 100
podway daemon logs --follow
```

## Build from source

Source builds require native Apple Silicon macOS and the Rust version pinned in
`rust-toolchain.toml`:

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

Contributors should use the complete verification commands documented under
[Contributor Documentation](docs/README.md), not treat a successful build as the
release-readiness gate.

## Quick start

Run Podway from anywhere inside a Git worktree:

```bash
podway init
podway preset list
podway start --preset sw-dev --task "add bounded retry backoff"
podway next
```

`podway next` describes the active stage, every missing required item, and the
Podway commands that can satisfy it. Do the corresponding work, record the items,
and advance one stage:

```bash
podway set goal "Retry transient writes with a bounded exponential delay."
podway add acceptance-criteria \
  "Transient write failures retry with bounded exponential backoff."
podway complete
podway next
```

Inspect the current session at any time:

```bash
podway status
podway status --verbose
```

Use `podway help <command>` for the exact grammar of a command.

## Procedures and rework

Inspect a built-in preset before starting it:

```bash
podway preset explain bug-fix
podway preset show bug-fix
```

You can also use a worktree-local YAML procedure:

```bash
podway procedure validate .podway/procedures/custom.yaml
podway start --procedure .podway/procedures/custom.yaml --task "review queue behavior"
```

Rework is part of the normal lifecycle:

- `podway retry --reason "..."` starts a clean attempt of the current stage.
- `podway return --to <stage> --reason "..."` returns to an earlier stage and
  marks reached downstream stages for redo.
- `podway block --reason "..."` prevents completion until
  `podway unblock --all` resolves the blocker.
- `podway reopen --to <stage> --reason "..."` reopens a completed session before
  it is reset.
- `--dry-run` previews destructive transitions that support it.

After a completed or cancelled task no longer needs its local history:

```bash
podway reset --yes
```

## Automation

Add `--json` to receive one versioned JSON object instead of human-readable text:

```bash
podway status --json
podway next --json
```

Automation must use JSON fields and stable error codes, never parse human output.
Mutations support idempotency keys, revision preconditions, detached admission,
job lookup, and durable outcome reconciliation.

## Local data and trust

Task state lives under `.podway/runtime/` in the owning worktree. Deleting the
worktree deletes that state. Podway keeps only user-global service metadata, a
minimal workspace registry, its socket, and bounded logs under `~/.podway/`.

Podway trusts processes running as the same operating-system user. It is not a
security boundary against malicious same-user processes. It performs no network
I/O and exposes no arbitrary command-execution or Git-mutation API.

## Upgrade and uninstall

Install both binaries from the same new release, then refresh and verify the
LaunchAgent:

```bash
podway daemon install --daemon-path "$HOME/.local/bin/podwayd"
podway daemon status
```

To uninstall:

```bash
podway daemon uninstall --yes
```

Then remove both binaries. Uninstalling the service preserves `.podway/` data in
existing worktrees and preserves daemon logs unless `--purge-logs` is requested.

## Contributing and license

Start with the [contributor documentation](docs/README.md) for architecture,
decisions, specifications, implementation guidance, open work, and verification.
Podway is available under the [MIT License](LICENSE).
