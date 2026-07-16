# ADR-0010: Integrate External Tools Through Generic CLI and JSON

- Status: Accepted
- Date: 2026-07-13

## Context

Product-specific packages and adapters increase coupling and can create competing coordination authority. Podway's procedure model is generic and sufficient for humans, scripts, and AI agents.

## Decision

Podway ships no Dolgorae-specific package and no Orca adapter. External tools query `status` and `next`, update items, and invoke transitions through the public CLI and versioned JSON contract.

External worker completion never automatically completes a Podway stage.

## Consequences

Positive:

- smaller release and clearer authority;
- one stable integration surface;
- no product-specific concepts in the core.

Negative:

- external systems must implement a small amount of CLI/JSON glue;
- no turnkey adapter-specific experience ships initially.
