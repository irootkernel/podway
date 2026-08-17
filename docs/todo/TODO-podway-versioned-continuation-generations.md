# Podway Versioned Continuation Generations

## Status and authority

- Document state: `Candidate`
- Dossier type: research and design candidate
- Owning roadmap epic: none
- Target product release: undecided
- Candidate contract target: a possible successor to `podway.procedure/v2`
- Repository scope: Podway only
- Research and planning baseline: August 18, 2026
- Related candidate: [Podway Graph Engineering Evolution](TODO-podway-graph-engineering-evolution.md)

This document preserves a candidate model for changing the execution topology of
one task after planning without deleting earlier Procedure declarations or runtime
history. It is not an adopted design dossier, accepted architecture decision,
roadmap commitment, or specification of implemented behavior. No property,
command, schema, storage shape, preset, or lifecycle described below exists merely
because it appears here.

The current product authorities remain unchanged. In particular, each Procedure
v2 session retains one immutable admitted snapshot and moves only through routes
declared in that snapshot. Promotion of any part of this candidate requires the
experiments, decisions, ADRs, adopted dossier, and roadmap ownership described
below.

## 1. Context

Task classification can select an initial Procedure or preset before enough work
has been done to know the best execution topology. A later planning attempt may
show that the initial sequence is unsuitable, that a different preset-like
execution shape is preferable, or that the task needs a purpose-built set of
actions, decisions, checks, and rework routes.

The current prepared-session lifecycle permits replacement before execution, but
replacement creates a new session. Once execution begins, changing the Procedure
requires destructive replacement. That preserves the immutability of one
Procedure snapshot but cannot retain classification, planning, execution, and
replanning as one inspectable session lineage.

A useful successor may instead retain an immutable base Procedure while allowing
an explicitly authorized planning placement to bind one immutable continuation at
a time. Replanning would create a fresh plan attempt and either reuse an existing
continuation definition or append a new one. Earlier definitions, activations,
attempts, and results would remain historical, while only the current activation
would participate in progression.

This idea is related to the composite fan group proposed by the graph-engineering
candidate, but it solves a different problem. A fan group changes the bounded work
frontier inside one active execution topology. A continuation generation changes
the topology selected after a plan.

## 2. Goal

The goal is to determine whether Podway should support bounded, append-only,
plan-bound execution generations while preserving its local procedure-guard role,
single authoritative cursor, sole-writer mutation order, explicit routing,
idempotency, crash recovery, and fail-closed stale-state handling.

A successful design would make the following facts inspectable:

- the immutable base Procedure and its initial classified continuation;
- each planning attempt and the plan revision or digest it produced;
- every immutable execution-group version proposed for that plan;
- which group version each plan activation selected;
- why a prior activation was superseded;
- which attempts and results belong to each activation; and
- which one activation, if any, currently governs progression.

The design should preserve history without allowing retired work to satisfy
current readiness or accept late mutations.

## 3. Non-goals

This candidate does not propose:

- mutating or deleting an admitted node or execution-group definition in place;
- arbitrary edge insertion at any active node;
- an unbounded executor-generated workflow;
- automatic graph activation from model output or worker completion;
- allowing fan workers, leases, or reports to change outer topology;
- multiple authoritative outer cursors or parallel task sessions in one worktree;
- treating historical evidence as current merely because it remains stored;
- turning `evidence.read` into an arbitrary history browser or evidence archive;
- executing models, commands, tools, Git mutations, or snapshot creation in
  Podway;
- judging whether a plan or execution group is semantically correct for the task;
- changing the v0.3.0 adopted dossiers or reinterpreting existing Procedure v2
  sessions; or
- adding a roadmap epic, release target, public command, or schema before this
  candidate is promoted through the repository's authority process.

## 4. Rough scope and conceptual model

### 4.1 Layered authority

The candidate separates four immutable or append-only concepts:

1. **Base Procedure snapshot.** The admitted task skeleton containing stable
   lifecycle placements such as classification, planning, assessment, and
   closeout.
2. **Continuation anchor.** A graph placement that is explicitly authorized to
   select a versioned execution continuation after its planning work is complete.
3. **Execution-group version.** One immutable, canonical, content-digested group
   of action, decision, goal-assessment, and eventually fan placements.
4. **Activation binding.** An append-only record binding one plan attempt or plan
   revision to one execution-group version.

An execution-group version is never changed from active to inactive by editing the
stored definition. Currentness is derived from the activation lineage. A later
activation supersedes an earlier activation, while both activation records and
their group definitions remain immutable history.

```text
immutable base Procedure
          |
          v
   plan anchor attempt 1
          |
          +---- activation 1 ----> execution group G1
          |                              |
          |                        rework to plan
          v                              |
   plan anchor attempt 2 <---------------+
          |
          +---- activation 2 ----> execution group G2
                                      or G1 reused
```

