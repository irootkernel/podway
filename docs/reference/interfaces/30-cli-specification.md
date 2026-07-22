# CLI Specification

## Command name and binaries

The public command is `podway`. The daemon binary is `podwayd` and is normally managed through `podway daemon ...`.

## Global options

```text
--json                         emit the versioned JSON contract
--worktree <path>              target an explicit Git worktree
--timeout <duration>           bound daemon connection or wait time
--no-color                     disable color in text output
--quiet                        suppress nonessential text output
--idempotency-key <string>     override generated mutation key
--detach                       return after durable job admission
--if-session-revision <n>      require an exact session revision
--if-attempt <uuid>            require an exact active attempt
--if-item-revision <n>         require an exact item revision
--yes                          approve a required destructive confirmation
```

Not every option applies to every command. Inapplicable options are usage errors rather than silently ignored.

Durations accept `ms`, `s`, and `m`, for example `500ms`, `30s`, and `2m`.

## Output modes

- Human-readable text is the default.
- `--json` emits exactly one success or error JSON object to stdout.
- Diagnostics not represented in the JSON object go to stderr only for process-level failures before JSON can be produced.
- Color is used only on a TTY and never in JSON.
- Text wording is not a stable API. JSON fields, schemas, error codes, and exit codes are the stable integration contract.

## Static commands

The following do not require a worktree or daemon unless explicitly noted:

```bash
podway help [topic]
podway version
podway preset list
podway preset show <name>
podway preset explain <name>
podway procedure validate <file>
podway procedure show <file>
podway daemon ...
```

`procedure validate` and `procedure show` use the same Rust schema and canonicalization library as the daemon.

## Help topics

```bash
podway help workflow
podway help rework
podway help automation
podway help procedures
podway help daemon
podway help artifacts
```

Help MUST include complete examples and must not require internet access.

## Daemon commands

### Install

```bash
podway daemon install [--daemon-path <path>]
```

Installs or updates the user LaunchAgent and waits for health.

### Uninstall

```bash
podway daemon uninstall [--purge-logs] [--yes]
```

Does not remove worktree-local state.

### Lifecycle

```bash
podway daemon start
podway daemon stop
podway daemon restart
podway daemon status
podway daemon logs [--follow] [--lines <n>]
```

`daemon status --json` returns service, socket, protocol, and queue summary.

## Workspace commands

### Initialize

```bash
podway init [--repair]
```

Creates or validates `.podway/` and initializes the worktree database. It never starts a task.

### Doctor

```bash
podway doctor [--deep]
```

Checks:

- daemon reachability and version;
- Git worktree validity;
- path containment and permissions;
- workspace configuration;
- runtime ignore status;
- database schema and integrity;
- workspace UUID conflicts;
- queue recovery state.

`--deep` additionally revalidates the Git-to-Store workspace binding. Doctor remains read-only; the store layer's deep SQLite integrity mode is not wired into this command in v0.1.0.

### Workspace inspection and repair

```bash
podway workspace show
podway workspace repair
```

Repair updates a moved worktree's minimal daemon registry after proving identity. It does not adopt conflicting copied state or change session semantics.

## Procedure commands

```bash
podway procedure validate <file> [--warnings-as-errors]
podway procedure show <file> [--canonical]
```

`--canonical` prints Podway Canonical JSON v1. With `--json`, validation returns digest, warnings, normalized metadata, and errors.

## Preset commands

```bash
podway preset list
podway preset show <name>
podway preset explain <name>
```

`show` emits source YAML or structured JSON. `explain` provides purpose, stage outline, and common rework examples.

## Session commands

### Start

```bash
podway start --preset <name> --task <title>
podway start --procedure <worktree-relative-file> --task <title>
podway start ... --replace --yes
```

Exactly one of `--preset` or `--procedure` is required. `--replace` deletes an existing session before start and requires confirmation. `--dry-run` validates and shows the first stage without creating a session.

### Status

```bash
podway status [--verbose] [--wait-for-idle] [--after-job <uuid>]
```

Reports:

- task and procedure;
- session lifecycle and revision;
- current stage and attempt;
- all stage statuses;
- active-stage item states;
- blockers;
- queued and running jobs;
- whether the stage is ready to advance.

