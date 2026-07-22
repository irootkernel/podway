# Release Readiness

Podway uses one required release gate: run `make test` locally from the repository root.
The revision is release-ready when that command exits successfully. The command runs,
in order, generated-source synchronization and static checks, unit tests, integration
tests, bounded protocol fuzzing, and real-binary end-to-end tests.

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
| `make test-prepare` | SOT import, formatting, vet, lint, dependency policy, architecture, product-acceptance mapping, crash-boundary mapping, and contract checks |
| `make test-unit` | Narrow library, binary, and documentation tests |
| `make test-int` | Multi-component scenarios using fixtures and test doubles without product binaries |
| `make test-fuzzing` | Fixed-run, fixed-seed frame-decoder and request-envelope fuzzing in disposable corpora |
| `make test-e2e` | User scenarios using the actual `podway` and `podwayd` binaries |

The architecture portion also exercises the contributor-only preset tooling against
the real Podway validator. `make preset-create` and `make preset-import` prepare
canonical source candidates; they do not add public CLI commands or expand the four
preset v0.1 catalog without the remaining SOT, catalog, and end-to-end work.

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
directories and proves that repeated construction produces the same archive digest.

## Current implementation state

The implementation and all SOT requirements are represented in the local gate. A
successful `make test` is the authoritative evidence for the tested tree; generated
reports from superseded qualification systems are not release inputs.
