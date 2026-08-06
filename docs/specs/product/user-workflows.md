# User Workflows

## First-time installation

```bash
# Install the podway and podwayd binaries by the selected distribution method.
podway daemon install
podway daemon status
```

`podway daemon install` installs and loads a user LaunchAgent. The daemon starts at login and may be controlled explicitly with the daemon subcommands.

## Initialize a worktree

From any directory inside a valid Git worktree — the repository's main checkout or any linked worktree:

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
podway add acceptance-criteria "Transient write failures retry with bounded exponential backoff."
podway complete
```

The next stage becomes active only after all required items are satisfied and no blocker remains.

## Bug-fix workflow

```bash
podway start --preset bug-fix --task "fix duplicate login session creation"
podway next
podway check baseline-established
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

A complete JSON-driven walkthrough, including precondition handling and lost-response recovery, is the [agent session example](../../examples/agent-session.md).

## Working modes

Podway state always lives in exactly one Git working tree — and a working tree includes the repository's main checkout, so starting a task does not require `git worktree add`. Three shapes of use follow from where sessions live:

| Mode | Sessions |
|---|---|
| Workspace | One session in the main checkout |
| Full worktree | One worktree and one session per task |
| Scratch worktree | One session in the main checkout; extra worktrees carry write work without Podway |

Workspace mode is the default, and every walkthrough in this document up to this point runs that way. Full worktree mode runs several independent tasks in parallel — see [parallel tasks](#parallel-tasks-across-worktrees). Scratch worktree mode keeps one governing session while isolating write work — see [scratch worktrees](#scratch-worktrees-within-one-task).

Choose by goal ownership: work that serves the current task's goal belongs to the workspace session, with scratch worktrees when its writes need isolation; work with an independent goal deserves its own worktree and its own session.

## Parallel tasks across worktrees

Podway serializes mutations within one worktree and processes independent worktrees concurrently ([queue invariants](terminology-and-invariants.md#queue-and-transaction-invariants), [ADR-0003](../../architecture-decision-records/0003-daemon-single-writer.md)). The worktree is therefore the unit of parallelism: to run several tasks at once, give each task its own Git worktree with its own session.

```bash
git worktree add ../retry-backoff
git worktree add ../login-race
(cd ../retry-backoff && podway init && podway start --preset sw-dev --task "add bounded retry backoff")
(cd ../login-race && podway init && podway start --preset bug-fix --task "fix duplicate login session creation")
```

- Each worktree keeps its own state database, one session, and one authoritative current stage.
- One daemon serves all worktrees and never interleaves two mutations inside the same worktree.
- A coordinator that assigns one worker per worktree gets task-level parallelism with no shared mutable state; inside its worktree, each worker still advances one explicit sequence.

Sessions do not move between worktrees: each worktree has its own state database and workspace identity, so there is no operation that transfers a session. Hand results across worktrees as recorded state — capture each branch session's summary with `podway status --json` before `reset` — and treat the merge of parallel branches as a task of its own, with each branch's recorded goals, decisions, and summaries as its reconciliation context. The [integration session example](../../examples/integration-session.md) shows that pattern end to end. Inside each worktree, the [multi-actor patterns](#multiple-actors-on-one-stage) apply unchanged; the two axes compose.

## Concurrent side work within one task

Work inside a task is often naturally parallel: reviews from several perspectives, a test matrix, or candidate approaches compared side by side. Podway keeps exactly one active stage attempt per session and does not schedule parallel stages ([non-goals](goals-and-non-goals.md#non-goals)). Run such side work outside Podway — concurrently when useful — and record its conclusion on the single active stage:

```bash
# The race suite and the authentication suite ran concurrently outside Podway.
podway check original-failure-resolved
podway check regression-check-passed
podway set verification-note "The race suite and the authentication suite passed."
podway complete
```

The stage gates on the recorded conclusions, not on how the work was scheduled. When side work stalls the task on an external dependency, record that as a blocker (`podway block --reason "..."`) instead of leaving it implicit. Parallel stage groups and synchronizing joins remain outside Podway ([deferred](goals-and-non-goals.md#deferred-not-required)). When several actors record their own conclusions instead of one caller, use the [multi-actor recipe](#multiple-actors-on-one-stage).

## Scratch worktrees within one task

When one task's write work should proceed in parallel — two implementation chunks, or a risky change tried in isolation — give the write work scratch worktrees while the session stays in the main checkout. The scratch worktrees never run `podway init`: they hold no Podway state and need none, because they are an execution detail of the active stage, not tasks of their own.

```bash
# The session lives here, in the main checkout, on its implement stage.
git worktree add ../scratch-store-split
git worktree add ../scratch-retry-path
# Two actors write concurrently, one per scratch worktree, outside Podway.
# Merge their branches back with normal Git tooling, then record the outcome:
podway set implementation-summary "Split the store and hardened the retry path."
podway complete
git worktree remove ../scratch-store-split
git worktree remove ../scratch-retry-path
```

The single active stage remains the gate: the session does not advance until the merged result is recorded, and each writing actor can record its own items concurrently as in the [multi-actor recipe](#multiple-actors-on-one-stage). Inside a scratch worktree the work is unguarded — no procedure applies there. When a chunk grows into work with its own goal and its own steps worth guarding, promote it to a [worktree of its own](#parallel-tasks-across-worktrees) with its own session, and fold the results back through an [integration session](../../examples/integration-session.md).

## Multiple actors on one stage

One active stage attempt does not mean one worker. Several actors — a review panel, for example — may work concurrently and each record their own items on the same attempt: "Multiple humans or agents may perform external work concurrently and update different items, but Podway has one authoritative current stage" ([ADR-0002](../../architecture-decision-records/0002-single-active-stage.md)).

Declare one item per actor on the stage — for example, a verdict and a note per review perspective — and let each actor record only its own items:

```bash
# Three reviewers, concurrently, against the same active attempt:
podway check review-performance-verdict --if-attempt <attempt-id> --json
podway check review-security-verdict --if-attempt <attempt-id> --json
podway set review-api-note "Naming is consistent with the existing surface." \
  --if-attempt <attempt-id> --if-item-revision 0 --json
```

Different items never conflict: each carries its own revision precondition, and the daemon's worktree queue serializes only the commits. The actors work in parallel; the ledger stays linear. The stage's required-item gate is the join: `podway complete` fails closed until every actor's conclusion is recorded.

Two multi-actor shapes compose in one worktree:

- **Concurrent division** — this recipe: several actors share one stage by splitting its items. The [side-work recipe](#concurrent-side-work-within-one-task) is the single-recorder variant of the same idea.
- **Stage relay** — serial multi-actor: each stage is owned by a different actor, and each successor takes over from recorded state exactly as in the [handoff walkthrough](../../examples/handoff-session.md).

Choose item division when one shared gate is enough. Choose separate stages — or, across tasks, separate worktrees — when each actor's work needs its own attempt lifecycle: retry and rework operate per stage attempt, not per item.

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
