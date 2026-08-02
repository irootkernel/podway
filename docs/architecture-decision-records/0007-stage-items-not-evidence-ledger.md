# ADR-0007: Model Typed Stage Items, Not a General Evidence Ledger

- Status: Accepted
- Date: 2026-07-13

## Context

A generic evidence envelope with issuers, claims, revocation, expiration, schemas, and lifecycle events is appropriate for audit-oriented systems but exceeds the need to prevent omitted steps in one current task.

## Decision

Stages declare typed items: `confirm`, `text`, `choice`, `integer`, `list`, and `artifact`. Completion checks current-attempt item satisfaction and blockers. Items do not have independent revocation, issuer, or cross-session lifecycle.

## Consequences

Positive:

- simple procedure authoring and CLI commands;
- direct `next` guidance;
- less storage and security complexity;
- clear attempt ownership.

Negative:

- Podway does not provide strong provenance or evidence queries;
- integrations must map their state into ordinary item updates.
