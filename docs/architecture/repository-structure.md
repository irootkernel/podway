# Repository Structure

## Workspace map

```text
assets/                 Canonical presets, public schemas and executable specifications
crates/                 Eight Rust workspace crates
contracts/              Executable repository and interface contracts
docs/                   Human-readable contributor documentation
quality/                Current crash-boundary registry
release/                Manifest-bound external adapter contract
skills/                 Optional source-distributed AI agent guidance
tests/fixtures/         Shared contract data and reference-path inventories
tools/                  Verification, E2E, fuzzing and release tools
artifacts/              Ignored host-local verification output
target/                 Ignored build and local execution output
dist/                   Ignored distribution or development-qualification output
```

## Crates

| Crate | Responsibility |
|---|---|
| `podway-core` | Pure domain values, invariants, transitions and status |
| `podway-config` | Workspace/Procedure parsing, validation, canonicalization and digests |
| `podway-presets` | Canonical embedded presets and catalog validation |
| `podway-protocol` | Public JSON, bounded direct inputs, identity and offline release verifier |
| `podway-store` | Direct SQLite state, atomic success outcomes, maintenance leases and retention |
| `podway-git` | Read-only worktree discovery, identity, containment and layout |
| `podway-engine` | Typed Direct execution, preparation, projections and native maintenance |
| `podway-cli` | Direct requests, grammar, validated rendering, authoring, help and completions |

`podway` is the only product binary. The feature-gated `podway-contract-verifier`
is an internal verification tool. No daemon/service crate or shipped `podwayd`
remains. Durable dependencies are declared by
[`cargo-adjacency.json`](../../contracts/cargo-adjacency.json).

## Dependency direction

Core has no infrastructure edge. Config, protocol, Store and Git depend on core;
presets depend on core/config. Engine composes these six crates and owns native
workspace execution. CLI invokes Engine in-process and has no production Store or
Git dependency. Executable architecture checks reject cycles, reverse edges and
unauthorized direct access. Detailed rules are in the
[Rust codebase reference](rust-codebase.md).

## Tests

Each crate owns its test targets:

- `arch_*` checks dependency and repository contracts without product execution;
- `int_*` combines components with deterministic fixtures or native boundaries;
- `e2e_*` launches the real CLI in owned scratch worktrees.

Modules are registered in aggregate suites rather than standalone unregistered
Cargo targets. Root `tests/fixtures/` contains shared data. Fuzz targets live in
`fuzz/`; current crash mappings live in `quality/`.

## Canonical assets

Edit schemas, specifications and presets only in `assets/schemas/`,
`assets/specifications/` and `assets/presets/`. Builds, tests and packaging consume
these canonical trees. Public logical paths remain `schemas/`, `spec/` and
`presets/`. The generated contract manifest is
`contracts/contract-manifest-v2.json`; use the documented manifest tool rather
than hand-editing it. There is no generated documentation mirror.

## Executable repository contracts

`contracts/` owns crate adjacency, command routes, manifest identity and internal
interface inputs. `quality/crash-boundaries-v1.json` owns current crash proof
locators. Historical ADRs and roadmap entries describe retired generations;
they do not reintroduce removed executables, schemas or runtime contracts.

Host-local artifacts and temporary qualification state are not canonical source
or durable passing evidence. `make test` is the development gate. Release gates
are `make dist` or the separately confirmed `make dist-patch` under the
[release workflow](../implementation-tips/release.md). A non-distribution native
dry run is nonpublishable and does not authorize release or installation.

`skills/` contains optional independently installable guidance. `use-podway`
owns explicitly requested runtime/lifecycle operations;
`create-podway-procedure` owns Procedure source authoring and distribution evidence.
Podway does not install these skills or include them in its archive. Procedure
runtime qualification remains a separately authorized operation.
