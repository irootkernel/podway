# Repository Structure

## Workspace map

```text
crates/                 Rust workspace crates
contracts/              Executable dependency, route, and handoff contracts
docs/                   Canonical contributor docs and source assets
presets/                Generated built-in preset mirror
quality/                Crash-boundary registry
release/                Product-acceptance mapping
schemas/                Generated JSON Schema mirror
spec/                   Generated executable specification mirror
tests/fixtures/          Shared contract fixtures
tools/                  Verification, synchronization, E2E, fuzz, and release tools
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

## Canonical and generated assets

Edit `docs/schemas/`, `docs/spec/`, and `docs/presets/`. The synchronization tool
copies those files byte-for-byte to the root mirrors consumed by Rust builds and
release packaging. The import contract records the expected paths and SHA-256
digests, so unreviewed drift fails closed.

The detailed crate and runtime rules are in the
[Rust codebase reference](reference/architecture/14-rust-codebase.md).
