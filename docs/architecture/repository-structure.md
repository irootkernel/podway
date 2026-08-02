# Repository Structure

## Workspace map

```text
assets/                 Canonical presets, schemas, and executable specifications
crates/                 Rust workspace crates
contracts/              Executable repository contracts and stable evidence
docs/                   Human-readable contributor documentation
quality/                Crash-boundary registry
release/                Product-acceptance mapping
tests/fixtures/          Shared contract fixtures
tools/                  Verification, E2E, fuzz, and release tools
artifacts/              Ignored, host-local verification output
```

## Crates

| Crate | Responsibility |
|---|---|
| `podway-core` | Pure domain types, invariants, transitions, status, and next-action derivation |
| `podway-config` | Workspace and procedure parsing, validation, canonicalization, and digests |
| `podway-presets` | Embedded built-in presets and catalog validation |
| `podway-protocol` | IPC framing, public envelopes, compatibility, and bounded decoding |
| `podway-store` | SQLite schema, repositories, durable jobs, transactions, and recovery |
| `podway-git` | Worktree discovery, identity, containment, and layout |
| `podway-service` | Platform paths, service-manager contract, and macOS LaunchAgent integration |
| `podway-daemon` | Socket server, registry, scheduler, dispatch, workers, and observability |
| `podway-cli` | Command grammar, daemon client, rendering, help, and shell completion |

## Dependency direction

`podway-core` has no infrastructure dependencies. Config and protocol may depend
on core value types. Store, Git, and service code implement infrastructure around
those contracts. The daemon composes infrastructure; the CLI communicates through
the protocol and never reaches into store internals. Cycles and reverse edges are
rejected by executable architecture contracts.

## Tests

Each crate owns its integration targets under `crates/<crate>/tests/`:

- `arch_*` checks dependency and repository contracts;
- `int_*` combines components with deterministic fixtures or test doubles;
- `e2e_*` may launch the real `podway` and `podwayd` binaries.

Root `tests/fixtures/` contains shared data, not independent Cargo test targets.
Fuzz targets live under `fuzz/`; release and crash mappings live under `release/`
and `quality/`.

## Canonical assets

Edit `assets/schemas/`, `assets/specifications/`, and `assets/presets/` directly.
Rust builds, tests, contract verification, and release packaging consume those
single canonical trees. Public logical paths remain `schemas/`, `spec/`, and
`presets/` even though their repository sources are grouped under `assets/`.

The detailed crate and runtime rules are in the
[Rust codebase reference](rust-codebase.md).

## Executable repository contracts

`contracts/` is not a documentation mirror. It contains the versioned canonical
import, dependency, command-route, manifest, requirement-evidence, and internal
interface inputs executed by repository verification tools.

`artifacts/` contains mutable reports, raw logs, fuzzing output, and other
machine-specific results. The entire directory is ignored and is not a release
input. Release readiness is decided only by `make dist` on the current tree.
