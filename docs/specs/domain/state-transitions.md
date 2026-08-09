# State Transitions

The transition table and algorithms below define the implemented
`podway.procedure/v1` command behavior. V2 commands and versioned dispatch do not
change these v1 transitions.

## General command rules

Every successful state-changing session mutation:

- is executed by `podwayd`;
- validates workspace identity;
- applies to the current session and expected attempt;
- commits atomically with its terminal job result;
- increments `session_revision` exactly once;
- returns the new current state summary.

A failed mutation changes no session rows. Dry-run commands are reads and create no job.

## Transition summary

The `Command` column uses canonical IPC command identifiers from [`../../../assets/specifications/command-catalog.yaml`](../../../assets/specifications/command-catalog.yaml).

| Command | Allowed lifecycle | Principal preconditions | Main effect |
|---|---|---|---|
| `session.start` | no session | valid procedure | create session and first attempt |
| `session.start_replace` | any existing session | confirmation and current session revision | atomically replace the current task |
| `item.*` | running | current attempt and item preconditions | update one item slot |
| `session.complete` | running | required items satisfied, no blockers | finish current attempt and advance |
| `session.skip` | running | stage skippable | skip current attempt and advance |
| `session.retry` | running | current attempt | abandon and recreate same stage |
| `session.return` | running | allowed earlier destination | abandon current, activate destination, mark reached downstream redo |
| `session.block` | running | current attempt | add blocker |
| `session.unblock` | running | blocker on current attempt | resolve blocker |
| `session.cancel` | running | current attempt | abandon attempt and cancel session |
| `session.reopen` | completed | allowed destination | reactivate destination and mark later stages redo |
| `session.reset` | any existing | confirmation and current session revision | delete session-scoped state |
| `workspace.reset_all` | any or unreadable workspace state | force confirmation | recreate all disposable workspace runtime state |

## Start

Input:

```text
task_title
procedure_snapshot
session_id
first_attempt_id
timestamp
```

Preconditions:

- workspace initialized;
- no session exists;
- task title is non-empty and within 500 characters;
- procedure snapshot is valid.

Effects:

1. insert snapshot;
2. create session `running`, revision 1;
3. create stage-progress rows, first `current`, remaining `pending`;
4. create attempt 1 for first stage as `active`;
5. set active stage and attempt.

`session.start_replace` is selected by `podway start --replace`. It first performs reset semantics in the same exclusive daemon operation, requires the existing session identity and revision when readable, and requires explicit confirmation.

## Item mutations

### Check

`check` applies only to `confirm` and sets value `true`.

### Uncheck or clear

`uncheck` clears a `confirm`. `clear` clears any item type.

Clearing a missing value is idempotent and returns success without incrementing the item revision, but it still creates a successful no-op job response. It does not increment session revision unless state changed. This is the one exception to the general mutation-revision rule: successful semantic no-ops report `changed=false` and keep the revision unchanged.

### Set

`set` applies to `text`, `choice`, and `integer`. CLI parsing converts the argument to the declared type before admission; the daemon validates again.

### Add and remove

`add` and `remove` apply to `list`.

- add rejects a duplicate when `unique=true`;
- remove fails with `LIST_VALUE_NOT_FOUND` unless `--ignore-missing` is used;
- list order is stable.

### Attach

`attach` applies to `artifact`.

For a local path, the daemon canonicalizes, opens read-only, hashes, determines size and media type, then stores metadata. For a reference, the caller supplies complete metadata.

### Item revision effects

A changed item mutation:

- increments the item slot revision;
- increments session revision once;
- updates timestamps.

Unrelated item slots are untouched.

## Complete

Preconditions:

- session is running;
- expected session revision and active attempt match;
- every required item is satisfied;
- no blocker is open;
- every required local artifact still exists and matches stored size and SHA-256;
- current attempt is active.

Effects for a non-final stage:

1. mark current attempt `completed` and set end time;
2. mark current stage `done`;
3. choose the next ordered stage;
4. create its next attempt number;
5. set next stage `current` and new attempt `active`;
6. clear its former `redo` status by activation;
7. update session cursor;
8. increment session revision.

Effects for final stage:

1. mark attempt `completed`;
2. mark final stage `done`;
3. set session `completed` and completion time;
4. clear active cursor;
5. increment revision.

## Skip

Preconditions:

- session running and attempt current;
- stage skip policy allows skip;
- reason supplied when required.

Required items and blockers do not prevent an explicitly permitted skip. This is intentional: skip is a distinct disposition.

Effects mirror complete, except attempt lifecycle and stage progress become `skipped`.

## Retry

Preconditions:

- session running;
- current attempt matches;
- non-empty reason.

Effects:

1. mark active attempt `abandoned` with reason and end time;
2. leave stage progress `current`;
3. increment the stage's latest attempt number;
4. create a fresh active attempt with no item values or blockers;
5. update session active attempt;
6. increment revision.

## Return

Preconditions:

- session running;
- destination index is lower than current stage index;
- destination is allowed by the procedure;
- non-empty reason;
- expected session revision and active attempt match.

