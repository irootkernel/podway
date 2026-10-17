# Rust Codebase Architecture

## Cargo workspace

The workspace contains eight crates: core, config, protocol, Store, Git, presets,
Engine and CLI. `podway` is the only shipped product executable.
`podway-contract-verifier` is a feature-gated repository verification binary,
not an installed product. The application commits `Cargo.lock` and standard
Cargo gates use `--locked` with the pinned `rust-toolchain.toml` toolchain.

Crate-owned tests use mechanically checked `arch_*`, `int_*` and `e2e_*` targets under
`crates/<crate>/tests/`. Integration modules are registered in aggregate
`int_suite` targets; real CLI scenarios are registered in `e2e_suite`.
Architecture targets cannot launch product binaries. Shared fixtures live in
`tests/fixtures/`, canonical assets in `assets/`, and host-local output in ignored
`artifacts/`, `target/` and `dist/` locations.

## Crate responsibilities

| Crate | Responsibility |
|---|---|
| `podway-core` | Pure domain values, transitions, satisfaction, graph status, invariants and errors |
| `podway-config` | Bounded workspace/Procedure parsing, semantics, canonical snapshots and safe source resolution |
| `podway-protocol` | Closed public JSON envelopes/results/errors, typed direct inputs, schema validation, build identity and offline release verifier |
| `podway-store` | Generation-15 persistence, coherent snapshots, atomic success outcomes, native maintenance leases, classification, retention and integrity |
| `podway-git` | Read-only native worktree discovery, canonical-path identity, containment, descriptor and replacement-race checks |
| `podway-presets` | Embedding and validation of canonical `assets/presets/` YAML and catalog metadata |
| `podway-engine` | Direct API composition, preparation, request identity, projections, native artifact/source adapters and maintenance protocols |
| `podway-cli` | Grammar, target selection, Direct requests, local authoring, validated rendering, help and completions |

The daemon and service crates, IPC framing, client transport and service controller
are retired under [ADR-0038](../architecture-decision-records/0038-remove-the-daemon.md).
Their historical design records remain history rather than current ownership.

## Dependency rules

The executable [`cargo-adjacency.json`](../../contracts/cargo-adjacency.json)
owns permitted production edges:

- core depends on no infrastructure crate;
- config, protocol, Store and Git may depend on core only within this workspace;
- presets may depend on core and config;
- Engine composes core, config, protocol, Store, Git and presets;
- CLI may depend on core, config, protocol, presets and Engine; it must not access
  Store internals or native Git directly;
- cycles and reverse infrastructure edges are prohibited.

Test-only dependencies do not grant production access. The Store's persistence
boundary accepts core types and canonical execution documents, not a protocol or
Engine dependency.

Core's `graph_v2`, `workflow_v2` and `goal_state_v2` modules own pure aggregate
state, transitions and history predicates. Store's `v2_state`, `v2_memory` and
`v2_goal` modules retain schema admission, row codecs and transactional
persistence, reexporting the same Core values for existing Rust consumers.
`ProcedureStateErrorV2` is a pure structural failure; explicit Store/Engine
conversion preserves its existing public classification separately from row
corruption and ordinary command-domain rejection.

## Runtime model

Commands use bounded synchronous I/O in the invoking process. There is no async
runtime, socket accept loop, workspace worker, scheduler or observability thread.
Each mutation commits in one SQLite IMMEDIATE transaction, except explicit
marker-governed filesystem maintenance. Independent CLI processes contend through
SQLite and native maintenance locks, with no FIFO admission guarantee.

Ordinary Store access holds a shared maintenance lease. Cold write-free snapshots
without required sidecars use stronger exclusive exclusion and an immutable/heap
WAL path. Doctor and maintenance take exclusive exclusion. A bounded process-local
connection gate prevents shared SHM mappings from weakening read-only connections;
it does not create a daemon singleton or persistent global state.

Engine preparation remains outside the state transaction. Git/Store binding,
prepared graph identity and local artifacts are revalidated inside the execution
boundary before persistence. Store transactions write changed domain rows and
retain the complete validated canonical success envelope atomically with the
outcome. Source-backed replay can recover the original pinned request identity
before reopening changed or missing Procedure input.

## Safe Rust and platform code

Production boundaries use safe Rust with unsafe code forbidden by their crate
roots. Native descriptor and lock operations use the existing reviewed `nix` and
`rustix` boundaries. Internal paths preserve non-UTF-8 bytes; public paths use the
validated display and lossless path representations required by their contracts.
No new platform, unsafe abstraction or service backend is implied by these traits.

