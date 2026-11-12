# Repository Workflow

## Prerequisites

- Native Apple Silicon macOS for complete release verification.
- rustup with the exact Rust version selected by `rust-toolchain.toml`.
- uv and the exact Python patch version in `.python-version` for contributor tools.
- `cargo-deny` for dependency and license checks.
- `nightly-2026-07-17` and `cargo-fuzz 0.13.2` for bounded fuzzing.

`Cargo.lock` is committed because Podway is an application. Do not replace the
pinned toolchain with an unrelated Rust installation from `PATH`.

## Rust version selection

Run ordinary `cargo` and `rustc` through rustup. On Apple Silicon macOS with
Homebrew rustup, put `/opt/homebrew/opt/rustup/bin` before Homebrew Rust in `PATH`,
and retain `$HOME/.cargo/bin` for installed Cargo tools. Apply this after login
PATH setup as well as in interactive and non-interactive shell initialization.
Check `command -v cargo rustc` and `rustup show active-toolchain` inside the repo.

`rust-toolchain.toml` is the single source for the exact stable build version.
Make and development/release qualification read it through the shared toolchain
reader; a missing installed version fails instead of selecting system Rust.
The workspace `rust-version` is the minimum supported Rust version, inherited by
all product crates, and is independent of the exact build pin. Fuzzing retains
its separately pinned nightly toolchain.

Updating Homebrew Rust or running `rustup update stable` does not change this
project's exact pin. To upgrade this project deliberately:

1. Install the selected version with `rustup toolchain install <version> --profile minimal --component clippy --component rustfmt`.
2. Change `toolchain.channel` in `rust-toolchain.toml` to that exact version.
3. Raise the workspace minimum only if support for the older compiler is being dropped.
4. Address applicable deferred toolchain-upgrade feedback, then run `make test` and `make lint-all` with the new pin.

No global default change or removal of another installed Rust is required.
Previously measured baselines retain their original compiler versions.

## Python version selection

`.python-version` is the single source for the exact Python interpreter version.
Use `./tools/python tools/<script>.py` for direct contributor commands, or the
standard Make targets. The launcher uses uv to find the exact installed version,
ignores active virtual environments and project/global uv configuration, and
never downloads Python or substitutes the system default. It verifies the
selected interpreter before executing the command. Bare `python3` and direct
script shebang execution do not select this project pin; use the launcher.

To upgrade deliberately, install the desired exact interpreter (for example,
`uv python install <version> --no-bin`), edit `.python-version`, then run
`make python-prepare` and `make test`. No shell or global Python default change is
required. The bootstrap launcher needs uv but does not need a system Python.
Make also uses it to read the Rust pin, removing the system-Python bootstrap gap.

Schema-validation dependencies are installed only in the versioned ignored
`target/dev-python/<version>` environment. A pin change selects a fresh
environment instead of reusing one created by a different Python. Existing
versioned environments are retained for reuse; a version mismatch fails with a
recreation instruction. This interpreter pin does not lock Python package
versions; `tools/requirements-dev.txt` remains the dependency policy.

## Change workflow

1. Identify the owning crate and affected public or internal contract.
2. Read the relevant architecture, specification, machine asset, and ADR.
3. State the invariant, migration, compatibility rule, or error contract affected.
4. Add or update a focused test that observes the required behavior.
5. Run focused checks, then the complete gate before release acceptance.

Changes to lifecycle semantics, commands, JSON, SQLite, worktree identity,
trust boundaries, artifact handling, or release scope require synchronized
documentation and contract updates.

## Canonical assets

Build-consumed assets have exactly one source:

- `assets/presets/`: built-in procedures;
- `assets/schemas/`: public JSON Schemas;
- `assets/specifications/`: executable catalogs, DDL, transition data and
  canonicalization rules.

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
