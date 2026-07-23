# Podway 0.1.0 release notes

> Draft: v0.1.0 has not been released. This document describes the target package;
> release remains blocked by the incomplete tasks in `docs/roadmap.md`.

## Release identity and compatibility

Podway 0.1.0 will publish the public v1 IPC, output, error, workspace, procedure, and SQLite contracts. It will also publish the automation-client contract assets. Existing uninitialized state or schema-0 state is upgraded transactionally to schema-v1; an incomplete upgrade is not accepted as an installed schema-v1 state.

Podway publishes and supports only native Apple Silicon macOS:
`aarch64-apple-darwin` / thin `arm64` Mach-O.
Each Apple Silicon archive contains both `podway` and `podwayd`. Cross-built,
translated, relabeled, universal, and fat binaries cannot satisfy native release
acceptance. The 11.0 value is a minimum build deployment target
and Mach-O load-command target only. This release makes no Apple Silicon runtime
lifecycle or Gatekeeper acceptance claim.

## Trust boundary

Podway is a same-user local tool. Its local IPC and workspace state are trusted only within the operating-system user account that owns them. It does not provide a multi-user access-control boundary.

## Signing and notarization status

The target Apple-Silicon public package is currently planned as unsigned and not notarized. Final status is recorded when the artifact is built; no released package exists yet.

Developer ID signing and notarization are recommended when the necessary
infrastructure is available, but they are not release-readiness requirements. The
authoritative gate for the source revision is a successful local `make test` run.

## Distribution metadata

`make dist` produces the deterministic
`podway-0.1.0-aarch64-apple-darwin.tar.gz` archive together with its SHA-256 file
and `podway-0.1.0-aarch64-apple-darwin.provenance.json`. The provenance records the
source commit, Rust 1.97.1 identity, Cargo.lock digest, target, binary digests,
release-gate result, contract manifest digest, and signing/notarization status.