## Error architecture

Internal errors are typed. Engine maps them to `podway.error/v2` with a stable
code, human message, retryability, structured details and mapped exit status.
Public consumers use machine fields, not OS/library source text or rendered prose.
Recovery recipes use `podway.recovery-recipe/v2` and bind the relevant command,
identities and exact retry intent.

Rejected mutations have no retained outcome. `WORKSPACE_BUSY` bounds lock
contention; `WORKSPACE_MAINTENANCE` indicates active/recoverable maintenance;
legacy and foreign writers fail closed. `MUTATION_OUTCOME_UNKNOWN` is reserved
for COMMIT I/O ambiguity. An internal failure's diagnostic ID is an opaque request
correlation value; no daemon log stream is available to look it up.

## IDs, time, and hashing

Native adapters supply workspace, session, attempt, blocker and artifact IDs,
UTC millisecond clocks and SHA-256 digests. Core receives explicit values;
Store invokes the transaction clock once for a new success and does not invoke
it on an exact replay. Request digests use the canonical bounded semantic input.
Build identity binds product, version, profile, source commit, target and contract
manifest digest. The Store records writer manifest identity separately from the
Procedure snapshot digest.

## Serialization

Public JSON is UTF-8 and validated against canonical closed schemas. Unknown,
missing, mistyped and cross-command fields fail validation. Result generation
changes are explicit; current success is output v4 and version identity uses
version-result v2/manifest v2 without IPC IDs. Wire paths and numeric limits follow
the [JSON contract](../specs/interfaces/json-contract.md).

The protocol lazily compiles only the applicable schema/command or error branch
against its embedded offline registry. Release verification independently checks
the full manifest asset/schema closure in source and packaged layouts.

## Coding standards

Use repository Make targets for formatting, warnings-as-errors lint, contracts,
tests and distribution. Avoid panics on user-controlled input; use bounded values,
explicit IDs/revisions and exhaustive public enum handling. Reuse existing crate
and domain boundaries. Schema/contract changes update their canonical assets and
meaningful success, failure and compatibility tests together.

## Adopted internal refinements

The [OPTIM, BOUND and READS sequence](../roadmap/README.md#current-internal-improvements)
is the adopted internal-improvement order. OPTIM is completed. Current read
composition, procedure-identity boundaries, and the unpublished batch draft are
in [System Architecture](system.md#request-scoped-read-composition). BOUND and
READS remain unfinished.

At the reviewed v0.2.14 baseline, several pure graph/workflow/goal transitions
still live beside Store row mapping, and CLI preflight performs avoidable
reconstruction. BOUND aligns those implementations with the ownership table
above; it does not create a new crate or reverse dependency. The completed
Engine read context retains the Config-owned validated Procedure and does not
attach that representation to Store's separate cached memory model. BOUND
separates Store-specific errors and schema/SQL admission from pure types while
preserving deferred-model error timing.
READS then changes data access and internal facts/view construction, including
goal/citation loops and selective changed-slot reloads, while preserving current
cold-read validation. Archive-list remains summary-only; graph windows still
validate their required hidden historical values.

BOUND's CLI move gives fence-preflight fetching/parsing one internal owner.
READS replaces that boundary, not completion status, start/reset dry-run or later
interactive-start retry reads. Preserving preflight means preserving failures
from the former status projection/schema/envelope as well as Store admission.
The [performance protocol](../implementation-tips/direct-cli-performance-qualification.md#internal-improvement-comparisons)
records the withdrawn OPTIM measurements and the separately registered future
BOUND and READS scope. Ordinary `make test` omits the ignored native cases.

The [remaining adopted dossiers](../todo/README.md#current-adopted-dossiers) own
BOUND and READS target interfaces, task boundaries, and acceptance until those
epics are complete.
Do not weaken Store admission, move required read checks to doctor, introduce
persistent caching or change public/storage identity to simplify this work.
Update this architecture with the actual accepted implementation at each epic's
closeout; do not present planned responsibility moves as current behavior.

## Testability requirements

Internal traits and test observers support deterministic clocks, IDs, artifact
hashing, worktree replacement and transaction/maintenance boundaries. Native
crash tests qualify actual process termination and cold recovery. CLI maintenance
pause and isolation probes exist only in debug builds; the actual release binary
must ignore every hook. Ordinary development tests and packaged qualification
verify different executable bytes and must not be described as interchangeable.
See [Testing and Conformance](../specs/quality/testing-and-conformance.md).
