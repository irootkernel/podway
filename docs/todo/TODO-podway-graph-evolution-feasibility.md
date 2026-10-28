# Podway Graph Evolution Feasibility

## Status and authority

- Document state: `Candidate`
- Dossier type: research and design candidate
- Working label: PROOF
- Owning roadmap epic: none
- Target product release: undecided
- Repository scope: Podway only

This working label groups proposed work; it reserves no roadmap ID. This
candidate adopts no implementation, public interface, release or consumer change.
The [roadmap](../roadmap/README.md) alone owns adopted work and task status.

## 1. Context

Procedure v2 already permits external parallel work whose results are
recorded atomically. The unresolved graph hypothesis is executor-neutral
authority over in-flight units. The distinct continuation hypothesis concerns
plan revision, topology replacement and inspectable activation lineage within
one task. Neither is established by the proposed model itself.

The [graph research](references/graph-engineering-research.md) and
[continuation research](references/continuation-generations-research.md) preserve
the original alternatives, dated survey, decision ledgers, illustrative models
and risks. This candidate is the single owner of the S1-S7 experiment scope.

## 2. Goal

Determine, per exercised work shape, whether existing Procedure v2 plus
external durable coordination is sufficient. Where it is insufficient, identify
the smallest specifically motivated Podway capability; retain unsupported
hypotheses as unexercised rather than promising a successor implementation.

## 3. Non-goals

No production claims, leases, fan groups, joins, failed lifecycle,
continuation activation, new Procedure generation, storage migration, executor
adapter or Web host. No blanket promotion of the research program, no new timing
or throughput acceptance thresholds, and no live/provider or external-repository
operation authorized by this document.

## 4. Rough Scope

Two distinct hypotheses share one experiment-definition owner: coordination
that must survive executor substitution, and topology lineage within one task.
Evaluate each shape independently; neither hypothesis licenses the other.
Section numbers inside the S1-S6 protocol refer to the
[graph research notes](references/graph-engineering-research.md); section numbers
inside S7 refer to the [continuation research notes](references/continuation-generations-research.md).
The external landscape in those notes is dated research, not a verified current
executor capability list. Freeze actual executor versions and supported public
interfaces in the preregistration before running experiments.

### S1-S6: coordination comparison

For a selected coordination requirement, first determine whether the gap described
in section 3.3 survives comparison with an existing composition, including a
durable executor where relevant. A sufficient existing solution removes the
corresponding motivation for sections 8 through 17 within that tested scope.

#### Preregistration

Before the first run, and not revised after any result is seen, record the task
selection, the shapes it covers, and the classification of every measure as
gating or descriptive. Tasks are drawn from this repository's completed work
rather than invented for the experiment, and each must be expressible as a
Procedure the author would actually run from a shipped preset or a small authored
procedure. No task count is prescribed. The set must cover every shape whose
mechanisms the experiment intends to license. Select the relevant requirements,
arms, faults, observations and executor/build scope in advance. No selection must
cover all S1-S7 shapes. A sufficient existing solution may end an unnecessary
comparison early; record the tested scope and what remains unexercised.

#### Arms

Three comparison arms are available for each selected shape. A failing stateless
fan-out alone cannot motivate a Podway feature; relevant existing alternatives
must be addressed before making that claim. Conversely, a valid existing solution
may establish sufficiency without running more elaborate arms:

- **Arm A** is the shipped path described in section 3, driven by an executor
  that keeps no durable coordination state.
- **Arm B** is the same path with a durable executor of the checkpoint, replay,
  and resume class listed in section 7.2, where the executor owns coordination.
- **Arm C** is arm B with the minimum coordination metadata for the shape under
  test additionally recorded as ordinary items through `podway record --stdin`:
  the unit inventory, the claims, and the executor run identity in every shape;
  the declared effect of each unit in S2 and S3; the immutable input basis in S3;
  and the expansion source in S4. It approximates fan-group authority using
  shipped code only. It is a low-cost starting point when relevant to the selected
  question; it is not a mandatory first run or an implementation of fan groups.

Arm B must name the executor and its version, and should cover two executors of
different families. Otherwise its result is scoped to the one executor tested,
consistent with the snapshot rule stated in section 7.2.

#### Work shapes

Arms are run over each shape the preregistration covers:

- **S1 read-only fan-out:** every unit is a snapshot read.
- **S2 write-bearing fan-out:** at least one unit mutates the owning worktree.
- **S3 overlap:** a read must observe a fixed input while a write proceeds.
- **S4 expansion:** the unit count derives from an upstream recorded list.
- **S5 outcome combination:** units produce pass, fail, or inconclusive results
  that must combine into one group outcome.
- **S6 failure and return:** a required unit fails and work must return to an
  earlier placement.

S1 through S4 are constructible today with shipped code, S2 and S3 by having one
unit mutate a temporary worktree while others read it. Whether S5 and S6 can be
exercised without first building a join mechanism is unresolved. If they cannot,
they remain unexercised and the mechanisms they govern cannot be promoted on this
experiment's evidence.

