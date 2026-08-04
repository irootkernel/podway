# ADR-0002: Permit One Active Stage Attempt

- Status: Superseded by [ADR-0015](0015-constrained-single-cursor-graph.md)
- Date: 2026-07-13

## Context

A daemon and queued writes permit concurrent callers and concurrent work across separate worktrees. They do not by themselves define parallel procedure branches, joins, partial returns, or branch-specific redo semantics.

The core product goal is a small omission-prevention model for one task.

## Decision

A running session has exactly one active stage attempt. Procedure stages are linearly ordered. Multiple humans or agents may perform external work concurrently and update different items, but Podway has one authoritative current stage.

No parallel stage group, join, arbitrary graph edge, or DAG scheduler is included.

## Consequences

Positive:

- `status` and `next` are unambiguous;
- return and redo are deterministic;
- queue preconditions remain simple;
- procedures are easy to author and review.

Negative:

- procedures with independent mandatory branches must represent them as items or sequential stages;
- adding parallel stages later would require new domain, JSON, CLI, and storage contracts.
