# Repository Workflow

## Prerequisites

- Native Apple Silicon macOS for complete release verification.
- rustup with Rust 1.97.1, selected by `rust-toolchain.toml` and the Makefile.
- `cargo-deny` for dependency and license checks.
- `nightly-2026-07-17` and `cargo-fuzz 0.13.2` for bounded fuzzing.

`Cargo.lock` is committed because Podway is an application. Do not replace the
pinned toolchain with an unrelated Rust installation from `PATH`.

## Change workflow

1. Identify the owning crate and affected public or internal contract.
2. Read the relevant architecture, specification, machine asset, and ADR.
3. State the invariant, migration, compatibility rule, or error contract affected.
4. Add or update a focused test that observes the required behavior.
5. Run focused checks, then the complete gate before release acceptance.

Changes to lifecycle semantics, commands, JSON, IPC, SQLite, worktree identity,
trust boundaries, artifact handling, or release scope require synchronized
documentation and contract updates.

## Canonical assets

Build-consumed assets have exactly one source:

- `assets/presets/`: built-in procedures;
- `assets/schemas/`: public JSON Schemas;
- `assets/specifications/`: executable catalogs, DDL, transition data,
  canonicalization rules, and the LaunchAgent template.

Create or import a preset candidate with:

```bash
make preset-create PRESET_ID=my-preset \
  PRESET_NAME="My preset" \
  PRESET_DESCRIPTION="Purpose of the preset"

make preset-import PRESET_FILE=/absolute/path/to/preset.yaml
```

These contributor commands validate through the real CLI and write to
`assets/presets/` by default. They do not automatically add a candidate to the
embedded catalog.

See the [repository structure](../architecture/repository-structure.md) for crate
ownership and dependency direction.
