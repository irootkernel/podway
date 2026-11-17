# ADR-0044: Version Execution Groups Under One Design Anchor

- Status: Proposed
- Date: 2026-11-17
- Proposed delivery owners: FRAME, GROUP, TRACE, ADOPT
- Planning release target: v0.2.18
- Design: [Execution Group Continuation](../todo/TODO-podway-continuation-groups.md)
- Architecture acceptance: not recorded; document approval does not accept this Proposed ADR
- Planned partial supersession: [ADR-0019](0019-procedure-v2-only-product.md), the v2-only product and fixed v2 preset identity clauses at cutover
- Planned extensions: [ADR-0017](0017-single-cursor-convergence.md), [ADR-0016](0016-recorded-item-workflow-memory.md), [ADR-0024](0024-bounded-evidence-scale-and-paged-read-back.md), [ADR-0027](0027-state-aware-session-start.md), [ADR-0035](0035-host-neutral-supervised-delegation.md), [ADR-0037](0037-bound-procedure-graphs-at-1024-nodes.md), [ADR-0039](0039-opt-in-contract-generations.md)
- Preserves: [ADR-0038](0038-remove-the-daemon.md), the single formal cursor, external execution and the same-user trust boundary

This Proposed ADR does not amend accepted authority or current implementation.
Its acceptance and later implementation remain separate from documentation
write approval, commit, release, installation and consumer adoption. Accepted
ADRs must receive reciprocal relationship links when this decision is actually
accepted; application of this proposal does not rewrite them.

## Context

An admitted Procedure v2 snapshot cannot change topology. Existing state-aware
start can preserve a superseded session and create another session. The owner
requires a task to retain one session while returning to design and selecting a
new execution topology, with every former DAG and recorded node attempt still
available for diagnosis and procedure improvement.

A topology change does not revert code or negate successful work. A former node
may have completed successfully and left changes that remain the input to new
work. Topology selection, execution outcome and current evidence applicability
must therefore be distinct facts.

On 2026-11-17 the owner selected this bounded continuation capability as a
direct product requirement, new sessions only, a single-model transition, and
Coordinator replacement within an approved scope. This narrowly changes the
earlier S7 research-adoption precondition for the selected scope. No S7
experiment result, superiority claim, or evidence for fan/claim/lease/join
mechanisms is asserted.
[PROOF](../todo/TODO-podway-graph-evolution-feasibility.md) retains S1-S6 and
the unexercised historical S7 comparison.

Native subagents and Herdr already permit parallel external contributions under
Aquarium's role/Writer contracts. A Podway cursor identifies formal
progression; it is not a reason to prohibit that parallelism or require
Dolgorae first.

## Proposed decision

1. **Immutable base and one anchor.** Introduce `podway.procedure/v3` and a
   `podway.execution-group/v1` document. One base action placement is the design
   anchor, mandatory even for simple successor presets. The base owns final goal
   assessment where enabled and closeout; a group owns bounded
   action/decision topology with one entry, one success exit and declared local
   rework. Reject nested groups, arbitrary in-place graph edits and cross-group
   progression references. The anchor and group-success return placement must
   dominate every terminal path; successful execution cannot bypass the group.
2. **One selected group, distinct runs.** Every activation binds a distinct
   `group_execution_id`, also its activation identity, to the exact design attempt,
   base/session, plan basis
   and definition digest, even when a definition is reused. At most one group is
   selected active; design has none. A group's completed execution may remain
   selected while the base evaluates its output. Replan or session termination
   closes selection without overwriting completed execution. Running groups become
   `abandoned` on replan/goal revision or `terminated_by_cancel` on cancellation;
   the closure cause is recorded separately.
