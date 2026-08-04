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
discriminator version. Integration-critical `result` objects now use
command-selected closed schemas. Daemon endpoint and contract failures, identity
and digest mismatches, revision and attempt conflicts, idempotency failures, and
wait timeouts also use code-selected closed detail schemas. Every closed result
and closed error-detail object carries its own `schema` discriminator; current
v1 decoders reject a missing, mismatched, or unknown discriminator. Manifest-
covered known-answer fixtures lock the catalogs, schemas, runtime decoders,
canonical digests, and compact envelope boundary together. See the
[automation contract](automation-client-contract.md#21-command-specific-json-schemas-aut-json-001004).

## Released v1 compatibility boundary

The v0.1.0, v0.1.1, and v0.1.2 releases publish byte-identical copies of the public v1
Procedure, output, error, status-result, next-result, and version-result schemas.
The v0.1.0 files were published below `schemas/`; the canonical v0.1.1 and v0.1.2 source
location moved to `assets/schemas/` without changing their bytes.

| Schema | SHA-256 |
|---|---|
| `procedure-v1.schema.json` | `cc2aed122f69032d612626248da614aee79bccbaa6a0a151e531119590b13e42` |
| `output-v1.schema.json` | `19355e4f256fba8b17a4813f332006603b4e103fd747786f5e13d6447c2c55cd` |
| `error-v1.schema.json` | `371ccd1e07a0f503bc70a5a4b167ff43a0dfbe9ed8e5b78533cd9a7848d06bf8` |
| `status-result-v1.schema.json` | `50e8a1da908dee02751bd70a19b820460925e40b2cb16ee8f6dc749725102032` |
| `next-result-v1.schema.json` | `a27e51dad161a9ef8c6de67da6f372a7b3d2337ff3ca2598f1ecb4f1ae627f56` |
| `version-result-v1.schema.json` | `fe92513aa0cb4f75bd02e220b9feb5bf19795105cf364518f4119689e02baf7c` |

No released v1 contract requires a v2 identifier or an unversioned migration.
Before v0.1.0, commit `754ff5d7e764e74234f0b22a0a6fe255bfa09ea4`
changed the Procedure schema's explicit `list.max_items` minimum from zero to
one. The core constructor already rejected zero; the change aligned schema and
configuration admission with that runtime behavior before the first public
release. A consumer pinned to an earlier pre-release schema-only snapshot must
replace `max_items: 0` with a supported value in `1..=1000`. Zero was never an
accepted runtime value and is not restored by v0.1.2.

## Procedure v2 result families

The released v1 envelopes and result schemas above remain unchanged. Existing
Procedure-aware routes select eight new `/v2` families for validation, detached
admission, start, compact status, status, next, stage transitions, and item
mutations. The version-neutral `job-result/v1` and `job-lookup-result/v1`
wrappers remain unchanged: a terminal v2 job identifies its closed inner result
by that result's own discriminator.

The thirteen v2-only routes are prepared by nine `/v1` families. Format,
scaffold, and convert share `procedure-source-result/v1` on success. Every v2
authoring route, including the existing `procedure.validate` route, selects
`procedure-diagnostics-result/v1` when it must return structured diagnostics;
vet, lint, and check also use that family for successful diagnostic reports.
Graph, preview, decide, rework, goal definition, goal revision, and criterion
assessment otherwise select the correspondingly named closed family.
`authoring-diagnostic/v1` is the standalone diagnostic object used by bounded
diagnostic results. Registering these schemas does not make the future routes
callable; route registration is a separate contract step.

Every family is closed, uses the Procedure v2 identifier and value bounds where
applicable, and is selected through the version-aware protocol registry. V2
validation success is metadata-only (`file`, Procedure schema, digest, and
`valid: true`) so it does not duplicate a maximum-size Procedure inside the
retained output envelope. Source and graph text projections cap one scalar at
131,072 characters; production encoding must still reject an oversized complete
serialized output.

Preview is one closed report rather than a loose graph summary. Every result
contains the source file, admissibility, validate/vet/lint checks, and bounded
diagnostics. An admissible result additionally contains procedure identity and
purpose, the canonical digest, goal policy, the complete bounded summary,
normalized graph, Mermaid, and a structured `session.start` suggestion whose
argv contains that digest. An inadmissible result cannot contain a start
suggestion.

Verbose `status-result/v2` adds six independently bounded, newest-first history
windows: current trace, stale attempts, decisions, rework, stale goal revisions,
and stale goal assessments. Every entry carries the execution `trace_sequence`
it belongs to; the shared `--history-before` cursor can therefore page all six
windows without adding a separate public history route. Standard status forbids
every history window.

The retained `podway.output/v1` envelope is intentionally still open. A v2
producer therefore applies an additional production guard: at most four warning
objects, each with exactly bounded `code` (64 characters), `path` (256
characters), and `message` (512 characters), followed by the complete frame-size check. This is a v2
production obligation and does not alter the released v1 envelope schema.

## Version identity envelope

`podway version --json --identity` and `podwayd version --json --identity`
each emit exactly one newline-terminated `podway.output/v1` document whose
command is `version`. The shared result is a closed
`podway.version-result/v1` object, including its required `schema`
discriminator and complete product, build, source, target, manifest, and IPC
identity. The two result objects are exactly equal; only envelope correlation
and generation metadata may differ.

Runtime probes decode and validate the complete output envelope and its
command-selected result. A bare result, error envelope, wrong outer or result
discriminator, wrong command, missing result field, or unknown result field is
rejected. The common output envelope remains open to additive envelope fields
as defined by the public schema; the version result remains closed.

### Manifest-bound validation registry

Release validation uses one feature-gated Rust verifier for both the source tree
and an extracted distribution. It validates the closed manifest shape and
canonical self-digest, verifies every member digest, and requires the manifest's
schema inventory to equal the physical version-named schema inventory with unique `$id`
values. Source logical paths map only to the canonical `assets/` authorities;
packaged logical paths resolve directly below `share/podway`.

The verifier registers schemas only by their manifest-bound `$id` and canonical
`podway:///schemas/...` packaged path. Local fragments are allowed. Missing,
unknown, network, external-filesystem, duplicate, stale, symlinked, or escaping
resources fail closed; the JSON Schema implementation has no fallback retriever.
Every registered schema must compile against this offline registry. The same
registry validates the complete binary identity envelope against
`output-v1.schema.json` and its result separately against
`version-result-v1.schema.json` before expected identity fields or binary
equality are evaluated.

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
  "result": {
    "schema": "podway.stage-transition-result/v1",
    "changed": true,
    "revision_before": 12,
    "revision_after": 13,
    "admission": {
      "admitted": true,
      "job_id": "6b8c38e8-0051-475a-a4d0-0cb07eb8fc12",
      "workspace_sequence": 41
    }
  },
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

The normative schemas are in [`../../../assets/schemas/output-v1.schema.json`](../../../assets/schemas/output-v1.schema.json) and [`../../../assets/schemas/error-v1.schema.json`](../../../assets/schemas/error-v1.schema.json).

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
  "schema": "podway.status-result/v1",
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

## Compact idle status result

`status --wait-for-idle --compact` returns a closed, value-free projection for
automation decisions. It is available only with the idle barrier; a successful
response always reports an idle queue from the authoritative post-barrier read.
Workspace UUID, root, and sequence remain in the common envelope's `workspace`
object, and its sequence must match the result queue sequence.

```json
{
  "schema": "podway.compact-status-result/v1",
  "procedure": {
    "id": "bug-fix",
    "version": "1",
    "digest": "sha256:..."
  },
  "session": {
    "id": "...",
    "lifecycle": "running",
    "revision": 17
  },
  "current": {
    "stage_id": "verify",
    "attempt_id": "...",
    "attempt_number": 2,
    "ready_to_complete": false
  },
  "items": [
    {
      "id": "relevant-checks-passed",
      "type": "confirm",
      "required": true,
      "satisfied": false,
      "revision": 0
    }
  ],
  "blockers": [
    {
      "id": "...",
      "attempt_id": "...",
      "state": "open"
    }
  ],
  "queue": {
    "pending_mutations": false,
    "queued_count": 0,
    "running_job_id": null,
    "latest_workspace_sequence": 41
  }
}
```

The compact form omits instructions, prompts, titles, item values, blocker
reasons, stage history, and previous-attempt narratives. Only open blockers are
listed; terminal sessions use `current: null` with empty `items` and `blockers`.
An attempt may contain at most 1,024 open blockers. Resolved blockers remain in
history without consuming that limit, which keeps every reachable
maximum-size compact projection within the wire-size budget.
The complete compact JSON envelope, including its trailing newline, is limited
to 262,144 UTF-8 bytes.

## Next result

```json
{
  "schema": "podway.next-result/v1",
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
  "schema": "podway.stage-transition-result/v1",
  "changed": true,
  "revision_before": 12,
  "revision_after": 13,
  "admission": {
    "admitted": true,
    "job_id": "...",
    "workspace_sequence": 42
  }
}
```

Each command and variant selects one closed result shape; undocumented fields are rejected.

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
    "schema": "podway.detached-admission-result/v1",
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
schema
id
sequence
command
state
submitted_at
claimed_at
finished_at
terminal_response
```

For succeeded and failed jobs, `terminal_response` is the complete immutable
original `podway.output/v1` or `podway.error/v1` response envelope. Its request
ID, command, completion timestamp, workspace, job, session/result, warnings, and
public error fields therefore survive response loss, daemon restart, and job-row
pruning. It is `null` only for queued or running jobs. A cancelled job uses the
closed summary `{ "kind": "cancelled", "payload": { "cancelled": true } }`.

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