The runtime trace remains append-only and single-cursor:

```text
plan#1 -> G1/action#1 -> G1/fan#1 -> replan
       -> plan#2 -> G2/action#1 -> ...
```

### 4.2 Anchor ownership

The ability to attach a continuation belongs conceptually to a graph placement,
not a reusable node definition. One definition may be used by several placements,
while only a particular placement may own the routing and lifecycle authority to
activate a continuation.

The exact authoring property is open. A later design may use a shape conceptually
similar to the following, but this fragment is not contractual:

```yaml
- id: plan
  use: planning
  continuation:
    mode: versioned-generation
    rework_target: true
```

The first supported shape should evaluate whether one continuation anchor per
session is sufficient. Multiple anchors, nested continuation generations, and
cross-anchor references remain out of initial scope unless concrete workflows
prove they are necessary.

### 4.3 Planning and activation lifecycle

Task classification may provide a default execution-group version. That version
is a recorded candidate, not evidence that execution has started. After the plan
anchor is satisfied, an external actor may:

- activate the default group;
- activate a previously admitted group version; or
- propose a new group version and activate it after complete validation and any
  required explicit authorization.

Activation must be one atomic, idempotent, fully fenced mutation. It binds the
base session, exact plan attempt or revision, group version and digest, and the
expected prior activation. It creates the first attempt at the group's declared
entry only after the complete group passes authoring, topology, resource, and
compatibility validation.

If execution reveals a planning problem, an explicit rework returns the outer
cursor to the continuation anchor and creates a fresh plan attempt. The prior
execution attempt is abandoned with a structured replan cause. The prior
activation becomes historical, and later mutations fenced to it fail as stale.
The new plan may reuse its group definition through a new activation or append and
activate another immutable group version.

### 4.4 Relationship to fan groups

A continuation generation and a fan group occupy different levels:

```text
session
  base Procedure and outer cursor
    current execution generation
      action or decision placement
      composite fan-group placement
        bounded internal work frontier
```

The division of responsibility should be:

- a change in the number of homogeneous files, packages, findings, or other work
  targets uses a fan group's bounded dynamic map over immutable recorded input;
- a change in node kinds, dependencies, effects, operation contracts, verification
  stages, or rework topology uses a new execution-group version; and
- an execution generation may contain fan placements only after the fan-group
  contract itself is adopted.

The fan-group parent attempt remains subordinate to the one outer cursor. Claims,
leases, work-unit attempts, reports, bases, and joins additionally bind the exact
continuation activation. A report from a superseded activation must be rejected
even if its fan-group-local lease generation otherwise appears current.

Replanning while fan work is outstanding requires one closed transition. A later
design must decide whether it atomically abandons every live work unit and lease,
requires an explicit fan close first, or forbids replan until a declared group
outcome is reached. Silent survival of live leases across activation change is not
acceptable.

### 4.5 Core invariants

Any promoted design should preserve at least these invariants:

- the base Procedure snapshot is immutable;
- each execution-group version is immutable and content-digested;
- activation and supersession history is append-only;
- at most one continuation activation is current;
- one running session retains one authoritative outer cursor and one active outer
  attempt;
- group activation occurs only at an admitted continuation anchor;
- the complete group is statically validated before activation;
- every activation and runtime mutation is bounded, atomic, idempotent, ordered,
  and fenced by exact identities and revisions;
- stale reports, item updates, decisions, joins, and lease renewals from a prior
  activation cannot affect current state;
- retired attempts and values remain inspectable but do not satisfy current
  progression automatically;
- Podway validates structure and formal readiness, not semantic plan quality; and
- every collection, generation count, node count, history projection, stored
  value, and public response remains explicitly bounded.

### 4.6 History and current evidence

Current progression and historical inspection remain separate concerns:

- `session.next` and `session.observe` should expose only the current activation's
  actionable state;
- verbose status may expose a bounded activation lineage and attempt summaries;
- existing current-evidence paging must not become an arbitrary stale-history
  browser; and
- if a fresh plan must consume an earlier execution failure, the replan mutation
  should create a bounded, explicit feedback record bound to the prior activation
  rather than silently making its stale items current.

Whether a separate bounded history query is necessary remains open. Retaining
history does not by itself require Podway to return every historical value through
one observation or to become a long-term evidence archive.

### 4.7 Presets and execution-group templates

The three current built-in presets are complete Procedures with their own entry,
goal, assessment, rework, and terminal semantics. They are not directly spliceable
as mid-session execution groups.

A successor may need two distinct catalogs:

