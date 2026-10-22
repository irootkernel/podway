# Errors and Exit Codes

## Contract

Current public failures use `podway.error/v2`, stable uppercase codes, and the
canonical [runtime catalog](../../../assets/specifications/error-codes.json).
Automation branches on `code`, `retryable`, exit class, and structured `details`,
never on human text. The closed outer envelope binds each code's exit/retryability
and each named detail family to its exact schema or discriminator. Some retained
simple errors use the generic JSON-object detail boundary expressly permitted by
the envelope; do not invent a named schema for them.

Authoring findings use the separate, disjoint
[authoring catalog](../../../assets/specifications/authoring-diagnostics.json).
A code belongs to one catalog. Registration does not prove every defensive code
is reachable from a coherent public request. The command catalog owns executable
routes, not historical reserved-capability lists.

The Engine executes synchronously in the caller. Failures contain no admission,
job, sequence, endpoint, queue, or runtime-mode metadata. Rejected mutations retain
no outcome. Only uncertain COMMIT I/O emits `MUTATION_OUTCOME_UNKNOWN`; response
loss or external process termination is caller-side uncertainty.

## Exit codes

| Exit code | Class | Meaning |
|---:|---|---|
| 0 | success | Read, local command, or committed mutation succeeded |
| 1 | domain | Valid request rejected by Procedure or session rules |
| 2 | usage | Invalid syntax, shape, bounds, retired command, or confirmation |
| 4 | conflict | Optimistic conflict, lock deadline, or uncertain COMMIT I/O |
| 5 | workspace | Git, path, configuration, database, generation, or writer identity failure |
| 6 | internal | Unexpected implementation failure |

Exit class 3 and daemon/IPC/socket/job/runtime-reset/workspace-mode error families
are retired. Retired syntax returns `COMMAND_RETIRED` before workspace resolution,
stdin consumption, or Store access; ordinary unknown syntax is a usage failure.
The [retired table](../../../assets/specifications/retired-commands-v1.json) is
canonical. `job.lookup` names `outcome.lookup`; a null replacement means none.

## Runtime code inventory

The following are the current catalog's codes, exits, and retryability. Detail
schemas and the behavioral distinctions below take precedence over a short
human explanation. Historical defensive entries do not establish new guarantees.

