# TODO and Adopted Design Dossiers

This directory owns planning documents for work that is not complete. Every
`TODO-*.md` file declares one of two states: `Candidate` or `Adopted`.

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

## Adopted design dossiers

An adopted dossier is the authoritative, decision-complete implementation plan
for one unfinished roadmap epic. It contains:

1. Status and authority
2. Verified context and evidence
3. Goals, non-goals, and scope
4. Accepted design decisions and interfaces
5. Failure handling and compatibility boundaries
6. Roadmap traceability
7. Verification and release acceptance
8. References

The active roadmap remains the sole owner of task order and status. A dossier may
map requirements to roadmap task IDs, but it must not maintain a competing status
table. Specifications, accepted ADRs, and canonical machine assets remain the
sources of truth for implemented behavior; a dossier records intended changes
until the owning task updates those sources.

## Lifecycle

- Keep an adopted dossier here while any task in its owning epic is incomplete.
- Update the dossier before implementation when a material design decision
  changes, and update the roadmap or specifications in the same change when their
  authority is affected.
- When the epic is complete, move the dossier into `docs/roadmap/archive/`, update
  its status to historical, and repair every inbound link.
- Do not leave generated reports, logs, mutable qualification receipts, or
  temporary plans in this directory.

Current adopted dossier:

- [Podway v0.1.2 contract recovery and native release](TODO-podway-v0.1.2-contract-recovery.md)
