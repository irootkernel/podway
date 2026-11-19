# Execution Group Model Contracts

## Authority and availability

[ADR-0044](../../architecture-decision-records/0044-versioned-execution-groups.md)
accepts the successor architecture. The canonical authoring shapes are
[`procedure-v3.schema.json`](../../../assets/schemas/procedure-v3.schema.json)
and [`execution-group-v1.schema.json`](../../../assets/schemas/execution-group-v1.schema.json).
[`execution-group-records-v1.schema.json`](../../../assets/schemas/execution-group-records-v1.schema.json)
owns closed identity, basis, provenance, assessment and scoped delegation record
components. These schemas enroll in the development contract manifest, offline
registry and embedded Protocol registry. Enrollment changes the development
writer's manifest digest; it introduces no predecessor or migration promise.

Core owns immutable successor authoring values and bounded identity records.
Config's dedicated `execution_groups` API admits base and group definitions,
checks their interfaces, and produces canonical identities and source/graph
projections. Core's session model applies the pure execution transitions
specified below. The current CLI, parser and presets use Procedure v3 and
Execution Group v1 in Store generation 16. Stores bound to the previous
manifest, including v0.2.17, require their original writer. Tests use isolated
fixtures to preserve the active development workspace.

The Direct Engine invokes Core lifecycle, item and delegation mutations inside
the [success-only transaction](../storage/transactions-concurrency-and-idempotency.md#execution-group-candidate-persistence).
It computes current projection and replay bounds before persisting a transition.
Replan commits the completed Decision and its exact guard evidence, the
selection closure, and the fresh base anchor together.

## Authoring boundaries

The base has exactly one `execution_group` boundary. It identifies its `anchor`
placement, `success_return`, selected anchor `plan_inputs`, required terminal
`result_items` and permitted base-review `replan_decisions`. The boundary belongs
to a placement rather than its reusable action definition. A group declares its
`plan_inputs` as existing item declarations; its terminal action supplies the
base's result contract. Neither boundary introduces an arbitrary output schema.

Successor evidence references identify `scope`: `local` also names a `node`,
while `plan` and `execution_result` select explicit port items without a local
node. Local references retain their existing optional whole-item selection.
Cross-scope references are resolved only through the declared current ports.
Guard predicates use the same closed source distinction in `evidence`:
`local` carries `node` and `item`; `plan` and `execution_result` carry `item`
without a local node. They reuse the existing predicate operators, AND bound
and value semantics. A base-review guard can therefore name the selected current
completed group's output without knowing that replaceable group's local terminal
node ID. Neither historical groups nor arbitrary group IDs are guard sources.
Successor decisions keep shared metadata separately from option-bound scoped
guards; legacy node-only guards cannot coexist in that metadata.
A `replan` decision route has no `to` target. It denotes an explicit boundary
operation, not an ordinary decision edge; ordinary advance/rework routes retain
their declared local targets. Replan guard eligibility and current truth remain
separate admission conditions, including for custom documents.

Selected base plan-input declarations must also be self-contained in boundary
selection order: each conditional controller must be selected earlier and remain
unconditionally required, just as in the matching group's plan interface.

Item definitions, predicates, guards and delegation declarations retain their
existing bounded contracts. Boundary comparison uses canonical declarations
with constructor defaults materialized. Only `prompt` is ignored: IDs, help,
types, conditional rules, constraints, choices, operation identities and accepted
outcomes must match, including declaration order. Activation freezes the exact
anchor attempt, item revisions, value digests and complete item-set digest.

## Source admission and projections

Config reuses the bounded YAML/JSON decoder, closed shared item and node metadata
constructors, existing predicate vocabulary and bounded local graph analysis.
Public authoring and session preparation use the closed successor model. Source
admission rejects unknown fields, duplicate mappings, nulls, unsupported YAML
features and resource overruns before constructing the model.

An anchor must be an unskippable Action with `next` equal to `success_return`.
That static edge describes the group boundary's declared successor, rather than
a second independent routing authority. Terminal anchors and mismatched targets
fail admission. Anchor and return dominate every actual base terminal. A group
has exactly one actual success-terminal Action. Every placement is reachable and
has a finite actual terminal or explicit replan exit; ordinary forward cycles
remain forbidden, including at decisions that also offer replan. Replan-only
branches are legal boundary exits on ordinary Decisions. Only a terminal path
can count as successful base completion. A Decision declaring session-goal
assessment cannot offer a replan route. All ordinary edges still undergo
reference, reachability and cycle analysis.

Required local evidence strictly dominates its consumer and cannot come from a
skippable action. Plan ports exist only inside a group; execution-result ports
exist only at or after the base's group-return placement. Guards require a
required evidence reference with an explicit item selection. An optional source
item is still eligible under the existing predicate contract; an optional
evidence reference is not. Missing or unevaluable current values grant no replan
permission. Core evaluates these values during the pure transition. Config
admits the declared sources and predicates. The default
verdict/stage/score inputs are unconditional, while
custom eligible guards need no mandatory default score record or native fixed
threshold.

Ordinary decision rework targets dominate their consumers and remain local.
Base decision rework cannot target the anchor or cross its boundary. After group
return, neither an Action's `next` nor a Decision's ordinary `advance` may escape
into prefix-side placements, including a shared region leading back to the anchor.
First prefix Action/Decision arrival at the anchor and the declared anchor-to-return
boundary edge remain legal. Base manual rework may declare prefix targets other
than the anchor; both Core's transitions and Config's graph projections limit
those targets to the period before the first activation. Session-goal
assessment belongs to a tracked base, with complete option and outcome coverage.
Every tracked base terminal must be dominated by an assessment at or after the
declared group return. A base may also record a supplemental assessment before
the return when its terminal paths meet this requirement.

Canonical bytes derive from admitted typed values with defaults materialized.
Author-ordered arrays retain their order; authoring-map key order normalizes
away. Comments and source formatting do not affect the definition digest.
Prompts remain part of that digest even though interface comparison ignores
them. Source rendering preserves authoring-map order and supported YAML comments;
JSON, YAML and canonical re-admission reach the same semantic identity. Graph
edges follow option declaration order and expose replan without inventing a
local target. Projections contain no live selection or attempt state.

Canonical JSON and unpaged authoring projections retain both scalar and
encoded-byte limits. Historical DAG reads page the same canonical graph
projection, so an admitted definition remains inspectable beyond the
unpaged graph response ceiling.
Formatted source retains the existing scalar-only projection ceiling. Protocol
owns wire-derived delegation-package sizing and placement-local next/static,
read-back, preview, token and envelope allocations, for a single scope as well
as combined base/group responses. Those checks require the successor wire
families; Config source admission proves neither package nor guidance readiness.
The Direct Engine bounds public mutation responses before persisting them. Existing
per-document, item, reference and source/projection caps remain in force.
Session-creation budgets
remain with their original transition/runtime owners.

Historical queries bind the exact base or group definition and retained
attempt rather than resolving a node name at the live cursor. The stored
slot is authoritative for recorded values and revisions. Historical
readback grants no current evidence eligibility.

## Identity and retained assertions

An activation has a fresh `group_execution_id`, even when its definition digest
matches a prior activation. Its immutable identity binds session, base digest,
anchor, design attempt, definition digest, current goal revision when tracked,
plan binding and latest predecessor. Scope plus session, base digest, node and
attempt identifies a contribution; a delegation package digest additionally
binds acceptance to that exact target. Reusing a local node ID or definition
digest never makes an old contribution current.

Every activation records the caller's nonempty working-copy basis descriptor
(up to 512 Unicode scalars) and SHA-256 digest. First activation has no prior-work
dispositions. With a predecessor, 1 to 128 records identify retained same-session
work, optionally a whole historical attempt or an exact item/value digest, a retain,
revalidate, modify or revert choice, and current-plan rationale (up to 2,000
scalars). The latest predecessor remains independent of which earlier work is
relevant: G3 can cite G1 after an empty G2. State admission must validate these
references against retained history and the current plan; values alone cannot
establish those relationships or verify working-copy truth.

Candidate history exposes each activation's plan and working-copy basis with
its original disposition order. The latest predecessor and the earlier source
of each disposition are separate links. A cited attempt or item can be opened
through its exact historical scope. Reuse still requires current evidence and
fresh assessment or check items wherever the new group declares them.

Selection is separate from execution outcome (`running`, `completed`,
`abandoned`, `terminated_by_cancel`) and closure cause (`replan`, `goal_revision`,
`reactivation`, `completion`, `cancel`). Assessment records bind exact source
attempts and selected item revisions/digests to verdict, stage and score 0..100.
Stage is `not-scored`, `lightweight` or `scored`. Decision entry ordinals start at
one and bind the selected group, including base-review decisions. They are facts,
not predicates or a quota. This selected-group entry record includes base review;
it does not replace the existing group-free base-prefix counters. The consumer's
screening/scoring schedule and 80-point default threshold remain those of ADR-0044;
these records add no scoring engine or guarantee of independent review. A default
assessment record is not a prerequisite for custom guards: custom documents may
use the existing predicate vocabulary over their exact bound items. Their frozen
attempt/item provenance and transition eligibility remain admission obligations.

Replan feedback records bind the exact decision attempt and option to a nonblank
reason (up to 2,000 Unicode scalars), the working-copy basis, and nonblank feedback
(up to 4,000 scalars). Recorded plan-item value digests require positive revisions;
revision zero denotes no recorded value in the existing item-slot contract.

## Pure execution lifecycle

Core keeps one active attempt and at most one selected execution. It validates
cursor, session, and goal-revision fences before transition; a single item edit
fences the exact scoped attempt and item revision. An atomic batch additionally
checks the cursor revision and validates every distinct slot before updating
the session revision once for any changes. Identical values and empty clears
preserve the corresponding slot data and revisions. When every slot stays
unchanged, the session revision stays the same. The caller supplies admitted
definitions and digests, fresh identities, time, and validated external facts.
The pure model makes no workspace or persistence calls.

Candidate Engine requests independently fence workspace and selected execution.
The selected-execution fence still applies when the active attempt is a Base
review. A current observation carries the exact cursor and selected execution,
current guidance and item revisions, the current or upcoming Decision ordinal,
and remaining activation and attempt counts. Item changes, reads, original
outcome lookup and exact replay preserve that ordinal. A distinct activation
of the same definition starts its own Decision count.

Activation is the design anchor's only successful exit. The anchor must satisfy
its required items, evidence, blockers, and any declared delegation gate. A
goal-tracked session must have its current goal defined. Activation binds the
complete anchor item-set digest and any present selected values with their exact
slot revisions and digests. An unset optional or inactive conditional selection
exports no value. Core checks both boundary contracts before starting the group.
The group's terminal checks the ordered result declarations again before
completing the execution and entering the declared base return. The shared
readiness check requires satisfactory values for each item whose matching
declaration requires it at completion. An optional result or a result with
inactive conditional requirements exposes no value when its slot is unset. The
completed group remains selected while the base uses its result.

Formal completion of a tracked session requires a current Goal assessment
bound to the selected completed execution and the current Goal revision.
Supplemental prefix outcomes remain in attempt history. After terminal closure,
readback uses an eligible assessment from the execution that closed with the
session. Cancellation retains its lifecycle and the prior assessment record.

Current evidence resolves inside the attempt's active scope or through the
declared plan and selected execution-result ports. At selection time, each
Decision freezes the exact source attempt for each declared evidence reference.
The retained definition and final item slots identify the evidence values
even after those attempts become stale. This applies to guarded and unguarded
options and confers no current evidence eligibility. A decision can replan only
through an explicit option whose declared guards hold on current values. A
permitted base-review decision also needs the authored anchor-return policy.
Core records the option, exact guard source attempts and item bindings, and
closure feedback before opening a fresh anchor attempt. Retrying a decision
increments its scoped entry ordinal. Each group-bound entry retains its exact
attempt and selected execution, including entries in base review. Base-prefix
counts have no group binding. Scoped rework stales the revisited path and keeps
its recorded values. Suffix rework in base retains the selected result.

A blocker starts open and keeps its identity, reason, and creation time.
Resolution updates that open row at the current cursor. Open and resolved rows
remain in the attempt history after closure.

Replan and Goal revision abandon a still-running group. Cancellation terminates
a still-running group. Each preserves an already completed outcome. After the
first activation, a goal revision replaces the current statement and criteria,
opens a fresh anchor, and retains the previous goal and immutable activation
records. Completed-session reactivation requires explicit authority and also
returns to the anchor. Goal-revision lineage preserves the prior group,
old and new attempt identities, closure cause and feedback even if replan or
completion already closed selection. The earlier group's closure remains part
of its own record.
Cancelled sessions cannot reactivate. Scoped delegation acceptance binds the
current declared action, exact target and independently validated package
digest. An actual item change or initial goal definition clears the live
acceptance for that attempt. An unchanged write preserves it. A fresh request
to accept an already accepted current action is rejected, while exact replay
recovers the original success.

## Bounds and responsibility

One Core-owned envelope limits activations to 64, placements per graph to 1,024,
edges per group to 8,192, cumulative session placements to 16,384, and cumulative
attempts to 65,536. Reusing a definition consumes a new activation and attempts,
not another definition copy. Existing item, document and response limits remain
in force. No aggregate limit permits deletion or eviction of retained history.

Config owns graph/reference/port admission and projections; Core owns pure
transitions; Store independently admits durable relationships; Engine composes
atomic operations. Protocol owns wire families and budgets. CLI admission and
new group-aware public delegation families belong to the product cutover.
The [roadmap](../../roadmap/README.md) alone owns current delivery status.
