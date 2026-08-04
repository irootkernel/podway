# Domain Model

The concrete rows and stage structures below define the implemented
`podway.procedure/v1` aggregate. V2 retains the one-session, one-active-attempt
boundary but adds separately versioned graph and workflow-record types.

## Aggregate boundary

The authoritative domain aggregate is the **current task session inside one workspace**. A workspace also owns queue and operational records, but only one session may exist at a time.

```text
Workspace
  + Workspace metadata
  + zero or one Task Session
      + one Procedure Snapshot
      + ordered Stage Progress rows
      + Stage Attempts
          + Item Values
          + Blockers
  + Mutation Jobs
  + Idempotency Records
  + Bounded Operational Journal
```

## Workspace

Fields:

| Field | Type | Meaning |
|---|---|---|
| `workspace_uuid` | UUID | Non-secret identity stored inside the worktree |
| `git_common_fingerprint` | string | Stable identity of the repository common directory |
| `git_worktree_fingerprint` | string | Stable identity of this worktree administrative directory |
| `last_validated_root` | path | Most recently validated canonical root |
| `next_workspace_sequence` | integer | Next durable mutation sequence |
| `created_at` | timestamp | Workspace initialization time |
| `updated_at` | timestamp | Last metadata update |

The root path may change after a move. Git identity and workspace UUID determine continuity.

## Workspace configuration

Configuration is tracked project data, not session state.

```yaml
schema: podway.workspace/v1
procedure_paths:
  - .podway/procedures
default_preset: sw-dev
job_queue:
  max_pending: 256
ui:
  show_stage_in_prompt: false
```

The configuration does not participate in a running session after its procedure snapshot is created, except for workspace-level UI and queue settings.

## Procedure snapshot

Fields:

| Field | Type | Meaning |
|---|---|---|
| `snapshot_id` | UUID | Internal immutable identity |
| `schema` | string | `podway.procedure/v1` |
| `procedure_id` | string | Stable author-defined ID |
| `procedure_version` | string | Author-defined version |
| `name` | string | Display name |
| `digest` | SHA-256 | Digest of Podway Canonical JSON v1 |
| `canonical_json` | JSON | Fully defaulted, validated procedure |
| `created_at` | timestamp | Snapshot creation time |

A snapshot is immutable and belongs to one session in v1. Deduplication is optional and does not change semantics.

## Task session

Fields:

| Field | Type | Meaning |
|---|---|---|
| `session_id` | UUID | Opaque session identity |
| `task_title` | string | Human-readable current task title |
| `procedure_snapshot_id` | UUID | Governing immutable procedure |
| `lifecycle` | enum | `running`, `completed`, or `cancelled` |
| `session_revision` | integer | Monotonic mutation revision, starts at 1 |
| `active_stage_index` | integer or null | Current ordered stage |
| `active_attempt_id` | UUID or null | Exactly one while running |
| `created_at` | timestamp | Start time |
| `completed_at` | timestamp or null | Final completion time |
| `cancelled_at` | timestamp or null | Cancellation time |
| `cancel_reason` | string or null | Required for cancellation |

A completed session may reopen. A cancelled session may only be reset.

## Stage progress

One row exists per stage in the snapshot.

Fields:

| Field | Type | Meaning |
|---|---|---|
| `stage_id` | string | Stable snapshot stage ID |
| `stage_index` | integer | Zero-based order |
| `progress_state` | enum | `pending`, `current`, `done`, `skipped`, `redo`, or `abandoned` |
| `latest_attempt_number` | integer | Highest created attempt number, starts at 0 |
| `latest_attempt_id` | UUID or null | Most recently created attempt |

`blocked` is derived when the `current` attempt has open blockers. `abandoned` is used only for the stage active when a session is cancelled. `blocked` is not stored as a separate progress state.

## Stage attempt

Fields:

| Field | Type | Meaning |
|---|---|---|
| `attempt_id` | UUID | Opaque identity |
| `session_id` | UUID | Owning session |
| `stage_id` | string | Snapshot stage |
| `attempt_number` | integer | Starts at 1 and increases per stage |
| `lifecycle` | enum | `active`, `completed`, `skipped`, or `abandoned` |
| `started_at` | timestamp | Activation time |
| `ended_at` | timestamp or null | Terminal time |
| `reason` | string or null | Retry, return, reopen, skip, or cancellation context |

Only one attempt may be active across the session.

## Item definition

An item definition is immutable snapshot data:

```text
id
type
prompt
help
required
type-specific constraints
```

Item definitions are not database rows separate from the canonical snapshot unless the store chooses to materialize them for query speed.

## Item value

One current value may exist per item and attempt.

Fields:

| Field | Type | Meaning |
|---|---|---|
| `attempt_id` | UUID | Owning attempt |
| `item_id` | string | Snapshot item ID |
| `item_type` | enum | Definition type |
| `item_revision` | integer | Starts at 1 on first value, increments on change |
| `value_json` | JSON | Canonical typed value |
| `created_at` | timestamp | First set time |
| `updated_at` | timestamp | Latest change time |

Clearing an item deletes the current value but stores the next logical item revision in a small revision row or tombstone so a stale writer cannot recreate from an obsolete revision. The reference DDL uses a persistent `item_slots` row with nullable value.

### Canonical value shapes

| Type | JSON value |
|---|---|
| `confirm` | `true` |
| `text` | string |
| `choice` | string |
| `integer` | integer |
| `list` | array of unique strings |
| `artifact` | artifact metadata object |

## Artifact metadata

Canonical artifact value:

```json
{
  "location_type": "path",
  "location": "tests/login_race.rs",
  "sha256_digest": "sha256:0123...",
  "size_bytes": 4812,
  "media_type": "text/x-rust"
}
```

For `path`, location is a normalized worktree-relative path. For `reference`, location is an opaque non-empty string supplied by the caller.

The daemon computes local-path digest and size. External references require explicit digest, size, and media type.

## Blocker

Fields:

| Field | Type | Meaning |
|---|---|---|
| `blocker_id` | UUID | Opaque identity |
| `attempt_id` | UUID | Active attempt at creation |
| `reason` | string | Required explanation |
| `state` | enum | `open` or `resolved` |
| `created_at` | timestamp | Creation time |
| `resolved_at` | timestamp or null | Resolution time |

Blockers do not carry into new attempts.

## Mutation job

Fields are defined in the daemon and storage specifications. A job is operational state, not part of the procedure domain. Its terminal result contains the public success or error envelope.

## Item satisfaction

An item is satisfied when a current value exists and all definition constraints hold:

- `confirm`: value is exactly `true`;
- `text`: trimmed Unicode scalar count is within configured bounds;
- `choice`: value exactly equals a declared choice;
- `integer`: value is within optional minimum and maximum;
- `list`: item count and each string length are within bounds, and uniqueness rules hold;
- `artifact`: all metadata fields are valid; a local path still exists and matches digest and size at completion time.

Optional items never block completion. Invalid optional values are rejected when set rather than stored in an invalid state.

## Stage completion readiness

A current attempt is ready to complete when:

```text
all required items are satisfied
AND no blocker is open
AND the session and attempt preconditions still match
```

Podway does not decide whether the user's claim is true. A checked confirmation is an explicit procedural assertion under the same-user trust model.

## Derived session view

`status` combines:

- snapshot stage metadata;
- stage progress;
- active attempt and item slots;
- open blockers;
- queued and running jobs;
- session and workspace revisions.

The view is derived from relational current state. The bounded journal is not required to reconstruct it.
