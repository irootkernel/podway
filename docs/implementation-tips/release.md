# Release Workflow

The repository-root `make test` command is the development gate. `make dist` is
the complete release gate and must run from the exact clean revision being
packaged.

After version and release-note preparation:

1. run `make dist` on native Apple Silicon macOS;
2. verify the archive, checksum, provenance, and Dolgorae handoff;
3. confirm signing and notarization statements match the artifact;
4. tag the exact source revision and publish all required files.

`make dist` always runs the development gate and bounded fuzzing before it builds
thin arm64 binaries and qualifies the packaged CLI/daemon contract in isolated dev
mode. Distribution construction rejects dirty trees, translated execution,
non-arm64 binaries, version mismatches, and stale layouts.
The native-host and clean-worktree preflight runs before the expensive test gate,
while packaging repeats those checks before writing release artifacts.

See the normative [release and packaging specification](../specs/operations/release-and-packaging.md)
and the [active roadmap](../roadmap/).
