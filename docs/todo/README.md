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
- Update the dossier before implementation when a material design decision
  changes. During implementation, promote every lasting decision and behavior to
  its proper durable authority: accepted ADRs, canonical machine assets,
  specifications, architecture, implementation tips, examples, and roadmap
  evidence as applicable.
- Before marking the epic complete, replace every roadmap and documentation link
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

- [Podway Bounded Evidence Scale and Read-back](TODO-podway-evidence-scaling.md)
  owns `V2SCL`.
- [Podway External Check Result Typing](TODO-podway-assurance-typing.md)
  owns `V2AST`.
- [Podway Typed Guards and Authoring Diagnostics](TODO-podway-typed-guards.md)
  owns `V2GRD`.
- [Podway Reference Procedures and Authoring Guidance](TODO-podway-reference-procedures.md)
  owns `V2REF`.