Effects:

1. abandon current attempt with reason;
2. for the destination stage, create the next attempt and set `current`;
3. for every stage after the destination and at or before the highest reached stage, set `redo`;
4. leave never-reached later stages `pending`;
5. do not delete old attempts or item values;
6. set session cursor to the destination attempt;
7. increment revision once.

The destination is displayed `current`, not `redo`, because its new attempt is already active.

## Block

Preconditions:

- session running;
- expected active attempt matches;
- non-empty reason.

Effect: create one open blocker on the attempt and increment revision.

Duplicate reason text is allowed because blockers have distinct identities.

## Unblock

A blocker may be resolved only while its attempt remains active. Resolving a blocker from an old attempt fails with `BLOCKER_NOT_CURRENT`.

`unblock --all` resolves every open blocker on the active attempt in one revision.

## Cancel

Preconditions:

- session running;
- expected active attempt matches;
- non-empty reason.

Effects:

1. abandon active attempt with cancellation reason;
2. change the current stage progress from `current` to `abandoned`;
3. set session `cancelled` and timestamp;
4. clear active cursor;
5. increment revision.

No further session mutation is allowed except reset.

## Reopen

Preconditions:

- session completed;
- destination exists and is permitted as a rework destination from the completed procedure position;
- non-empty reason;
- expected completed session revision matches.

Effects:

1. set session lifecycle to `running` and clear completion time;
2. create a fresh destination attempt as `active`;
3. set destination `current`;
4. mark every reached later stage `redo`;
5. set active cursor;
6. increment revision.

A cancelled session cannot reopen.

## Reset

`session.reset`, selected by `podway reset`, deletes:

- task session row;
- procedure snapshot for that session;
- stage progress;
- attempts;
- item slots;
- blockers;
- session-scoped idempotency receipts.

It preserves workspace identity, schema metadata, tracked files, and daemon installation.

Reset is destructive and requires confirmation in interactive mode or `--yes` in non-interactive mode.

`workspace.reset_all`, selected by `podway reset --all --force`, follows the filesystem-marker recovery protocol in [Recovery, Retention, and Maintenance](../storage/recovery-retention-and-maintenance.md). It is a separate canonical command because it can operate when the SQLite database is unreadable and it recreates workspace runtime state rather than deleting only a valid session.

## Dry run

`return`, `reopen`, `reset`, and `start --replace` support `--dry-run` where meaningful. Dry run:

- reads current committed state;
- validates the proposed command and preconditions;
- returns affected stages and records;
- creates no job;
- does not reserve a revision;
- may become stale immediately, so the real command still revalidates.

## Transition response

Every successful mutation response includes:

```text
changed
session_revision_before
session_revision_after
active_stage_before
affected_stages
active_stage_after
job identity and sequence
```

No-op item clears return equal before and after revisions with `changed=false`.

## Procedure v2 cursor and history transitions

A running Procedure v2 session owns one cursor and exactly one active graph-node
attempt. Completing or skipping an action, or deciding an option, terminates the
current attempt and either activates one fresh target attempt or completes the
session. Retry abandons the active attempt and creates a fresh attempt of the
same placement, whether it is an action or decision. Only that active attempt
becomes stale: its item values, blockers, criterion state, evidence snapshot, and
terminal reason remain immutable history. The fresh attempt receives the next
per-node attempt number and session trace sequence, begins with empty item,
blocker, and criterion state, and resolves every declared evidence reference at
its activation time. Retry does not increment rework-traversal counters or alter
unrelated attempts. No transition activates parallel attempts or waits for
another branch.

Evidence references resolve once when a decision attempt is activated and bind
the exact source attempt and complete recorded-item digest. A reference becomes
stale when rework invalidates its source. Stale references and records remain
reportable but cannot satisfy readiness or decision preconditions.

Declared rework and manual rework target graph placements, create a fresh target
attempt, and causally invalidate the affected trace suffix. A manual target must
be allowed by the immutable Procedure and occur on the current trace. Historical
attempts, decisions, rework records, goal revisions, and assessments are retained
in bounded newest-first status windows and remain non-satisfying after staleness.

## Procedure v2 goal and criterion transitions

`goal define` is accepted exactly once for an opted-in session. `goal revise`
requires the current goal revision, a non-empty reason, an allowed revision-safe
rework target, and `--reactivate` when the session is completed. It atomically
creates the next immutable goal revision, invalidates the affected suffix, and
activates a fresh target attempt. Cancelled sessions cannot be reactivated.

`goal assess-criterion` is valid only on the active goal-assessment decision
attempt. It validates the current goal revision, criterion identity, homogeneous
assessment mode, required reason, and every citation target. Evidence citations
must name fresh resolved references; local citations must name items persisted
on the active decision attempt. `not_applicable` accepts no citations. Retry or
staleness clears the satisfying effect of attempt-local criterion state.

The assessment decision succeeds only after every current criterion has a fresh
result and the selected option matches the deterministically derived goal
outcome. Podway validates formal state and identity but not the semantic truth or
relevance of a result.
