# Podway Supervised External Delegation

## Status and authority

- Document state: `Candidate`
- Dossier type: research and design candidate
- Owning roadmap epic: none
- Target product release: undecided
- Candidate contract target: an optional additive extension to
  `podway.procedure/v2`
- Repository scope: Podway only
- Research and planning baseline: September 10, 2026
- Related candidate:
  [Podway Graph Engineering Evolution](TODO-podway-graph-engineering-evolution.md)
- Related accepted authority:
  [ADR-0010](../architecture-decision-records/0010-generic-cli-json-integration.md)
  and
  [ADR-0017](../architecture-decision-records/0017-single-cursor-convergence.md)

This document preserves a candidate design for optional, action-scoped
supervised delegation. It is not an adopted design dossier, accepted
architecture decision, roadmap commitment, or specification of implemented
behavior. No field, command, result, Store row, adapter behavior, or completion
gate described below exists merely because it appears in this candidate.

The active [roadmap](../roadmap/README.md) does not own this work. Current
specifications, canonical assets, executable contracts, source, tests, and
runtime evidence remain authoritative. The directions below are preferred
research outcomes to test before promotion, not product requirements.

## 1. Context

Procedure v2 already permits an external agent to keep one action attempt open,
perform work outside Podway, record bounded results, and explicitly complete the
placement. Several humans or agents may contribute to that external work while
Podway retains one authoritative cursor and one active attempt. The current
product does not, however, provide a machine-readable action-placement field
that tells a primary agent to delegate the action to a specialist.

The existing Dolgorae handoff is a compatibility contract over Podway's generic
CLI and versioned JSON. It does not make Podway interpret Dolgorae Workflows or
Roles, hire a worker, manage a worker lifecycle, or treat worker completion as a
Podway transition. An author can place delegation prose in an action
definition's `instructions`, but that prose is not a closed automation contract
and cannot gate completion.

The motivating workflow is narrower than the parallel fan groups explored by
the related graph-engineering candidate:

1. When the active action placement has no delegation declaration, the primary
   agent performs it directly as today.
2. When the placement declares a Dolgorae Role, the primary agent becomes the
   semantic supervisor for that placement.
3. The supervisor uses an external adapter to hire or reuse a worker with the
   exact declared Role, supplies a bounded task package, and gives iterative
   feedback.
4. The supervisor accepts the final result only after the action goal is met and
   the worker has reached the declared post-accept lifecycle state.
5. Podway records an attempt-bound structural receipt. A later explicit
   `complete` mutation, not worker completion, advances the cursor.

Dolgorae's External Specialist Engagement is the adjacent executor model: an
external AI remains the semantic control plane while Dolgorae owns specialist
hire, task delivery, result recovery, reuse, release, and operational state.
The September 10, 2026 Dolgorae planning baseline still assigns common and
project Role sources plus immutable Role resolution to its planned durable
orchestration work. That external contract must be revalidated before this
candidate is promoted or qualified; Podway does not own it.

## 2. Goal

Determine whether Podway should make optional supervised delegation visible,
deterministic, and completion-relevant without becoming an agent runtime or
changing its single-cursor graph.

A successful design would make these facts inspectable:

- whether the active action is direct or requires supervised delegation;
- which executor adapter and immutable Role the Procedure expects;
- the bounded input basis and feedback-round budget for the current attempt;
- which external engagement, worker, and final task produced the submitted
  result;
- whether the supervisor accepted that result;
- whether the worker was retained idle or released as declared; and
- why a retry, rework, cancellation, or user decision is required when
  delegation cannot finish.

The design should remain useful as a sequential Procedure v2 feature. A later
fan-group design may reuse its work description and receipt, but parallelism is
not required to justify or ship it.

## 3. Non-goals

This candidate does not propose:

- invoking Dolgorae, a model, a command, or any other executor from Podway;
- putting Dolgorae credentials, Role source bytes, Profile selection, model
  selection, access policy, or process controls in a Procedure;
- authenticating an executor or proving that a model actually performed the
  reported work;
- storing worker transcripts, prompts, artifact bytes, or reasoning content;
- allowing direct execution to fall back when a placement requires delegation;
- delegating decision nodes, goal assessments, or Podway transition authority;
- advancing the cursor when a worker finishes or a receipt is accepted;
- adding parallel active placements, claims, leases, fan groups, or joins;
- letting a worker change the outer Procedure, routes, goal, or current attempt;
- making a Specialist responsible for hiring another first-class Specialist;
- adding a Podway-owned user-notification service; or
- changing, migrating, or reinterpreting an already admitted Procedure
  snapshot.

