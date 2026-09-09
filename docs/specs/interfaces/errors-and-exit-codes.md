# Errors and Exit Codes

## Contract

Public failures use stable uppercase error codes. Human messages may improve without a schema change. Automation must branch on `code`, `retryable`, and structured `details`.

The machine-readable runtime catalog is
[`../../../assets/specifications/error-codes.json`](../../../assets/specifications/error-codes.json).
Authoring-time findings use the separate, disjoint
[`../../../assets/specifications/authoring-diagnostics.json`](../../../assets/specifications/authoring-diagnostics.json)
catalog. A code belongs to exactly one catalog.

The tables below are the implemented machine contract. Automation errors have
closed detail schemas, canonical assets, and executable contract tests.

The v2 contract baseline registers 26 additive runtime codes and the exhaustive
authoring diagnostic inventory. Goal-bearing forms of the shared start routes and
the `goal.define`, `goal.revise`, and `goal.assess_criterion` routes are executable
through their typed Procedure v2 boundaries. A well-formed registered but unserved
request returns
`UNSUPPORTED_V2_CAPABILITY`; a malformed request returns `REQUEST_INVALID`; and
a route absent from the registry retains ordinary unknown-command or usage
behavior. Manifest registration alone does not imply executable capability.
`workspace.remove` is executable and uses the existing
`WORKSPACE_PATH_UNSAFE`, `WORKSPACE_UUID_MISMATCH`,
`WORKSPACE_MAINTENANCE`, and `WORKTREE_GONE` families rather than introducing a
parallel removal-specific error taxonomy.

