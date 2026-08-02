# Podway Roadmap

This document owns adopted work, execution order, and current status. Candidate
work remains in [TODO](../todo/) until its goal and promotion conditions are clear.
Completed release history is preserved under [archive](archive/).

## Status definitions

- `Planned`: adopted but not started
- `In Progress`: implementation or verification is underway
- `In Review`: implementation is complete and acceptance is being reviewed
- `Completed`: explicit acceptance has passed
- `Deferred`: intentionally removed from the current release scope
- `Blocked`: cannot progress without an external decision or prerequisite

## REL11 — Podway v0.1.1 Release

| id | title | status | goal | references |
|---|---|---|---|---|
| `REL11001` | Reorganize public and contributor documentation | Completed | Publish a user-oriented README, contributor knowledge base, and single canonical asset tree. | [Contributor documentation](../README.md) |
| `REL11002` | Advance the product and contract identity | Planned | Update the product version and every version-bound release and contract input to v0.1.1. | [Release specification](../specs/operations/release-and-packaging.md) |
| `REL11003` | Pass the complete release gate | Planned | Run the authoritative clean-tree test gate for the exact release revision. | [Testing](../implementation-tips/testing.md) |
| `REL11004` | Build, qualify, and publish v0.1.1 | Planned | Produce the deterministic distribution, verify its metadata, tag the revision, and publish the release. | [Release workflow](../implementation-tips/release.md) |

Tasks are completed in table order. At most the first incomplete task may be `In
Progress`, `In Review`, or `Blocked`; later tasks remain `Planned`.
