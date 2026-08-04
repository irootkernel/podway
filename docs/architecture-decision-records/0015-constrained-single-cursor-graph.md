# ADR-0015: Permit a Constrained Single-Cursor Graph

- Status: Accepted
- Date: 2026-08-04
- Supersedes: [ADR-0002](0002-single-active-stage.md)

## Context

ADR-0002 deliberately restricted v1 procedures to a linear stage sequence so
that `status`, `next`, retry, and return behavior remained deterministic. Podway
v2 must additionally represent review decisions, declared rework, and
goal-assessment outcomes without becoming a parallel scheduler or arbitrary
workflow engine.

Linear order cannot represent those choices without moving the routing decision
into prose or external orchestration. Allowing a general graph, parallel
branches, joins, or executable conditions would break the product boundary and
substantially expand persistence and concurrency semantics.

## Decision

Podway v1 remains a linearly ordered stage contract with unchanged semantics.

Podway v2 may use a finite, declarative graph of action and decision placements
under all of these constraints:

- one worktree has one current session;
- a running session has exactly one active node attempt and one authoritative
  cursor;
- transitions are declared data, not expressions or executable hooks;
- decision options select one declared route;
- rework creates a fresh attempt and conservatively invalidates the affected
  trace suffix;
- vetting proves identity, reference, reachability, terminal-route, cycle,
  dominance, and resource-budget rules before normal admission;
- there are no parallel branches, joins, background graph executions, plugins,
  or procedure-defined commands.

Normal release builds must not admit v2 sessions until the complete v2
acceptance gate passes. A development-only unlock may admit v2 only in isolated,
disposable state and must be absent from release artifacts.

## Consequences

Positive:

- decisions and rework are represented explicitly and replayably;
- `status` and `next` remain unambiguous;
- persistence and mutations retain the one-writer, one-active-attempt model;
- v1 procedures and sessions remain unchanged.

Negative:

- authors must satisfy stricter graph vetting and digest confirmation;
- graph history, invalidation, recovery, and projections add contract and
  storage surface;
- workflows requiring parallel execution or joins remain outside Podway.