| Code | Exit | Retryable | Meaning |
|---|---:|---|---|
| `ARTIFACT_CHANGED` | 1 | yes | The local artifact no longer matches stored metadata. |
| `ARTIFACT_MEDIA_TYPE_NOT_ALLOWED` | 1 | no | The artifact media type is not allowed. |
| `ARTIFACT_NOT_FOUND` | 1 | no | The local artifact does not exist. |
| `ARTIFACT_UNREADABLE` | 5 | no | The local artifact cannot be opened or hashed. |
| `ATTEMPT_CONTENT_LIMIT_EXCEEDED` | 1 | no | Recorded text and list content for the attempt would exceed the per-attempt aggregate. |
| `ATTEMPT_NOT_CURRENT` | 4 | yes | The target attempt is no longer active. |
| `ATTEMPT_NOT_FOUND` | 1 | no | The requested attempt was not found in the session. |
| `BARE_GIT_REPOSITORY` | 5 | no | Bare repositories are unsupported. |
| `BLOCKERS_PRESENT` | 1 | no | Open blockers prevent completion. |
| `BLOCKER_LIMIT_REACHED` | 1 | no | The active attempt already has the maximum number of open blockers. |
| `BLOCKER_NOT_CURRENT` | 4 | yes | The blocker belongs to an old attempt. |
| `BLOCKER_NOT_FOUND` | 1 | no | The blocker does not exist. |
| `COMMAND_RETIRED` | 2 | no | The command or flag belongs to the retired daemon-era surface. |
| `CONFIRMATION_REQUIRED` | 2 | no | The command requires explicit confirmation. |
| `CRITERION_CITATION_INVALID` | 1 | no | A criterion citation does not name resolved attempt state. |
| `CRITERION_MODE_MIXED` | 1 | no | Criterion applicability and assessment modes cannot be mixed in one attempt. |
| `CRITERION_NOT_FOUND` | 1 | no | The requested goal criterion does not exist. |
| `CRITERION_RESULT_MISSING` | 1 | no | A required criterion result is missing. |
| `DECISION_REASON_MISSING` | 1 | no | The decision requires a reason. |
| `DELEGATION_ALREADY_ACCEPTED` | 1 | no | This attempt already has an immutable delegation receipt. |
| `DELEGATION_BINDING_MISMATCH` | 1 | no | The supplied execution identity does not match the declaration or package. |
| `DELEGATION_NOT_APPLICABLE` | 1 | no | The selected attempt is not a delegated action. |
| `DELEGATION_RECEIPT_REQUIRED` | 1 | no | The delegated attempt has no accepted receipt. |
| `DELEGATION_RECEIPT_STALE` | 1 | no | The accepted receipt no longer matches the current delegated attempt. |
| `DIGEST_CONFIRMATION_REQUIRED` | 2 | no | A custom Procedure start requires explicit digest confirmation. |
| `EVIDENCE_NOT_AVAILABLE` | 1 | no | The declared evidence reference is unresolved or skipped, so the item has no readable page. |
| `EVIDENCE_PAGE_TOKEN_EXHAUSTED` | 2 | no | The evidence page token offset is at or past the logical end of the value. |
| `EVIDENCE_PAGE_TOKEN_STALE` | 4 | yes | The evidence page token no longer matches the current consumer, source value, or session identity. |
| `EVIDENCE_REFERENCE_STALE` | 4 | yes | A resolved evidence reference is stale. |
| `EVIDENCE_REFERENCE_UNRESOLVED` | 1 | no | A required evidence reference is unresolved. |
| `FRESH_GOAL_ASSESSMENT_MISSING` | 1 | no | A fresh final goal assessment is required. |
| `GOAL_ASSESSMENT_OUTCOME_NOT_ALLOWED` | 1 | no | The selected option does not match the determined goal outcome. |
| `GOAL_REVISION_STALE` | 4 | yes | The expected goal revision is no longer current. |
| `GOAL_REVISION_TARGET_NOT_ALLOWED` | 1 | no | The goal revision rework target is not allowed by policy. |
| `GOAL_REVISION_TARGET_NOT_REVISION_SAFE` | 1 | no | The goal revision rework target is not revision-safe. |
| `GOAL_TRACKING_NOT_ENABLED` | 1 | no | The Procedure does not enable session-goal tracking. |
| `GRAPH_NODE_NOT_FOUND` | 1 | no | The requested graph node does not exist. |
| `GRAPH_NODE_TYPE_MISMATCH` | 1 | no | The active graph node has the wrong type for the command. |
| `IDEMPOTENCY_KEY_REUSED` | 2 | no | The idempotency key is bound to another request. |
| `INTERNAL_ERROR` | 6 | no | An unexpected internal error occurred. |
| `ITEM_ALREADY_SET` | 4 | yes | The item was expected to be unset. |
| `ITEM_CONSTRAINT_FAILED` | 1 | no | The item value violates its constraints. |
| `ITEM_NOT_FOUND` | 1 | no | The item does not exist in the active node definition. |
| `ITEM_REVISION_CONFLICT` | 4 | yes | The item changed after it was observed. |
| `ITEM_TYPE_MISMATCH` | 1 | no | The command is incompatible with the item type. |
| `LEGACY_RUNTIME_STATE_UNSUPPORTED` | 5 | no | Daemon-era state requires explicitly confirmed reset-all. |
| `LIST_VALUE_DUPLICATE` | 1 | no | The unique list already contains the value. |
| `LIST_VALUE_NOT_FOUND` | 1 | no | The list value does not exist. |
| `MANUAL_REWORK_TARGET_NOT_ALLOWED` | 1 | no | The graph node is not an allowed manual rework target. |
| `MANUAL_REWORK_TARGET_NOT_ON_TRACE` | 1 | no | The manual rework target has no valid attempt on the current trace. |
| `MIGRATION_FAILED` | 5 | no | Workspace migration failed. |
| `MUTATION_OUTCOME_UNKNOWN` | 4 | yes | COMMIT I/O leaves the commit outcome uncertain; reconcile the original key. |
| `NODE_DEFINITION_NOT_FOUND` | 1 | no | The requested node definition does not exist. |
| `NOT_A_GIT_WORKTREE` | 5 | no | No valid Git worktree contains the path. |
| `OPTION_GUARD_UNSATISFIED` | 1 | no | The selected decision option has an unmet or unevaluable guard. |
| `OPTION_NOT_ALLOWED` | 1 | no | The selected decision option is not allowed. |
| `PATH_OUTSIDE_WORKTREE` | 5 | no | A requested path escapes the worktree. |
| `PRESET_NOT_FOUND` | 1 | no | Built-in preset does not exist. |
| `PROCEDURE_DIGEST_MISMATCH` | 4 | no | The canonical Procedure digest differs from the expected digest. |
| `PROCEDURE_INVALID` | 1 | no | Procedure is invalid. |
| `PROCEDURE_NOT_FOUND` | 1 | no | Procedure cannot be resolved. |
| `PROCEDURE_SCHEMA_UNSUPPORTED` | 1 | no | Procedure schema is unsupported. |
| `PROCEDURE_V2_SCHEMA_INVALID` | 1 | no | The Procedure v2 document violates its closed schema. |
| `REACTIVATION_FLAG_REQUIRED` | 1 | no | Reactivating a completed session requires the explicit flag. |
| `REQUEST_INVALID` | 2 | no | Request is malformed or violates schema. |
| `REQUEST_TOO_LARGE` | 2 | no | A bounded stdin document or complete typed request exceeds its named input ceiling. |
| `REQUIRED_ITEMS_MISSING` | 1 | no | Required items are missing. |
| `ROUTE_NOT_ALLOWED` | 1 | no | The selected transition route is not allowed. |
| `SESSION_ALREADY_EXISTS` | 1 | no | A task session already exists. |
| `SESSION_ARCHIVE_LIMIT_REACHED` | 1 | no | Historical capacity result; current archival has no inactive-session count cap and does not emit it. |
| `SESSION_ARCHIVE_NOT_ELIGIBLE` | 1 | no | The current session is not eligible for archival. |
| `SESSION_ARCHIVE_NOT_FOUND` | 1 | no | The requested inactive session was not found. |
| `SESSION_CANCELLED` | 1 | no | The session was cancelled. |
| `SESSION_GOAL_ALREADY_DEFINED` | 1 | no | The session goal is already defined. |
| `SESSION_GOAL_MISSING` | 1 | no | The session goal has not been defined. |
| `SESSION_ID_MISMATCH` | 4 | no | The session ID differs from the expected identity. |
| `SESSION_NOT_FOUND` | 1 | no | No current task session exists. |
| `SESSION_NOT_RUNNING` | 1 | no | The command requires a running session, or begin requires a prepared session. |
| `SESSION_NOT_TERMINAL` | 1 | no | The command requires a completed or cancelled session. |
| `SESSION_RESET_NOT_ELIGIBLE` | 1 | no | The current session is not eligible for default reset or replacement. |
| `SESSION_REVISION_CONFLICT` | 4 | yes | The session changed after it was observed. |
| `SESSION_START_DECISION_REQUIRED` | 5 | no | Starting a successor session requires an explicit decision about the current session. |
| `SESSION_START_STATE_CONFLICT` | 4 | yes | The existing session state changed after the start policy was selected. |
| `STAGE_NOT_SKIPPABLE` | 1 | no | The active stage cannot be skipped. |
| `WORKSPACE_ALREADY_INITIALIZED` | 1 | no | Workspace is already initialized. |
| `WORKSPACE_BUSY` | 4 | yes | A direct connection or SQLite admission wait expired without a state effect. |
| `WORKSPACE_CONFIG_INVALID` | 5 | no | Workspace configuration is invalid. |
| `WORKSPACE_CONTRACT_MISMATCH` | 5 | no | The Store writer contract is not supported by this binary. |
| `WORKSPACE_ID_CONFLICT` | 5 | no | Persisted workspace binding conflicts with the resolved canonical Git/worktree identity. |
| `WORKSPACE_INIT_CONFLICT` | 5 | no | Existing Podway files conflict with safe initialization. |
| `WORKSPACE_MAINTENANCE` | 4 | yes | Process-independent maintenance exclusion or a published recovery marker blocks access. |
| `WORKSPACE_NOT_INITIALIZED` | 5 | no | Podway workspace state is absent. |
| `WORKSPACE_PATH_UNSAFE` | 5 | no | Podway workspace path is unsafe. |
| `WORKSPACE_SCHEMA_UNSUPPORTED` | 5 | no | The Store generation is newer than this Podway supports. |
| `WORKSPACE_STATE_UNREADABLE` | 5 | no | Workspace state is corrupt or inaccessible. |
| `WORKSPACE_UUID_MISMATCH` | 4 | no | The workspace UUID differs from the expected identity. |
| `WORKTREE_GONE` | 5 | no | The bound Git worktree no longer exists. |