- complete Procedure presets for current Procedure v2 sessions; and
- bounded execution-group templates that satisfy the continuation-anchor entry,
  exit, evidence, and rework interface.

This candidate does not add a template catalog or change the identity, digest, or
selection boundary of `small-change-v2`, `bug-fix-v2`, or `sw-dev-v2`. The exact
relationship between a future successor's presets and group templates is open.

## 5. Interaction with current TODO dossiers

### 5.1 Bounded evidence scale and read-back

[Podway Bounded Evidence Scale and Read-back](TODO-podway-evidence-scaling.md)
provides useful current-evidence paging and response-budget foundations while
preserving one session, cursor, and active attempt. A generation design must not
widen its `evidence.read` route into arbitrary stale history. It needs separate
bounds for generation count, total retained attempts and work units, lineage
projections, and any historical-value query.

Only the active generation should contribute complete actionable content to
`next` and `observe`. Retired generation summaries require independent pagination
or truncation rules so accumulated history cannot break the 1 MiB frame contract.

### 5.2 External check result typing

[Podway External Check Result Typing](TODO-podway-assurance-typing.md) is largely
compatible. A `check_result` remains an attempt-local, structurally bound external
result whose operation and input basis belong to an immutable group definition and
activation. A successor may extend its runtime binding with continuation,
fan-group, work-unit, map-instance, or lease identity without changing its honest
trust boundary or turning it into a separate evidence ledger.

### 5.3 Typed guards and diagnostics

[Podway Typed Guards and Authoring Diagnostics](TODO-podway-typed-guards.md)
explicitly excludes dynamic routes from Procedure v2 and statically validates
guard sources, dominance, selection, and freshness. Those rules remain valid
inside one immutable execution-group version.

Cross-generation guards should be forbidden in the first design. Historical
results needed by a new plan or group should cross the boundary only through an
explicit typed feedback or import contract. Group activation is a new successor
mutation and must not be disguised as a Procedure v2 decision guard.

### 5.4 Reference Procedures and authoring guidance

[Podway Reference Procedures and Authoring Guidance](TODO-podway-reference-procedures.md)
should complete before implementation of this candidate. It gives Procedure v2 a
stable reference baseline, supplies an actual planning and rework workflow for
comparison, and completes the evidence, result, guard, and authoring foundations a
successor can reuse.

The `sw-dev-v2` planning placement is a useful experiment source, but V2REF's
adopted Procedure graphs, preset identities, digests, and v0.3.0 scope must not be
changed to anticipate this candidate.

## 6. Compatibility and sequencing direction

Procedure v2 is a closed released contract built around one immutable admitted
snapshot. Versioned continuation generations change admission-time topology
proofs, runtime identity, persistence, observation, stale fencing, and history.
They must not be added to v2 by implication.

The preferred sequencing for investigation is:

```text
V2SCL -> V2AST -> V2GRD -> V2REF
                              |
                              v
      continuation-generation experiment and design closure
                              |
                              v
            accepted ADR and adopted design dossier
                              |
                              v
       bounded continuation-generation implementation epic
                              |
                              v
          separately motivated fan-group implementation
```

This is a research preference, not roadmap authority. The eventual architecture
decision must choose direct cutover, bounded coexistence, or another explicit
transition policy consistent with ADR-0019. Existing Procedure v2 documents,
snapshots, sessions, results, and errors must remain unchanged unless a later
accepted ADR explicitly chooses and specifies a breaking transition.

A bounded continuation-generation implementation may be one epic only if it
excludes fan claims, leases, dynamic maps, joins, and executor-owned snapshot
coordination. Combining those mechanisms would be release-program scale and must
be decomposed according to independently demonstrated workflow gaps.

## 7. Open decisions

The following questions prevent adoption:

1. Is a continuation anchor a placement property, a distinct placement kind, or a
   composite subprocedure boundary?
2. Is one anchor per session sufficient, and are nested generations forbidden?
3. What immutable identity binds base Procedure, plan revision, group version, and
   activation?
4. Does every successful replan create a new logical activation even when it
   reuses an identical group digest?
5. What is the exact atomic transition for proposal, validation, activation,
   supersession, and entry-attempt creation?
6. Which actions require explicit human authorization rather than ordinary
   progression authority?
7. What entry, success, replan, failure, and terminal ports must every group
   declare?
8. How are node and item identifiers namespaced across group versions?
9. Which references may cross the base/group boundary, and is every
   cross-generation progression reference initially forbidden?
10. What structured feedback from a superseded activation is available to a fresh
    plan attempt?
11. How are goal revisions related to plan revisions and group activations?
12. What hard maxima apply to group versions, activations, nodes per group, total
    session nodes, attempts, fan work units, and retained history?
