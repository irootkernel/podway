# JSON Contract

## Stability policy

Every public JSON response declares a schema identifier.

Current common-envelope compatibility rules:

- existing field meanings do not change;
- required fields are not removed;
- enum values are not repurposed;
- additive optional fields may be introduced only in an explicitly open schema;
- clients of the current generic envelope ignore unknown envelope fields;
- breaking changes require a new schema identifier such as `podway.output/v2`;
- human-readable `message` text is not stable; `code` and structured details are stable.

All JSON is UTF-8 and emitted as one object followed by a newline.

The accepted v0.1.0 automation target introduces discriminated, command-specific
closed result and error-detail schemas. Unknown fields in those closed objects are
invalid; adding a field requires a new result/detail schema identifier or
discriminator version. The current generic `result` and `details` objects remain
the implemented machine schema until the `MCONT` epic updates schemas, catalogs,
fixtures, and binaries together. See the
[automation contract](34-automation-client-contract.md#21-command-specific-json-schemas-aut-json-001004).

## Common success envelope

```json
{
  "schema": "podway.output/v1",
  "request_id": "2037d76d-6ea8-42c2-a11f-883248bb8774",
  "command": "session.complete",
  "generated_at": "2026-07-13T03:10:04.123Z",
  "workspace": {
    "uuid": "83f9fbaa-3df8-4c23-8253-f94098b0af63",
    "root": "/Users/example/src/project-wt",
    "latest_workspace_sequence": 41
  },
  "job": {
    "id": "6b8c38e8-0051-475a-a4d0-0cb07eb8fc12",
    "sequence": 41,
    "state": "succeeded",
    "submitted_at": "2026-07-13T03:10:04.100Z",
    "finished_at": "2026-07-13T03:10:04.121Z"
  },
  "session": {
    "id": "c1ddd90d-5b6b-40a9-a814-885c6215b916",
    "title": "add bounded retry backoff",
    "lifecycle": "running",
    "revision_before": 12,
    "revision_after": 13
  },
  "result": {},
  "warnings": []
}
```

Optional top-level fields are omitted when not applicable. Static commands omit `workspace`, `job`, and `session`. Read commands omit `job` unless waiting on a named job is relevant.

## Common error envelope

```json
{
  "schema": "podway.error/v1",
  "request_id": "2037d76d-6ea8-42c2-a11f-883248bb8774",
  "command": "session.complete",
  "generated_at": "2026-07-13T03:10:04.123Z",
  "code": "REQUIRED_ITEMS_MISSING",
  "message": "The active stage is missing required items.",
  "retryable": false,
  "exit_code": 1,
  "workspace": {
    "uuid": "83f9fbaa-3df8-4c23-8253-f94098b0af63",
    "root": "/Users/example/src/project-wt"
  },
  "details": {
    "stage_id": "verify",
    "missing_item_ids": [
      "relevant-checks-passed",
      "verification-note"
    ]
  }
}
```

The normative schemas are in [`../../schemas/output-v1.schema.json`](../../schemas/output-v1.schema.json) and [`../../schemas/error-v1.schema.json`](../../schemas/error-v1.schema.json).

If a mutation response is lost after transmission may have begun, the local CLI
preserves the original correlation and key without guessing admission:

```json
{
  "schema": "podway.error/v1",
  "request_id": "2037d76d-6ea8-42c2-a11f-883248bb8774",
  "command": "session.start",
  "generated_at": "2026-07-13T03:10:04.123Z",
  "code": "MUTATION_OUTCOME_UNKNOWN",
  "message": "mutation outcome is unknown; reconcile by idempotency key",
  "retryable": true,
  "exit_code": 4,
  "details": {
    "schema": "podway.mutation-outcome-unknown-details/v1",
    "outcome": "unknown",
    "idempotency_key": "original-key",
    "reconcile": {
      "command": "job.lookup",
      "idempotency_key": "original-key"
    }
  }
}
```

## Status result

```json
{
  "task": {
    "title": "fix duplicate login session creation",
    "procedure": {
      "id": "bug-fix",
      "version": "1",
      "name": "Bug Fix",
      "digest": "sha256:..."
    }
  },
  "session": {
    "id": "...",
    "lifecycle": "running",
    "revision": 17,
    "created_at": "2026-07-13T03:00:00.000Z",
    "completed_at": null,
    "cancelled_at": null
  },
  "current": {
    "stage_id": "verify",
    "stage_index": 4,
    "title": "Verify the result",
    "attempt_id": "...",
    "attempt_number": 2,
    "blocked": false,
    "ready_to_complete": false
  },
  "stages": [
    {
      "id": "reproduce",
      "index": 0,
      "title": "Reproduce the problem",
      "status": "done",
      "latest_attempt_number": 1
    },
    {
      "id": "verify",
      "index": 4,
      "title": "Verify the result",
      "status": "current",
      "latest_attempt_number": 2
    },
    {
      "id": "review",
      "index": 5,
      "title": "Review the result",
      "status": "redo",
      "latest_attempt_number": 1
    }
  ],
  "items": [
    {
      "id": "relevant-checks-passed",
      "type": "confirm",
      "prompt": "Relevant verification completed successfully.",
      "required": true,
      "satisfied": false,
      "revision": 0,
      "value": null
    }
  ],
  "blockers": [],
  "queue": {
    "pending_mutations": false,
    "queued_count": 0,
    "running_job_id": null,
    "latest_workspace_sequence": 41
  }
}
```

For a completed or cancelled session, `current` is `null`.

## Next result

```json
{
  "stage": {
    "id": "verify",
    "title": "Verify the result",
    "attempt_id": "...",
    "attempt_number": 2,
    "instructions": [
      "Run the relevant verification for the current implementation."
    ]
  },
  "missing_required_items": [
    {
      "id": "verification-note",
      "type": "text",
      "prompt": "Summarize what was verified."
    }
  ],
  "blockers": [],
  "allowed_actions": {
    "complete": false,
    "skip": false,
    "retry": true,
    "return_to": ["reproduce", "diagnose", "regression", "fix"],
    "cancel": true
  },
  "next_stage_after_completion": {
    "id": "review",
    "title": "Review the result"
  },
  "suggestions": [
    {
      "command": "item.set",
      "argv": ["podway", "set", "verification-note", "<text>"],
      "item_id": "verification-note"
    }
  ]
}
```

Suggestions are structured argument arrays, not shell source. Renderers may quote them for display.

## Mutation result

A transition result uses a common shape:

```json
{
  "changed": true,
  "revision_before": 12,
  "revision_after": 13,
  "active_before": {
    "stage_id": "implement",
    "attempt_id": "...",
    "attempt_number": 1
  },
  "affected_stages": [
    {
      "stage_id": "implement",
      "before": "current",
      "after": "done"
    },
    {
      "stage_id": "verify",
      "before": "pending",
      "after": "current"
    }
  ],
  "active_after": {
    "stage_id": "verify",
    "attempt_id": "...",
    "attempt_number": 1
  }
}
```

Command-specific fields may be added, for example the attached artifact metadata or resolved blocker IDs.

## Detached admission result

```json
{
  "schema": "podway.output/v1",
  "command": "session.complete",
  "workspace": {
    "uuid": "...",
    "root": "/...",
    "latest_workspace_sequence": 42
  },
  "job": {
    "id": "...",
    "sequence": 42,
    "state": "queued",
    "submitted_at": "2026-07-13T03:10:04.100Z",
    "finished_at": null
  },
  "result": {
    "admission": {
      "admitted": true,
      "job_id": "...",
      "workspace_sequence": 42
    },
    "detached": true
  },
  "warnings": []
}
```

Exit code is 0 after successful durable admission, even though the mutation may later fail.
Every successful mutation, detached or terminal, carries the same closed
`result.admission` object. Its job ID and workspace sequence exactly match the
top-level `job` projection.

## Job result

`job status` and `job wait` include:

```text
id
sequence
command
state
submitted_at
claimed_at
finished_at
terminal_response
```

`terminal_response` is the immutable terminal response projection for succeeded,
failed, or cancelled jobs. It is `null` only for queued or running jobs. A
cancelled job uses `{ "kind": "cancelled", "payload": { "cancelled": true } }`.

## Artifact JSON

```json
{
  "location_type": "path",
  "location": "tests/login_race.rs",
  "sha256_digest": "sha256:...",
  "size_bytes": 4812,
  "media_type": "text/x-rust"
}
```

Local artifact locations are worktree-relative. External references remain opaque strings.

## Null and omission rules

- Optional top-level sections are omitted when inapplicable.
- Fields that are part of a stable object shape but currently have no value use `null`, such as `completed_at` on a running session.
- Empty collections are emitted as `[]` or `{}` rather than omitted when clients need to distinguish “known empty” from “not applicable.”
- Booleans are never represented as integers or strings.

## Ordering

Arrays have deterministic semantic ordering:

- stages by stage index;
- items by procedure definition order;
- blockers by creation time then ID;
- jobs by descending sequence in list output unless requested otherwise;
- allowed return destinations by stage order;
- suggestions by item order.

JSON object key order is not a public contract.
