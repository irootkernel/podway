# Release Workflow

The repository-root `make test` command is the development gate. Agent-operated
release gate selection is owned by [AGENTS.md](../../AGENTS.md#release-gate-selection):
ask Master for full or reduced patch qualification, and obtain the separate
explicit prior-`make test` confirmation before a reduced gate. `make dist` is the
full release gate and runs from the exact clean release commit. Neither a gate
nor a development dry run authorizes publication, installation, or activation.

When the contract surface changes, regenerate the downstream adapter catalog from
repository authorities. Refresh the manifest to validate changed schema bytes,
then refresh it again to bind the new adapter catalog:

```bash
python3 tools/contract_manifest.py --write
python3 tools/create_dolgorae_handoff.py prepare-adapter
python3 tools/contract_manifest.py --write
```

This updates Podway's manifest-bound prepared contract. It does not modify
Dolgorae, qualify an archive, or establish downstream acceptance. The helper's
`self-test` rejects catalog, schema-pin, route, error, reactivation,
writer-predecessor, and handoff drift.

## Stable release gates

After canonical version and changelog preparation, the full gate runs `make dist`
on native Apple Silicon macOS. It performs the development gate, release helper
sentinels, bounded fuzzing, a release-profile build of the single `podway` binary,
archive construction, packaged qualification, Dolgorae handoff, and independent
bundle verification. Inspect the final receipt and signing/notarization statements
before any separately authorized tag or publication operation.

The native-host and clean-source preflight runs before the expensive gate and is
repeated before packaging. Distribution construction rejects dirty source,
translated execution, non-thin-arm64 binaries, stale contract layouts, identity
mismatches, and enabled test hooks. `podway.output/v4`,
`podway.version-result/v2`, and `podway.contract-manifest/v2` carry the current
identity; there is no IPC identity or matching daemon executable.

For a confirmed reduced patch gate, record the clean tested baseline before
changing version metadata. From the clean version-bumped release commit run:

```bash
make dist-patch PRIOR_MAKE_TEST_PASSED=yes PATCH_BASE_COMMIT=<tested-baseline-sha>
```

Only canonical version and release metadata may differ from that baseline.
Missing confirmation, unrelated changes, or a failed check stops the release;
there is no automatic fallback to the full gate. Patch evidence omits fuzzing and
packaged conformance rather than recording invented `skipped` results.

In the open-development commit after each stable release is published, append
that release's contract-manifest digest to both
`RELEASED_CONTRACT_MANIFESTS_V1`
([`crates/podway-protocol/src/result_contract.rs`](../../crates/podway-protocol/src/result_contract.rs))
and the released-writer table of the runtime skill
([`recovery.md`](../../skills/use-podway/references/recovery.md)). Both tables
stay aligned; the append follows ADR-0039 decision 5 and feeds
`actual_product_version` for generation-2 writer-mismatch details. No digest is
appended for an unreleased candidate. The same commit co-updates the pinned
expectations: the exact-contents unit test
`guide007_released_manifest_table_pins_published_digests` in
`crates/podway-protocol/src/result_contract.rs`, the length assertion in
`crates/podway-protocol/tests/int_guide007_contract_generations.rs`, and a rerun
of the known- and unknown-digest mismatch checks.

## Non-distribution development qualification

V2DFC-009 requires a native dry run before delegated acceptance and its normal
completion commit. The exact frozen development candidate may be uncommitted;
record its real dirty state, HEAD/tree, and source snapshot rather than presenting
it as a clean distribution. Freeze source before the final gates and build:

```bash
make development-qualification DEVELOPMENT_OUTPUT_DIR=<fresh-absolute-owned-output>
```

`tools/qualify_development.py` captures one source snapshot before `make test`,
`make release-prepare`, `make test-fuzzing`, and the pinned release build. It
rechecks that original snapshot before and after each step and stops on drift.
Its `development-checks.json` receipt records the exact commands, exits, source
bindings and built CLI digest. Development packaging requires this completed
receipt and the original `--development-source-snapshot-sha256`; a new snapshot
at package time cannot replace the checked source. The same runner performs
package, qualification, handoff and final bundle verification with
`--gate development`. No clean release guard is relaxed.

Every stage rechecks the source snapshot. This class uses the gate label
`make test + make release-prepare + make test-fuzzing: passed`, keeps all test hooks
disabled, and is explicitly nonpublishable. It records truthful `source_dirty=true`
and permits an open current changelog section or one newer `Unreleased` section
followed immediately by the published current version. It never relaxes
distribution or patch guards.
A source change invalidates the bound evidence and requires the affected gates
and dry run again. Do not execute `make dist`, bump the version, install a binary,
or control an installed runtime merely to complete this development proof.

## Runtime isolation and cleanup

Qualification runs the extracted, digest-pinned single CLI against standalone Git
repositories under one helper-owned private fixture root. It snapshots the CLI,
uses that binary in the real CLI E2E scenarios, and bounds each child process and
its process group. `TMPDIR` and the fixture-root override point to that same owned
root. Prefer `/Volumes/RootKernel/tmp` only after verifying the volume is mounted
and writable; otherwise use the system temporary directory. Source-repository
worktrees and installed account runtimes are never fixture targets.

Cleanup is part of qualification. Success requires every child to be reaped,
every fixture lock released, and the exact owned worktree directory emptied.
Failure or interruption reports recoverable owned paths and requires bounded
reconciliation before retrying. Never broadly delete a temporary directory or
modify an installed LaunchAgent, legacy account runtime, or unrelated workspace.
Existing legacy contributor/Aquarium runtime state remains untouched until its
original owner performs a separately authorized retirement.
For a real installed upgrade, complete the ordered
[legacy-writer retirement](../specs/operations/release-and-packaging.md#upgrade)
before installing the new CLI or resetting legacy worktrees. Qualification on
private frozen fixtures does not establish that installed prerequisite.

The scenario list is the closed `PACKAGED_CONFORMANCE_SCENARIOS` registry in
`tools/release_evidence.py`. It covers actual direct lifecycle and Goal rework,
fence conflicts with no outcome, bounded SQLite contention without an outcome,
atomic stdin recording, delegation, response loss after commit and exact replay,
legacy refusal and explicit reset, evidence paging, retirement guidance, and
disabled debug hooks. Packaged qualification is
development evidence; Aquarium's independent release QA separately executes
[upgrade QA](release-upgrade-qa.md) and, when the delta affects mutations or
reconciliation, [direct mutation outcome QA](release-admission-qa.md).

Qualification atomically changes conformance from `pending` to `passed` only after
all scenarios and cleanup succeed. Failure leaves the pending provenance bytes
unchanged. Handoff creation rejects incomplete evidence; final verification
independently re-extracts and compares archive, checksum, single binary, manifest,
adapter catalog, lockfile, source identity, provenance, and handoff.

See the normative [release and packaging specification](../specs/operations/release-and-packaging.md)
and the [active roadmap](../roadmap/README.md).
