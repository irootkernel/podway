# Podway 0.1.0 release notes

Podway 0.1.0 was published on August 2, 2026 as the first public Podway release.
The release and its artifacts are available from the
[GitHub release page](https://github.com/irootkernel/podway/releases/tag/v0.1.0).

## Release identity and compatibility

Podway 0.1.0 publishes the public v1 IPC, output, error, workspace, procedure, and SQLite contracts together with the automation-client contract assets. Existing
uninitialized state or schema-0 state is upgraded transactionally to schema-v1;
an incomplete upgrade is not accepted as an installed schema-v1 state.

The supported release target is native Apple Silicon macOS:
`aarch64-apple-darwin` with thin `arm64` Mach-O binaries. The archive contains the
matching `podway` and `podwayd` executables. Cross-built, translated, relabeled,
universal, and fat binaries do not satisfy release acceptance.

`podway version --json` returns the compact product identity. Complete release and
contract identity is available through `podway version --json --identity`.

## Trust boundary

Podway is a same-user local tool. Its local IPC and workspace state are trusted
only within the operating-system user account that owns them.
It does not provide a multi-user access-control boundary.

## Signing and notarization

The v0.1.0 Apple Silicon package is unsigned and not notarized. The release makes
no Gatekeeper acceptance claim. Users should verify the attached SHA-256 checksum
before installing the binaries.

## Distribution metadata

The release provides:

- `podway-0.1.0-aarch64-apple-darwin.tar.gz`;
- its `.sha256` checksum file;
- `podway-0.1.0-aarch64-apple-darwin.provenance.json`;
- `podway-0.1.0-aarch64-apple-darwin.dolgorae-handoff.json`.

The provenance records the source commit, Rust 1.97.1 identity, `Cargo.lock`
digest, target, binary digests, release-gate result, contract-manifest digest, and
signing/notarization status. The deterministic Dolgorae handoff binds the binary,
contract, provenance, source-tree, and toolchain identities required for consumer
pinning.

The archive was built after the complete local `make test` gate passed. Packaged
conformance exercised the release binaries through the isolated foreground dev
daemon mode without administrator authorization or a temporary operating-system
account.

## Known limitations

- Only native Apple Silicon macOS is supported.
- The binaries are unsigned and not notarized.
- The service is a per-user LaunchAgent. It starts after GUI login and does not
  run before login.
- The daemon version interface in this tag predates the normalized version-first
  grammar introduced after v0.1.0.