## 4. Rough scope and preferred direction

### 4.1 Authority split

The preferred split is:

| Concern | Owner |
|---|---|
| Action intent, required evidence, cursor, attempt, and completion | Podway |
| Decision to accept the worker's semantic result | Primary agent acting as supervisor |
| Role resolution, worker, task, thread, lifecycle, and recovery | External executor such as Dolgorae |
| Credential handling and executor authorization | Executor adapter |
| User-facing notification after exhaustion | Primary-agent host |

Podway retains formal progression authority. The supervisor retains semantic
judgment. The executor retains operational worker authority. None may infer or
overwrite another owner's state.

### 4.2 Action-placement declaration

The preferred authoring direction adds one optional closed `delegation` object
to an action placement, not to a reusable action definition. This permits two
placements of one definition to select different execution behavior. Decision
placements reject the field.

The following shape is illustrative and non-contractual. Field names, string
patterns, and numeric ceilings are not reserved until an adopted contract owns
them.

```yaml
graph:
  nodes:
    - id: implement
      use: implementation
      delegation:
        executor: dolgorae.external-specialist/v1
        role_ref: rust-implementer
        role_snapshot_digest: sha256:<64-lowercase-hex>
        max_rounds: 8
        after_accept: retain
      next: verify
```

Absence means direct execution. Presence means delegation is mandatory for the
placement and direct fallback is invalid. The declaration contains:

- a bounded, opaque executor-adapter identifier that Podway does not interpret;
- the executor-owned Role reference;
- the expected immutable Role snapshot digest;
- a required positive, bounded feedback-round limit; and
- a required `retain` or `release` disposition after supervisor acceptance.

The complete declaration participates in canonicalization, the Procedure
snapshot, and the Procedure digest. Procedure validation checks only the closed
syntax and Podway-owned bounds. The external adapter resolves the Role and fails
closed before hire when the Role is absent or its captured digest differs.

Role identity does not select a model, Profile, access policy, credential, or
Agent Configuration. Those remain explicit executor policy. A submitted receipt
records the resulting immutable Agent Configuration digest so a retained worker
is reused only when its complete executor-owned configuration remains
compatible.

### 4.3 Canonical work package

Observation of an active delegated action should expose a bounded canonical work
package and its input-basis digest. The package should derive only from admitted
Podway state:

- workspace, Procedure, session, placement, and attempt identity;
- action title, intent, description, instructions, and item prompts;
- current goal revision and bounded goal criteria when goal tracking is active;
- explicitly selected upstream evidence identities, complete-value digests, and
  existing bounded previews; and
- the delegation declaration.

The package is a read projection, not a mutation or an executor request. Full
selected values continue to use the existing evidence-read contract. The
supervisor may add turn-local feedback in Dolgorae, but Podway does not store or
canonicalize that conversation. The final receipt binds to the original package
digest and a digest of the accepted result.

### 4.4 Supervisor and worker loop

The external adapter should derive stable idempotency and provenance from the
Podway workspace, session, placement, attempt, and round identity. It may open a
new External Specialist Engagement or recover the existing one, then hire or
reuse a member whose Role and Agent Configuration match exactly.

One round is one external Specialist task accepted for this Podway attempt. The
initial assignment is round one; every accepted feedback assignment increments
the count. A failure before external task acceptance does not consume a round.
An accepted task with an uncertain outcome does consume a round because silently
reissuing it could duplicate work.

The supervisor may continue while the declared budget remains and does not
submit acceptance merely because the worker returned a response. Podway needs no
separate delegation-begin mutation in the sequential design: the executor owns
in-flight recovery, and deterministic external provenance lets a restarted
supervisor recover the same engagement and task rather than create another one.

### 4.5 Structural acceptance receipt

The preferred public surface is a separate mutation conceptually equivalent to
`delegation accept --stdin`. The name and exact grammar are illustrative and
non-contractual. Acceptance records the worker result and supervisor decision
without advancing the cursor. Ordinary action `complete` subsequently requires
both the action's normal item satisfaction and one current accepted delegation
receipt.

