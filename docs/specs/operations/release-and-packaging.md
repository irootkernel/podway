# Release and Packaging

## Release scope

The complete `v0.1.2` public release is a macOS product. It includes:

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
product binary version: 0.1.2
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
      tests/fixtures/v2/
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

From a clean native Apple Silicon working tree, `make dist` always runs the
native-host and clean-worktree preflight before the development gate, release helper
sentinels, and bounded fuzzing. Packaging repeats the preflight after the gate. It
then builds both release binaries and writes the
deterministic archive, its SHA-256 file, a provenance JSON document, and a Dolgorae
compatibility handoff under `dist/`. The archive
builder rejects a translated or non-arm64 host, non-thin-arm64 Mach-O binaries,
version mismatches, incomplete archive contents, a Rust toolchain other than 1.97.1,
and any dirty tracked or untracked source state.
The first preflight derives the effective account home from the operating-system
account database instead of ambient `HOME`. If the production runtime and lock
exist, it requires an owner-private `0700` real directory and owner-private `0600`
regular lock file, then probes the daemon's non-blocking exclusive singleton lock.
A held or unsafe lock fails before formatting or compilation. The probe releases a
free lock immediately and never reads, removes, or replaces the daemon socket.
The builder first snapshots both executable inputs through non-symlink file
descriptors. Binary validation, isolation classification, packaged content, recorded
digests, and provenance all use those immutable snapshot bytes. Probe failure,
timeout, or ambiguity rejects both artifact classes.

Before packaging, the internal Rust release-contract verifier reconstructs the
source manifest and its exact offline schema registry, validates both complete
identity envelopes and their closed version results, and requires the two result
objects to be equal. Qualification invokes that same verifier against the
extracted archive's `share/podway` contract root. Python release tooling only
orchestrates the verifier and consumes its receipt; it does not maintain a
parallel partial schema or identity interpretation. Manifest self/member drift,
missing or extra schemas, duplicate paths or `$id` values, unregistered `$ref`
targets, and external network or filesystem references are release-blocking.

The provenance document records the product, shared binary build identity, source
commit and Git tree, clean-tree state, Rust toolchain identifier, Cargo.lock digest,
contract manifest identity, target architecture, both binary digests, archive
digest, successful development and fuzzing gate result, artifact class, and
signing/notarization status. Release
packaging rejects debug-only isolation capability. After packaging, `make dist`
extracts the release-profile archive and qualifies its binaries through the isolated
foreground dev daemon mode. The test-only
`--allow-dirty` switch is invalid for `artifact_class=distribution`; distribution
construction always requires a clean tree.

Packaging initially writes the exact required conformance scenario list with a
`pending` result. Qualification is an automatic, unprivileged step of `make dist`;
there is no separate qualification target or receipt. It uses a private temporary
`PODWAY_DEV_HOME`, starts the extracted `podwayd --dev`, and runs packaged fenced
lifecycle, conflict, admitted-timeout, response-loss, reconciliation, and identity
scenarios through the extracted `podway --dev`. Success requires orderly termination
and absence of every temporary daemon socket. Only after every extracted-archive
check and scenario succeeds does qualification atomically replace the provenance
result with `passed`; failure preserves the pending document byte-for-byte. This
verifies Podway's executable and IPC interfaces; it does not retest macOS launchd
itself.

After packaged conformance passes, the deterministic
`podway-0.1.2-aarch64-apple-darwin.dolgorae-handoff.json` document publishes the
archive and binary digests, build and contract identities, provenance digest,
source commit and Git tree, Rust toolchain, and Cargo.lock digest. Dolgorae pins
this closed identity set rather than inferring compatibility from a version string.
Handoff creation rejects pending, incomplete, unknown, or malformed evidence and
repeats the exact release-gate, signing/notarization, and packaged-conformance
results. A final offline verifier then independently re-extracts the archive and
compares checksum, archive layout, binaries, manifest-bound identities, provenance,
handoff, source commit/tree, and Cargo.lock in both directions. `make dist` succeeds
only after that final verification and after proving no qualification socket or
daemon process remains.

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
- the successful `make test` and bounded fuzzing result for the source revision.

## Upgrade

Upgrade procedure:

1. install both new binaries;
2. refresh or reinstall the LaunchAgent so it points to the new `podwayd` path;
3. restart daemon;
4. verify protocol health;
5. migrate worktree databases lazily on first access.

New worktree databases begin in schema-0/uninitialized state; on first access, the daemon
transactionally initializes them to canonical schema-v3. Existing canonical schema-v1 and
schema-v2 databases migrate forward lazily to schema-v3 on first access.

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

The repository-root `make test` command is the required development gate. It runs
static preparation, four-worker unit/architecture/integration tests by default,
and serial real-binary end-to-end scenarios. The preparation target includes
product-code lint, dependency/license review, architecture guardrails,
product-acceptance mapping, crash-boundary mapping, and contract validation.

A revision is release-ready only when `make dist` exits successfully after adding
all-target lint, release sentinels, bounded fuzzing, release builds, archive
qualification, and the handoff. The gate validates formatting without rewriting
the tree.

No hosted CI run, independent signature, approval quorum, holdout run,
qualification archive, or attestation bundle is required. Signing, notarization,
and publication remain distribution operations after `make dist` succeeds.

### V2 admission and GA boundary

Normal builds MUST refuse `podway.procedure/v2` session admission until all ten
PV2GA epics and every v2 acceptance category are complete. Intermediate work may
ship read-only authoring surfaces, but no release artifact may admit or persist a
partial v2 session contract.

Development dogfooding may admit v2 only when all of these conditions hold: the
binary was compiled with the explicit development-only feature, existing
development mode is active, the workspace is marked disposable, and separate
socket and state directories are in use. The unlock MUST reject an installed
daemon, LaunchAgent, or normally registered workspace. Development v2 state is
discardable and receives no migration-preservation promise.

V0.2.0 reaches full-feature GA only after the complete integrated development,
compatibility, payload, persistence, native runtime, recovery, and distribution
gates pass. Release qualification MUST prove that the development admission
unlock is absent before public v2 admission can be enabled.

## Support policy

Podway releases only for currently supported macOS major versions on native Apple
Silicon. The exact minimum deployment target is recorded in release engineering
configuration and release notes. Supporting another architecture or operating
system requires a superseding architecture decision and native release gate.

## License

The source repository and release archives include the MIT License. Third-party dependencies retain their own licenses and must be reviewed for compatibility.
