# Podway

Podway is a local, worktree-scoped procedure runner for durable software-delivery workflows.

## Status

The workspace is at **v0.1.0** and implements the frozen **schema v1** interfaces. The
[`sot/`](sot/) directory is the authoritative source of truth for product behavior,
contracts, and compatibility requirements.

## Platform and safety boundary

Podway release artifacts target only untranslated native Apple Silicon macOS
(`aarch64-apple-darwin`, `arm64`, host architecture `arm64`, Mach-O architecture
`arm64`). Cross-built, relabeled, translated, and universal/fat binaries cannot
satisfy the supported platform contract. A revision is release-ready when the
repository-root `make test` command succeeds locally. Signing and notarization
status describe a published artifact and do not add another release gate.

The product is intentionally local and constrained:

- It performs no network I/O.
- It never executes arbitrary commands or provides a command runner.
- It exposes no Git mutation APIs and never mutates Git state.

## Development

Use the pinned Rust 1.97.1 toolchain in [`rust-toolchain.toml`](rust-toolchain.toml).
The root Makefile resolves that exact rustup toolchain for Cargo, rustc, and all
verification subprocesses, even when another Rust installation appears earlier in
the invoking shell's `PATH`. `Cargo.lock` is committed because Podway is an
application. The repository's canonical verification entry point is the root
`Makefile`:

- `make test` runs prepare, unit, integration, and end-to-end verification in order.
- `make test-prepare` synchronizes generated SOT assets, rewrites Rust formatting,
  and runs vet, lint, dependency, architecture, and contract guardrails.
- `make test-unit` runs narrow library, binary, and documentation tests.
- `make test-int` runs component-integrated scenarios with deterministic fixtures
  and test doubles, without launching Podway product binaries.
- `make test-e2e` builds and launches the real `podway` and `podwayd` binaries and
  includes the ignored production-vertical scenarios.
- `make dist` reruns `make test`, builds both release binaries, and writes the
  deterministic archive, SHA-256 checksum, and provenance document under `dist/`.

The prepare target intentionally modifies generated files and formatting. Install
`cargo-deny` before running the complete suite. Release tags and archives must use
the resulting formatted tree; later source changes require another complete run.
Distribution requires a clean native Apple Silicon working tree.

See the [`docs/` index](docs/README.md) for the implementation/operator guide and
the boundary between those guides and the normative SOT. The release policy is in
[`docs/release-readiness.md`](docs/release-readiness.md).