The closed input should carry normal workspace, session revision, active
attempt, and idempotency fences plus:

- executor adapter identity and version;
- external engagement, member Run, and final task identities;
- Role reference and Role snapshot digest;
- immutable Agent Configuration digest;
- work-package input-basis digest;
- used round count;
- final result digest and a bounded non-secret summary;
- observed worker disposition, `idle` or `released`; and
- required supervisor actor attribution and explicit acceptance.

Acceptance rejects a Role, basis, attempt, round count, or disposition that does
not match the active declaration. `retain` requires an idle worker; `release`
requires completed member release before acceptance. Exact idempotent replay
returns the original receipt, while same-key drift and stale identities fail
closed. The receipt is current-attempt state and immutable history after retry or
rework invalidates it.

All receipt facts remain caller supplied. Podway validates structure, declared
identity equality, digest format, bounds, freshness, and lifecycle vocabulary.
It does not query Dolgorae, validate a credential, inspect a transcript, verify
external bytes, or claim factual attestation. This follows Podway's existing
same-user and external-check-result trust boundary.

### 4.6 Retention, exhaustion, and failure

After acceptance:

- `retain` leaves the exact compatible worker idle for possible reuse by a later
  delegated placement in the same Podway session; and
- `release` requires the adapter to finish member release before it submits the
  acceptance receipt.

A later placement may reuse a retained worker only when executor, Role
reference, Role snapshot digest, and Agent Configuration digest all match. A
different or uncertain identity requires a new worker. A terminal Podway session
does not retroactively depend on post-terminal executor cleanup; the adapter
closes retained engagement state separately and reports any recovery debt. An
author who requires pre-transition cleanup uses `release` on that placement.

When all declared rounds are consumed without supervisor acceptance, the
primary agent should:

1. leave the worker idle;
2. record an ordinary Podway blocker on the active action;
3. notify the user with the used rounds and a bounded summary of the last known
   result; and
4. wait for an explicit choice to retry, rework, or cancel.

No automatic retry or failure route occurs. Retry creates a fresh action attempt
and a fresh round budget; it may reuse the exact idle worker. Rework and
cancellation require the adapter to release affected worker state according to
its own durable lifecycle. Results or receipts from the old attempt remain
inspectable history but cannot satisfy the new attempt. A late receipt from an
invalidated attempt is rejected as stale.

Role-resolution failure, digest drift, unavailable executor state, ambiguous
external outcome, receipt mismatch, and required-release failure also leave the
action incomplete. Existing blocker, retry, rework, cancellation, response-loss,
and job-lookup mechanisms remain the control paths; this candidate does not
invent automatic graph movement for those failures.

### 4.7 Compatibility direction

The preferred direction is an optional Procedure v2 extension rather than a
second authoring model or a dependency on the possible Procedure successor in
the graph-engineering candidate.

Procedures that omit `delegation` should retain their existing canonical bytes,
digests, projections, and runtime behavior. Existing admitted sessions continue
from their immutable snapshots and are never rewritten. A Podway release that
does not know the new closed field rejects a delegated Procedure rather than
ignoring it. Automation discovers the new route and result schemas through the
manifest-bound capability catalogs and never infers support from product version
text.

Promotion must decide the exact versioning of changed observation, preview,
graph, acceptance, completion-error, and stored-state contracts. An additive
authoring field does not by itself justify silently adding fields to an existing
closed result schema.

### 4.8 Relationship to future parallel work

This candidate retains exactly one active outer placement and attempt. It adds
neither an execution frontier nor concurrent Podway work units.

If the graph-engineering candidate later earns promotion, one fan work unit may
reuse the delegation descriptor, canonical work-package basis, external result
identity, and structural receipt. The parallel contract would additionally need
work-unit identity, claim or lease generation, dependency readiness, effects,
and join semantics. Those additions remain owned by that candidate and must not
be smuggled into this sequential feature.

## 5. Open decisions

The preferred direction is not ready for implementation. These questions remain
material:

1. Whether a real workflow experiment demonstrates a recovery or omission gap
   that Procedure instructions, ordinary recorded items, and Dolgorae's own
   durable engagement state cannot already close adequately.
2. The exact field names, identifier grammar, Unicode and encoded-size limits,
   and hard maximum for `max_rounds`.