#### Faults

Use the same selected faults across the compared arms for each selected shape:
an executor killed
before any result is recorded; an executor killed after some units finish but
before recording; a lost response on the record mutation; two executors started
against one task; and an executor replaced, or a human substituted for one unit,
on resume.

#### Gating invariants

These are binary and observable within the recorded state of the system under
test. A failure records an unmet requirement; its cause still needs evaluation
before it can motivate a capability. Podway does not observe external
processes or the filesystem, so no invariant below asserts anything about what an
executor actually did. Evaluate the applicable invariants under the selected
faults within the preregistered comparison scope:

- no work unit had two concurrent owners, meaning no two live claims on one unit
  coexisted at any instant;
- at most one result was accepted per work attempt and lease generation;
- no unit was reissued after an accepted terminal result;
- the system under test had an authoritative answer for what work remained after
  the fault, whichever component owned that answer;
- resume did not redo units whose results had already been accepted; and
- recovery is achievable using only documented read-only commands and declared
  mutations.

Additionally in S2 and S3:

- no two workspace-write leases coexisted at any instant; and
- every accepted result bound to the immutable input basis its unit declared, and
  no incompatible lease was issued for the duration of that binding.

Two of these are deliberately weaker than they might appear. Duplicate external
execution is not gated, because Podway cannot prevent or observe it; section 19.2
records it as a standing risk, and gating on it would make the experiment
unrunnable. The remaining-work invariant is functional rather than
Podway-specific: an executor that can authoritatively answer the question
satisfies it. Requiring Podway to be the answering authority would make arm B
fail by construction and would decide the comparison in advance.

#### Descriptive metrics

Recorded but never gating: which component owned the authoritative remaining-work
answer in each arm; whether duplicate external execution was detected, which
Podway cannot prevent; operator steps to recover; context duplication; model
cost; merge work; and wall clock. A descriptive metric may narrow or order scope
after a gating invariant has already admitted a mechanism, but may never admit
one on its own. No threshold may be introduced after data collection.

Which component owned the remaining-work answer is the datum that separates the
arms once more than one of them satisfies the functional invariant. The
substitution fault is what prices it: an answer that does not survive executor or
human substitution is executor-local, and that cost appears in the substitution
result rather than in a failed gate.

#### Evidence-to-scope rule

Apply this ordered decision independently to each mechanism's selected shape,
requirements and executor/build scope. Check evidence sufficiency for the
particular conclusion; do not require the entire possible matrix to support a
bounded conclusion.

1. Establish whether the preregistered fixtures, configurations and observations
   are valid and sufficient for that conclusion. Missing or invalid evidence
   cannot motivate a feature. Evidence sufficient to show that one existing
   composition works need not establish why every other arm failed.
2. If any applicable existing composition satisfies all selected gating
   requirements, classify the corresponding feature motivation as **Removed**
   for that scope. This takes precedence over failure in a more elaborate arm:
   Arm B passing and Arm C failing is Removed, never also Motivated. A valid
   Arm A or Arm C success can likewise establish sufficiency.
3. Otherwise, identify the specific unmet requirement and investigate whether
   failure reflects a real limitation of the evaluated existing compositions.
   A correctable fixture or adapter defect, unsupported executor configuration,
   missing relevant comparison or incomplete observation establishes no feature
   motivation. Describe the evidence limitation and withhold promotion; a run
   with inconclusive evidence is not automatically Motivated or Removed.
4. Only after relevant alternatives and explanations are addressed, classify
   the smallest capability closing a demonstrated gap as **Motivated**. Bind the
   conclusion to the failed requirement and tested scope, not an entire model.

A shape that was not exercised remains **Unexercised**. Do not transfer any
conclusion to another shape, executor or untested mechanism. These are prose
research outcomes, not a new machine-readable status model. A valid conclusion
cannot classify the same mechanism and scope as both Removed and Motivated.

| Observed situation | Interpretation for the selected scope |
|---|---|
| Arm B meets all selected requirements; Arm C fails | Removed: the existing composition suffices |
| Arm C meets all selected requirements | Removed: ordinary items plus the executor suffice |
| Arm A genuinely meets all selected requirements | Removed: no demonstrated gap requires richer machinery |
| Existing compositions fail because of a fixture or adapter defect | No motivation established; correct the defect before drawing that conclusion |
| Valid comparison demonstrates a requirement the evaluated existing compositions cannot meet | Only the smallest capability closing that gap may be Motivated |
| A relevant shape was not exercised or required observations are missing | Unexercised for an unrun shape; otherwise report the evidence limitation and withhold promotion |

| Shape | Mechanisms it can license |
|---|---|
| S1 | Executor-neutral unit inventory, claims, and readiness for reads: section 8.2 and the read half of section 10.2 |
| S2 | Workspace-write lease 10.1, effects 10.3, write ordering 10.4 |
| S3 | Read and write overlap whitelist 10.5, snapshot ownership 10.6 |
| S4 | Dynamic map expansion 10.7; ordered write maps require S2 and S4 together |
| S5 | Join and group outcome 11.1 through 11.3 |
| S6 | Rework and terminal failure 11.4 |

