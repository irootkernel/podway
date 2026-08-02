# Podway 0.1.1 release notes

Podway 0.1.1 was published on August 3, 2026 as a maintenance release. The
release and its artifacts are available from the
[GitHub release page](https://github.com/irootkernel/podway/releases/tag/v0.1.1).

## Changes since 0.1.0

- Normalize the daemon version command grammar and align packaged daemon identity
  probes with the public interface.
- Simplify the local development and distribution gates by removing cached test
  receipts and duplicate release-only test execution.
- Stabilize inactive-workspace SQLite reconciliation tests by comparing logical
  state instead of transient WAL and SHM file layouts.
- Handle the macOS process-group reuse race after a launchctl child exits.
- Capture recursive crash-test output so nested libtest headers do not pollute the
  parent test run.

## Compatibility

Podway 0.1.1 preserves the public v1 IPC, output, error, workspace, procedure, and SQLite contracts together with the automation-client contract assets. Existing
uninitialized state or schema-0 state is upgraded transactionally to schema-v1;
an incomplete upgrade is not accepted as an installed schema-v1 state.

The supported release target remains native Apple Silicon macOS:
`aarch64-apple-darwin` with thin `arm64` Mach-O binaries. The archive contains the
matching `podway` and `podwayd` executables.

Podway is a same-user local tool. Its IPC and workspace state are trusted only
within the operating-system user account that owns them. It does not provide a multi-user access-control boundary.

## Distribution metadata

The release provides:

- `podway-0.1.1-aarch64-apple-darwin.tar.gz`;
- its `.sha256` checksum file;
- `podway-0.1.1-aarch64-apple-darwin.provenance.json`;
- `podway-0.1.1-aarch64-apple-darwin.dolgorae-handoff.json`.

The provenance records the source commit, Rust 1.97.1 identity, `Cargo.lock`
digest, target, binary digests, release-gate result, contract-manifest digest, and
signing/notarization status. Packaged conformance exercises the release binaries
through the isolated foreground dev daemon mode.

## Signing and known limitations

The v0.1.1 Apple Silicon package is unsigned and not notarized. Users should
verify the attached SHA-256 checksum before installation.

- Only native Apple Silicon macOS is supported.
- The service is a per-user LaunchAgent. It starts after GUI login and does not
  run before login.
