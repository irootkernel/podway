# Podway

Podway is a local procedure guard for one task in one Git worktree. It keeps the current task on an explicit sequence, shows what is missing, and makes retry and rework visible instead of leaving them in shell history or chat context.

Podway does not run your build commands, edit Git state, contact a remote service, or store artifact contents. You do the work; Podway keeps the process explicit.

Podway ships one binary, `podway`. Each command runs to completion in its own
process. The Direct Engine reads a committed worktree snapshot or atomically
commits a fenced mutation and its validated success outcome. There is no resident
process, login-start service, socket, or user-global task registry.

## When to use Podway

Use Podway when a task has explicit graph nodes that should not be skipped,
especially when work may be retried, handed between people or agents, or sent
through a declared rework path. `podway preset list` is the authority for the
installed binary's exact catalog. The current source embeds four built-in
procedures for common work:

| Preset | Best for |
|---|---|
| `analysis-v2` | Source-backed research or technical analysis with evidence-gap and review-owner rework |
| `sw-dev-v2` | Graph-based implementation work with decisions, evidence read-back, rework, and goal assessment |
| `bug-fix-v2` | Graph-based defect repair with decision rework and goal closeout |
| `small-change-v2` | Short inspect, implement, verify, review, and closeout path without goal tracking |

Podway admits only Procedure v2 through the normal runtime and emits successful
responses through `podway.output/v4` and failures through `podway.error/v2`.

Podway is not a project manager, CI system, command runner, Git automation layer, artifact store, or multi-user access-control service.

### For AI agents and harnesses

Scripts and AI agents use the same public contract as humans. The single
`podway observe --json` command returns one authoritative,
self-contained committed task observation, so an agent re-derives what happens
next from the worktree instead of from its conversation history. Sessions survive context
loss and transfer between actors mid-task, and mutations accept explicit
preconditions and idempotency keys so retried or concurrent callers cannot
corrupt state. See the [Procedure v2 workflow](docs/examples/v2-workflow.md).

## Requirements

Podway supports native Apple Silicon macOS only. Intel macOS, Rosetta execution, universal binaries, Linux, and Windows are not supported release targets.

Check the selected release's signing and notarization statements and verify its
published checksum before installing. Unsigned artifacts are supported by the
release contract. macOS may require you to authorize the downloaded binary in
System Settings before its first run.

## Install a release

This checkout documents the daemon-free `v0.2.13` target; source implementation
is not evidence that a matching stable release has been published. Select an
archive whose identity matches this direct generation, or use an explicitly
chosen source build without claiming stable-release qualification.