`--verbose` includes previous attempt summaries for the current session. It does not provide an audit export.

### Next

```bash
podway next [--wait-for-idle] [--after-job <uuid>]
```

Reports:

- current stage title and instructions;
- missing required items;
- open blockers;
- structured Podway command suggestions;
- whether complete, skip, retry, return, or cancel is currently allowed;
- next stage after completion.

### Complete

```bash
podway complete
```

Requires all required items and no blockers. The CLI automatically supplies current revision and attempt unless explicit precondition flags are given.

### Skip

```bash
podway skip --reason <text>
```

Fails when the active stage is not skippable. A reason is always accepted and is mandatory when the procedure requires it.

### Retry

```bash
podway retry --reason <text>
```

Creates a clean attempt for the current stage.

### Return

```bash
podway return --to <stage-id> --reason <text>
podway return --to <stage-id> --reason <text> --dry-run
```

Dry run shows the new destination attempt number and downstream stages that would become `redo`.

### Reopen

```bash
podway reopen --to <stage-id> --reason <text>
podway reopen --to <stage-id> --reason <text> --dry-run
```

Allowed only for a completed session.

### Block and unblock

```bash
podway block --reason <text>
podway unblock <blocker-id>
podway unblock --all
```

Exactly one blocker ID or `--all` is required.

### Cancel

```bash
podway cancel --reason <text>
```

Cancels a running session. A cancelled session cannot reopen.

### Reset

```bash
podway reset [--dry-run] [--yes]
podway reset --all --force --yes
```

Normal reset deletes the session but preserves workspace initialization. `--all --force` recreates disposable runtime state and is also the corruption-recovery path.

## Item commands

### Confirm

```bash
podway check <item-id>
podway uncheck <item-id>
```

### Text, choice, and integer

```bash
podway set <item-id> <value>
podway set <item-id> --stdin
```

For `integer`, the value must parse as a base-10 signed integer. For `choice`, it must exactly match a declared choice. `--stdin` is valid for text only and reads until EOF.

### List

```bash
podway add <item-id> <value>
podway remove <item-id> <value> [--ignore-missing]
```

### Artifact path

```bash
podway attach <item-id> <worktree-relative-path> [--media-type <type>]
```

The daemon computes size and digest. A supplied media type overrides the embedded extension mapping and must satisfy the item allowlist. Unknown extensions default to `application/octet-stream`.

### Artifact reference

```bash
podway attach <item-id> \
  --reference <opaque-reference> \
  --digest sha256:<hex> \
  --size <bytes> \
  --media-type <type>
```

The four reference options are required together.

### Clear

```bash
podway clear <item-id>
```

Clears the current item value.

## Job commands

```bash
podway job list [--state queued|running|succeeded|failed|cancelled]
podway job status <job-id>
podway job wait <job-id>
podway job cancel <job-id>
```

Job commands are scoped to the current worktree. `job wait` honors `--timeout`. `job cancel` only succeeds for queued jobs.

## Automatic and explicit preconditions

For interactive use, the CLI reads current state immediately before submitting a mutation and includes the observed preconditions.

Automation SHOULD pass explicit values obtained from `status --json`:

```bash
podway complete \
  --if-session-revision 12 \
  --if-attempt 6f8e... \
  --idempotency-key task-42-complete-verify \
  --json
```

Item updates additionally accept `--if-item-revision`. For an unset item, use revision `0`.

## Destructive confirmation policy

Commands requiring confirmation:

- `start --replace`;
- `reset` always, except for `--dry-run`;
- `reset --all --force`;
- `daemon uninstall` when service files will be removed.

On an interactive TTY, Podway prompts unless `--yes` is present. With `--json` or non-TTY input, missing `--yes` fails with `CONFIRMATION_REQUIRED`. Prompt text is never emitted in JSON mode.

## Shell completion

The release ships completion for:

- zsh;
- bash;
- fish.

Completion includes commands, flags, built-in preset names, active-stage item IDs, allowed return destinations, open blocker IDs, and current-worktree job IDs where dynamic completion is safe and fast.

Dynamic completion MUST use read-only daemon queries and MUST degrade silently when the daemon or workspace is unavailable.