3. **Explicit atomic boundary.** Activation validates the complete group and
   plan/item interface, closes the satisfied design attempt and creates the group
   entry attempt atomically. Group-terminal completion returns to the declared
   base successor. Explicit replan completes the selected decision attempt, closes
   current selection and creates a fresh anchor attempt. Replan is admitted only at
   declared ordinary decision placements with a replan option, met current failure
   guards over recorded assessment verdict/stage/score (at least 80 in defaults).
   No request score overrides these items. Vetting rejects every unguarded
   replan option, including custom Procedures. Base review
   additionally requires its anchor-return policy. No arbitrary Action escape is
   admitted. The replan request records the selected option and closure atomically;
   ordinary decide cannot execute it or bypass its feedback payload.
   These operations use the existing Direct transaction and success-only
   idempotency architecture. A
   separately authorized goal revision closes any selected activation and creates
   a fresh design attempt atomically. Only explicit group replan from declared
   base-review placements returns to the anchor; ordinary decision/manual rework
   into the anchor is rejected at vetting, as are ordinary crossing decision
   edges; declared guarded replan options are explicit boundary operations. Base
   manual prefix targets may be declared but are rejected at runtime after the
   first group; they remain legal beforehand. Preset decision rework remains
   group-local; suffix-local base rework retains selected output.
   Completed-session reactivation still needs its original explicit authority and
   creates a fresh anchor attempt. Once any group exists, goal-revision rework
   targets only the anchor; every such closure has the same reason/basis/feedback
   as replan. Never rebind an old group to a new goal. Anchor complete/skip is not
   an activation bypass. Goal-tracked sessions require a current goal before
   activation; untracked sessions have no goal binding.
   Default Procedures count each decision's entries per group: first-entry
   failure uses ordinary rework, second-entry failure gets lightweight screening,
   third and every later failure gets scored assessment and a current CO route
   judgment. The schedule is consumer policy: native ordinals are observable data,
   not new predicates. Assessment Actions use an explicit stage and required
   verdict/score items (0 placeholder when unscored), with conditional explanatory
   fields; guarded sources remain unconditional. Blocked means a blocker on the
   current attempt, not a new routed outcome. Success advances immediately. A
   serious structural defect may be
   assessed earlier at a declared condition. The default rubric assigns 50/30/20
   points to structural need, lack of viable local repair and demonstrated lack
   of progress. Scores below 80 forbid replan; 80 or above permit, never compel,
   CO's choice. Affected Evaluators supply reasoning; reuse applicable reports
   rather than forcing full panels or provider calls. Do not manufacture failures,
   loops or inflated scores. Native counters/route admission do not attest agent
   reasoning. Goal revision is no fabricated bypass.
4. **Item-based inputs and results.** Keep ADR-0016's typed recorded items and
   evidence selectors instead of inventing arbitrary typed-output schemas. The
   anchor declares selected plan inputs and required terminal item IDs/types;
   activation freezes exact plan attempt/value digests and checks compatibility.
   Exposed item declarations must be canonically equal after defaults except for
   their presentation-only prompt, including constraints, conditional rules and
   check-result operation/outcome contracts; enforce this again at group return.
   The base consumes only the exact current completed group's declared results.
   Existing dominance, freshness, guard and boundedness rules apply inside each
   admitted scope. Historical evidence never silently becomes current.
5. **Preserved work and lineage.** Retain every group definition, activation,
   design relationship and recorded node attempt, including completed work and
   partial abandoned values. Inactive groups are immutable and inspectable.
   Unvisited nodes gain no attempts. Keep final retained item state per attempt,
   not every overwritten edit or raw external transcript. Replan records reason,
   caller-supplied working-copy basis and feedback/provenance. The new design
   states what work to retain, revalidate, modify or revert; Podway never rolls
   back Git or assumes semantic validity from an old successful result. Once
   any earlier group exists, activation requires 1..128 fresh bounded dispositions
   against any retained same-session group and the current plan; keep the latest
   predecessor as a separate lineage link. Only first-ever activation has no
   dispositions. Initial and later activation each records a bounded caller
   snapshot descriptor/digest. Basis/dispositions are explicit activation-time
   Coordinator assertions, not automatically reviewed plan content or readiness.
6. **Bounded retrospective reads.** Add separate paged group/attempt/DAG/item
   history queries, including archived sessions. Preserve current-only observe,
   next and evidence.read semantics. Paging must expose all admitted content
   with totals/truncation and identity-bound tokens while keeping existing
   response ceilings. Activation budgets the complete current base+group counters
   and guidance before effects; old group counters remain in paged history. Group
   history must not depend on short-lived journal or
   outcome retention. No automatic group eviction, individual group purge or
   historical-group restoration is introduced.
7. **Creation budgets without hidden deletion.** Use at most 64 activations,
   1,024 placements and 8,192 edges per group, 16,384 cumulative base/distinct-group
   placements and 65,536 cumulative attempts per session. Existing document/item/
   response limits remain. Creation over budget rejects atomically; read,
   cancellation and archival remain possible. Existing explicitly destructive
   reset/purge boundaries disclose that group history is also removed.