## Current detail families

All identifiers below carry the `podway.` prefix. Their canonical schemas under
[`assets/schemas/`](../../../assets/schemas/) own closure and code-specific fields.
Named details contain no admission or job facts.

| Code or family | Current detail schema | Gen | Important facts |
|---|---|---|---|
| `WORKSPACE_BUSY` | `workspace-busy-details/v1` | 1, 2 | `boundary`: `connection_gate`, `sqlite_read_admission`, or `sqlite_write_lock`; `timeout_ms`, refresh |
| `WORKSPACE_MAINTENANCE` | `workspace-maintenance-details/v1` | 1, 2 | Boundary, timeout, boundary-specific recovery |
| `LEGACY_RUNTIME_STATE_UNSUPPORTED` | `legacy-runtime-state-unsupported-details/v1` | 1, 2 | Store generation, marker names, authorized reset |
| `WORKSPACE_CONTRACT_MISMATCH` | `workspace-contract-mismatch-details/v1` | 1 | Expected/actual writer manifest digest, diagnosis |
| `WORKSPACE_CONTRACT_MISMATCH` (generation 2) | `workspace-contract-mismatch-details/v2` | 2 (reserved) | Adds `actual_product_version` from the released-manifest table (null for an unknown digest); the recovery reason names the original writer |
| `COMMAND_RETIRED` | `command-retired-details/v1` | 1, 2 | `retired`, nullable `replacement`, bounded `guidance` |
| `WORKSPACE_UUID_MISMATCH` | `workspace-uuid-mismatch-details/v3` | 1, 2 | Expected/actual UUID, diagnosis |
| `SESSION_ID_MISMATCH` | `session-id-mismatch-details/v3` | 1, 2 | Expected ID, nullable actual ID, refresh |
| `PROCEDURE_DIGEST_MISMATCH` | `procedure-digest-mismatch-details/v3` | 1, 2 | Expected/actual canonical digest, refresh |
| `SESSION_REVISION_CONFLICT`, `ITEM_REVISION_CONFLICT` | `revision-conflict-details/v3` | 1, 2 | `expected_revision` and/or `current_revision`, refresh |
| `ATTEMPT_NOT_CURRENT` | `attempt-conflict-details/v3` | 1, 2 | Expected/actual attempt, refresh |
| `EVIDENCE_REFERENCE_STALE`, `GOAL_REVISION_STALE`, `EVIDENCE_PAGE_TOKEN_STALE` | `recoverable-v2-runtime-error-details/v2` | 1, 2 | Code-bound `kind`, bounded facts, refresh |
| `CRITERION_CITATION_INVALID` | `v2-runtime-error-details/v2` | 1 | Code-bound `kind`, rejected criterion and citation |
| `CRITERION_CITATION_INVALID` (generation 2) | `criterion-citation-invalid-details/v1` | 2 (reserved) | Adds the citable evidence source nodes (at most eight) and item identifiers (at most 128) computed with the citation admission rule |
| Other typed graph/goal/guard/Procedure-v2/evidence errors | `v2-runtime-error-details/v2` | 1, 2 | `kind` equals outer code; closed code-specific fields |
| `BLOCKER_LIMIT_REACHED` | `blocker-limit-details/v2` | 1, 2 | `maximum_open_blockers: 64` |
| `IDEMPOTENCY_KEY_REUSED` | `idempotency-key-reused-details/v2` | 1, 2 | Closed schema marker |
| `MUTATION_OUTCOME_UNKNOWN` | `mutation-outcome-unknown-details/v3` | 1, 2 | Original key and matching lookup/recovery key |
| `WORKSPACE_STATE_UNREADABLE` | `workspace-recovery-details/v2` | 1, 2 | Diagnosis |
| `WORKSPACE_SCHEMA_UNSUPPORTED` | `workspace-schema-unsupported-details/v1` | 1, 2 | Actual/supported Store generations and compatible-Podway guidance; no recovery recipe |
| `SESSION_RESET_NOT_ELIGIBLE` | `session-reset-not-eligible-details/v2` | 1, 2 | Lifecycle, disposition, required action |
| `SESSION_START_DECISION_REQUIRED` | `session-start-decision-required-details/v2` | 1, 2 | Lifecycle, disposition, allowed actions |
| `SESSION_START_STATE_CONFLICT` | `session-start-state-conflict-details/v2` | 1, 2 | Current lifecycle/disposition, allowed actions |
| `SESSION_ARCHIVE_NOT_ELIGIBLE` | `session-archive-not-eligible-details/v2` | 1, 2 | Lifecycle/disposition, required action |
| `SESSION_ARCHIVE_LIMIT_REACHED` | `session-archive-limit-details/v2` | 1, 2 | Historical maximum; not emitted by current archival |
| `SESSION_ARCHIVE_NOT_FOUND` | `session-archive-not-found-details/v2` | 1, 2 | Exact inactive session ID |
| Five `DELEGATION_*` codes | `delegation-error-details/v2` | 1, 2 | Closed code/reason pairing |
| `INTERNAL_ERROR` | `internal-error-details/v2` | 1, 2 | Diagnostic UUID and optional bounded message |

