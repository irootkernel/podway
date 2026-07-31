# Release Readiness

Podway uses one executable release gate: run `make test` locally from the repository
root. That command is authoritative only for requirements and evidence already
registered in the tested tree. Podway v0.1.0 is not release-ready while any
release-blocking task in the [roadmap](roadmap.md) remains incomplete; a current
successful run proves the implemented tree, not completion of the remaining
release tasks.

Once all release tasks are complete, the final clean-tree
`make test` run is the sole executable source-readiness decision. It runs generated
source checks, unit and integration tests, bounded fuzzing, and real-binary E2E.

The gate requires rustup and resolves exact toolchains. Rust 1.97.1 builds and tests
the product. The isolated fuzzing target uses `nightly-2026-07-17` and
`cargo-fuzz 0.13.2` for sanitizer and coverage instrumentation only. A Homebrew or
system Rust earlier in the caller's `PATH` is not used by either path.

The gate is intentionally local. GitHub Actions, a hosted release pipeline,
independent signatures, approval quorums, holdout runs, qualification archives, and
attestation bundles are not part of Podway release readiness.

`make test-prepare` rewrites generated files and Rust formatting before checking the
workspace. A release tag or archive must therefore be created from the resulting
formatted tree, not from stale pre-format bytes.

## Gate composition

| Target | Scope |
|---|---|
| `make test-prepare` | canonical asset synchronization, formatting, lint, dependency policy, architecture guardrails, product-acceptance mapping, crash-boundary mapping, and contract checks |
| `make test-rust` | Unit and architecture targets plus one integration suite per crate in one Cargo invocation |
| `make test-unit` | Focused library and binary tests for iteration |
| `make test-int` | Focused component integration, including a product component with controlled doubles |
| `make test-fuzzing` | Fixed-run, fixed-seed frame-decoder and request-envelope fuzzing in disposable corpora |
| `make test-e2e` | User journeys using actual product binaries, shells, and release archives |

The architecture portion also exercises the contributor-only preset tooling against
the real Podway validator. `make preset-create` and `make preset-import` prepare
canonical source candidates; they do not add public CLI commands or expand the four
preset v0.1 catalog without the remaining documentation, catalog, and end-to-end work.

Signing, notarization, archive assembly, checksum publication, and release-note
publication may still be performed when distributing a build. They describe the
published artifact and do not introduce another release-readiness gate.

## Distribution

Run `make dist` from a clean native Apple Silicon working tree. The target reruns
the complete `make test` gate, builds thin arm64 release binaries, and creates:

- `dist/podway-0.1.0-aarch64-apple-darwin.tar.gz`;
- the archive's `.sha256` file;
- `dist/podway-0.1.0-aarch64-apple-darwin.provenance.json`.

The archive builder rejects non-arm64 Mach-O binaries, incomplete layouts, stale
binary versions, a non-1.97.1 Rust toolchain, and a dirty tracked or untracked tree.
The local gate exercises the same builder with real debug binaries in temporary
directories, records `artifact_class=test-fixture` and `release_gate=test-fixture`, requires
both binaries to expose debug-only isolation before any service mutation, and proves
that repeated construction produces the same archive digest. `make dist` instead
requires `artifact_class=distribution` and rejects binaries that expose that isolation
capability. It also rejects `--allow-dirty`, which is reserved for test fixtures.
Final packaged release conformance runs only under a disposable macOS
account with an isolated launchd user domain; it must not reuse the debug fixture
override against a real user account.
Rebuild the archive whenever history is rewritten after packaging; published
provenance `source_commit` must equal the exact release-tag commit.

## Current implementation state

The historical baseline and the automation client work through `DOLGI` are
represented in the local gate. The `REL10` tasks remain incomplete, so
`REL10005` stays blocked until contract freeze, the final clean-tree gate,
distribution construction, and compatibility handoff are complete. Generated
reports from superseded qualification systems are not release inputs.