8. **External roles stay external.** Podway supplies scope and attempt identity,
   stale fences and formal gates. Aquarium owns actual agents, independent roles,
   assignment settlement and one Writer. Multiple subagents/Herdr workers may
   contribute concurrently to one action. Replan does not kill workers; the
   caller settles descendants and uncertain external effects before any replan
   or goal-revision/reactivation closure. In-scope
   topology replacement retains the original grant and needs current plan review,
   not a repeated Master approval; changed scope or uncovered effects require new
   authority, and consumed grants do not revive. Podway
   records assertions without authenticating their truth. No claims, leases,
   fan groups, joins, scheduler, new failed-session state or mandatory adapter
   belongs to this slice. Preserve ordinary single-item concurrency across session
   revisions with workspace/session ID, activation, attempt and item fences;
   cursor/selection mutations and record-many retain session-revision fences.
9. **New-session-only single-model cutover.** The first complete feature release
   replaces v2 authoring/runtime support with the successor. Existing v2 sessions,
   snapshots and outcomes are not converted or deleted automatically. Finish
   them with the original writer; reject legacy Stores without modification or
   sidecar creation. Explicit reset retains separate destructive authorization.
   Do not claim transparent migration, downgrade support or v2/v3 coexistence.
   Later writer upgrades require their own declared compatibility contract.
10. **Complete staged delivery.** Preserve opt-in base/group action delegation
    with new activation-bound package/receipt families. Retain presentation
    generations 1/2 and default 1, versioning affected families without selecting
    v2 semantics; keep the shared Output v4 and Error v2 envelopes. Migrate the
    four builtin IDs to their -v3 successors with one anchor each and their
    existing goal policies. Use isolated test-only real-
    process Engine/Store drivers before exposure; candidate DDL/Store admission stay
    out of the public v2 Store path and have no release predecessor. Successor
    schemas enroll automatically in the development manifest, offline registry
    and Protocol's embedded registry from FRAME-001, changing the manifest digest
    bound by development v2 writers without new runtime admission.
    Candidate canonical DDL is directly tested but stays outside the active
    manifest DDL selection until cutover. No schema exclusion is introduced.
    ADOPT-001
    atomically switches CLI/runtime, Store/writer/manifests and presets together.
    Small-change-v3 explicitly gains option guards while preserving assertion-only
    verification and no typed external results. Every preset includes the rubric,
    complete node responsibilities, inputs/results and
    explicit failure/decision routes. ADOPT-002 owns guidance and clearly separate
    normal, correction and justified-replan examples, not a later preset change.
    Examples never instruct real agents to manufacture failure or fill counters;
    deliberate fault injection belongs only to isolated tests.
    Implement FRAME, GROUP, TRACE and ADOPT in
    order. Keep incomplete successor capability outside public runtime admission
    until the integrated product is coherent. First-release history inspection
    is mandatory. Current specifications/assets change in their implementation
    tasks; no feature is implemented by this proposal. Actual release gating,
    installed qualification and external consumer adoption remain distinct.

## Alternatives and reasons

- In-place edits of the admitted DAG: obscure original topology and make identity,
  evidence applicability and late-request handling ambiguous.
- Preserve only the latest DAG or mark all former work failed: loses the very
  execution facts needed for retrospective analysis and misrepresents completed
  code changes.
- Automatically reuse old completion by node name/digest: confuses successful
  historical work with currently applicable evidence.
- Require a new session for every plan change: preserves cross-session history
  but does not provide the requested design-to-group lineage inside one task.
- Add leases, joins or native parallel nodes first: adds unrelated authority and
  compatibility work without enabling the immediate continuation requirement.
- Keep two product models for a migration window: duplicates admission, runtime,
  storage, presets and conformance paths; the owner selected direct cutover.
- Turn cursor ownership into external worker serialization: restricts existing
  native/Herdr collaboration without solving topology replacement.

## Consequences and acceptance

A task can explain which design produced each group and which work actually
completed, while current progression has one exact applicable topology. The
cost is additional identity, persistence, history paging and consumer binding
work, plus an intentional breaking cutover. Preserving history does not
preserve external artifact bytes or prove that recorded actions occurred.

Before implementation, obtain actual acceptance of this ADR and its
relationship to accepted decisions. Before delivery, verify canonical
source/runtime agreement, atomic activation/replan/base return, stale rejection
and exact replay, fresh process recovery, maximum-value historical paging,
no-rollback code continuity, archive retention, explicit destructive operations
and byte-identical legacy refusal using owned fixtures. Native/Herdr examples
may demonstrate protocol compliance without claiming real provider, adapter or
installed consumer success. The
[dossier](../todo/TODO-podway-continuation-groups.md) owns task-level
acceptance; the roadmap alone owns status. No acceptance is recorded by this
file's creation.
