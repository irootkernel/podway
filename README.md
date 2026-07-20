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
satisfy native Apple Silicon release acceptance. The current version is
unreleased: no Apple-Silicon-specific release evidence, signing, notarization, or
runtime-lifecycle claim is made here.

The product is intentionally local and constrained:

- It performs no network I/O.
- It never executes arbitrary commands or provides a command runner.
- It exposes no Git mutation APIs and never mutates Git state.

## Development

Use the pinned stable Rust toolchain in [`rust-toolchain.toml`](rust-toolchain.toml).
CI checks formatting, builds, Clippy warnings, tests, and contract verification on
macOS and Linux. `Cargo.lock` is committed because Podway is an application.