The Gen column records which contract generations select the family
([ADR-0039](../../architecture-decision-records/0039-opt-in-contract-generations.md);
the declaration is
[`contract-generations.json`](../../../assets/specifications/contract-generations.json)).
Families marked `1, 2` are shared by both generations. The two `2 (reserved)`
families are validatable but emitted by no runtime path until generation 2 is
selected and supported; the generation-1 families of `WORKSPACE_CONTRACT_MISMATCH`
and `CRITERION_CITATION_INVALID` remain the emitted and catalog-named
(`details_schema`) ones until then.

`ITEM_CONSTRAINT_FAILED` may use a generic object; if its details include a parsed
`field`, the envelope selects the closed named-bound branch. Typed v2 runtime
messages are bounded to 512 Unicode scalars; the outer message bound is separate.

## Direct workspace and contention failures

`WORKSPACE_BUSY` reports the invocation's bounded `timeout_ms` and the actual
expired wait: `connection_gate` for process-local connection exclusion,
`sqlite_read_admission` for SQLite read snapshot admission/recovery, or
`sqlite_write_lock` for a SQLite writer. Reads never report `sqlite_write_lock`.
`WORKSPACE_MAINTENANCE` distinguishes `maintenance_lock`,
`intent_marker`, `reset_marker`, `removal_marker`, and `predecessor_migration`.
One absolute invocation deadline bounds all of these waits. Each boundary makes
one non-blocking attempt even with an exhausted budget; acquired locks allow the
operation to finish past the deadline. Preflight, preparation and confirmation
consume the original wait budget without creating a new one;
it is not a hard timer for stdin, human confirmation, preparation, filesystem
cleanup, or rendering. Neither contention error retains a rejected outcome.
Reads never resume published maintenance. Markerless maintenance contention
without a Store, including init and reset during final removal, emits read-only
`version --identity`; wait for the owner and retry the original request with its
key and fences. State refresh applies only when a Store is present.
If Git metadata changes while constructing maintenance recovery guidance, the
original maintenance refusal remains with read-only `version --identity` guidance.
Its boundary names the observed marker, or `maintenance_lock` when no marker was
observed; the changed diagnostic snapshot grants no recovery authority.

