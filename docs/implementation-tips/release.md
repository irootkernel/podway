# Release Workflow

The repository-root `make test` command is the development gate. `make dist` is
the complete release gate and must run from the exact clean revision being
packaged.

After version and release-note preparation:

1. run `make dist` on native Apple Silicon macOS;
2. inspect the final verifier receipt for the archive, checksum, provenance, and
   Dolgorae handoff;
3. confirm signing and notarization statements match the artifact;
4. tag the exact source revision and publish all required files.

`make dist` always runs the development gate and bounded fuzzing before it builds
thin arm64 binaries and qualifies the packaged CLI/daemon contract in isolated dev
mode. Distribution construction rejects dirty trees, translated execution,
non-arm64 binaries, version mismatches, and stale layouts.

The native-host and clean-worktree preflight runs before the expensive test gate,
while packaging repeats those checks before writing release artifacts. The early
preflight probes the effective account's fixed production singleton lock without
trusting `HOME` or touching its socket, so stop any production or raw foreground
`podwayd --dev` daemon before starting the gate. The managed
[contributor development runtime](dev-runtime.md) uses a disjoint debug account
root and does not satisfy or replace that production-lock preflight.

Qualification changes packaged-conformance evidence from `pending` to `passed`
only after all extracted scenarios succeed; handoff generation rejects anything
else. The last `make dist` command independently re-extracts and cross-checks the
complete archive/provenance/handoff identity set.

See the normative [release and packaging specification](../specs/operations/release-and-packaging.md)
and the [active roadmap](../roadmap/).
