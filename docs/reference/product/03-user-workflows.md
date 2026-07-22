# User Workflows

## First-time installation

```bash
# Install the podway and podwayd binaries by the selected distribution method.
podway daemon install
podway daemon status
```

`podway daemon install` installs and loads a user LaunchAgent. The daemon starts at login and may be controlled explicitly with the daemon subcommands.

## Initialize a worktree

From any directory inside a valid Git worktree:

```bash
podway init
```

Expected result:

- `.podway/config.yaml` is created when absent;
- `.podway/.gitignore` ignores `runtime/`;
- `.podway/runtime/state.sqlite3` is initialized by the daemon;
- a workspace UUID and Git identity are recorded;
- no task session is started automatically.

Outside a Git worktree, the command fails with `NOT_A_GIT_WORKTREE`.

## Start and perform a software task

```bash
podway start --preset sw-dev --task "add bounded retry backoff"
podway status
podway next
```

Typical `next` output identifies the active stage, its instructions, missing required items, and concrete Podway commands.

```bash
podway set goal "Retry transient writes with a bounded exponential delay."
podway check acceptance-criteria-defined
podway complete
```

The next stage becomes active only after all required items are satisfied and no blocker remains.

## Bug-fix workflow

```bash
podway start --preset bug-fix --task "fix duplicate login session creation"
podway next
podway check reproduced
podway set expected-behavior "A successful login creates exactly one session."
podway set actual-behavior "Two sessions may be created under concurrent callbacks."
podway attach reproduction-reference tests/login_race.rs
podway complete
```

Continue stage by stage through diagnosis, regression coverage, fix, verification, review, and finish.

## Block and resume

```bash
podway block --reason "waiting for the API owner to confirm compatibility"
podway status
```

The active stage remains current but cannot complete.

```bash
podway unblock --all
podway complete
```

Blockers are scoped to the current attempt and do not carry into retry or return attempts.

## Retry the current stage

Use retry when the active stage must be performed again without moving to an earlier stage.

```bash
podway retry --reason "the first verification environment used the wrong feature flags"
```

Effects:

- the active attempt becomes `abandoned`;
- a new attempt for the same stage becomes active;
- item values and blockers start empty;
- the old attempt remains only until session reset.

## Return to an earlier stage

Use return when a later stage reveals that earlier work must change.

```bash
podway return --to implement --reason "review found an unhandled cancellation path"
```

Effects:

- the current attempt becomes `abandoned`;
- a fresh attempt of `implement` becomes active;
- reached downstream stages become `redo`;
- never-reached stages remain `pending`;
- old item values do not satisfy the new attempts.

Preview the effect without mutation:

```bash
podway return --to implement --reason "review finding" --dry-run
```

## Complete and reopen

Completing the final stage changes the session to `completed`.

```bash
podway complete
podway status
```

Before reset, a completed session may be reopened:

```bash
podway reopen --to verify --reason "a new issue was found before reporting the result"
```

Reopen uses the same redo rules as return. A cancelled session cannot be reopened.

## Cancel and reset

```bash
podway cancel --reason "the requested change is no longer needed"
```

Cancel ends the session. To begin another task:

```bash
podway reset --yes
podway start --preset sw-dev --task "next task"
```

`reset` deletes the current session and its attempts, items, blockers, and session-scoped receipts. It preserves workspace initialization and tracked procedure files.

## Detached mutation jobs

Mutations wait for completion by default. Detached mode returns after durable admission:

```bash
podway complete --detach --json
podway job status <job-id> --json
podway job wait <job-id> --json
```

A queued job may be cancelled before it begins:

```bash
podway job cancel <job-id>
```

Running state transitions are short and are not interrupted.

## Automation and AI-assisted use

The recommended loop is:

```text
1. podway status --json
2. podway next --json
3. perform only the active-stage work
4. update required items with explicit preconditions
5. complete, retry, return, or block
6. repeat
```

Example:

```bash
podway status --json > /tmp/podway-status.json
podway next --json > /tmp/podway-next.json
podway set implementation-summary "Added bounded retry and cancellation checks" \
  --if-attempt 6f8e... \
  --if-item-revision 0 \
  --json
```

An external worker finishing does not complete a Podway stage. The caller must set the required items and invoke `podway complete`.

## Custom procedure workflow

```bash
podway procedure validate .podway/procedures/custom.yaml
podway start \
  --procedure .podway/procedures/custom.yaml \
  --task "perform custom task"
```

The daemon validates and stores an immutable canonical snapshot. Later edits apply only to future sessions.

## Diagnostics

```bash
podway doctor
podway daemon status
podway daemon logs --follow
podway workspace show
```

When state is corrupt and cannot be read, the current-task model permits destructive reinitialization:

```bash
podway reset --all --force --yes
```

This removes the disposable runtime state and creates a new empty workspace database. It does not modify tracked procedure files.
