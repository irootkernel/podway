# Contributing

## Prerequisites

- Native Apple Silicon macOS for complete release verification.
- rustup with Rust 1.97.1, selected by `rust-toolchain.toml` and the Makefile.
- `cargo-deny` for dependency and license policy checks.
- The separately pinned nightly toolchain and `cargo-fuzz 0.13.2` for bounded fuzzing.

`Cargo.lock` is committed because Podway is an application. Do not replace the
pinned product toolchain with a different Rust installation from `PATH`.

## Before changing code

1. Identify the owning crate and the public or internal contract involved.
2. Read the relevant core guide, detailed reference, machine contract, and ADR.
3. Decide which invariant, error code, migration, or compatibility rule the change
   affects before editing implementation code.
4. Add or update a focused test that observes the intended behavior.

Changes to lifecycle semantics, public commands, JSON or IPC, SQLite, worktree
identity, trust boundaries, artifact handling, or release scope require synchronized
documentation and contract updates.

## Implementation tips

- Keep domain transitions pure and pass clocks, IDs, digests, and prepared inputs
  explicitly.
- Preserve the daemon's single-writer and per-worktree serialization boundaries.
- Bound user-controlled input, queues, frame sizes, file reads, and shutdown waits.
- Avoid panics on user input and keep public errors stable and structured.
- Treat paths as platform bytes internally; validate display strings only at public
  serialization boundaries.
- Check containment without following untrusted symlinks.
- Keep migrations forward-only, deterministic, and transactional.
- Add a regression test for every state, queue, recovery, or path-safety bug.
- Prefer exhaustive matches for public enums and explicit failure over hidden fallback.

## Canonical assets and presets

Edit only the sources under `docs/schemas/`, `docs/spec/`, and `docs/presets/`, then run:

```bash
make sync-docs-assets
```

Create or import a built-in preset candidate with:

```bash
make preset-create PRESET_ID=my-preset \
  PRESET_NAME="My preset" \
  PRESET_DESCRIPTION="Purpose of the preset"

make preset-import PRESET_FILE=/absolute/path/to/preset.yaml
```

These are contributor tools, not public Podway commands. Expanding the shipped
catalog also requires documentation, embedded-catalog, schema, and end-to-end work.

## Verification layers

```bash
make test-prepare   # sync, format, static checks, contracts, architecture
make test-unit      # library, binary, and documentation tests
make test-int       # component integration with fixtures and doubles
make test-fuzzing   # bounded deterministic protocol fuzzing
make test-e2e       # real podway and podwayd scenarios
make test           # complete release-readiness gate
```

Run the narrowest relevant layer while iterating and `make test` before treating a
revision as release-ready. `test-prepare` synchronizes assets and runs `rustfmt`, so
review its resulting diff before committing.

## Documentation rules

- Write `README.md` and all Markdown under `docs/` in English.
- Link to the most specific stable heading that supports a claim.
- Update the detailed reference and machine contract together when behavior changes.
- Keep the roadmap historical; new work belongs there only after the project adopts
  a roadmap policy for future releases.
- Run the documentation verifier through `make test-prepare` after renaming or moving
  a document.

## Release artifacts

`make dist` runs the complete gate, builds thin arm64 release binaries, and creates
the deterministic archive, SHA-256 checksum, and provenance JSON under `dist/`.
It requires a clean native Apple Silicon working tree. Signing and notarization
describe the distributed artifact but are not additional source-readiness gates.

See [quality](reference/quality/60-testing-and-conformance.md),
[product acceptance](reference/quality/61-product-acceptance.md), and
[release packaging](reference/operations/52-release-and-packaging.md) for the full contracts.