If an older daemon-based generation is installed, complete the separately
authorized [legacy cutover](#legacy-state-and-cutover) before installing this CLI
or resetting its worktrees.

Download the selected Apple Silicon archive and its `.sha256` file from [GitHub Releases](https://github.com/irootkernel/podway/releases). Then verify and extract it:

```bash
shasum -a 256 -c podway-<version>-aarch64-apple-darwin.tar.gz.sha256
tar -xzf podway-<version>-aarch64-apple-darwin.tar.gz
```

Install the single CLI on your `PATH`:

```bash
mkdir -p "$HOME/.local/bin"
install -m 755 podway-<version>-aarch64-apple-darwin/bin/podway \
  "$HOME/.local/bin/podway"
```

Add this line to `~/.zprofile` if `~/.local/bin` is not already on your login shell path, then open a new terminal:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

Verify the exact CLI and require Output v4, version-result v2 and manifest v2:

```bash
podway version --json --identity
```

## Legacy state and cutover

The daemon-free contract has no backward compatibility with daemon-era output,
error codes or Store generations 1–13. An old worktree or old maintenance marker
returns `LEGACY_RUNTIME_STATE_UNSUPPORTED` without changing its bytes, migrating,
checkpointing or creating SQLite SHM. Reset is destructive: it makes no backup and
performs no conversion. Active sessions and retained runtime history are lost.

The supported order is:

1. Finish or explicitly abandon active sessions using their original generation.
2. Under separate authorization, use the old binary's supported `daemon uninstall`
   operation to remove its LaunchAgent and establish that no old writer remains.
   The new CLI's `daemon` routes return `COMMAND_RETIRED` and cannot retire it.
3. If Aquarium's managed development service is installed, retire it under its
   own separate authority through the original Aquarium controller before
   installing the new CLI or resetting any shared worktree. Removing Podway's
   ordinary LaunchAgent does not stop that distinct writer.
4. Install the new single binary, then remove the old `~/.podway` account state
   only within an explicit legacy-runtime cleanup grant. Current Podway neither
   reads nor creates that state. An installation or workspace-reset request alone
   does not authorize account cleanup.
5. After identifying every legacy worktree and accepting its irreversible history
   loss, reset each exact worktree through the current supported command:

   ```bash
   podway --worktree <legacy-worktree> --json reset --all --yes --idempotency-key <new-key>
   ```

The no-old-writer prerequisite matters: a still-running old daemon could write to
unlinked files during reset. Preserve the original request and key if reset is
interrupted; follow its marker recovery recipe rather than generating a new key
or deleting files manually. Reset preserves configuration unrelated to retired mode, Procedures,
`.gitignore` and unrelated worktree files, then establishes a fresh UUID and
current Store. Start a new session from the fresh workspace.

These instructions describe the supported cutover; this source change authorizes
no installation, service action or deletion. See
[ADR-0038](docs/architecture-decision-records/0038-remove-the-daemon.md) and
[upgrade QA](docs/implementation-tips/release-upgrade-qa.md).

## Optional: configure an AI coding agent

Podway does not modify a project's `AGENTS.md` or install an agent skill. Both integrations below are optional. They may be used independently or together, and omitting both does not affect the CLI.

For a small project-wide policy, add this template to the project's `AGENTS.md`:

```markdown
## Podway

- Podway supports Procedure v2 only. Treat `LEGACY_RUNTIME_STATE_UNSUPPORTED` as an explicit destructive-reset boundary, after separately authorized retirement of every old writer; do not edit the database or attempt conversion.
- When the current workflow authorizes Podway use and `.podway/config.yaml` exists, read `podway observe --json` before task work. Require Output v4 and `podway.observation-result/v5` for every Procedure, and re-read after every mutation.
- Treat the active Podway graph node and attempt as the current work boundary. Perform the work before recording an item. Side work may run outside Podway, but record only conclusions supported by current evidence on the active attempt.
- Use JSON fields, stable error codes, explicit preconditions, and idempotency keys for mutations. Never parse human-readable output as an API.
- You may update items and advance an existing active v2 session when the work supports it. Do not run `podway init`, start or replace a session, cancel or reset state, remove a workspace, or reactivate a completed session through `rework` or `goal revise --reactivate` unless the user explicitly requests it.
- Podway records assertions; it does not run the work or prove semantic truth.
```

For fuller runtime guidance, install the complete [`use-podway` skill](https://github.com/irootkernel/podway/tree/main/skills/use-podway) directory under `~/.agents/skills/`:

```bash
podway_skill_dir="$HOME/.agents/skills/use-podway"
mkdir -p "$podway_skill_dir/references"

curl -fsSLo "$podway_skill_dir/SKILL.md" \
  https://raw.githubusercontent.com/irootkernel/podway/main/skills/use-podway/SKILL.md

for reference in lifecycle goal recovery delegation; do
  curl -fsSLo "$podway_skill_dir/references/$reference.md" \
    "https://raw.githubusercontent.com/irootkernel/podway/main/skills/use-podway/references/$reference.md"
done
```

The runtime skill covers the active-session loop and loads separate references only for lifecycle, session-goal, recovery, or explicitly delegated work.

Install [`create-podway-procedure`](https://github.com/irootkernel/podway/tree/main/skills/create-podway-procedure) separately only when the agent needs to create, revise, review, or harden custom Procedure source:

```bash
procedure_skill_dir="$HOME/.agents/skills/create-podway-procedure"
mkdir -p "$procedure_skill_dir/references" "$procedure_skill_dir/agents"

curl -fsSLo "$procedure_skill_dir/SKILL.md" \
  https://raw.githubusercontent.com/irootkernel/podway/main/skills/create-podway-procedure/SKILL.md

for reference in authoring capabilities distribution delegation; do
  curl -fsSLo "$procedure_skill_dir/references/$reference.md" \
    "https://raw.githubusercontent.com/irootkernel/podway/main/skills/create-podway-procedure/references/$reference.md"
done

curl -fsSLo "$procedure_skill_dir/agents/openai.yaml" \
  https://raw.githubusercontent.com/irootkernel/podway/main/skills/create-podway-procedure/agents/openai.yaml
```

The authoring skill produces and validates declarative source, then hands off preview's exact digest-fenced start suggestion without starting or mutating a session. Consult the agent's documentation for skill reload requirements. The commands above require network access and `curl`, and overwrite existing files with the same names. Replace `main` with one exact release tag or commit SHA when a reproducible, pinned skill version is required.

For supervised delegation, follow the [worked review example](docs/examples/supervised-delegation.md)
and its canonical YAML. The example is exercised by the real direct CLI in
an isolated worktree test with a synthetic executor. It is not a built-in preset or a live
Dolgorae adapter; source validation does not establish installed-release support.

## Build from source

Source builds require native Apple Silicon macOS and the Rust version pinned in `rust-toolchain.toml`:

```bash
git clone https://github.com/irootkernel/podway.git
cd podway
cargo build --release --locked -p podway-cli --bin podway

mkdir -p "$HOME/.local/bin"
install -m 755 target/release/podway "$HOME/.local/bin/podway"
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
podway observe --json
```

`podway next` describes the active graph node, its required items, allowed actions,
and command suggestions. Perform the work outside Podway, record the requested
items with the returned item and attempt identities, then use the suggested
Procedure v2 action with the current revision fences:

```bash
podway --json status
podway --json next
podway --json observe
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
podway procedure scaffold --template minimal > .podway/procedures/custom.yaml
# Review and edit the scaffold, then qualify the resulting source.
podway procedure check .podway/procedures/custom.yaml --warnings-as-errors
podway --json procedure preview .podway/procedures/custom.yaml
# Run the exact start suggestion returned by preview.
podway start \
  --procedure .podway/procedures/custom.yaml \
  --expect-procedure-digest sha256:<digest-from-preview> \
  --task "review queue behavior"
podway begin
```

Selected evidence is read one bounded page at a time. `podway next --json` carries each
item's identity, digest, and total size, and may add a short preview with a continuation
token. `podway observe --json` carries the same metadata but never a preview or a token.
`podway evidence read --source <graph-node-id> --item <item-id>` returns the complete value,
following `next_page_token` until `truncated` is `false`.

Rework is part of the normal lifecycle:

- `podway retry --reason "..."` starts a clean attempt of the current graph node.
- `podway rework --to <node> --reason "..."` reactivates an allowed graph node and applies declared evidence invalidation.
- `podway block --reason "..."` prevents completion until `podway unblock --all` resolves the blocker.
- `podway goal revise --reactivate ...` reactivates a completed goal-tracked session when explicitly authorized.
- `--dry-run` previews destructive transitions that support it.

`start` creates a disposable prepared session. `begin` creates the first active
attempt. A prepared session can be reset immediately. A completed or cancelled
session must first record its current terminal disposition before it can be
archived or reset:

```bash
podway disposition not-required --reason "No external handoff is required."
podway archive
```

Plain `podway start` automatically archives a disposed terminal session before
creating the next prepared session. If prepared, running, or undisposed terminal
state exists in a human terminal, `start` first offers lifecycle-specific choices:
continue without creating a session, preserve the prior work where applicable,
or delete it. An undisposed terminal session instead offers handed-off and
not-required disposition choices. Preserving running work cancels and archives it as `superseded`,
including the reason and successor session ID.

Automation never receives a prompt. Use `--on-existing preserve` with
`--supersede-reason` to retain running work, or `--on-existing delete` to remove
prepared or running state. Running deletion additionally requires
`--progress-summary` and `--yes`. The former `--replace-eligible` and `--replace`
CLI flags are removed. `reset` remains destructive and never archives implicitly.

Inactive sessions are retained per worktree until explicitly purged or the
workspace runtime is reset. Podway never evicts one automatically. Inspect or
remove retained sessions with:

```bash
podway archive list
podway archive show --session-id <uuid>
podway archive show --session-id <uuid> --verbose --history-before <n>
podway archive purge --session-id <uuid> --if-session-revision <n> --yes
```

`archive show` includes the immutable terminal disposition, so a superseded
session identifies why it ended and which prepared session succeeded it.

Inactive sessions preserve their complete read-only Procedure state and cannot
be restored or mutated. Purge and reset are permanent deletions.

## Automation

Add `--json` to receive one versioned JSON object instead of human-readable text:

```bash
podway status --json
podway next --json
podway observe --json
```

Automation must use JSON fields and stable error codes, never parse human output.
Pass an explicit stable `--idempotency-key` and preserve resolved revision and
identity fences for every mutation. Only successful mutations retain outcomes;
rejections have no state effect or outcome. After response loss, use
`podway outcome lookup --idempotency-key <original-key> --json`, then replay only
the identical canonical request when authorized. Replay returns the entire original
success, including its original request ID. Retention is session-scoped or, for
workspace outcomes, bounded to 100 entries or 30 days.

`--timeout` defaults to five seconds and bounds maintenance-lease, connection-gate
and SQLite read/write admission waits. Each boundary tries once even at `0ms` or
after the budget expires. Input, preparation and human confirmation consume the
original budget without renewing it; admitted work may finish after the deadline.
`WORKSPACE_BUSY` or `WORKSPACE_MAINTENANCE` leaves no partial effect.
`MUTATION_OUTCOME_UNKNOWN` is limited to an I/O failure during COMMIT. Daemon, job,
mode, detach and wait controls are retired; they fail with `COMMAND_RETIRED`.
`podway record --stdin` accepts one closed, at-most-1-MiB JSON document to
record or clear 1..128 uniquely identified active-attempt items atomically. The
document carries every workspace, session, attempt, item-revision, and
idempotency fence; see the [Procedure v2 walkthrough](docs/examples/v2-workflow.md).
Common stale-state, uncertain-outcome and workspace failures include a closed
`details.recovery` recipe. Validate RecoveryRecipe v2 and its command/argv pair;
read-only recovery and explicitly authorized destructive recovery are distinct.
A recipe never grants permission for a lifecycle action or weakens a fence.

Podway implements evidence-gated, goal-directed workflow memory:
Procedure v2 documents are declarative graphs with recorded decisions, selected
evidence read-back, explicit rework, goal revision, and goal assessment, still with
exactly one active attempt. Static authoring and inspection are available in the
normal CLI. Procedure v2 runs through the same Direct Engine for every caller;
development builds use disposable worktrees and exact writer-manifest identity.
See the [Procedure v2 walkthrough](docs/examples/v2-workflow.md)
and the [roadmap](docs/roadmap/README.md).

## Local data and trust

Task state, success outcomes, bounded operational journal and maintenance locks
live under `.podway/runtime/` in the owning worktree. Deleting the worktree deletes
that state. Podway neither reads nor creates `~/.podway`, a global workspace
registry, daemon logs, sockets or service metadata. A different writer manifest
fails closed unless the release explicitly declares a compatible predecessor;
`v0.2.13` declares none.

Podway trusts processes running as the same operating-system user. It is not a security boundary against malicious same-user processes. It performs no network I/O and exposes no arbitrary command-execution or Git-mutation API.

## Upgrade and uninstall

For the daemon-era cutover, follow [legacy state and cutover](#legacy-state-and-cutover)
before installing or resetting. Later direct-to-direct upgrades require declared
writer-predecessor compatibility; a version string alone does not permit migration.

Remove the CLI through the original installation mechanism when no longer needed.
That does not delete `.podway` in any worktree. Explicitly confirmed
`podway workspace remove` deletes one selected worktree's complete `.podway` tree
while preserving its Git worktree. Workspace removal, account cleanup and old
service retirement are distinct grants; do not combine them implicitly.

## Contributing and license

Start with the [contributor documentation](docs/README.md) for architecture, decisions, specifications, implementation guidance, open work, and verification. Podway is available under the [MIT License](LICENSE).