The rule applies to mechanisms whose absence produces an observable fault.
Section 10.4 states a static vetting rule, so its evidence is indirect: S2
order-dependent outcomes may motivate it only after the rule above establishes
their cause and a corresponding composition gap. Section 10.8 records a
non-decision and needs no evidence.

This experiment needs no new Podway code for S1 through S4. It is the cheapest
question in this document and the one the rest of the document depends on.

### S7: plan revision and topology replacement

Before adopting continuation implementation work, select a preregistered
`S7: plan revision and topology replacement` comparison against the implemented
foundations in section 5. Use realistic completed-task shapes and the same
selected user requirements to evaluate:

- Procedure v2 plus external planning and the shipped history-preserving
  state-aware start path: preserve running work by cancelling it, recording a
  successor-linked `superseded` disposition, archiving it and creating a new
  session;
- that existing lifecycle with an external durable executor owning plan and
  topology lineage; and
- the smallest disposable continuation-generation model that records the lineage
  in Podway.

The existing path is specified by
[rework and lifecycle](../specs/domain/rework-and-lifecycle.md#terminal-disposition-archival-and-reset-ownership)
and covered by the
[direct start replacement regression](../../crates/podway-engine/tests/int_v2dfc006_direct_execution.rs).
It preserves cross-session history. Same-session plan-to-topology bindings,
activation lineage and evidence applicability are potential residual needs to
test, not requirements already adopted. Explicit destructive replacement may be
compared separately where relevant; it cannot stand in for all shipped behavior.

Apply the evidence-sufficiency, existing-solution precedence and scope discipline
of the evidence-to-scope rule to S7's own comparisons. Its disposable continuation
model is not S1-S6 Arm C, and its success or failure alone cannot establish a
missing production capability. A sufficient existing lifecycle/composition may
end the selected investigation; a positive claim requires a demonstrated
residual requirement and the relevant alternatives addressed.

For the selected S7 requirements, choose the relevant scenarios before running:

- classification selects an unsuitable default execution path;
- planning chooses a different fixed template;
- planning produces a bounded custom topology;
- execution partially progresses and then returns to plan;
- replanning reuses the same group version;
- replanning appends and activates a different group version;
- an old mutation or fan report arrives after supersession;
- a foreground caller exits or is killed before and after activation, and the
  next invocation reopens the worktree Store and reconciles the original
  idempotency identity;
- the activation response is lost and reconciled by idempotency identity; and
- history remains inspectable while retired work cannot satisfy current
  progression.

### Research and implementation boundary

Use isolated fixtures and disposable owned worktrees with bounded child cleanup.
No comparison may use an active workspace or installed legacy runtime, access
credentials, run providers or introduce a resident process without the required
separate authority. S7 may evaluate a minimal disposable model; that model does
not become a production API or evidence that unexercised fan mechanisms work.

Retain each supported conclusion's original scope; explicitly report evidence
limitations where no conclusion is supported. A sufficient existing composition
settles the selected requirement in its favor. A negative demand result is a
valid research outcome.
Future implementation requires a separately adopted, bounded design; the fan
and continuation implementation conditions remain in the supporting notes.

## 5. Open Decisions

- Which completed tasks and authored Procedures represent each shape, and
  which exact durable executor builds can run the comparison through permitted
  interfaces? Results from one executor family retain that scope.
- Can S5 and S6 be exercised honestly without implementing the mechanism being
  tested? Otherwise keep them and their mechanisms unexercised.
- What minimum disposable S7 model and preregistered lineage/recovery criteria
  compare composition against Podway-owned generations without favoring one arm?
- Which inputs, process faults, state observations, bounds, cleanup receipts and
  result retention make the comparison reproducible?
- Which specific failed invariant, if any, justifies a future implementation
  candidate? The supporting notes retain its unresolved architectural decisions.

## 6. Roadmap Promotion Conditions

Before adopting a bounded feasibility epic, close its task selection,
executor capability evidence, preregistration, runnable matrix, faults,
observation methods and safe fixture lifecycle. Fix gating versus descriptive
measures before the first run; do not revise them after seeing results.

Before adopting any resulting feature, apply the per-shape evidence-to-scope
rule, record removed and unexercised mechanisms, close that feature's design and
compatibility decisions, obtain the required accepted ADRs and register its own
roadmap ownership. Research adoption cannot substitute for implementation
adoption. No release target is selected by these experiments.

## 7. References

- [TODO authority and lifecycle](README.md)
- [Graph engineering research](references/graph-engineering-research.md)
- [Continuation generations research](references/continuation-generations-research.md)
- [Procedure specification](../specs/domain/procedure-and-item-specification.md)
- [State transitions](../specs/domain/state-transitions.md)
- [Direct execution authority](../architecture-decision-records/0038-remove-the-daemon.md)
- [Real executor integration candidate](TODO-podway-executor-integration.md)
