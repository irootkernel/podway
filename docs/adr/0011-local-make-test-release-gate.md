# ADR-0011: Use the local Makefile test suite as the release gate

- Status: Accepted
- Date: 2026-07-22

## Context

Podway is developed and released from a local macOS environment without GitHub
Actions. A separate qualification system introduced signatures, approval roles,
holdout runs, evidence archives, and a hidden service-wrapper installation path.
Those mechanisms duplicated the behavioral checks while making release readiness
depend on infrastructure and identities the project does not operate.

## Decision

The repository-root `make test` command is the sole required release-readiness
gate. It runs `test-prepare`, one combined `test-rust` invocation, `test-fuzzing`,
and `test-e2e` sequentially. The fuzzing target uses a pinned nightly toolchain only
for bounded protocol-input fuzzing. The end-to-end target builds and executes the
real `podway` and `podwayd` binaries with the product's pinned stable toolchain.

This decision does not let a passing older gate waive accepted requirements that
have not yet been implemented or registered as executable evidence. A release
candidate exists only after its release-blocking roadmap work has entered the
tested tree; `make test` then remains the single executable gate for that tree.

Generated-source synchronization and formatting remain corrective operations in
`test-prepare`. A release tag or archive uses the resulting formatted tree. Any
later source change requires another complete `make test` run.

There is no additional signature, approval, holdout, qualification, attestation,
or hosted-CI requirement. Signing, notarization, archive assembly, checksums, and
release notes may be used for distribution, but they do not form another
release-readiness gate.

## Consequences

- Contributors have one reproducible command for the complete gate.
- Unit, integration, bounded fuzzing, and actual-binary scenarios retain distinct
  test layers. Integration sources are registered in one Cargo suite per crate,
  and the CLI's actual-binary sources share one E2E suite.
- Product-acceptance and crash-boundary mappings remain machine-validated as part
  of `test-prepare`.
- The former `REL-007` detached-approval and quorum requirement is retired. Its
  identifier remains historical and must not be reused.
- Release status is attached to the exact tested tree, not to a detached evidence
  bundle.
- The service exposes only the production daemon installation topology.
