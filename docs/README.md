# Implementation and operator documentation

This directory is the entry point for documentation about the implemented repository
and its release operation. Normative product behavior and compatibility contracts live
under [`sot/`](../sot/); when implementation guidance and the SOT disagree, follow the
[SOT precedence rule](../sot/README.md#normative-language-and-source-precedence) and
fix the stale guidance.

## Start here

- [Release readiness](release-readiness.md) explains the sole local `make test` gate,
  its test layers, and deterministic distribution construction.
- [Root README](../README.md) gives the supported platform, safety boundary, and
  contributor commands.
- [Implementation status](../sot/IMPLEMENTATION_STATUS.md) records which delivery
  goals are exercised by the current local gate.
- [Release notes](../RELEASE_NOTES.md) describe the current public contract,
  migration baseline, and artifact signing/notarization status.

## Architecture and behavior

- [System architecture](../sot/docs/10-architecture/10-system-architecture.md)
- [Rust workspace and runtime](../sot/docs/10-architecture/14-rust-codebase.md)
- [Daemon and durable write queue](../sot/docs/10-architecture/11-daemon-and-write-queue.md)
- [Git worktree and filesystem boundary](../sot/docs/10-architecture/12-git-worktree-and-filesystem.md)
- [macOS LaunchAgent integration](../sot/docs/10-architecture/13-macos-service.md)
- [CLI specification](../sot/docs/30-interfaces/30-cli-specification.md)
- [SQLite model](../sot/docs/40-storage/40-sqlite-model.md)

## Operations, limits, and support boundary

- [Security, same-user trust, and explicit limitations](../sot/docs/50-operations/50-security-and-trust.md)
- [Logging and diagnostics](../sot/docs/50-operations/51-observability.md)
- [Release, packaging, installation, and upgrade](../sot/docs/50-operations/52-release-and-packaging.md)
- [Errors and exit codes](../sot/docs/30-interfaces/33-errors-and-exit-codes.md)
- [Testing and conformance](../sot/docs/60-quality/60-testing-and-conformance.md)
- [Requirement-to-test traceability](../sot/docs/60-quality/62-requirements-traceability.md)

The complete normative reading order and document catalog are maintained in the
[SOT index](../sot/README.md).
