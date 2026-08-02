# ADR-0008: Use Relational Current State, Not Event Sourcing

- Status: Accepted
- Date: 2026-07-13

## Context

Event sourcing would support replay and audit, but Podway does not need long-term history. It would increase migration, projection, integrity, and tooling complexity.

## Decision

SQLite relational tables are authoritative. Mutations update current state atomically. A bounded operational journal exists only for diagnostics and cannot reconstruct the product state by itself.

## Consequences

Positive:

- simpler queries, reset, and migrations;
- direct invariant enforcement;
- reduced implementation scope.

Negative:

- no event replay or historical export;
- corruption recovery is fail-closed plus destructive reset rather than forensic rebuild.
