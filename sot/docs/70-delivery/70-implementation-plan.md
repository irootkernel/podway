# Implementation Plan

## Delivery model

The project produces one complete macOS product. The phases below are internal integration gates, not reduced public editions. Public release occurs only after the full acceptance criteria pass.

## Gate S: source-of-truth reconciliation

Gate S reconciles only the design package before any product workspace is created. It prepares the complete public `v0.1.0` release from design package `1.0.1-design`; all public and storage contracts remain v1.

S0 baseline:

- records the reconciled intent identified by payload SHA-256 `022167d808f5f0f85711bfdfa94d1a0165de711a6eda51bb9209e9e873ea342d`: complete public release `v0.1.0`, design package `1.0.1-design`, unchanged v1 public and storage contracts, and transactional initial migration from `schema-0`/uninitialized (`uninitialized-database`) to `schema-v1`.

S1 delivery:

- contains only derivative SOT edits that implement the recorded S0 baseline.

S2 validation:

- validates the reconciled design package;
- emits checksums only after validation succeeds, as the final S2 action.

S3 acceptance:

- performs read-only acceptance of the reconciled package;
- completes before any product workspace is created.

Gate S plans coverage for Phases 0 through 8. Phase 8 closes through the
repository-local `make test` release gate.

## Phase 0: Repository and contract lock

Deliverables:

- Cargo workspace and dependency-direction checks;
- committed schemas, DDL, error catalog, command catalog, and presets from this package;
- public type naming and version constants;
- a root Makefile for formatting, lint, schema validation, document checks, and tests;
- test fixture conventions and requirement-ID mapping.

Exit gate:

- machine-readable assets validate;
- no unresolved contradiction in public contracts;
- owners assigned for all team streams.

## Phase 1: Pure domain and procedure engine

Deliverables:

- domain types and IDs;
- procedure/config parser and semantic validator;
- canonicalization and digest;
- all item types and satisfaction rules;
- start, item mutation, complete, skip, retry, return, reopen, block, cancel, and reset transitions;
- derived status and next-action planner;
- unit and property tests for all invariants;
- all presets parsed as ordinary procedures.

Exit gate:

- no infrastructure dependency in core;
- property suite cannot produce an invalid session;
- state-transition matrix is fully covered.

## Phase 2: SQLite store and durable jobs

Deliverables:

- `schema-v1` migration framework, including transactional initial migration from `schema-0`/uninitialized (`uninitialized-database`);
- workspace, snapshot, session, stage, attempt, item, blocker repositories;
- durable admission, job claiming, terminal results, idempotency;
- per-item and cursor precondition persistence;
- bounded job and journal pruning;
- integrity checks and destructive reset marker;
- transaction fail points and crash test harness.

Exit gate:

- DDL and migration tests pass;
- crash tests through transaction boundaries pass without duplicate effects;
- idempotency and concurrency suites pass in-process.

## Phase 3: Git worktree and filesystem boundary

Deliverables:

- main and linked worktree discovery;
- Git identity fingerprints;
- path containment and symlink checks;
- initialization layout and ignore management;
- worktree move and registry repair;
- copied UUID conflict detection;
- local artifact hashing and completion revalidation;
- worktree deletion behavior.

Exit gate:

- all Git/filesystem fixtures pass;
- no path escape can reach outside the worktree;
- state deletion follows worktree deletion.

## Phase 4: IPC and daemon

Deliverables:

- frame codec and protocol types;
- Unix socket server and peer UID checks;
- daemon singleton and graceful shutdown;
- minimal workspace registry;
- per-worktree scheduler and concurrent worktree execution;
- read service, wait-for-idle, after-job, and job control;
- restart recovery;
- structured logging and diagnostics.

Exit gate:

- protocol fuzz suite passes;
- daemon crash matrix passes;
- two-worktree concurrency and one-writer invariants are demonstrated.

## Phase 5: CLI and JSON

Deliverables:

- complete command grammar;
- automatic precondition reads;
- synchronous and detached behavior;
- text rendering and versioned JSON;
- destructive confirmation policy;
- help topics and examples;
- zsh, bash, and fish completion;
- JSON schema validation and golden tests.

Exit gate:

- every command has help, success JSON, error JSON, and exit-code tests;
- text and JSON agree on state;
- automation scenario uses only public JSON.

## Phase 6: macOS service integration

Deliverables:

- path calculation and service metadata;
- LaunchAgent plist generation;
- install, update, start, stop, restart, status, logs, and uninstall;
- socket and lock lifecycle;
- log rotation;
- package integration.

Exit gate:

- isolated macOS service suite passes;
- daemon starts at login;
- explicit stop works despite keep-alive;
- upgrade refreshes binary path safely.

## Phase 7: Preset dogfooding and UX correction

Use Podway itself for:

- one non-trivial feature task with return;
- one bug fix with retry and return;
- one documentation-only task;
- one analysis task.

Collect:

- number of Podway commands per task;
- unclear prompts or items;
- unnecessary required items;
- cases where `next` failed to prevent omission;
- queue or revision friction for AI-assisted callers.

Changes in this phase may refine preset data and text UX but MUST NOT weaken locked invariants or public contract semantics without reviewed updates.

Exit gate:

- all four presets complete real tasks;
- return and retry paths remain understandable;
- no preset requires special core code.

## Phase 8: Hardening and release

Deliverables:

- `make test-prepare` for generated assets, formatting, vet, lint,
  dependency review, architecture guardrails, quality mappings, and contracts;
- `make test-unit` for narrow tests;
- `make test-int` for multi-component scenarios and deterministic fixtures;
- `make test-e2e` for actual-binary user scenarios;
- distribution layout, checksums, release notes, and install guide;
- automated migration coverage proving that `schema-0`/uninitialized
  (`uninitialized-database`) migrates transactionally to `schema-v1`.

Exit gate:

- the repository-root `make test` command exits successfully.

This is the sole required release-readiness gate. Hosted CI, independent
signatures, approval quorums, holdout runs, qualification archives, and
attestation bundles are not required. Signing and notarization are distribution
choices documented in release notes.

## Critical path

```text
contract lock
  -> pure domain
  -> store transactions
  -> daemon scheduler and IPC
  -> CLI integration
  -> LaunchAgent integration
  -> full conformance and release
```

Git/filesystem, presets, JSON schemas, service packaging, and test harness can progress in parallel once core interfaces are frozen.

## Change control during implementation

A change to any of the following requires design review and synchronized documentation/tests:

- session or attempt lifecycle;
- return or redo semantics;
- public command grammar;
- JSON or IPC schema;
- error code or exit mapping;
- database schema or retention;
- worktree identity and state location;
- local trust boundary;
- artifact storage policy;
- release scope.

Small implementation choices that preserve these contracts remain team-owned.