Normal reads, writes, and replay refuse daemon-era Store generations 1–13 and
markers without conversion, deletion, or SHM creation. The unreleased older direct
generation 14 also reports `LEGACY_RUNTIME_STATE_UNSUPPORTED` with an explicitly
confirmed reset recipe; it is not an admitted predecessor. Unsupported newer state
uses `WORKSPACE_SCHEMA_UNSUPPORTED`; corruption uses `WORKSPACE_STATE_UNREADABLE`.
An unsupported writer digest uses `WORKSPACE_CONTRACT_MISMATCH` before key replay.
Unsupported legacy workspace configuration fails validation before Store access.
Confirmed reset-all is destructive recovery; a recipe does not grant permission.

## Identity and conflict recovery

Workspace UUID and writer identity are classified before idempotency replay.
A matching numeric revision cannot prove session/workspace identity. UUID,
session, and Procedure-digest mismatches are non-retryable exit-4 conflicts;
revision/attempt conflicts are retryable exit-4 conflicts. Retryability permits
safe reassessment, not blind resubmission with new fences.

Revision details carry `expected_revision` and/or `current_revision` plus refresh.
A null `actual_session_id` means the expected session is no longer current.
Use the returned `observe` or `doctor` recipe, reassess ownership and graph state,
and derive a separately justified new request. Uncertain-response exact replay
retains original resolved fences and key; substituting a fresh revision changes
the request and cannot be called replay.

