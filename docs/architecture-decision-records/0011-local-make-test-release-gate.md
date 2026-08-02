# ADR-0011: Separate the local development and distribution gates

- Status: Accepted
- Date: 2026-07-22

## Context

Podway is developed and released from a local macOS environment without GitHub
Actions. A separate qualification system introduced signatures, approval roles,
holdout runs, evidence archives, and a hidden service-wrapper installation path.
Those mechanisms duplicated the behavioral checks while making release readiness
depend on infrastructure and identities the project does not operate.

## Decision

The repository-root `make test` command is the required development gate. It runs
strict preparation, unit/architecture/integration tests with four workers by
default, real-binary E2E serially, and preset-tool verification. The repository-root
`make dist` command is the release gate: it always runs `make test`, all-target
Clippy, release helper sentinels, bounded protocol fuzzing, release builds, one
distribution package, qualification, and the Dolgorae handoff.

This decision does not let a passing older gate waive accepted requirements that
have not yet been implemented or registered as executable evidence. A release
candidate exists only after its release-blocking roadmap work has entered the
tested tree; `make dist` remains the single executable release gate for that tree.

Canonical-asset validation and formatting checks remain preparation operations in
`test-prepare`. Distribution does not reuse a cached gate result; every invocation
runs the complete gate against the tree it packages.

Make-driven Cargo gates disable incremental compilation so repeated full-workspace
verification does not accumulate unbounded codegen objects. Direct Cargo commands
retain their normal incremental behavior.

There is no additional signature, approval, holdout, qualification, attestation,
or hosted-CI requirement. Signing, notarization, archive assembly, checksums, and
release notes may be used for distribution, but they do not form another
release-readiness gate.

## Consequences

- Contributors have one reproducible development gate and one release command.
- Unit, integration, bounded fuzzing, and actual-binary scenarios retain distinct
  test layers. Integration sources are registered in one Cargo suite per crate,
  and the CLI's actual-binary sources share one E2E suite.
- Product-acceptance and crash-boundary mappings remain machine-validated as part
  of `test-prepare`.
- The former `REL-007` detached-approval and quorum requirement is retired. Its
  identifier remains historical and must not be reused.
- Release status is established by the successful `make dist` invocation.
- The service exposes only the production daemon installation topology.
