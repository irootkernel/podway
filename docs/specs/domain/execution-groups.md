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
Config's dedicated `execution_groups` API admits internal base/group definitions,
checks their interfaces, and produces canonical identities and source/graph
projections. Pure execution transitions remain a separate obligation;
constructing or admitting a definition alone performs no execution transition. The public parser, CLI, Store and presets continue using
Procedure v2. Successor runtime admission is reserved for the atomic product
cutover. Stores bound to the previous manifest, including v0.2.17, remain with
their original writer; no predecessor or migration is declared. Tests use
isolated fixtures rather than opening the active development workspace.

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

## Internal source admission and projections

Config reuses the bounded YAML/JSON decoder, closed shared item and node metadata
constructors, existing predicate vocabulary and bounded local graph analysis.
The shipped `parse_procedure_document` dispatch still accepts only Procedure v2.
Internal admission rejects unknown fields, duplicate mappings, nulls, unsupported
YAML features and resource overruns before constructing the successor model.
YAML dispatch retains the existing refusal of JSON-form root documents.

An anchor must be an unskippable Action with `next` equal to `success_return`.
That static edge describes the group boundary's declared successor, rather than
a second independent routing authority. Terminal anchors and mismatched targets
fail admission. Anchor and return dominate every actual base terminal. A group
has exactly one actual success-terminal Action. Every placement is reachable and
has a finite actual terminal or explicit replan exit; ordinary forward cycles
remain forbidden, including at decisions that also offer replan. Replan-only
branches are legal boundary exits and never count as successful base completion.
All ordinary edges still undergo reference, reachability and cycle analysis.

Required local evidence strictly dominates its consumer and cannot come from a
skippable action. Plan ports exist only inside a group; execution-result ports
exist only at or after the base's group-return placement. Guards require a
required evidence reference with an explicit item selection. An optional source
item is still eligible under the existing predicate contract; an optional
evidence reference is not. Missing or unevaluable current values grant no replan
permission; current-value evaluation belongs to FRAME-003's pure transition rules
and later runtime composition, not to Config source admission. The default
verdict/stage/score inputs are unconditional, while
custom eligible guards need no mandatory default score record or native fixed
threshold.

Ordinary decision rework targets dominate their consumers and remain local.
Base decision rework cannot target the anchor or cross its boundary. After group
return, neither an Action's `next` nor a Decision's ordinary `advance` may escape
into prefix-side placements, including a shared region leading back to the anchor.
First prefix Action/Decision arrival at the anchor and the declared anchor-to-return
boundary edge remain legal. Base manual rework may declare prefix targets other
than the anchor; graph projections mark
them as available only before first activation. Enforcing that history-dependent
condition belongs to the later pure transition owner. Group definitions cannot
declare session-goal assessment; tracked bases retain complete outcome coverage
and assessment dominance over their terminal paths.

Canonical bytes derive from admitted typed values with defaults materialized.
Author-ordered arrays retain their order; authoring-map key order normalizes
away. Comments and source formatting do not affect the definition digest.
Prompts remain part of that digest even though interface comparison ignores
them. Source rendering preserves authoring-map order and supported YAML comments;
JSON, YAML and canonical re-admission reach the same semantic identity. Graph
edges follow option declaration order and expose replan without inventing a
local target. Projections contain no live selection or attempt state.

Canonical JSON and graph projections retain both scalar and encoded-byte limits.
Formatted source retains the existing scalar-only projection ceiling. Protocol
owns wire-derived delegation-package sizing and placement-local next/static,
read-back, preview, token and envelope allocations, for a single scope as well
as combined base/group responses. Those checks require the successor wire
families; Config source admission proves neither package nor guidance readiness.
GROUP-002 owns pre-effect activation/return budget admission, and ADOPT-001 owns
public family/cutover selection. Existing per-document, item, reference and
source/projection caps remain in force throughout. Session-creation budgets
remain with their original transition/runtime owners.

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