3. The exact canonical work-package schema, preview budgets, digest domain, and
   treatment of goal revisions made while the action is active.
4. The exact acceptance input, result, observation, history, and error schema
   versions and their manifest registrations.
5. Whether the receipt belongs in dedicated attempt storage or can reuse an
   existing immutable recorded-value representation without confusing it with
   authored evidence items.
6. The exact adapter configuration that selects executor-owned Profile, model,
   access, and Agent Configuration without hidden inference or a second Role
   instruction body.
7. Whether external engagement and member identifiers may appear in every
   public history projection or require a more compact opaque correlation
   identity.
8. Exact recovery behavior when supervisor acceptance is durable but the later
   action-complete response is lost, or when executor release succeeds but
   receipt acceptance does not.
9. Exact terminal-session cleanup reporting when retained executor state cannot
   be closed.
10. Which accepted ADR extends ADR-0010 to allow the generic declaration and
    structural receipt while preserving its no-adapter and no-auto-completion
    decisions, and whether ADR-0019 permits this additive Procedure v2 evolution
    without further amendment.

## 6. Roadmap promotion conditions

Promotion requires all of the following:

1. Run a preregistered disposable experiment using current Procedure v2 and a
   real compatible External Specialist Engagement. Cover direct execution,
   required delegation, repeated feedback, retained reuse, release, supervisor
   restart, executor restart, response loss, round exhaustion, retry, rework,
   cancellation, and late results.
2. Compare prose-only instructions plus executor-owned durable state with the
   proposed declaration and receipt. Identify a concrete failed invariant or
   omission that the Podway feature fixes. If the existing composition is
   sufficient, remove or narrow this candidate instead of promoting it.
3. Revalidate Dolgorae's then-current checked Role-source, Role-resolution,
   External Specialist Engagement, immutable Agent Configuration, task-result,
   reuse, and release contracts. A planned external task or private draft is not
   an implementation dependency authority.
4. Close every material decision in section 5, including all bounds, public
   schema versions, storage ownership, failure semantics, and external adapter
   prerequisites.
5. Accept the necessary architecture decision for generic supervised delegation
   and explicitly confirm preservation of the single-cursor, no-execution,
   no-provider-core, no-semantic-attestation, and explicit-completion boundaries.
6. Perform a same-user threat and privacy review covering forged receipts,
   leaked executor identifiers, credential exclusion, stale external results,
   idempotency conflicts, and malicious or buggy adapters.
7. Register one owning roadmap epic and create a decision-complete adopted
   dossier with implementation tasks, compatibility handling, focused tests,
   development-gate acceptance, and any later release qualification.

The adopted verification design must cover at least:

- byte- and digest-stable behavior for Procedures without delegation;
- rejection of delegation on non-action placements and rejection of malformed,
  unknown, over-bound, or incomplete declarations;
- deterministic work-package projection and input-basis digesting;
- refusal to complete without a current accepted receipt;
- Role, Role-digest, basis, attempt, round, lifecycle, and Agent Configuration
  mismatches;
- acceptance idempotency, detached response loss, lookup, replay, daemon restart,
  and stale-attempt rejection;
- retained worker reuse and required worker release;
- exhaustion blocker creation and the user-notification handoff;
- executor and supervisor restart without duplicate engagement, member, or task
  creation; and
- proof that no worker event or receipt acceptance automatically advances the
  Podway cursor.

## 7. References

- [Documentation authority and precedence](../README.md)
- [TODO candidate lifecycle](README.md)
- [ADR-0010: Integrate External Tools Through Generic CLI and JSON](../architecture-decision-records/0010-generic-cli-json-integration.md)
- [ADR-0017: Permit Single-Cursor Convergence](../architecture-decision-records/0017-single-cursor-convergence.md)
- [ADR-0019: Make Procedure v2 the Only Product Model](../architecture-decision-records/0019-procedure-v2-only-product.md)
- [ADR-0025: Bind External Check Results Structurally](../architecture-decision-records/0025-structurally-bound-external-check-results.md)
- [Procedure and Item Specification](../specs/domain/procedure-and-item-specification.md)
- [Automation Client Contract](../specs/interfaces/automation-client-contract.md)
- [User Workflows](../specs/product/user-workflows.md)
- [Podway Graph Engineering Evolution](TODO-podway-graph-engineering-evolution.md)
