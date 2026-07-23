# Release Readiness

Podway uses one executable release gate: run `make test` locally from the repository
root. That command is authoritative only for requirements and evidence already
registered in the tested tree. Podway v0.1.0 is not release-ready while any
release-blocking task in the [roadmap](roadmap.md) remains incomplete; a current
successful run proves the implemented baseline, not the planned automation target.

Once all target requirements and tests are included, the final clean-tree
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
| `make test-prepare` | canonical asset synchronization, formatting, vet, lint, dependency policy, architecture, product-acceptance mapping, crash-boundary mapping, and contract checks |
| `make test-unit` | Narrow library, binary, and documentation tests |
| `make test-int` | Multi-component scenarios using fixtures and test doubles without product binaries |
| `make test-fuzzing` | Fixed-run, fixed-seed frame-decoder and request-envelope fuzzing in disposable corpora |
| `make test-e2e` | User scenarios using the actual `podway` and `podwayd` binaries |

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
directories and proves that repeated construction produces the same archive digest.
Rebuild the archive whenever history is rewritten after packaging; published
provenance `source_commit` must equal the exact release-tag commit.

## Current implementation state

The completed historical baseline is represented in the local gate. The automation
client contract is accepted target behavior, but its `RPATH` through `DOLGI`
implementation and conformance tasks remain planned. `REL10005` is therefore
blocked regardless of whether the unchanged baseline currently passes `make test`.
Generated reports from superseded qualification systems are not release inputs.

Raw verification reports and logs are host-local files under ignored `artifacts/`.
They must never be referenced directly by a tracked contract. When a Phase 0
handoff requires durable proof, `python3 tools/run_verification.py --attest`
publishes a host-neutral, content-addressed summary under `contracts/evidence/`;
the handoff binds that stable file by digest. A fresh source tree must validate
all tracked receipts without any pre-existing `artifacts/` directory.