## Mutation outcome unknown details

Only uncertain COMMIT I/O emits this closed shape:

```json
{
  "schema": "podway.mutation-outcome-unknown-details/v3",
  "outcome": "unknown",
  "idempotency_key": "original-key",
  "reconcile": {
    "command": "outcome.lookup",
    "idempotency_key": "original-key"
  },
  "recovery": {
    "action": "reconcile_mutation",
    "command": "outcome.lookup",
    "argv": ["podway", "--json", "outcome", "lookup", "--idempotency-key", "original-key"],
    "reason": "Look up the exact successful outcome before any retry.",
    "requires_explicit_authorization": false
  }
}
```

The validator checks agreement of the key in details, reconciliation, and recovery
argv. Lost stdout/SIGKILL can leave a caller uncertain without an error response;
the same original-key lookup and exact-request replay discipline applies.
A miss proves no retained success only within retention. Failed mutations are
absent by design. Session outcomes survive archival/purge until session or runtime
reset; workspace outcomes retain at most 100 entries or 30 days, pruned on writes.
Complete workspace removal destroys lookup retention; pending removal uses its
exact marker-backed retry, and already-absent success proves convergence rather
than original-response replay.

## Structured recovery recipes

`podway.recovery-recipe/v2` contains exactly `action`, `command`, `argv`, `reason`,
and `requires_explicit_authorization`; the recipe has no `schema` field.
Argv has 2..10 nonempty strings of at most 256 Unicode scalars each; reason has
at most 256. Recipes omit `--worktree`: retain the original selected worktree in
the client's controlled invocation context.

| Owning condition | Allowed recipe | Authorization flag |
|---|---|---|
| Session/revision/attempt/Procedure digest, stale goal/evidence, SQLite contention | `refresh_state` / `session.observe` | false |
| Workspace UUID mismatch | `diagnose_workspace` / `workspace.doctor` | false |
| Invalid Store or writer contract | `diagnose_writer` / `version --identity` with conditional explicit discard guidance | false |
| Maintenance lock with a Store | `refresh_state` / `session.observe` after the owner finishes | false |
| Store-less final-removal or residual lock | `diagnose_writer` / `version --identity`; wait for the owner and retry the original request | false |
| Removal parent repopulated before cleanup | `diagnose_writer` / `version --identity`; preserve current contents and inspect the workspace | false |
| Init intent | `resume_maintenance` / `workspace.init` with original key and repair policy | true |
| Foreign writer/root or predecessor intent | `diagnose_writer` / `version --identity`; recovery belongs to the original writer and worktree | false |
| Uncertain COMMIT I/O | `reconcile_mutation` / `outcome.lookup` with original key | false |
| Unsupported legacy state | `reset_legacy_state` / `workspace.reset_all` with `<new-idempotency-key>` | true |
| Published reset marker | `resume_maintenance` / `workspace.reset_all` with original key | true |
| Reset/removal intent-only window | Corresponding `resume_maintenance` recipe with original parameters | true |
| Published removal marker | `resume_maintenance` / `workspace.remove` with original key, fence and force policy | true |