The account-runtime reset error family is defined in the
[runtime reset specification](../operations/runtime-reset.md#public-results-errors-and-bounds)
and `podway.runtime-reset-error-details/v1`. Its seven `RUNTIME_RESET_*` codes
are catalogued together; read-only plan errors are executable while apply and
control remain reserved. Partial
retirement is a non-success result with exact retry information, never rollback.

## Exit codes

| Exit code | Class | Meaning |
|---:|---|---|
| 0 | success | Operation succeeded, including detached durable admission |
| 1 | domain | Valid request rejected by procedure or session rules |
| 2 | usage | Invalid CLI syntax, option combination, request shape, or confirmation |
| 3 | daemon | Service, IPC, or protocol unavailable/incompatible |
| 4 | conflict | Concurrency conflict, queue pressure, or wait timeout; usually retryable |
| 5 | workspace | Git worktree, path, database, migration, or local-state failure |
| 6 | internal | Unexpected implementation failure |

## Daemon and protocol errors

| Code | Exit | Retryable | Meaning |
|---|---:|---:|---|
| `DAEMON_NOT_INSTALLED` | 3 | no | User service is not installed |
| `DAEMON_UNAVAILABLE` | 3 | yes | Socket cannot be reached |
| `DAEMON_STARTING` | 3 | yes | The control plane is reachable but normal admission is not open |
| `DAEMON_READINESS_TIMEOUT` | 3 | yes | Verified readiness was not reached before a bounded deadline |
| `DAEMON_SHUTTING_DOWN` | 3 | yes | Daemon is draining and not accepting work |
| `DAEMON_VERSION_INCOMPATIBLE` | 3 | no | CLI and daemon cannot share a supported contract |
| `DAEMON_CONTRACT_MISMATCH` | 3 | no | CLI and daemon product or manifest identity differs |
| `PROTOCOL_VERSION_UNSUPPORTED` | 3 | no | Requested IPC protocol is unsupported |
| `REQUEST_TOO_LARGE` | 2 | no | IPC frame exceeds limits |
| `REQUEST_INVALID` | 2 | no | Request is malformed or violates schema |
| `SOCKET_ENDPOINT_INVALID` | 2 | no | An explicit Unix socket path is invalid or cannot be safely resolved |

`SOCKET_ENDPOINT_INVALID.details` uses
`podway.socket-endpoint-error-details/v1`. Its stable `reason` is one of
`empty`, `relative`, `unnormalized`, `workspace_local`, `path_too_long`, or
`effective_user_unavailable`. Mutation failures also include
`admission: {"admitted": false}`.

Service lifecycle failures retain `DAEMON_UNAVAILABLE`, exit code 3,
retryability, and the existing endpoint details schema. Their human-readable
message may distinguish launchd failure, launchctl timeout or oversized output,
lifecycle-lock timeout, permission denial, unavailable service state, and an
unexpected process transition. Automation must not branch on those messages.

## Worktree and workspace errors

| Code | Exit | Retryable | Meaning |
|---|---:|---:|---|
| `NOT_A_GIT_WORKTREE` | 5 | no | No valid worktree contains the path |
| `BARE_GIT_REPOSITORY` | 5 | no | Bare repositories are unsupported |
| `WORKTREE_GONE` | 5 | no | Registered worktree no longer exists |
| `WORKSPACE_NOT_INITIALIZED` | 5 | no | `.podway/runtime` state is absent |
| `WORKSPACE_ALREADY_INITIALIZED` | 1 | no | Initialization requested where compatible state already exists in a non-idempotent mode |
| `WORKSPACE_INIT_CONFLICT` | 5 | no | Existing `.podway` content conflicts with safe initialization |
| `WORKSPACE_ID_CONFLICT` | 5 | no | Workspace UUID-to-root metadata is non-unique or conflicts with live identity evidence |
| `WORKSPACE_UUID_MISMATCH` | 4 | no | Authoritative workspace UUID differs from the expected UUID |
| `WORKSPACE_MODE_MISMATCH` | 4 | no | Runtime ownership modes disagree at the endpoint, daemon, config, Store, or registry boundary |
| `WORKSPACE_MODE_SWITCH_CONFLICT` | 4 | no | The plan is stale or an incomplete switch requires its exact recovery token; plan again only when no partial marker is pending |
| `WORKSPACE_CONFIG_INVALID` | 5 | no | Workspace config fails schema or semantic validation |
| `WORKSPACE_STATE_UNREADABLE` | 5 | no | SQLite state is corrupt or inaccessible |
| `WORKSPACE_SCHEMA_UNSUPPORTED` | 5 | no | Database schema is newer or otherwise unsupported |
| `WORKSPACE_QUEUE_FULL` | 4 | yes | Pending-job limit reached |
| `WORKSPACE_MAINTENANCE` | 4 | yes | A destructive reset, replace, migration, or maintenance barrier blocks admission |
| `WORKSPACE_PATH_UNSAFE` | 5 | no | `.podway` or runtime path violates containment or symlink rules |
| `PATH_OUTSIDE_WORKTREE` | 5 | no | Procedure or artifact path escapes the worktree |
| `MIGRATION_FAILED` | 5 | no | Transactional schema migration failed |

## Procedure and preset errors

| Code | Exit | Retryable | Meaning |
|---|---:|---:|---|
| `PROCEDURE_NOT_FOUND` | 1 | no | Requested local procedure cannot be resolved |
| `PROCEDURE_INVALID` | 1 | no | Procedure fails schema or semantic validation |
| `PROCEDURE_SCHEMA_UNSUPPORTED` | 1 | no | Procedure schema identifier is unsupported |
| `PRESET_NOT_FOUND` | 1 | no | Built-in preset name does not exist |

## Session and graph errors

| Code | Exit | Retryable | Meaning |
|---|---:|---:|---|
| `SESSION_NOT_FOUND` | 1 | no | Workspace has no current session |
| `SESSION_ID_MISMATCH` | 4 | no | Authoritative session ID differs from the expected ID, including when no current session exists |
| `SESSION_ALREADY_EXISTS` | 1 | no | Start requested while a session exists |
| `SESSION_START_DECISION_REQUIRED` | 5 | no | Noninteractive start needs an explicit lifecycle-specific decision |
| `SESSION_START_STATE_CONFLICT` | 4 | yes | Existing-session state changed after a start policy was selected |
| `SESSION_NOT_RUNNING` | 1 | no | Command requires a running session, or `begin` requires a prepared session |
| `SESSION_NOT_TERMINAL` | 1 | no | Terminal disposition requires a completed or cancelled session |
| `SESSION_RESET_NOT_ELIGIBLE` | 1 | no | Eligible reset or replacement requires prepared state or a current terminal disposition |
| `SESSION_ARCHIVE_NOT_ELIGIBLE` | 1 | no | Current session is not terminal with a current disposition |
| `SESSION_ARCHIVE_LIMIT_REACHED` | 1 | no | Inactive-session archive already contains 32 sessions |
| `SESSION_ARCHIVE_NOT_FOUND` | 1 | no | Requested inactive session does not exist |
| `SESSION_CANCELLED` | 1 | no | Cancelled session cannot perform the operation |
| `SESSION_REVISION_CONFLICT` | 4 | yes | Observed session revision is stale |
| `ATTEMPT_NOT_CURRENT` | 4 | yes | Observed attempt is no longer active |
| `STAGE_NOT_SKIPPABLE` | 1 | no | Skip is not permitted |
| `REQUIRED_ITEMS_MISSING` | 1 | no | Active action node lacks required satisfied items |
| `BLOCKERS_PRESENT` | 1 | no | Open blockers prevent completion |

## Item and artifact errors

| Code | Exit | Retryable | Meaning |
|---|---:|---:|---|
| `ITEM_NOT_FOUND` | 1 | no | Item is absent from the active node definition |
| `ITEM_TYPE_MISMATCH` | 1 | no | Command is incompatible with item type |
| `ITEM_CONSTRAINT_FAILED` | 1 | no | Value violates item constraints |
| `ITEM_REVISION_CONFLICT` | 4 | yes | Same-item value changed since observation |
| `ITEM_ALREADY_SET` | 4 | yes | First-write precondition expected an unset item |
| `LIST_VALUE_NOT_FOUND` | 1 | no | Remove target is absent |
| `LIST_VALUE_DUPLICATE` | 1 | no | Unique list already contains value |
| `ARTIFACT_NOT_FOUND` | 1 | no | Local artifact path does not exist |
| `ARTIFACT_UNREADABLE` | 5 | no | Local artifact cannot be opened or hashed |
| `ARTIFACT_CHANGED` | 1 | yes | Required local artifact no longer matches stored metadata |
| `ARTIFACT_MEDIA_TYPE_NOT_ALLOWED` | 1 | no | Media type violates item allowlist |

## Blocker errors

| Code | Exit | Retryable | Meaning |
|---|---:|---:|---|
| `BLOCKER_NOT_FOUND` | 1 | no | Blocker ID does not exist |
| `BLOCKER_NOT_CURRENT` | 4 | yes | Blocker belongs to an old attempt |
| `BLOCKER_LIMIT_REACHED` | 1 | no | Active attempt already has 1,024 open blockers |

## Job and idempotency errors

| Code | Exit | Retryable | Meaning |
|---|---:|---:|---|
| `IDEMPOTENCY_KEY_REUSED` | 2 | no | Same key was bound to a different canonical request |
| `JOB_NOT_FOUND` | 1 | no | Job ID is absent or pruned |
| `JOB_NOT_CANCELLABLE` | 1 | no | Job is running or terminal |
| `JOB_WAIT_TIMEOUT` | 4 | yes | Wait expired; admitted job may continue |
| `MUTATION_OUTCOME_UNKNOWN` | 4 | yes | Response was lost after mutation transmission may have begun |

## Confirmation and internal errors

| Code | Exit | Retryable | Meaning |
|---|---:|---:|---|
| `CONFIRMATION_REQUIRED` | 2 | no | Destructive non-interactive command lacks `--yes` |
| `INTERNAL_ERROR` | 6 | no | Unexpected implementation failure |

## Registered v2 runtime errors

The additive runtime inventory covers Procedure v2 schema rejection; graph-node
and definition lookup and type mismatches; option and route rejection; decision
reason and evidence-reference failures; manual rework eligibility; goal opt-in,
definition, revision, reactivation, criterion, and final-assessment checks; digest
confirmation; and unsupported registered capability. Exact codes, exit classes,
and retryability are frozen by `error-codes.json`. In particular,
`EVIDENCE_REFERENCE_STALE` and `GOAL_REVISION_STALE` are retryable exit-4
conflicts, while `DIGEST_CONFIRMATION_REQUIRED` is a non-retryable exit-2 usage
failure and `UNSUPPORTED_V2_CAPABILITY` is a non-retryable exit-3 compatibility
failure.

Request decoding precedes these domain checks. A missing or blank reason in the
current public `session.decide` or `session.rework` request shape is malformed and
returns `REQUEST_INVALID` before durable admission. `DECISION_REASON_MISSING`
remains the stable fail-closed result at the decision-transition boundary. A
semantically vetted Procedure has a complete option-to-route map, and coherent
session reconstruction rejects a valid evidence consumer whose bound source is
stale. Consequently, `ROUTE_NOT_ALLOWED` and `EVIDENCE_REFERENCE_STALE` also
remain registered defensive domain results rather than expected outcomes from a
coherent current public request.

Registered v2 runtime codes use closed code-bound details inside the retained
`podway.error/v1` envelope. `EVIDENCE_REFERENCE_STALE` and `GOAL_REVISION_STALE` use
`podway.recoverable-v2-runtime-error-details/v1`; every other registered code uses
`podway.v2-runtime-error-details/v1`. `V2SCL-002` adds `EVIDENCE_PAGE_TOKEN_STALE`
to the recoverable family and the other three bounded-evidence codes to the
non-recoverable one, extending both closed `kind` unions. The required `kind` exactly equals the
outer error code; code-specific fields are bounded, unknown fields are rejected,
and optional `admission` retains the ordinary admission metadata contract. V2
runtime error messages remain bounded to 512 characters.

Authoring diagnostics never use runtime error codes. The authoring catalog
separately enumerates every validate, vet, graph-projection, and lint condition from Procedure v2,
including the mandatory stable codes
`EVIDENCE_SOURCE_DOES_NOT_DOMINATE_CONSUMER`, `SKIPPABLE_EVIDENCE_SOURCE`,
`EVIDENCE_SELECTOR_UNKNOWN_ITEM`, `READBACK_BUDGET_EXCEEDED`,
`NEXT_STATIC_BUDGET_EXCEEDED`, `GRAPH_PROJECTION_BUDGET_EXCEEDED`,
`REWORK_TARGET_NOT_DOMINATING`, and
`NO_REACTIVATION_PATH`.

`INTERNAL_ERROR` is never marked retryable. A client may make an out-of-band retry decision, but the daemon does not prove that an unexpected failure committed no mutation. When the daemon cannot reconstruct a mutation response after durable admission, `podway.internal-error-details/v1` uses the `ADMITTED_RESPONSE_RECONSTRUCTION_FAILED` discriminator, carries a diagnostic ID equal to the current response request ID, and retains the admitted job ID and workspace sequence. This classification avoids emitting an invalid `podway.workspace-recovery-details/v1` object, whose recovery contract permits only pre-admission failures.

## Typed condition failures

ADR-0028 adds `OPTION_GUARD_UNSATISFIED` as a non-retryable exit-1 v2 runtime
error. Its closed `podway.v2-runtime-error-details/v1` branch identifies the
selected option and carries bounded predicate statuses. The error is evaluated
only after identity, revision, and evidence-freshness checks, so a stale required
source continues to return retryable `EVIDENCE_REFERENCE_STALE` instead.

The authoring catalog replaces `LARGE_OPTION_SET` and `LARGE_CYCLE` with
`OPTION_LABELS_NOT_DISTINCT` and `OPTION_CRITERIA_WEAK`. Invalid predicate shape,
type, controller order, source selection, and bounds remain authoring failures
under stable field-specific diagnostics; runtime errors never substitute for
invalid Procedure admission.

`OPTION_LABELS_NOT_DISTINCT` compares every pair of option labels after lowercasing,
collapsing whitespace, trimming, and removing trailing `.`, `!`, and `?`. It reports
the later option for a duplicate fingerprint. `OPTION_CRITERIA_WEAK` reports missing
or non-concrete criteria and the later option in each pair of duplicate normalized
criteria. These pairwise rules emit at most eight findings each. The schema's hard
maximum of eight options is unchanged; option count alone is not a warning.

`REWORK_TOPOLOGY_CONFUSING` reports a decision that routes rework back to itself or
whose multiple rework routes cannot be distinguished by distinct normalized labels
and concrete, distinct normalized criteria. Multiple rework routes to distinct
phase-owner targets are otherwise valid. Rework-region size alone is not a warning;
advance cycles remain invalid through graph validation. Existing compatibility
diagnostics such as `INDISTINGUISHABLE_OPTION_LABELS`,
`IDENTICAL_EFFECTIVE_ROUTES`, and `WEAK_CRITERIA_GUIDANCE` retain their registered
meanings and may accompany the replacement diagnostics when both rules apply.

## Bounded evidence scale failures

[ADR-0024](../../architecture-decision-records/0024-bounded-evidence-scale-and-paged-read-back.md) adopts four codes. `V2SCL-002` registered them in `error-codes.json`, the closed details schemas, and the protocol catalog. `V2SCL-003` made `ATTEMPT_CONTENT_LIMIT_EXCEEDED` reachable on the item mutation path, and `V2SCL-004` made the three evidence-read codes reachable through the `evidence.read` route.

| Code | Exit | Retryable | Meaning |
|---|---:|---|---|
| `EVIDENCE_NOT_AVAILABLE` | 1 | no | Nothing is readable for the current attempt at the named source and item: the reference is undeclared, unresolved, or skipped; the item is outside its selector; or the item holds no recorded value. The closed details name the reference state. |
| `ATTEMPT_CONTENT_LIMIT_EXCEEDED` | 1 | no | Recorded text and list content for the attempt would exceed the per-attempt aggregate. |
| `EVIDENCE_PAGE_TOKEN_EXHAUSTED` | 2 | no | The token offset is at or past the logical end of the value. The daemon never issues such a token. |
| `EVIDENCE_PAGE_TOKEN_STALE` | 4 | yes | A previously valid token no longer matches the current consumer attempt or the recorded value it was issued against. A token naming another session or another item is not stale but unbound, and is refused as `REQUEST_INVALID`. |

`EVIDENCE_PAGE_TOKEN_STALE` uses the state-refresh recovery recipe, matching `EVIDENCE_REFERENCE_STALE`, and `EVIDENCE_PAGE_TOKEN_EXHAUSTED` is a usage failure because the daemon never issues a token past the logical end. Only the per-attempt aggregate maps to `ATTEMPT_CONTENT_LIMIT_EXCEEDED`; every other scale bound is a declaration-time rejection and keeps its own constraint failure. A genuinely stale reference keeps `EVIDENCE_REFERENCE_STALE`; `EVIDENCE_NOT_AVAILABLE` never substitutes for it. An availability failure is decided before any token comparison, so a token supplied against a reference that has since become unreadable answers `EVIDENCE_NOT_AVAILABLE` rather than a token code. A recorded value of zero scalars or zero entries is complete, not exhausted: it answers with one empty terminal page. Malformed, oversized, cross-session, and wrong-binding tokens are decoding failures and return `REQUEST_INVALID` before any state comparison. No evidence-read failure changes state.

Bound failures carry structured details containing `field`, `actual`, `maximum`, and `unit`, and name the constraint actually violated: entry count, per-entry length, total list content, or the per-attempt aggregate. A generic message such as `invalid constraints` is not sufficient when the exact exceeded limit is known. `REQUEST_TOO_LARGE` is the intentional exception, because frame decoding precedes JSON parsing and no field path exists.

## Prepared lifecycle failures

`SESSION_NOT_RUNNING` applies to every cursor, item, blocker, goal, completion,
cancellation, retry, skip, decision, and rework mutation attempted while the
session is prepared. For compatibility, it also reports `begin` against a
non-prepared session; callers should re-read status rather than interpret the
code name as a lifecycle observation. `SESSION_NOT_TERMINAL` applies when a disposition is
attempted before completion or cancellation.

`SESSION_RESET_NOT_ELIGIBLE` is a non-retryable domain result for default reset
and the compatible eligible-replacement wire shape. Its closed details identify the observed lifecycle,
always report `current_terminal_disposition: false`, and identify the required
next mode as `record_disposition` or `force`. They contain no Git, roadmap, process, external
reference, item value, or progress-summary data. A caller must reassess ownership
before choosing a new mutation; the error never recommends force deletion.
The details use `podway.session-reset-not-eligible-details/v1`.

`SESSION_START_DECISION_REQUIRED` is a non-retryable state conflict for a
noninteractive `start` that encounters prepared, running, or undisposed terminal
state without a sufficient `--on-existing` policy. Its closed details report the
observed lifecycle, whether the terminal disposition is current, the allowed
actions, and `admission.admitted: false`. The error never prompts and never
changes state.

`SESSION_START_STATE_CONFLICT` is a retryable fenced-state conflict. It applies
when an explicit start resolution was valid when selected but lifecycle or
terminal-disposition state no longer admits that resolution. Its closed details
report the current lifecycle, disposition state, allowed next actions, and
whether the failed request had already been durably admitted. Callers must
re-observe before deriving another start request.

Archive failures use three distinct non-retryable codes. `SESSION_ARCHIVE_NOT_ELIGIBLE`
reports the current lifecycle, disposition state, and the required lifecycle
action. `SESSION_ARCHIVE_LIMIT_REACHED` reports the fixed maximum of 32 and never
evicts retained history. `SESSION_ARCHIVE_NOT_FOUND` reports the exact inactive
session ID that was not found; after an uncertain purge response, callers
reconcile with a fresh `archive list` rather than assuming who performed deletion.

Missing or blank force progress summary, incompatible disposition fields, initial
goal fields supplied to start instead of begin, and unsupported mode combinations
are malformed requests and return `REQUEST_INVALID` before durable admission.

## Conflict remediation

On revision conflicts, details include current values:

```json
{
  "expected_session_revision": 12,
  "actual_session_revision": 13,
  "expected_attempt_id": "...",
  "actual_attempt_id": "..."
}
```

Clients should run the returned `observe --json --wait-for-idle` recipe, reassess
the active graph node, and derive a new command rather than blindly changing
only the revision.

## Identity-conflict details

`WORKSPACE_UUID_MISMATCH` uses the closed
`podway.workspace-uuid-mismatch-details/v2` object with
`expected_workspace_uuid`, `actual_workspace_uuid`, and `admission`.
`SESSION_ID_MISMATCH` uses the closed
`podway.session-id-mismatch-details/v2` object with `expected_session_id`,
nullable `actual_session_id`, and `admission`. A null actual session means the
expected session no longer exists as the workspace's current session.

Before durable admission, `admission` is exactly `{ "admitted": false }`. A
terminal conflict discovered after admission uses `{ "admitted": true,
"job_id": "<uuid>", "workspace_sequence": <positive integer> }`. The two
identity errors are non-retryable exit-4 conflicts: callers must observe fresh
identity before deciding whether a new operation is valid.

`WORKSPACE_MODE_MISMATCH` is a non-retryable exit-4 conflict. Its closed
`podway.workspace-mode-mismatch-details/v1` object contains only the bounded
`expected_mode`, `actual_mode`, and one of the `endpoint`, `daemon`, `config`,
`store`, or `registry` boundaries. It deliberately excludes raw paths.

`PROCEDURE_DIGEST_MISMATCH` is a non-retryable exit-4 conflict. Its closed
`podway.procedure-digest-mismatch-details/v2` object contains the expected and actual canonical
Procedure digests plus `{ "admitted": false }`; the comparison always precedes durable admission.

Every daemon mutation error now carries `details.admission`. Pre-admission
errors use exactly `{ "admitted": false }`; terminal errors and
`JOB_WAIT_TIMEOUT` use `{ "admitted": true, "job_id": "<uuid>",
"workspace_sequence": <positive integer> }`. The normative target is the
[automation error contract](automation-client-contract.md#22-error-and-exit-code-requirements-aut-err-001005).

`BLOCKER_LIMIT_REACHED` uses the closed
`podway.blocker-limit-details/v1` object. `maximum_open_blockers` is `64`.
Its `admission` follows the same pre-admission or terminal form above; terminal
details also carry the matching top-level `job_id` and `job_sequence` fields.

## Mutation outcome unknown details

When the CLI loses a mutation response after request transmission may have begun,
it emits `MUTATION_OUTCOME_UNKNOWN` with this closed details object:

```json
{
  "schema": "podway.mutation-outcome-unknown-details/v2",
  "outcome": "unknown",
  "idempotency_key": "original-key",
  "reconcile": {
    "command": "job.lookup",
    "idempotency_key": "original-key"
  },
  "recovery": {
    "action": "reconcile_mutation",
    "command": "job.lookup",
    "argv": ["podway", "--json", "job", "lookup", "--idempotency-key", "original-key"],
    "reason": "Reconcile the original idempotency key before considering another mutation.",
    "requires_explicit_authorization": false
  }
}
```

The object deliberately omits `admission`, job ID, and sequence because none is
known without a trustworthy response. `retryable=true` means the caller can
recover safely: run `job lookup --idempotency-key <original-key>` first and do
not submit a new-key mutation until reconciliation proves that is appropriate.

## Structured recovery recipes

The following errors add one required closed `recovery` object through a new
details-schema version while preserving the outer `podway.error/v1`, stable
code, retryability, exit class, and admission facts:

| Error family | Details schema | Read-only command |
|---|---|---|
| `DAEMON_UNAVAILABLE` | `podway.endpoint-error-details/v2` | `daemon.status` |
| `DAEMON_STARTING` | `podway.daemon-readiness-error-details/v1` | `daemon.wait-ready` |
| `DAEMON_READINESS_TIMEOUT` | `podway.daemon-readiness-timeout-details/v1` | `daemon.wait-ready` |
| `DAEMON_CONTRACT_MISMATCH` | `podway.daemon-contract-mismatch-details/v2` | `daemon.status` |
| `WORKSPACE_UUID_MISMATCH` | `podway.workspace-uuid-mismatch-details/v2` | `workspace.doctor` |
| `WORKSPACE_STATE_UNREADABLE`, `WORKSPACE_SCHEMA_UNSUPPORTED` | `podway.workspace-recovery-details/v1` | `workspace.doctor` |
| `PROCEDURE_DIGEST_MISMATCH` | `podway.procedure-digest-mismatch-details/v2` | `session.observe` |
| `SESSION_ID_MISMATCH` | `podway.session-id-mismatch-details/v2` | `session.observe` |
| `SESSION_REVISION_CONFLICT`, `ITEM_REVISION_CONFLICT` | `podway.revision-conflict-details/v2` | `session.observe` |
| `ATTEMPT_NOT_CURRENT` | `podway.attempt-conflict-details/v2` | `session.observe` |
| `EVIDENCE_REFERENCE_STALE`, `GOAL_REVISION_STALE`, `EVIDENCE_PAGE_TOKEN_STALE` | `podway.recoverable-v2-runtime-error-details/v1` | `session.observe` |
| `MUTATION_OUTCOME_UNKNOWN` | `podway.mutation-outcome-unknown-details/v2` | `job.lookup` |
| `JOB_WAIT_TIMEOUT` | `podway.job-wait-timeout-details/v2` | `job.wait` when the job ID is known, otherwise `session.observe` |

`podway.recovery-recipe/v1` contains exactly `action`, `command`, `argv`,
`reason`, and `requires_explicit_authorization`. Commands are limited to
`session.observe`, `job.lookup`, `job.wait`, `daemon.status`, `daemon.wait-ready`, and
`workspace.doctor`; argv is limited to 2..8 non-empty strings and reason to 256
Unicode scalars. Every current recipe is read-only and therefore reports
`requires_explicit_authorization=false`. The object never authorizes retry,
restart, repair, reset, reinstall, fence weakening, or another mutation.

`DAEMON_STARTING` is a pre-admission response with the current state, stage,
elapsed time, bounded worktree progress, and `admission.admitted=false`.
`DAEMON_READINESS_TIMEOUT` additionally records the lifecycle operation,
deadline, installed, loaded, socket-present, reachable, and contract-verified
observations plus `same_command_retry_safe=true`. Timeout preserves the existing
LaunchAgent, endpoint ownership, registry, workspace databases, and sessions.

## Error redaction

Error details may include local paths needed for remediation. They MUST NOT include:

- item text values unless the error directly validates that submitted value;
- full canonical requests;
- environment variables;
- file contents;
- secrets or access tokens;
- artifact bytes.
