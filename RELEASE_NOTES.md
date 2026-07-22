# Podway 0.1.0 release notes

## Release identity and compatibility

Podway 0.1.0 publishes the public v1 IPC, output, error, workspace, procedure, and SQLite contracts. Existing uninitialized or schema-0 state is upgraded transactionally to schema-v1; an incomplete upgrade is not accepted as an installed schema-v1 state.

Podway 0.1.0 supports native Apple Silicon only: `aarch64-apple-darwin` / `arm64`.
Each Apple Silicon archive contains both `podway` and `podwayd`. Cross-built,
translated, relabeled, universal, and fat binaries cannot satisfy native release
acceptance. The 11.0 value is a minimum build deployment target
and Mach-O load-command target only. This release makes no Apple Silicon runtime
lifecycle or Gatekeeper acceptance claim.

## Trust boundary

Podway is a same-user local tool. Its local IPC and workspace state are trusted only within the operating-system user account that owns them. It does not provide a multi-user access-control boundary.

## Signing and notarization status

The current Apple-Silicon public package is **unsigned and not notarized**. Developer ID signing and notarization were not attempted because the required credentials and infrastructure are unavailable; zip stapling is not applicable, and no Gatekeeper acceptance is claimed. This status is frozen for this release.

Developer ID signing and notarization are recommended when the necessary
infrastructure is available, but they are not release-readiness requirements. The
authoritative gate for the source revision is a successful local `make test` run.

## Distribution metadata

`make dist` produces the deterministic
`podway-0.1.0-aarch64-apple-darwin.tar.gz` archive together with its SHA-256 file
and `podway-0.1.0-aarch64-apple-darwin.provenance.json`. The provenance records the
source commit, Rust 1.97.1 identity, Cargo.lock digest, target, binary digests,
release-gate result, and the unsigned/not-notarized status above.
