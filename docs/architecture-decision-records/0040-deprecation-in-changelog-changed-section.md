# ADR-0040: Record Generation Deprecation in the CHANGELOG Changed Section

- Status: Accepted
- Acceptance date: 2026-10-22
- Date: 2026-10-24
- Acceptance owner: the owner, deciding during GUIDE delivery planning; recorded by GUIDE-011
- Supersedes in part: [ADR-0039](0039-opt-in-contract-generations.md)
- Delivery owner: [GUIDE](../roadmap/README.md#guide-agent-guidance-and-contract-generation-2)

## Context

ADR-0039 decision 7 declares the generation-1 deprecation in ADR-0039 itself and
the specifications, the CHANGELOG `Deprecated` section, `contract-generations.json`,
and help text. The Aquarium release-notes inspector that gates Podway stable
releases accepts only the `Added`, `Changed`, `Fixed` and `Removed` sections; a
`Deprecated` section is not admitted.

## Decision

The generation-1 deprecation is recorded as a concise entry in the release
CHANGELOG's `### Changed` section instead of a `Deprecated` section. This
supersedes in part only the "the CHANGELOG `Deprecated` section" bullet of
ADR-0039 decision 7. Everything else in that decision stands: the deprecation is
still declared in ADR-0039 and the specifications, in
`contract-generations.json`, and in help text, and no runtime warning is
emitted.

## Consequences

Positive:

- The deprecation reaches consumers through the release-notes gate that every
  stable release already passes.

Negative:

- None identified; only the CHANGELOG section placement changes.
