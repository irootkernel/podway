# Podway 0.1.2 release notes

Podway 0.1.2 is a contract-recovery maintenance release candidate. Publication
and the immutable GitHub release verification remain a separate release task.

## Changes since 0.1.1

- Repair CLI and daemon build identity so both emit the same complete,
  schema-conformant `podway.version-result/v1` object.
- Make runtime daemon probes decode the complete `podway.ipc/v1` response and
  reject the malformed v0.1.1 result that omitted its result-level schema.
- Add one offline Rust verifier for source and packaged contract manifests,
  schema registries, references, complete envelopes, and binary identity.
- Add an early production singleton diagnostic before the expensive release gate.
- Close provenance and Dolgorae handoff evidence, atomically record extracted
  packaged conformance, and independently verify the final local bundle.

## Compatibility

Podway 0.1.2 preserves byte-identical public v1 Procedure, output, error,
status-result, next-result, and version-result schemas. It preserves the public v1 IPC, output, error, workspace, procedure, and SQLite contracts together with the
automation-client contract assets. Existing uninitialized state or schema-0 state is upgraded transactionally to schema-v1; an incomplete upgrade is not accepted as
an installed schema-v1 state.

Before the first public release, the Procedure schema changed `list.max_items`
from allowing zero to the supported runtime range `1..=1000`. Consumers pinned to
that pre-release schema-only snapshot must replace zero with a supported value.
Released v0.1.0 and v0.1.1 consumers require no contract migration.

The supported release target remains native Apple Silicon macOS:
`aarch64-apple-darwin` with thin `arm64` Mach-O binaries. The archive contains the
matching `podway` and `podwayd` executables.

Podway is a same-user local tool. Its IPC and workspace state are trusted only
within the operating-system user account that owns them. It does not provide a multi-user access-control boundary.

## Distribution metadata

The release candidate produces:

- `podway-0.1.2-aarch64-apple-darwin.tar.gz`;
- its `.sha256` checksum file;
- `podway-0.1.2-aarch64-apple-darwin.provenance.json`;
- `podway-0.1.2-aarch64-apple-darwin.dolgorae-handoff.json`.

The provenance records product, source commit and tree, Rust 1.97.1 identity,
`Cargo.lock` digest, target, archive and binary digests, release-gate result,
contract-manifest identity, exact packaged-conformance scenarios, and
signing/notarization status. Packaged conformance exercises the extracted release
binaries through the isolated foreground dev daemon mode before the handoff and
final bundle verification can succeed.

## Signing and known limitations

The v0.1.2 Apple Silicon package is unsigned and not notarized. Users should
verify the attached SHA-256 checksum before installation.

- Only native Apple Silicon macOS is supported.
- The service is a per-user LaunchAgent. It starts after GUI login and does not
  run before login.