Read-only argv uses `podway --json observe`, `podway --json doctor`, or
`podway --json version --identity`; lookup uses
`podway --json outcome lookup --idempotency-key <original-key>`. The
`version --identity` recipe is local: it opens no workspace and takes no
`--worktree` or other worktree request option. Legacy reset uses
`reset --all --yes --idempotency-key <new-idempotency-key>`; replace the placeholder
with a fresh caller-owned key only after destructive authorization. Published
reset recovery retains its original key. Removal retains its original force and
UUID assertions. The JSON recipe includes `--force --yes` and the original UUID.
A typed request without JSON authorization uses a human-terminal recipe; an
unfenced recipe omits `--json` and `--yes` and requires typing the exact canonical
worktree path. It keeps the original key and does not add force or a fence.
Human errors show the recipe reason, including terminal confirmation and owning
writer requirements. `version --identity` diagnoses a foreign marker without
opening its Store; it does not resume it. Reading a recipe never authorizes
execution or resumes a marker. No recipe weakens fences, changes a replay key,
alters a historical service, or executes external work.

## Prepared lifecycle failures

`SESSION_NOT_RUNNING` rejects cursor/item/blocker/goal/completion/cancellation/
retry/skip/decision/rework in prepared state. It also rejects `begin` against
non-prepared state: re-read lifecycle rather than infer it from the code name.
Disposition before completion/cancellation returns `SESSION_NOT_TERMINAL`.

Default reset/replacement eligibility uses `SESSION_RESET_NOT_ELIGIBLE` with
lifecycle, current disposition, and required action (`record_disposition` or
`force`). These are state facts, not force authority. Noninteractive start without
sufficient policy returns exit-5 `SESSION_START_DECISION_REQUIRED` without prompt
or mutation. If an explicitly selected policy becomes invalid, retryable exit-4
`SESSION_START_STATE_CONFLICT` requires fresh observation.

Archival never automatically evicts history; current builds do not emit
`SESSION_ARCHIVE_LIMIT_REACHED`. `SESSION_ARCHIVE_NOT_FOUND` names the missing
inactive ID. After uncertain purge stdout, original-key lookup identifies the
success; archive list confirms absence but not which caller performed deletion.
Missing/blank force progress summary, incompatible disposition fields, start
with initial goal fields instead of begin, duplicated stdin identities, and
unsupported options are malformed input, not admitted work.

## Typed conditions and authoring diagnostics

`OPTION_GUARD_UNSATISFIED` is a non-retryable exit-1 typed runtime error. Its
closed branch names the option and bounded predicate statuses. Identity/revision
checks precede evidence freshness, which precedes guards; stale evidence retains
retryable `EVIDENCE_REFERENCE_STALE`. Missing/blank public decision or rework
reason is `REQUEST_INVALID` at decoding. `DECISION_REASON_MISSING` remains a
transition-boundary defensive result.

Authoring diagnostics never substitute runtime codes for invalid Procedure
structure. Their inventory includes `EVIDENCE_SOURCE_DOES_NOT_DOMINATE_CONSUMER`,
`SKIPPABLE_EVIDENCE_SOURCE`, `EVIDENCE_SELECTOR_UNKNOWN_ITEM`,
`READBACK_BUDGET_EXCEEDED`, `NEXT_STATIC_BUDGET_EXCEEDED`,
`OBSERVATION_STATIC_MEMBERS_BUDGET_EXCEEDED`, `SOURCE_PROJECTION_BUDGET_EXCEEDED`,
`GRAPH_PROJECTION_BUDGET_EXCEEDED`, `REWORK_TARGET_NOT_DOMINATING`, and
`NO_REACTIVATION_PATH`. Canonical-source diagnostics distinguish Unicode and
encoded-byte overflow. Human lists report truncation and shown/total counts;
machine output retains `diagnostics_truncated` and `diagnostics_total`.