13. What projection and pagination contracts keep current and historical reads
    below public frame limits?
14. What happens to live fan claims and leases when execution returns to plan?
15. Can a group be proposed by any caller, or must proposal and activation be
    separate roles or commands?
16. How are externally authored custom groups digest-confirmed and reviewed before
    activation?
17. Are execution-group templates a new catalog, part of successor presets, or
    worktree-local authored documents only?
18. Can Procedure v2 and a successor coexist, or does the repository require a
    direct cutover after a migration window?
19. What storage generation, downgrade behavior, and legacy-state recovery policy
    preserve existing v2 state?
20. Which CLI/JSON result families, stable error codes, observation fields, and
    recovery recipes are required?

## 8. Experiment and roadmap promotion conditions

Before an implementation dossier or roadmap epic is adopted, run a preregistered
`S7: plan revision and topology replacement` experiment after V2REF. It should use
realistic completed-task shapes and compare at least:

- Procedure v2 plus external planning and destructive session replacement;
- Procedure v2 plus an external durable executor that owns plan and topology
  lineage; and
- the smallest disposable continuation-generation model that records the lineage
  in Podway.

The experiment should cover:

- classification selects an unsuitable default execution path;
- planning chooses a different fixed template;
- planning produces a bounded custom topology;
- execution partially progresses and then returns to plan;
- replanning reuses the same group version;
- replanning appends and activates a different group version;
- an old mutation or fan report arrives after supersession;
- the daemon restarts before and after activation;
- the activation response is lost and reconciled by idempotency identity; and
- history remains inspectable while retired work cannot satisfy current
  progression.

Promotion requires all of the following:

1. At least one concrete workflow demonstrates a failure that Procedure v2 plus a
   durable external executor cannot adequately resolve under the preregistered
   criteria.
2. The evidence-to-scope rule in the graph-engineering candidate admits only the
   mechanisms motivated by that failure.
3. The layered relationship between base Procedure, continuation generation, and
   fan group is decision-complete.
4. Identity, transition, stale fencing, replan, evidence, history, bounds,
   persistence, recovery, and compatibility decisions are closed.
5. Every affected accepted decision is changed through a named superseding,
   amending, or extending ADR.
6. One adopted dossier owns a bounded implementation scope and the active roadmap
   registers its epic or release-program dependencies after V2REF.
7. The target release and applicable complete gate are selected according to
   repository policy.

If the durable-executor comparison satisfies the required workflow without Podway
owning continuation generations, this candidate should remain unadopted or be
narrowed to the smallest independently motivated mechanism.

## 9. Risks

- Dynamic topology may turn Podway from a procedure guard into an arbitrary
  workflow engine.
- A generated group may be structurally valid but semantically unrelated to the
  plan.
- Unbounded generations and fan instances may make local storage and projections
  grow beyond current assumptions.
- Cross-generation references may accidentally make stale work appear current.
- Replanning during external writes may leave the worktree in a state that the new
  plan does not understand, even when Podway's own state is consistent.
- Reusing a group definition may conceal a materially different plan unless the
  activation binds both identities explicitly.
- Automatic activation may grant a transient model context more authority than the
  user intended.
- Multiple template catalogs may confuse Procedure selection and compatibility.
- A successor model may duplicate v2 parsing, domain, persistence, protocol, and
  test paths during coexistence.
- History retention may be mistaken for semantic evidence preservation or an
  evidence archive.

## 10. References

- [TODO and Adopted Design Dossiers](README.md)
- [Podway Graph Engineering Evolution](TODO-podway-graph-engineering-evolution.md)
- [Podway Bounded Evidence Scale and Read-back](TODO-podway-evidence-scaling.md)
- [Podway External Check Result Typing](TODO-podway-assurance-typing.md)
- [Podway Typed Guards and Authoring Diagnostics](TODO-podway-typed-guards.md)
- [Podway Reference Procedures and Authoring Guidance](TODO-podway-reference-procedures.md)
- [ADR-0017: Permit Single-Cursor Convergence](../architecture-decision-records/0017-single-cursor-convergence.md)
- [ADR-0019: Make Procedure v2 the Only Product Model](../architecture-decision-records/0019-procedure-v2-only-product.md)
- [ADR-0021: Separate Session Preparation from Execution](../architecture-decision-records/0021-separate-session-preparation-from-execution.md)
- [Procedure and Item Specification](../specs/domain/procedure-and-item-specification.md)
- [Rework and Lifecycle](../specs/domain/rework-and-lifecycle.md)
- [State Transitions](../specs/domain/state-transitions.md)
- [Procedure Evidence and Reference Quality roadmap](../roadmap/README.md#procedure-evidence-and-reference-quality)
