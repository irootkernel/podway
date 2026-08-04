# Rework and Session Lifecycle

This document defines the implemented `podway.procedure/v1` lifecycle. V2 graph
rework is governed by the v2 contract baseline and does not reinterpret v1
`return`, `reopen`, or `redo`.

## Purpose

Rework is a primary Podway behavior. It ensures that a later discovery cannot leave earlier-dependent work marked complete without repeating the relevant ordered stages.

## Migration continuity

Migration is storage-only and MUST preserve the meaning of every retained session. It MUST NOT change a session lifecycle or cursor; create, abandon, reactivate, renumber, or reuse an attempt; alter stage progress including `redo`; or lose item values and revisions, artifact metadata, blockers, or durable job and idempotency state.

After migration, retry, return, reopen, and reset MUST obey the same attempt boundaries and lifecycle rules in this document. A migration MUST NOT duplicate a user mutation, and it MUST NOT reconstruct session history removed by reset. If those invariants cannot be preserved, the migration fails without changing the prior state.

Podway uses conservative ordered invalidation:

```text
return to stage N
  -> start a fresh attempt of N
  -> mark every reached stage after N as redo
  -> leave never-reached stages pending
```

There is no dependency graph or selective evidence invalidation.

## Attempt boundaries

Every attempt owns its own:

- item values;
- artifact metadata;
- blockers;
- start and end timestamps;
- terminal lifecycle and reason.

New attempts start empty. The implementation MUST NOT copy old values automatically, even when the user expects to repeat the same work. Explicit repetition is the omission-prevention mechanism.

The UI MAY show previous attempt summaries in verbose status, but MUST visually separate them from the active attempt and MUST NOT present them as satisfying current requirements.

## Retry example

Before:

```text
implement  done
verify     current, attempt 1
review     pending
finish     pending
```

Command:

```bash
podway retry --reason "verification used the wrong configuration"
```

After:

```text
implement  done
verify     current, attempt 2, empty items
review     pending
finish     pending
```

`verify` attempt 1 is `abandoned`.

## Return example

Before:

```text
understand  done
plan        done
implement   done
verify      done
review      current
finish      pending
```

Command:

```bash
podway return --to implement --reason "review found a missing cancellation path"
```

After:

```text
understand  done
plan        done
implement   current, new attempt
verify      redo
review      redo
finish      pending
```

The old review attempt is abandoned. Old implement and verify attempts remain session-local history but do not satisfy the new path.

## Progressing through redo

When the new `implement` attempt completes:

```text
implement  done
verify      current, new attempt
review      redo
finish      pending
```

When verification completes:

```text
verify      done
review      current, new attempt
```

The normal forward transition automatically creates a new attempt when entering a `redo` stage.

## Return to the first stage

With `allow_return_to: any_previous`, returning to the first stage is valid from any later stage. Every reached later stage becomes `redo`. Never-reached stages remain `pending`.

The return destination cannot equal the current stage. Use retry for that case.

## Return allowlist

A restricted procedure may allow only:

```yaml
rework:
  allow_return_to:
    - plan
    - implement
    - verify
```

Attempting to return elsewhere fails with `RETURN_NOT_ALLOWED` and reports the allowed destinations.

## Reopen completed work

A completed session remains inspectable until reset. If a problem is found before the worktree begins another task:

```bash
podway reopen --to verify --reason "new failure observed before final report"
```

Reopen:

- changes lifecycle back to `running`;
- creates a new attempt at the selected stage;
- marks reached later stages `redo`;
- preserves previous attempts for the current session.

Reopen is not a long-term post-mortem feature. Once reset occurs, the session cannot be restored.

## Skip interaction

A previously skipped stage becomes `redo` when it lies downstream of a return destination. When reactivated, it receives a new attempt and may be completed or skipped again according to the procedure's current immutable skip policy.

The previous skip reason does not automatically apply.

## Blocker interaction

Blockers belong to attempts.

- retry abandons all blockers with the old attempt;
- return abandons blockers on the current attempt;
- downstream completed attempts normally have no open blockers;
- a new attempt starts unblocked.

## Artifact interaction

Artifact metadata belongs to an attempt. A new attempt requires a new `attach` operation for any required artifact item. Podway does not reuse the old digest automatically.

Local artifact paths are rehashed at completion. If the file changed after attachment, completion fails with `ARTIFACT_CHANGED`; the user must attach it again.

## Session lifecycle diagram

```text
                 final complete
        +------------------------------+
        |                              v
     running ----------------------> completed
        |                               |
        | cancel                        | reopen
        v                               |
     cancelled                          +----> running

running/completed/cancelled --reset--> no session
```

Rules:

- only running sessions have an active attempt;
- only completed sessions may reopen;
- cancelled sessions show the interrupted stage as `abandoned` and require reset;
- completed and cancelled session data is retained only until reset or worktree deletion.

## Reset boundary

Reset is the product's history boundary.

After reset, Podway intentionally cannot answer:

- what previous attempts existed;
- why a previous return occurred;
- which items were set;
- which artifacts were referenced;
- which jobs executed for that task.

This is acceptable and intentional because Podway manages the current task, not long-term evidence or audit.
