# ADR-0016: Use Recorded Items for Workflow Memory

- Status: Accepted
- Date: 2026-08-04
- Extends: [ADR-0007](0007-stage-items-not-evidence-ledger.md)

## Context

ADR-0007 models work results as typed, attempt-local stage items rather than a
general evidence ledger. Podway v2 needs later actions and decisions to read
prior recorded work, and it needs durable records for routing decisions,
declared rework, goal revisions, and criterion assessments.

An earlier v2 design considered separate typed producer outputs connected to
consumer input slots. That would add output schemas, revisions, bindings,
mutation commands, canonicalization, and storage even though Podway cannot
validate the semantic truth of external data or interpret external schemas.

## Decision

Podway v2 extends the existing item model instead of adding typed producer
outputs:

- an action records work only through its declared typed items;
- an evidence reference identifies a prior terminal placement and may select a
  bounded subset of its recorded item IDs for read-back;
- the digest attests to the source attempt's complete recorded item values,
  while selection limits only what is presented to the consumer;
- item type and presence are machine-checked, but semantic fitness remains the
  external actor's judgment;
- routing decisions, rework transitions, goal revisions, criterion results,
  and goal assessments are immutable session-scoped records with bounded
  attribution and reasons;
- stale or invalidated records remain inspectable but cannot satisfy current
  progression;
- records expire with the current task session and have no cross-session
  identity or lifecycle.

Typed producer outputs and a general evidence ledger are rejected. Podway adds
no issuer authority, signatures, revocation, expiration, artifact bytes,
external schema execution, or factual verification.

## Consequences

Positive:

- v2 reuses the established item contracts and mutation model;
- read-back, decisions, rework, and goals remain bounded and attributable;
- Podway can explain its recorded path without claiming semantic truth;
- storage and authoring remain smaller than a separate output-binding system.

Negative:

- producer and consumer semantic compatibility is not statically typed;
- integrations requiring stronger provenance or authorization must provide it
  outside Podway;
- conservative trace-suffix invalidation may discard otherwise reusable work.
