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

Linux and Windows are not part of the first release gate.

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

Release CI builds and tests the sole supported release target:

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
  LICENSE
  README.md
  RELEASE_NOTES.md
```
Each Apple Silicon archive contains both executables, `podway` and `podwayd`; they are not separate architecture artifacts.

Presets are embedded in the binary for runtime availability. Source copies are shipped for inspection and customization.

## Installation

The binary installation mechanism may be a release archive or package manager. Regardless of mechanism:

1. `podway` and `podwayd` versions must be installed together;
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
- verified in release CI or a controlled release pipeline.

Unsigned internal builds remain supported for development. The release notes clearly identify signing status.

## Checksums and provenance

Every downloadable archive has a published SHA-256 checksum. The release process records:

- source commit;
- Rust toolchain identifier;
- dependency lockfile digest;
- target architecture;
- binary checksums;
- signing/notarization result;
- conformance-suite result.

## Upgrade

Upgrade procedure:

1. install both new binaries;
2. refresh or reinstall the LaunchAgent so it points to the new `podwayd` path;
3. restart daemon;
4. verify protocol health;
5. migrate worktree databases lazily on first access.

New worktree databases begin in schema-0/uninitialized state; on first access, the daemon transactionally initializes or migrates each database to canonical schema-v1.

The daemon handles one workspace migration failure without disabling other workspaces.

Phase 8 migration evidence is emitted at `release/migration-evidence-v1.json`, rather than required at S3.

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

## Release gates

A release candidate must pass:

- all unit, property, integration, crash, and fuzz tests;
- Apple Silicon target build and native arm64 validation;
- LaunchAgent install/start/stop/restart/uninstall tests;
- clean install and upgrade tests;
- schema and preset validation;
- JSON golden tests;
- CLI help and completion tests;
- four preset end-to-end scenarios with retry and return;
- dependency and license review;
- checksum generation;
- acceptance criteria in the product acceptance document.

## Support policy

The first release targets currently supported macOS major versions selected by the project at release time. The exact minimum deployment target is recorded in release engineering configuration and release notes. The architecture does not depend on a single hard-coded macOS version beyond required Unix socket and LaunchAgent behavior.

## License

The source repository and release archives include the MIT License. Third-party dependencies retain their own licenses and must be reviewed for compatibility.
