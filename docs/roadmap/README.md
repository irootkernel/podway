# Podway Roadmap

This document owns adopted work, execution order, and current status. Candidate
work and adopted design dossiers live in [TODO](../todo/) under their distinct
lifecycle rules. Completed release history is preserved under [archive](archive/),
including the [v0.1.1 release roadmap](archive/v0.1.1.md).

## Status definitions

- `Planned`: adopted but not started
- `In Progress`: implementation or verification is underway
- `In Review`: implementation is complete and acceptance is being reviewed
- `Completed`: explicit acceptance has passed
- `Deferred`: intentionally removed from the current release scope
- `Blocked`: cannot progress without an external decision or prerequisite

## REL12 — Podway v0.1.2 Contract Recovery and Release

| id | title | status | goal | references |
|---|---|---|---|---|
| `REL12001` | Freeze the v0.1.2 recovery design | Completed | Adopt the decision-complete design, authority boundaries, release constraints, and ordered implementation plan. | [Design authority](../todo/TODO-podway-v0.1.2-contract-recovery.md#status-and-authority) |
| `REL12002` | Audit the v1 compatibility boundary | Completed | Prove released-schema compatibility and record the exact pre-release consumer migration boundary. | [V1 compatibility boundary](../todo/TODO-podway-v0.1.2-contract-recovery.md#v1-compatibility-boundary) |
| `REL12003` | Repair the version identity contract | Planned | Make both binaries emit one identical schema-conformant identity and reject malformed runtime probes. | [Version identity result](../todo/TODO-podway-v0.1.2-contract-recovery.md#version-identity-result) |
| `REL12004` | Enforce authoritative packaged-schema validation | Planned | Validate complete identity envelopes using only the exact manifest-bound packaged contract set. | [Authoritative packaged schema registry](../todo/TODO-podway-v0.1.2-contract-recovery.md#authoritative-packaged-schema-registry) |
| `REL12005` | Harden qualification and release evidence | Planned | Add early singleton diagnostics and close provenance, handoff, digest, and conformance validation. | [Qualification and release evidence](../todo/TODO-podway-v0.1.2-contract-recovery.md#qualification-and-release-evidence) |
| `REL12006` | Build and qualify the native v0.1.2 distribution | Planned | Advance the version and pass every clean native arm64 and extracted-distribution release gate. | [Local gate](../todo/TODO-podway-v0.1.2-contract-recovery.md#local-gate) |
| `REL12007` | Publish and independently reverify v0.1.2 | Planned | Publish the annotated immutable release and reverify all downloaded bytes and closed identities. | [Publication order](../todo/TODO-podway-v0.1.2-contract-recovery.md#publication-order) |

Tasks are completed in table order. At most the first incomplete task may be `In
Progress`, `In Review`, or `Blocked`; later tasks remain `Planned`.
