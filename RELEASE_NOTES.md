# Podway 0.1.0 release notes

## Release identity and compatibility

Podway 0.1.0 publishes the public v1 IPC, output, error, workspace, procedure, and SQLite contracts. Existing uninitialized or schema-0 state is upgraded transactionally to schema-v1; an incomplete upgrade is not accepted as an installed schema-v1 state.

This release is supported only on native Apple Silicon: `aarch64-apple-darwin` (`arm64`). The 11.0 value is a minimum build deployment target and Mach-O load-command target only; runtime lifecycle qualification was performed only on the recorded current Apple-Silicon release host. This release does not claim macOS 11 runtime support without host evidence. Intel, Rosetta-translated, and universal-binary builds are not qualified release artifacts.

## Trust boundary

Podway is a same-user local tool. Its local IPC and workspace state are trusted only within the operating-system user account that owns them. It does not provide a multi-user access-control boundary.

## Signing and notarization status

The default release-candidate posture is **unsigned-internal**. In that posture, codesigning and notarization are not attempted because release credentials are unavailable, zip stapling is not applicable, and no Gatekeeper acceptance is claimed.

A **signed-public** release candidate is a separate, immutable qualification branch. It is eligible to claim signatures, notarization, or Gatekeeper verification only when those steps actually complete and are recorded in detached release attestations. This document does not claim that signing or notarization credentials are present.
