# Podway

Podway is a local procedure guard for one task in one Git worktree. It keeps the
current task on an explicit sequence, shows what is missing, and makes retry and
rework visible instead of leaving them in shell history or chat context.

Podway does not run your build commands, edit Git state, contact a remote service,
or store the contents of your artifacts. You do the work; Podway keeps the process
honest.

> [!IMPORTANT]
> Podway 0.1.0 is a pre-release for native Apple Silicon macOS only. Its release
> archive is unsigned and not notarized.

## Install

Podway ships two matching binaries: `podway`, the CLI, and `podwayd`, the local
daemon. Extract the release archive and install both on your `PATH`:

```bash
mkdir -p "$HOME/.local/bin"
install -m 755 podway-0.1.0-aarch64-apple-darwin/bin/podway "$HOME/.local/bin/podway"
install -m 755 podway-0.1.0-aarch64-apple-darwin/bin/podwayd "$HOME/.local/bin/podwayd"
export PATH="$HOME/.local/bin:$PATH"

podway daemon install --daemon-path "$HOME/.local/bin/podwayd"
podway daemon status
```

The daemon is installed as a per-user LaunchAgent and starts at login. See the
[release notes](RELEASE_NOTES.md) for compatibility and signing details.

## Quick start

Run Podway from inside a Git worktree:

```bash
podway init
podway preset list
podway start --preset sw-dev --task "add bounded retry backoff"
podway next
```

`podway next` describes the active stage, lists every missing item, and suggests
the exact Podway commands that satisfy it. Record those items after doing the
corresponding work, then advance:

```bash
podway set goal "Retry transient writes with a bounded exponential delay."
podway check acceptance-criteria-defined
podway complete
podway next
```

Continue until the final stage completes. Inspect the session at any time with:

```bash
podway status
podway status --verbose
```

When the completed task no longer needs its local history, prepare the worktree
for another task:

```bash
podway reset --yes
```

## Choose a procedure

Podway includes four procedures:

| Preset | Use it for |
|---|---|
| `sw-dev` | Feature and implementation work |
| `bug-fix` | Reproduction, diagnosis, repair, and regression coverage |
| `docs-only` | Documentation changes with source and link validation |
| `analysis` | Investigation that ends in supported conclusions |

Inspect a preset before starting it:

```bash
podway preset explain bug-fix
podway preset show bug-fix
```

You can also start from a worktree-local YAML procedure:

```bash
podway procedure validate .podway/procedures/custom.yaml
podway start --procedure .podway/procedures/custom.yaml --task "review queue behavior"
```

## Rework and blockers

- `podway retry --reason "..."` starts a clean attempt of the current stage.
- `podway return --to <stage> --reason "..."` returns to an earlier stage and
  marks reached downstream stages for redo.
- `podway block --reason "..."` prevents completion while an external issue is
  unresolved; `podway unblock --all` resumes the attempt.
- `podway reopen --to <stage> --reason "..."` reopens a completed session before
  it is reset.

Use `--dry-run` on destructive transitions that support it to preview their
effect.

## Automation

Add `--json` to receive one versioned JSON object instead of human-readable text:

```bash
podway status --json
podway next --json
```

Automation should use JSON fields and stable error codes, not parse text output.
Mutations support idempotency keys, revision preconditions, and detached job
admission for reliable local integrations.

## Safety and local data

Podway trusts processes running as the same operating-system user. It is not a
multi-user security boundary. Task state lives in the worktree under
`.podway/runtime/`; deleting the worktree deletes that state. Podway performs no
network I/O and exposes no Git mutation or arbitrary command-execution API.

## Uninstall

```bash
podway daemon uninstall --yes
```

Then remove both binaries using the method that installed them. Uninstalling the
daemon does not delete `.podway/` directories in existing worktrees.

## Contributing

The [contributor documentation](docs/README.md) explains the project goals,
repository structure, architecture, implementation rules, detailed contracts,
and completed roadmap.
