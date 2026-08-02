# Release Workflow

The repository-root `make test` command is the sole source-readiness gate. A
release must use the exact clean revision and toolchains bound by that successful
gate.

After version and release-note preparation:

1. run the complete clean-tree gate;
2. run `make dist` on native Apple Silicon macOS;
3. verify the archive, checksum, provenance, and Dolgorae handoff;
4. confirm signing and notarization statements match the artifact;
5. tag the exact source revision and publish all required files.

`make dist` reuses an exact valid test receipt or runs the full gate. It builds
thin arm64 binaries and qualifies the packaged CLI/daemon contract in isolated dev
mode. Distribution construction rejects dirty trees, translated execution,
non-arm64 binaries, version mismatches, and stale layouts.

See the normative [release and packaging specification](../specs/operations/release-and-packaging.md)
and the [active roadmap](../roadmap/).
