# ADR-0043: Atomic Assessment and Commit-Bound Observations

- Status: Accepted
- Date: 2026-11-06
- Acceptance date: 2026-11-06
- Acceptance owner: the owner, through approval of the documentation registration plan
- Delivery owner: [AGENT](../roadmap/README.md#agent-agent-interaction-simplification)
- Design: [AGENT dossier](../todo/TODO-podway-agent-interaction-simplification.md)
- Supersedes in part: [ADR-0039](0039-opt-in-contract-generations.md), decision 4's generation-only result selection, for the explicit semantic response choice below
- Preserves: ADR-0039's presentation-only generation selector, default and retirement policy

This accepts a future design, not delivered behavior. Registration changes no
executable contracts, commands, schemas, manifest, Store or installed runtime.
AGENT-001 records the contracts through the existing reservation workflow;
AGENT-003 and AGENT-005 implement the corresponding capabilities. Reserved
contracts must not advertise unsupported runtime routes or result emission.

## Context

Single criterion assessments require separate mutations. Ordinary mutation
results omit the next observation. A batch and optional commit-bound observation
reduce normal caller invocations without combining assessment with decision or
recording with completion. Persisted response bytes require a semantic request
choice independent of invocation-level contract generations.

## Decision

### Atomic assessment batch

The future spelling is `podway goal assess-criteria --stdin`, machine route
`goal.assess_criteria`. Its closed input is
`podway.criterion-assessment-many-input/v1`. Required fields are `schema`,
`workspace_uuid`, `session_id`, `session_revision`, `attempt_id`, `goal_revision`,
`idempotency_key` and `assessments`; `actor` is optional and batch-wide.
Each assessment requires `criterion_id`, `status` and `reason`, with optional
`evidence` and `items` arrays defaulting to empty, using the existing single
assessment identifier and citation meanings. Unknown fields are rejected.
Existing field bounds apply, including four citations per assessment and the
existing actor/reason limits. Session revision is u64; goal revision is a
positive u64. Input is bounded to 1,048,576 bytes, JSON depth 64 and 1-16 entries.
The document owns identity, revision and key fences; conflicting argv flags
remain rejected. Preflight must never replace these fences.

One batch targets one current session, active checkpoint attempt and goal
revision. Partial subsets are allowed. Empty batches and duplicate criterion
IDs are invalid requests. Unknown or already-assessed criteria, mixed assessment
and applicability modes, invalid reasons and citations, ineligible sources and
stale evidence retain existing single-assessment rejection meanings. Ordered
entry validation reports the first invalid entry after whole-request shape,
resource and duplicate checks and the existing transaction fence checks.

Input entry order participates in canonical request identity; reordering is a
changed request. Stored and returned results normalize to criterion declaration
order. Actor attribution applies to every submitted entry; the transaction clock
is read once and supplies their common recorded time. One unpublished successor
advances session revision and workspace sequence once, preserves the cursor and
goal definition revision, and persists all entries and one success atomically.
The public single-assessment mutation must not be looped and counters repaired.

The closed result `podway.criterion-assessment-many-result/v1` uses the existing
single-assessment result fields and semantics, replacing `result` with `results`
for the submitted entries in declaration order. `complete` and optional
`determined_outcome` derive from the final state, including previous assessments.
Its `revision` describes one session revision change. Existing single-assessment
contracts remain unchanged.

### Optional observation response

The route-local flag `--with-observation` is supported only for `session.start`
(plain preset/Procedure start, including its existing disposed-terminal branch),
`session.begin`, `item.record_many`, `session.complete`, `session.decide` and
`goal.assess_criteria`. Replacement start, reset, archive, removal, delegation
and other mutations are excluded. Unsupported flag use is a request error.
The flag is also allowed beside stdin-owned requests; it supplies no fences.

Only a selected request adds `return_observation: true` to canonical request
identity. Absence preserves existing request bytes and result families. Same-key
reuse with a different mode or payload is the existing idempotency conflict.
Both supported invocation generations use this same semantic choice and stored
result. `--contract-generation` remains excluded from request identity and
stored outcomes; no default or retirement policy changes.

Selected successes retain Output v4 and use the new closed result
`podway.mutation-observation-result/v1` with required fields `schema`,
`mutation_result`, `observation` and `workspace_sequence` (u64).
`mutation_result` is the route's unchanged command-specific result, including the
new batch family where applicable. `observation` is an observation-result/v6
object, independent of invocation generation; `workspace_sequence` is the
sequence committed with this outcome. Command/result registrations admit only
these six route/family pairings, not arbitrary nested results or wrappers.
Released result families do not gain optional fields.

### Projection, admission and replay

Engine composes the successor observation in the mutation transaction using one
coherent graph, workflow memory, current attempt, goal assessment, receipt and
readiness context and the assigned sequence. Filesystem preparation retains its
existing pre-writer ownership. No postcommit observe or second database
acquisition manufactures this response. Start projects prepared state; begin
projects running state; complete/decide project the resulting cursor or terminal
state. Record-many no-op preserves item/session revisions while recording its
normal success and workspace sequence.

Before commit, Protocol admission validates the entire stored success and its
recoverable lookup representation against 1,048,576 bytes and depth 64. Lookup
headroom charges its complete wrapper and maximum-width legal invocation
metadata, rather than a guessed constant. Existing bounded observation windows,
exact totals and honest truncation remain; no extra truncation or omitted
observation substitutes for an oversized promised response. Projection,
serialization, depth or size failure rolls back state, sequence, outcome and
journal. Existing error classification and precommit response-admission failures
apply. Store independently admits loaded and newly supplied persisted state.

Replay and lookup return the original stored observation and canonical success;
no projection refresh and no variable replay marker alters stored bytes.
Callers generate a new unique key for each new logical request and track its
first submission and any retries. Only a successful, known first submission
without hidden retries can replace the next observe on the normal path. After
retry, lookup, lost output, contention requiring refresh, or uncertain submission
history, callers observe before deciding a new mutation. A returned observation
is as of commit, never a guarantee against subsequent writers; fences remain
mandatory. No server-provided freshness discriminator is introduced.

## Consequences and verification

New closed families and conditional result registrations require coherent
manifest/adapter generation in AGENT-001. Existing writer compatibility,
predecessor, migration and retention policies remain in force. This decision
authorizes no consumer modification, release, installation or activation.

The dossier maps batch rollback/replay, coherent lifecycle projection, full
response/lookup budgets, recovery and existing-contract compatibility to their
implementation tasks. Structural CLI counts are correctness goals, not latency
measurements: three ready checkpoint criteria and decision take two normal
calls; recording and completing a satisfied action take two. Recovery may need
extra observations. Assessment/decision and record/complete stay separate.
