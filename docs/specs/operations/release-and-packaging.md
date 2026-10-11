# Release and Packaging

## Release scope

Podway releases one native macOS executable, `podway`, with the embedded
`analysis-v2`, `bug-fix-v2`, `small-change-v2` and `sw-dev-v2` presets, reviewable
schemas/specifications, documentation, shell completion, MIT License, checksums
and cumulative changelog. There is no daemon or LaunchAgent in the archive.
Linux, Windows, Intel macOS, translated, universal, fat and cross-built artifacts
are not release or support targets.

## Versioning

Product releases use semantic versioning. Product and contract versions are
independent. Current public identities are `podway.output/v4`, `podway.error/v2`,
`podway.version-result/v2`, `podway.contract-manifest/v2`, workspace configuration
v1/v2, Procedure v2, and direct SQLite generation 15. The manifest has no
`supported_ipc_ids`. Breaking public changes require an explicit versioned
contract and adopted compatibility plan; there is no inferred migration promise.

The Aquarium managed development producer/controller and their bundle/service
schemas are retired by V2DFC-009. No replacement Aquarium publication or activation
contract is adopted. Development qualification below is a nonpublishable Podway
verification artifact, not a managed development service generation.

## Target architectures

The sole supported target is `aarch64-apple-darwin`. Validation runs on native
Apple Silicon with host architecture and thin Mach-O architecture both `arm64`.
The release builder rejects translated execution, non-thin-arm64 binaries,
cross-built output and relabeled artifacts, and requires the Rust toolchain pinned
by `rust-toolchain.toml`.

## Release archive contents

```text
podway-<version>-<target>/
  bin/podway
  share/completions/podway.zsh
  share/completions/podway.bash
  share/completions/podway.fish
  share/podway/presets/
  share/podway/schemas/
  share/podway/spec/
  share/podway/docs/examples/json/
  share/podway/tests/fixtures/contract/
  share/podway/tests/fixtures/v2/
  share/podway/contracts/contract-manifest-v2.json
  LICENSE
  README.md
  CHANGELOG.md
```

Presets remain embedded for runtime availability; source copies permit inspection
and customization. The packaged specs, fixtures and manifest let consumers pin the
same contract identity as the executable. Canonical source assets and manifest
membership, not a parallel documentation copy, determine the exact packaged set.

### Local archive construction