`OPTION_LABELS_NOT_DISTINCT` compares labels after lowercasing, whitespace
collapse/trim, and removal of trailing `.`, `!`, and `?`, reporting the later
duplicate. `OPTION_CRITERIA_WEAK` reports missing/non-concrete criteria and later
duplicate normalized criteria. Each pairwise rule emits at most eight findings.
The hard maximum of eight options is unchanged; option count is not a warning.
`REWORK_TOPOLOGY_CONFUSING` identifies self-rework or multiple rework routes that
lack distinct normalized labels and concrete distinct criteria. Distinct routes
to phase-owner targets are valid; region size is not a warning, while advance
cycles remain invalid. Registered compatibility findings
`INDISTINGUISHABLE_OPTION_LABELS`, `IDENTICAL_EFFECTIVE_ROUTES`, and
`WEAK_CRITERIA_GUIDANCE` keep their meanings when their rules also apply.

## Bounded input and evidence failures

Both `record --stdin` and typed `RecordMany` share the 1,048,576-byte complete-input
ceiling, including fences/key/operations. Delegation acceptance has a separate
complete 32 KiB stdin ceiling. Item limits remain independent: 65,536 text scalars;
1,000 list entries, 8,192 per entry, 1,000,000 total list scalars; 16,777,216
text/list scalars per attempt; 128 items. Only the attempt aggregate maps to
`ATTEMPT_CONTENT_LIMIT_EXCEEDED`. Parsed bound failures name `field`, `actual`,
`maximum`, and `unit`. `REQUEST_TOO_LARGE` is the exception for bounded acceptance
or encoding before a parsed field path is available.

Malformed/oversized/cross-session/wrong-binding tokens fail `REQUEST_INVALID`
before state comparison. Undeclared/unresolved/skipped references, unselected
items and absent values use `EVIDENCE_NOT_AVAILABLE`. Availability precedes token
comparison when a reference becomes unreadable. Changed bound state uses retryable
`EVIDENCE_PAGE_TOKEN_STALE`; stale references keep `EVIDENCE_REFERENCE_STALE`.
An offset at/past logical end uses `EVIDENCE_PAGE_TOKEN_EXHAUSTED`; the Engine never
issues such a continuation token. A zero-length recorded value is complete and
returns one empty terminal page. No evidence-read error writes state.

## Delegation failure behavior

All five delegation codes are non-retryable exit 1 for unchanged input. Details
v2 pair NOT_APPLICABLE with `direct_attempt`, RECEIPT_REQUIRED with `receipt_missing`,
RECEIPT_STALE with `package_changed` or `items_changed`, BINDING_MISMATCH with
`binding_mismatch`, `role_mismatch`, `disposition_mismatch` or `round_limit`, and
ALREADY_ACCEPTED with `duplicate_acceptance`.
Package reads require a running delegated action. Acceptance checks ordinary
fences first; a stale attempt cannot bypass them to reach a delegation error.
Receipt reads use `ATTEMPT_NOT_FOUND` for unknown attempts in the selected session.
Accepted item edits preserve an immutable stale receipt; explicit retry/rework
creates a fresh attempt. Exact replay returns its original success and does not
prove current receipt applicability.

## Error redaction and internal failures

Details may contain bounded identities/paths required for remediation. They MUST
NOT expose full requests, environment variables, file contents, credentials,
artifact bytes, or unrelated item values. `INTERNAL_ERROR` is non-retryable with
a diagnostic UUID. Success is validated before commit; an internal failure is
not a job receipt or evidence of admission/continued work. Caller-side uncertainty
requires original-key reconciliation rather than assumed fresh-key retry.

For an invalid Store, `WORKSPACE_STATE_UNREADABLE` and
`WORKSPACE_CONTRACT_MISMATCH` can provide the read-only `diagnose_writer` recipe
`podway --json version --identity`, which succeeds without opening that Store.
Its reason distinguishes restoring access or returning to the original writer
from explicitly discarding invalid state. After retiring other writers, the
operator may confirm an unfenced `reset --all` with a new idempotency key, or
perform unfenced removal in a human terminal by typing the exact worktree path.
This guidance grants no destructive authorization and never permits a newer
Store generation. `WORKSPACE_ID_CONFLICT` human output names the original
worktree and the same explicitly confirmed discard alternatives. In human mode,
`WORKSPACE_CONTRACT_MISMATCH` also prints hint lines naming the stored and
current manifest digests and repeating the original-writer or explicitly
confirmed unfenced reset alternatives, and `CRITERION_CITATION_INVALID` prints
the citation admission rule; JSON output is unchanged.
