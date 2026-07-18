# Testing and Conformance

## Quality strategy

Podway's main risks are semantic drift, concurrency bugs, crash ambiguity, path mistakes, and contract instability. Tests are organized around those risks rather than around code coverage alone.

Required layers:

1. pure domain unit tests;
2. property and model-based tests;
3. schema and canonicalization tests;
4. SQLite transaction and migration tests;
5. daemon queue and crash-injection tests;
6. IPC protocol tests and fuzzing;
7. Git worktree and filesystem fixtures;
8. macOS LaunchAgent integration tests;
9. CLI text and JSON golden tests;
10. end-to-end preset conformance scenarios.

## Pure domain tests

Every transition has table-driven cases for:

- valid preconditions;
- each invalid lifecycle;
- stale attempt and revision;
- item satisfaction and constraint failures;
- blocker interaction;
- final-stage behavior;
- exact affected stage states;
- revision behavior;
- no-op behavior.

The pure domain suite uses deterministic IDs and timestamps.

## Property-based tests

Generate valid procedures, sessions, and command sequences and continuously assert:

- at most one session;
- exactly one active attempt while running;
- no active attempt while completed or cancelled;
- linear stage ordering;
- monotonic session, item, attempt, and workspace sequences;
- old item values never satisfy new attempts;
- return only creates `redo` for reached downstream stages;
- no failed transition changes state;
- serialization round trips preserve domain values.

A reference model independent of the production transition implementation SHOULD be used for stateful command sequences.

## Procedure and canonicalization tests

- all built-in presets validate;
- YAML and equivalent JSON produce the same canonical digest;
- map key order and insignificant whitespace do not affect digest;
- defaults do affect canonical output consistently;
- duplicate YAML keys are rejected;
- aliases, depth, size, and collection limits are enforced;
- invalid constraints produce stable diagnostics;
- canonical bytes are deterministic on the Apple Silicon (`aarch64-apple-darwin`) release target.

## SQLite tests

- reference DDL creates an empty valid database;
- the deterministic non-file `uninitialized-database` fixture migrates from `schema-0-uninitialized` to `schema-v1`;
- every migration applies from each supported prior schema;
- migration checksums are verified;
- deterministic schema-0 to v1 conformance evidence verifies required pragmas, transactional initialization, no user task-state loss, no duplicated mutation, and no partial installation;
- foreign keys and checks are active;
- session reset cascades only session-scoped rows;
- idempotency responses survive terminal-job pruning;
- journal and job pruning obey minimum retention;
- quick and deep integrity checks detect injected corruption;
- unsupported newer schema fails closed.

## Queue and concurrency tests

Required scenarios:

- FIFO order within one worktree;
- one running mutation per worktree;
- concurrent execution across two or more worktrees;
- queue capacity and backpressure;
- cancellation of queued job;
- rejection of running-job cancellation;
- same idempotency key and request returns one result;
- same idempotency key and different request fails;
- two different item updates both succeed on one attempt;
- same-item stale update fails;
- complete queued behind item updates sees their committed values;
- item update queued behind complete fails `ATTEMPT_NOT_CURRENT`;
- return queued before complete causes stale complete conflict;
- no later executable job overtakes an earlier queued job.

## Crash-injection matrix

The test harness terminates the daemon or injects failure at:

```text
C01 before admission transaction
C02 after job insert before admission commit
C03 after admission commit before response
C04 after claim commit
C05 during procedure validation preparation
C06 during artifact hashing
C07 after state transaction begin
C08 after relational updates before job terminal update
C09 after job terminal update before commit
C10 after commit before response
C11 during domain-error result commit
C12 during pruning
C13 during migration
C14 during reset-all after marker creation
C15 during reset-all after database deletion
C16 during reset-all after new database creation
```

For each point, restart and assert one valid outcome: no admission, one queued retry, one terminal result, or idempotent completion. No state effect may occur twice.

## Phase 8 migration evidence

Phase 8 release conformance MUST produce `release/migration-evidence-v1.json` from the deterministic `uninitialized-database` schema-0 to v1 fixture. The evidence MUST identify predecessor `schema-0-uninitialized` and result `schema-v1`, and record the conformance assertions for required pragmas, transactional initialization, retained user task state, non-duplicated mutation, and atomic installation.

## IPC tests and fuzzing

- fragmented header and body;
- zero, small, exact-limit, and oversized frames;
- invalid UTF-8 and JSON;
- duplicate keys where parser configuration applies;
- unsupported protocol;
- unknown command;
- malformed preconditions;
- peer UID mismatch;
- connection loss before and after admission;
- wait timeout without job cancellation;
- frame decoder fuzzing;
- request schema deserializer fuzzing;
- response schema validation for every command.

## Git and filesystem tests

Fixtures cover:

- main and linked worktrees;
- nested invocation;
- bare repository rejection;
- missing and malformed `.git` metadata;
- worktree move and registry repair;
- copied workspace UUID conflict;
- `.podway` symlink escape;
- procedure path escape;
- artifact path escape;
- unreadable artifact;
- artifact changed before complete;
- worktree deletion with queued jobs;
- runtime ignored and accidentally tracked;
- permissions and stale WAL files.

## macOS service tests

- plist generation matches template contract;
- install is idempotent;
- binary-path update restarts correctly;
- user-login startup in an isolated account or VM;
- explicit stop defeats keep-alive;
- stale socket cleanup;
- duplicate daemon prevention;
- socket permissions and peer-user checks;
- log creation and rotation;
- uninstall preserves worktree data;
- incompatible binary/protocol reporting.

## CLI and JSON tests

Every command requires:

- `--help` snapshot or semantic assertions;
- valid text output in normal, empty, and error states;
- valid success and error JSON against shipped schemas;
- documented exit-code assertion;
- no ANSI escapes with `--json` or `--no-color`;
- deterministic array ordering;
- non-interactive confirmation behavior;
- shell completion generation.

Text snapshots should be reviewed for clarity but are not public compatibility contracts.

## Preset end-to-end scenarios

### `sw-dev`

Start, complete through implementation, return from review to implement, repeat verify and review, complete.

### `bug-fix`

Reproduce, diagnose, define regression, fix, retry verification once, return from review to fix once, complete.

### `docs-only`

Ground sources, define audience, outline, draft, return from validation to draft, review, complete.

### `analysis`

Define question, collect sources, analyze, retry challenge, return to source collection, synthesize, complete.

Each scenario asserts `next` suggestions and JSON at every step.

## Performance tests

Measure without making network assumptions:

- daemon cold start;
- status and next latency on empty and maximum-size procedures;
- admission throughput across multiple worktrees;
- item update throughput;
- artifact hashing throughput separately;
- database growth and pruning;
- memory behavior under maximum IPC and queue limits.

Performance regression thresholds are set from an accepted baseline and must not weaken correctness pragmas.

## Continuous integration

Required CI lanes:

- formatting and lint;
- unit and property tests;
- schema/example validation;
- SQLite and migration tests;
- protocol fuzz smoke corpus;
- native macOS Apple Silicon (`aarch64-apple-darwin`) build and integration;
- release packaging dry run;
- dependency and license checks.

Long-running crash and service suites may run in a dedicated release lane but must pass before public release.
