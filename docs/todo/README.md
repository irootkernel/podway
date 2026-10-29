# TODO and Adopted Design Dossiers

This directory owns planning documents for work that is not complete. Every
`TODO-*.md` file declares either `Candidate` or `Adopted`. Completed work does not
remain in this directory.

## Candidate documents

A candidate has not entered the active roadmap. It contains:

1. Context
2. Goal
3. Non-goals
4. Rough Scope
5. Open Decisions
6. Roadmap Promotion Conditions
7. References

A candidate does not contain task status, implementation order, completion
evidence, or normative product behavior. Promotion requires resolving its
material decisions and registering the adopted work in the active roadmap.

### Research candidates

A candidate whose purpose is to preserve investigation results may additionally
contain a research baseline date, an external landscape survey, a ledger of
preferred and rejected directions, illustrative non-contractual sketches, risks,
and experiment questions. It declares `Dossier type: research and design
candidate` in its status block.

The additional material is subject to the same authority limit as the rest of the
document. A preferred direction is a record of what an investigation favored, not
a decision the product has taken, and an illustrative sketch must be marked
non-contractual where it appears. External comparisons are snapshots and must
carry the date on which they were reviewed.

A research candidate must still satisfy the seven-part structure above, and its
promotion conditions must state which of its recorded preferences require an
accepted ADR before they bind anything.

## Adopted design dossiers

An adopted dossier is the authoritative, decision-complete implementation plan
for one unfinished roadmap epic or for one release program composed of a closed
set of related epics. It contains:

1. Status and authority
2. Verified context and evidence
3. Goals, non-goals, and scope
4. Accepted design decisions and interfaces
5. Failure handling and compatibility boundaries
6. Roadmap ownership, dependencies, and traceability
7. Verification and release acceptance
8. References

The active roadmap remains the sole owner of task order and status. A dossier may
map requirements to roadmap task IDs, but it must not maintain a competing status
table. Specifications, accepted ADRs, and canonical machine assets remain the
sources of truth for implemented behavior; a dossier records intended changes
until the owning task updates those sources.

A release-program dossier must name its release target, enumerate every owning
epic, define their dependency graph, and map each epic's tasks independently. It
must not use the release-program identifier as a synthetic epic or task prefix.

## Lifecycle

- Keep an adopted dossier here while any task in its owning epic is incomplete.
- When only part of a candidate is adopted, promote only the selected findings
  into their actual roadmap tasks. Retain unselected findings under a continuing
  candidate owner; extracting a bounded adopted dossier can leave the original
  candidate for the remainder. Do not implicitly adopt or resolve the rest.
- Before converting or deleting a candidate or adopted dossier, preserve the
  full details of every still-open DF entry in its continuing candidate or
  independent register entry and update all references. Each Open entry keeps
  one detailed owner, including its source, rationale and revisit condition;
  duplicate detail tables and a second status authority are unnecessary.
  Promotion closes the selected deferral, not implementation or acceptance;
  the roadmap alone owns those states. For example, adopting DF-013 and DF-014
  from TYPED leaves its other eleven entries Open under their continuing owner.
- Update the dossier before implementation when a material design decision
  changes. During implementation, promote every lasting decision and behavior to
  its proper durable authority: accepted ADRs, canonical machine assets,
  specifications, architecture, implementation tips, examples, and roadmap
  evidence as applicable.
- After the owner accepts the exact closeout candidate and before marking the
  epic complete, replace every roadmap and documentation link
  to the dossier with the appropriate durable authority, remove the dossier from
  this index, and delete its `TODO-*.md` file. No completed behavior, rationale,
  operational tip, compatibility boundary, or verification claim may exist only
  in the deleted dossier.
- Do not move a completed TODO dossier into `docs/roadmap/archive/`. That directory
  is reserved for compacting historical roadmap records when the active roadmap
  becomes too large; it is not a TODO completion destination.
- Do not bind executable evidence to a temporary TODO path. Do not leave generated
  reports, logs, mutable qualification receipts, or temporary plans here.

## Current adopted dossiers

- [CALLS agent workflow guidance](TODO-podway-calls-guidance.md) — the execution
  dossier for the [CALLS epic](../roadmap/README.md#calls-agent-workflow-guidance).

## Related candidates

These seven documents group possible epic scopes. Their working labels reserve
no roadmap identifiers and establish no task order or implementation commitment.
Each remains `Candidate`; adoption requires closing its decisions and registering
bounded work in the active roadmap. Existing-interface guidance is adopted as
CALLS; optional interaction semantics remain a continuing candidate.

Select work from a concrete product need and each finding's revisit condition.
These groupings are not seven sequential implementation commitments: TYPED is
not a prerequisite for INTEG, and independent improvements do not wait for the
entire PROOF program. A selected scope carries its applicable correctness and
verification obligations; grouping does not waive them.

| Working label | Candidate | Proposed outcome | Source feedback |
|---|---|---|---|
| Future interaction semantics | [Agent workflow simplification](TODO-podway-agent-workflow-simplification.md) | Assess combined operations or returned observations against a concrete caller need | Unselected interaction research |
| TYPED | [Typed boundary ownership](TODO-podway-typed-boundary-ownership.md) | Explicit owners for existing values, bounds, extraction and validation reuse | DF-002, DF-003, DF-005, DF-011, DF-012, DF-013, DF-014, DF-016, DF-017, DF-022, DF-024, DF-025, DF-027 |
| MAINT | [Maintenance and recovery safety](TODO-podway-maintenance-recovery-safety.md) | Bounded recovery policy, confirmation and fault verification | DF-029, DF-030, DF-031, DF-032, DF-033, DF-034, DF-036, DF-039, DF-040, DF-041, DF-042, DF-043 |
| CNTRT | [Public contract consistency](TODO-podway-public-contract-consistency.md) | Diagnostic, command/schema and known-answer consistency | DF-004, DF-008, DF-046 |
| INTEG | [Executor integration](TODO-podway-executor-integration.md) | Real adapter and selected-build qualification beyond the local foundation | Existing real executor candidate |
| PROOF | [Graph evolution feasibility](TODO-podway-graph-evolution-feasibility.md) | Per-shape evidence for coordination and topology-lineage needs | Existing graph and continuation research |
| WEBUI | [Operations console](TODO-podway-operations-console.md) | Foreground human operations after the graph direction is settled | Existing Web console candidate |

The [deferred-feedback register](../deferred-feedback/README.md) retains each
feedback ID and source. Candidate-owned details live in the linked candidate;
grouping alone does not resolve a deferral or populate its promoted task.
DF-026, DF-028, DF-045 and DF-050 retain their full independent entries there.

## Supporting research notes

The `references/` documents preserve dated research, rejected alternatives,
illustrative models, risks and unresolved implementation questions. They are
non-normative supporting material for PROOF, not additional epic candidates,
adopted dossiers or competing experiment plans. Future designs must revalidate
external comparisons and obtain accepted authority for any product change.

- [Graph engineering research](references/graph-engineering-research.md)
- [Continuation generations research](references/continuation-generations-research.md)
