# Rust Codebase Architecture

## Cargo workspace

```text
Cargo.toml
Cargo.lock
crates/
  podway-cli/
  podway-config/
  podway-core/
  podway-daemon/
  podway-git/
  podway-presets/
  podway-protocol/
  podway-service/
  podway-store/
schemas/
presets/
release/
tests/
  fixtures/
tools/
```

Each crate owns its Cargo integration-test targets under `crates/<crate>/tests/`.
Those targets use the mechanically checked `arch_*`, `int_*`, and `e2e_*` prefixes;
only `e2e_*` targets may launch the real `podway` or `podwayd` binaries. The root
`tests/fixtures/` directory contains shared contract fixtures rather than test
targets. The repository commits `Cargo.lock` because Podway is an application.

## Crate responsibilities

### `podway-core`

Owns:

- immutable domain types;
- command and transition types;
- item satisfaction rules;
- stage-status derivation;
- invariants;
- pure state transition functions;
- domain error codes.

Must not depend on async, SQLite, Git, filesystem, IPC, or service-manager crates.

### `podway-protocol`

Owns:

- IPC v1 framing types;
- request and response envelopes;
- JSON schema-aligned public types;
- protocol compatibility negotiation;
- bounded deserialization helpers;
- public error envelope serialization.

### `podway-config`

Owns:

- workspace config parsing;
- procedure parsing;
- schema validation;
- semantic validation;
- Podway Canonical JSON v1;
- procedure snapshot digest generation;
- path-safe local procedure resolution.

### `podway-store`

Owns:

- SQLite connection setup;
- DDL and migrations;
- workspace metadata;
- session repository;
- durable jobs and idempotency;
- transaction orchestration;
- pruning and integrity checks;
- test fault-injection hooks.

### `podway-git`

Owns:

- worktree discovery;
- non-bare validation;
- Git common-directory and worktree-admin identity;
- path containment;
- move detection;
- copied UUID conflict inputs.

No Git mutation APIs are exposed from this crate.

### `podway-service`

Owns:

- platform service trait;
- macOS LaunchAgent implementation;
- service install metadata;
- socket/runtime/log path calculation;
- future Linux systemd user implementation.

### `podway-presets`

Owns:

- embedded built-in YAML;
- validated parsed preset snapshots;
- preset listing and explanatory metadata;
- schema-conformance tests for all presets.

The source YAML under the repository `presets/` directory is the reviewable source. The crate embeds exactly those files at build time.

### `podway-daemon`

Owns:

- daemon process lifecycle;
- singleton lock and socket server;
- request routing;
- peer-user checks;
- worktree scheduler registry;
- durable admission and job execution;
- read query service;
- graceful shutdown;
- structured logging.

### `podway-cli`

Owns:

- command grammar;
- static help;
- worktree path selection;
- daemon client;
- automatic concurrency-precondition reads;
- text rendering;
- JSON passthrough and validation;
- shell completion generation.

## Dependency rules

- `podway-core` has no infrastructure dependencies.
- `podway-config` and `podway-protocol` may depend on core value types, not daemon or store.
- `podway-store`, `podway-git`, and `podway-service` may depend on `podway-core` only.
- `podway-daemon` composes all infrastructure crates.
- `podway-cli` depends on protocol and presentation types, not store internals.
- Cyclic crate dependencies are prohibited.

## Runtime model

The implementation uses bounded synchronous I/O and operating-system threads; it
does not use an async runtime. The daemon's blocking Unix-domain socket accept loop
admits same-user connections through a bounded handler budget. Each admitted
connection is handled by a dedicated OS thread with absolute read and write deadlines.

Each active workspace has one worker thread that claims and executes its durable FIFO
queue. Mutexes and condition variables coordinate scheduler generations, progress,
retirement, maintenance, and shutdown. A daemon-wide bounded blocking executor limits
simultaneous workspace operations, while the per-workspace serialization boundary
permits exactly one mutation execution for that identity. SQLite, Git discovery,
artifact hashing, and filesystem work run inside those bounded synchronous execution
paths rather than on an event-loop reactor.

Observability uses a separate bounded queue and sink thread. Saturation and sink
failure are accounted explicitly, and graceful shutdown drains admitted handlers,
workspace workers, and observability state before removing the owned socket.

## Safe Rust and platform code

- Safe Rust is the default.
- Unavoidable macOS FFI or peer-credential code is isolated in a small module.
- Every unsafe block requires a safety comment and focused tests.
- Unsafe code is denied in all other crates when practical.
- Path and byte handling must not assume UTF-8 internally; public JSON paths use validated display strings plus lossless internal platform paths.

## Error architecture

Internal errors use typed enums with source chains. Public errors contain:

```text
stable code
human message
retryable boolean
structured details
mapped exit code
```

Internal library or OS messages are not exposed as stable API text. Sensitive or path-rich details are included only where needed for local remediation.

## IDs, time, and hashing

Infrastructure supplies:

- random UUIDs for workspace, session, attempt, blocker, artifact, and job IDs;
- UTC epoch-millisecond timestamps;
- SHA-256 digests;
- deterministic canonical request digests.

Core transitions receive these as explicit values so tests are deterministic.

## Serialization

- All public JSON uses UTF-8.
- Unknown fields are rejected for authoring schemas in v1.
- Unknown additive fields are tolerated by public response clients.
- Enums serialize as lowercase snake-case or the exact kebab-case strings defined by schemas.
- Numbers that represent revisions and byte sizes are JSON integers.
- Paths shown in JSON are absolute display paths for workspace roots and worktree-relative paths for local artifacts.

## Coding standards

- Format with `rustfmt`.
- Treat linter warnings as errors in the local `make test` gate, with explicit reviewed exceptions.
- Avoid panics on user-controlled input.
- Use bounded collections for protocol and config input.
- Use explicit newtypes for IDs and revisions.
- Keep domain transitions small and table-driven.
- Prefer exhaustive matches over default branches for public enums.
- Keep migrations forward-only and deterministic.
- Add a conformance test for every fixed bug in state or queue semantics.

## Testability requirements

The implementation must support injected:

- clocks;
- ID generators;
- file hashing readers;
- transaction fail points;
- daemon crash points;
- service-manager command runners;
- worktree identity fixtures.

Production behavior remains fixed; injection exists only behind internal traits or test features.
