# ADR-0014: Use One Canonical Build Asset Tree

- Status: Accepted
- Date: 2026-08-02

## Context

Podway previously stored canonical schemas, executable specifications, and presets
under `docs/`, then copied them to repository-root mirrors consumed by Rust builds
and release packaging. That mixed human documentation with machine inputs and
created two visible copies of every asset.

The public contract manifest and release archive already use stable logical paths
such as `schemas/`, `spec/`, and `presets/`. Those external identifiers do not
require the repository source tree to use the same layout.

## Decision

Podway stores each build-consumed asset exactly once:

```text
assets/
  presets/
  schemas/
  specifications/
```

Rust compilation, verification, preset tooling, and packaging consume this tree
directly. Explicit mappings preserve the existing logical contract paths and
release archive paths. Podway has no generated asset mirrors or synchronization
step.

## Consequences

Positive:

- `docs/` contains only human-readable contributor knowledge and examples;
- contributors have one obvious edit location for every machine asset;
- tests cannot pass against a stale generated mirror;
- the project root remains compact.

Negative:

- build and packaging tools must map physical asset paths to stable logical paths;
- changing that mapping requires contract and archive tests.

The mapping must not alter public contract-manifest identities or archive layout
without an explicitly versioned compatibility decision.
