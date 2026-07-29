# Release and Packaging

## Release scope

The complete `v0.1.0` public release is a macOS product. It includes:

- `podway`;
- `podwayd`;
- LaunchAgent support;
- four embedded presets;
- JSON schemas and user documentation;
- zsh, bash, and fish completion;
- MIT License;
- checksums and release notes.

Linux, Windows, Intel macOS, translated, universal, fat, and cross-built artifacts
are not Podway release or support targets.

## Versioning

Podway uses semantic versioning for product releases.

Independent versioned contracts:

```text
product binary version: 0.1.0
IPC: podway.ipc/v1
output: podway.output/v1
error: podway.error/v1
workspace config: podway.workspace/v1
procedure: podway.procedure/v1
SQLite schema: integer migration version
```

A product minor release may add backward-compatible fields or commands. Breaking public contract changes require a new contract version and migration plan.

## Target architectures

The local release gate builds and tests the sole supported release target:

- Apple Silicon (`aarch64-apple-darwin`) with `arch`, host architecture, and thin Mach-O architecture all `arm64`.

Native validation must run on an arm64 host against thin arm64 Mach-O binaries. Translated execution, universal or fat binaries, cross-built output, and relabeled artifacts do not satisfy this requirement.

## Release archive contents

Reference archive:

```text
podway-<version>-<target>/
  bin/
    podway
    podwayd
  share/
    completions/
      podway.zsh
      podway.bash
      podway.fish
    podway/
      presets/
      schemas/
      spec/
      docs/examples/json/
      tests/fixtures/contract/
      contracts/contract-manifest-v1.json
  LICENSE
  README.md
  RELEASE_NOTES.md
```
Each Apple Silicon archive contains both executables, `podway` and `podwayd`; they are not separate architecture artifacts.

Presets are embedded in the binary for runtime availability. Source copies are shipped for inspection and customization.

The specs, fixtures, and contract manifest are packaged with the schemas so a
consumer can inspect and pin the same contract identity as the binaries.

### Local archive construction

From a clean native Apple Silicon working tree, `make dist` reruns the complete
`make test` release gate, builds both release binaries, and writes the deterministic
archive, its SHA-256 file, and a provenance JSON document under `dist/`. The archive
builder rejects a translated or non-arm64 host, non-thin-arm64 Mach-O binaries,
version mismatches, incomplete archive contents, a Rust toolchain other than 1.97.1,
and any dirty tracked or untracked source state.

The provenance document records the shared binary build identity, source commit,
Rust toolchain identifier, Cargo.lock digest, contract manifest identity, target
architecture, both binary digests, archive digest, successful local-gate result,
and signing/notarization status. `make test` exercises
the same archive builder with real binaries in disposable directories and verifies
that repeated construction produces the same archive digest; it does not publish a
distribution artifact.

## Installation

The binary installation mechanism may be a release archive or package manager. Regardless of mechanism:

1. `podway` and `podwayd` product and contract-manifest identities must match;
2. the user runs `podway daemon install` or the package post-install equivalent;
3. service install records the absolute daemon path;
4. `podway daemon status` verifies health;
5. worktrees are initialized independently with `podway init`.

No installer scans for or modifies existing worktrees.

## Code signing and notarization

Public macOS artifacts SHOULD be:

- signed with an appropriate Developer ID;
- submitted for notarization;
- stapled where packaging permits;
- verified on the local release host.

Unsigned builds are valid release artifacts. Release notes clearly identify the
signing and notarization status of distributed files. Signing status does not
change release readiness.

## Checksums and provenance

Every downloadable archive has a published SHA-256 checksum. Published release
metadata SHOULD record:

- source commit;
- Rust toolchain identifier;
- dependency lockfile digest;
- target architecture;
- binary checksums;
- signing/notarization result;
- the successful `make test` result for the source revision.

## Upgrade

Upgrade procedure:

1. install both new binaries;
2. refresh or reinstall the LaunchAgent so it points to the new `podwayd` path;
3. restart daemon;
4. verify protocol health;
5. migrate worktree databases lazily on first access.

New worktree databases begin in schema-0/uninitialized state; on first access, the daemon transactionally initializes or migrates each database to canonical schema-v1.

The daemon handles one workspace migration failure without disabling other workspaces.

## Downgrade

Database downgrade is unsupported. An older daemon encountering a newer schema fails closed. Users may upgrade again or destructively reset the worktree runtime state.

## Uninstall

Uninstalling the service or binaries does not delete `.podway/` in any worktree.

Recommended steps:

```bash
podway daemon uninstall --yes
# Remove binaries through the original installation mechanism.
```

Users may delete `.podway/runtime/` or the whole worktree when task state is no longer needed.

## Release readiness

The repository-root `make test` command is the sole required release-readiness
gate. It runs static preparation, unit tests, integration scenarios, bounded
protocol fuzzing, and real-binary end-to-end scenarios sequentially. The
preparation target includes
dependency/license review, architecture guardrails, product-acceptance mapping,
crash-boundary mapping, and contract validation.

A revision is release-ready when `make test` exits successfully. Because the gate
synchronizes generated files and applies formatting, the tag or archive MUST use
the resulting tree. A later source change requires a new complete run.

No hosted CI run, independent signature, approval quorum, holdout run,
qualification archive, or attestation bundle is required. Archive construction,
checksum publication, signing, notarization, and release notes are distribution
operations performed after the source revision is release-ready.

## Support policy

Podway releases only for currently supported macOS major versions on native Apple
Silicon. The exact minimum deployment target is recorded in release engineering
configuration and release notes. Supporting another architecture or operating
system requires a superseding architecture decision and native release gate.

## License

The source repository and release archives include the MIT License. Third-party dependencies retain their own licenses and must be reviewed for compatibility.