Agent-operated full/reduced release selection is owned by
[AGENTS.md](../../../AGENTS.md#release-gate-selection). A full gate runs `make dist`
from the exact clean release commit on native Apple Silicon. It performs native
and clean-source preflight before the development gate, release sentinels and
bounded fuzzing; repeats preflight at packaging; builds the single release CLI;
and writes deterministic archive, checksum, provenance and Dolgorae handoff under
`dist/`. Full qualification and final bundle verification are required.

The builder snapshots the non-symlink executable through a file descriptor.
Identity validation, hook probes, archive bytes and recorded binary digest all use
that immutable snapshot. A failed probe, timeout, ambiguous identity, unexpected
layout, missing member or stale manifest blocks packaging. Release-profile test
hooks must be disabled for every artifact class. Distribution rejects dirty tracked
or nonignored untracked source and rejects `--allow-dirty`. Its changelog must use
the current release's publication date with no remaining `Unreleased` section.

The Rust `podway-contract-verifier` reconstructs the source/package manifest and
its exact offline schema registry, validates the complete Output v4 and closed
version-result v2, and recomputes release-profile build identity. Its interface is:

```bash
podway-contract-verifier --contract-root <source-or-packaged-root> --podway <release-profile-podway> --expected-target aarch64-apple-darwin --expected-source-commit <sha>
```

The `podway.contract-verification/v2` receipt identifies the single binary,
product/version/target/source, manifest schema/digest, build identity and asset/
schema counts. Python tooling orchestrates this verifier; it does not implement a
second partial identity or schema validator. Manifest self/member drift, duplicate
paths/IDs, missing/extra schemas, unregistered references, and external filesystem
or network references block qualification.

### Non-distribution development qualification

The closed artifact classes are `distribution` and `development-qualification`.
The latter enables the V2DFC-009 precommit native dry run without asserting a clean
committed distribution. It must finish before delegated acceptance and the normal
completion commit. Freeze the exact development source, capture
`tools/release_archive.py source-snapshot`, and pass `make test`,
`make release-prepare` and `make test-fuzzing` before building and packaging that
unchanged release-profile CLI.

Use `package --artifact-class development-qualification --gate development
--allow-dirty --development-source-snapshot-sha256 <hash>
--development-checks <frozen-checks-receipt>`, then the `qualify`,
`create` handoff and `verify` bundle commands with `--gate development`, as listed
in the [release workflow](../../implementation-tips/release.md#non-distribution-development-qualification).
This is explicitly nonpublishable. It permits an open current changelog section
or one newer `Unreleased` section followed immediately by the published current
version. It records truthful `source_dirty` and keeps separate HEAD/tree and source
snapshot identities. The snapshot covers sorted tracked and nonignored untracked source
paths, regular-file modes and bytes, including tracked deletions; ignored runtime
and private artifacts are excluded. Every stage checks the snapshot against the
current source. A changed candidate invalidates the corresponding gate evidence.
No distribution clean-source, patch-baseline, native-target or disabled-hook rule
is waived.

### Packaged qualification and cleanup

Qualification invokes the same Rust verifier against the extracted
`share/podway` root, then runs the exact scenario registry in
`tools/release_evidence.py` against the extracted digest-pinned CLI. The real CLI
E2E harness creates standalone scratch Git worktrees, applies actual direct
commands, and proves lifecycle/Goal rework, fences with no effects/outcome,
response loss and full replay, stdin recording, delegation, evidence continuations,
legacy refusal/reset, retired-route guidance and disabled debug hooks.

One helper-owned private root contains the immutable CLI snapshot and every
fixture. Prefer mounted writable `/Volumes/RootKernel/tmp`, otherwise the system
temporary directory. Explicit fixture-root and `TMPDIR` bindings, bounded deadlines
and process-group cleanup prevent scenarios from falling back to active worktrees.
No managed account runtime, socket, daemon process or service control is involved.
Success requires reaped children, released fixture locks, and an empty owned
worktree directory. Failure/interruption reports exact recoverable owned paths;
reconcile only those paths before retrying. Installed legacy account runtime and
LaunchAgent state remain untouched.

Packaging first records the closed required scenario list as `pending`.
Qualification changes it atomically to `passed` only after every scenario and
cleanup passes. Failure leaves pending provenance byte-for-byte unchanged.
Handoff creation rejects pending, malformed or incomplete evidence. Final offline
bundle verification independently re-extracts the archive and compares its layout,
checksum, single binary, manifest-bound assets, adapter catalog, source identities,
lockfile, provenance and handoff in both directions.

### Reduced patch qualification

A reduced gate requires Master's explicit choice, a separately confirmed successful
`make test` on the same clean code candidate, an exact patch-version increment,
and the recorded tested baseline. Run
`make dist-patch PRIOR_MAKE_TEST_PASSED=yes PATCH_BASE_COMMIT=<sha>` only from the
clean version-bumped release commit. After the baseline, only canonical version and
release metadata may change. The predecessor declaration and manifest path must
match the patch verifier's closed policy. Any compiled predecessor registry or
migration implementation must already be in that tested baseline; the reduced
gate cannot add or alter them. Other changes, missing confirmation or a
failed guard stop the release; switching to full requires a new instruction.

Patch qualification does not run fuzzing or packaged runtime scenarios. Its closed
provenance/handoff omit those fields and never represent them as `skipped` or
`passed`. Its own sentinels and final bundle verification remain required.

## Checksums and provenance

Every downloadable archive has a SHA-256 checksum. Full/development evidence uses
`podway.release-provenance/v3` and `podway.dolgorae-compatibility-handoff/v4`;
patch evidence uses provenance v4 and handoff v5. These closed contracts bind
artifact class, single binary digest, archive digest, build and manifest identities,
source commit/tree/snapshot and dirty state, target, toolchain, Cargo.lock, signing/
notarization status and the exact gate evidence. Development evidence uses
`make test + make release-prepare + make test-fuzzing: passed` and is nonpublishable.

The handoff embeds the manifest-bound
[`podway.dolgorae-adapter-contract/v3`](../../../release/dolgorae-adapter-contract-v3.json)
with direct routes, schemas, errors, diagnostics, reactivation and writer-predecessor
rules plus downstream acceptance requirements. The catalog omits the final manifest
digest to avoid self-reference; the handoff supplies that pin. Source/package
catalog drift is rejected. Preparing this contract does not certify Aquarium or
Dolgorae adoption or change either external repository.

### Consumer adoption prerequisites

Every public contract correction must land before the stable `v0.2.13` candidate
is qualified. Aquarium and Dolgorae pin that final release's exact contract
identity once, after the canonical assets, manifest and adapter are settled.
Intermediate source identities and earlier development receipts do not authorize
consumer activation or promise compatibility with the final release.

Installation of `v0.2.13` waits for Aquarium and Dolgorae releases that support
the direct contract generation. Their implementation and qualification belong
to their own repositories; a prepared Podway handoff alone proves neither.

Aquarium must consume Output v4, Error v2 and observation result v5, recognize
committed direct mutation success, and replace job/admission settlement with
read-only `outcome lookup`. It must remove daemon readiness and runtime-mode
routing and use scratch worktrees for development isolation. Procedure v2's
single-cursor lifecycle, fences, Goal, evidence and delegation semantics,
input/output bounds and idempotency requirements remain.

Dolgorae must adopt adapter contract v3 and pin the exact manifest v2 and version
result v2 identities, routes, schemas, error catalog and Store compatibility.
Version text alone cannot establish consumer compatibility. Both consumers must
qualify and release their changes before installation; legacy writer retirement
still requires the original owners and separate authorization described below.

## Installation

Install the single qualified `podway` through the archive or original package
manager. Verify its version/manifest/build identity, then initialize each desired
worktree with `podway init`. No daemon or service installation, automatic worktree
scan or account-state initialization is required. An existing legacy installed
service is retired only under separate explicit authorization through its original
supported owner; installing this CLI does not authorize stopping it.

## Code signing and notarization

Public macOS artifacts SHOULD be Developer ID signed, notarized, stapled where
supported, and verified on the release host. Unsigned artifacts are valid when
release notes accurately describe signing/notarization status. Signing status does
not replace release readiness. Any signing or artifact-byte change after
qualification requires qualification of the final changed bytes.

## Upgrade

Before installing the current CLI or resetting legacy worktrees, finish or
abandon active sessions and retire every legacy writer. Under separate explicit
authorization, uninstall the old daemon with the old generation's
`podway daemon uninstall` and retire any Aquarium managed development service
through its original controller. Confirm that those writers have stopped; a
running old daemon can write to unlinked files during reset. If retirement is
unresolved, stop the real upgrade at read-only inspection. Installing the new
CLI or receiving a reset recipe never grants service-control authority.

After retirement, install the current single CLI, retire the legacy `~/.podway`
account state under the original owner's authorization, and inspect worktrees
explicitly. The `v0.2.13` cutover treats SQLite generations 1–13 and old
maintenance markers as unsupported legacy input. Classification refuses without
opening for writes, migration,
checkpointing or SHM creation and reports `LEGACY_RUNTIME_STATE_UNSUPPORTED` with
an explicit reset recipe. There is no daemon-era state conversion.
The unreleased generation-14 direct baseline is also refused unchanged with this
older-state error and reset recipe; generation 15 declares no predecessor for it.

After backing up any desired historical evidence and confirming the exact target,
run `podway --worktree <root> --json reset --all --yes --idempotency-key <new-key>`.
This deliberately discards legacy runtime history and initializes a fresh direct
workspace while preserving configuration unrelated to retired runtime modes,
Procedure files, `.gitignore` and Git/unrelated files. A normal start and begin
then establishes a new session.
[Upgrade QA](../../implementation-tips/release-upgrade-qa.md) proves unchanged
refusal, explicit reset and the working fresh lifecycle independently on private
frozen fixtures. It does not retire installed writers or establish the real
upgrade's retirement prerequisite.

A direct Store records its writer contract-manifest digest. A different digest
fails with `WORKSPACE_CONTRACT_MISMATCH` unless the build explicitly declares that
forward-migratable predecessor. Predecessors are added only in a release commit or
the following open-development commit. `v0.2.13` declares none; direct predecessor
migration begins with a later release. Development builds with changed assets use
scratch worktrees rather than runtime modes.

## Downgrade

Database downgrade is unsupported. A newer schema or undeclared writer digest
fails closed without changing the Store. Use a compatible writer for newer
generations; an older binary refuses even destructive reset and removal. A
compatible generation that fails writer, schema or binding qualification,
including unreadable rows or pages, can instead be discarded by a confirmed
unfenced reset or interactive removal. A version string
alone never permits reopening incompatible state.

## Uninstall

Remove the binary through its original installation mechanism. This does not
delete any worktree's `.podway`. Use the supported, explicitly confirmed
`workspace remove` operation if that complete workspace content is no longer
wanted. Account service removal, if any legacy generation remains installed,
requires separate authorization and its original generation's owner.

## Release readiness

`make test` is the required development gate. Full release readiness requires
successful `make dist` on the exact clean release commit, including release
sentinels, bounded fuzzing, release build, packaged direct qualification, disabled
hook proof, handoff and final verification. A confirmed patch gate establishes
readiness only through its own reduced contract. A development-qualification dry
run establishes the adopted development proof and never stable-release readiness.

No hosted CI, independent signature, approval quorum, holdout archive or attestation
bundle is required. Publication, installation and runtime activation are separately
authorized operations. Changed executable/contract/source or artifact bytes
invalidate the affected exact-candidate qualification.

### Qualification and publication sequence

After separate explicit publication authorization, retain the qualified source and
all four artifact bytes (archive, checksum, provenance and handoff) unchanged:

1. reverify the qualified clean commit/tree, artifact names and SHA-256 identities;
2. record selected previous-release tag, asset IDs and digests and keep them intact;
3. create and push the annotated version tag for the exact qualified commit;
4. confirm repository release immutability before creating the release;
5. create a draft tied to that tag and upload exactly the four qualified files;
6. compare host-reported digests, download each draft asset by immutable asset ID,
   and independently verify the complete bundle;
7. publish only after those checks, then require `immutable=true`;
8. download every published asset by ID, verify again, and recheck previous assets;
9. record authorization, publisher/UTC time, URL/release/tag IDs, source/tree,
   asset IDs/digests, binary/manifest/lockfile/build/provenance/handoff identities,
   signing/notarization and verification results in the release's publication report.

Do not rebuild, rewrite, resign or mutate qualified artifacts during publication.
If immutability is unavailable, a mutable release is not an automatic fallback:
a separate explicit release-time decision must identify the missing control and
pinned asset identities. Otherwise leave/remove the draft and do not claim a
published stable release. The historical v0.2.0 sequence and its release-specific
owners remain recorded in the
[v0.2.0 release report](../../roadmap/archive/v0.2.0-release-report.md).

## Support policy

Podway supports currently supported macOS major versions on native Apple Silicon.
The native gate checks the deployment load command without promising a separately
pinned minimum macOS version. Another architecture/OS requires a superseding ADR
and native release gate.

## License

Source and release archives include the MIT License. Third-party dependencies keep
their own licenses and require compatibility review.
