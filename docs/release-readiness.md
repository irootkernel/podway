# Release Readiness

Podway uses one required release gate: run `make test` locally from the repository root.
The revision is release-ready when that command exits successfully. The command runs,
in order, generated-source synchronization and static checks, unit tests, integration
tests, and real-binary end-to-end tests.

The gate requires rustup and resolves the exact Rust 1.97.1 toolchain for every Cargo,
rustc, and Python-launched verification process. A Homebrew or system Rust earlier in
the caller's `PATH` is not used by the gate.

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
| `make test-e2e` | User scenarios using the actual `podway` and `podwayd` binaries |

Signing, notarization, archive assembly, checksum publication, and release-note
publication may still be performed when distributing a build. They describe the
published artifact and do not introduce another release-readiness gate.

## Current implementation state

The implementation and all SOT requirements are represented in the local gate. A
successful `make test` is the authoritative evidence for the tested tree; generated
reports from superseded qualification systems are not release inputs.
