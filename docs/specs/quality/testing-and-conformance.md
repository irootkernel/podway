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

## Machine-contract drift tests

- command result and error-detail bindings match the generated catalogs exactly;
- every binding resolves to a Draft 2020-12 schema containing its discriminator;
- known-answer requests and responses pass the production Rust decoders;
- missing, unknown, mistyped, mismatched, and cross-command compact fields fail closed;
- every JSON known answer is manifest-covered with its exact SHA-256 digest;
- the complete compact envelope boundary is tested at 262,144 bytes and one byte over.

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
P01 after the reset target destination link and before temporary-link removal — `phase2_crash_child_aborts_at_configured_failpoint`
D01 during atomic reset-marker publication — `d01_reset_marker_publication_interrupted_before_link_publishes_no_marker`
D02 during atomic registry rename — `abort_process_failpoints_recover_exact_documents_without_accepting_temporaries`
S01 during atomic service plist or metadata publication — `atomic_service_publication_crash_child_leaves_no_partial_state`
S02 after the bootstrap side effect and before metadata publication — `bootstrap_side_effect_crash_child_reconciles_to_one_installed_state`
S03 after the first non-purge service-file removal — `service_removal_crash_child_preserves_complete_prior_state`
```

For each point, restart and assert one valid outcome: no admission, one queued retry, one terminal result, or idempotent completion. No state effect may occur twice.

## Migration conformance

The local integration suite MUST exercise the deterministic
`uninitialized-database` schema-0 to v1 fixture. It verifies predecessor
`schema-0-uninitialized`, result `schema-v1`, required pragmas, transactional
initialization, retained user task state, non-duplicated mutation, and atomic
installation. A separate release-evidence file is not required.

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
- same-version/different-manifest and different-version/same-IPC rejection;
- replaced installed-executable detection before launchctl observation;
- reinstall-based upgrade refresh with ordered restart, refreshed receipt identity,
  and a new daemon process UUID.

Target `podway daemon install` MUST resolve and validate the actual `podwayd`
binary without staging or copying it. The LaunchAgent records that canonical
absolute path and does not support a wrapper or ambient `PATH` lookup.

`AUT-T-PATH` is implemented by the sanitized user-environment and Dolgorae
consumer E2E suites. They invoke a CLI symlink by command name outside a
worktree, exercise explicit, resolved-CLI-sibling, and controlled-`PATH` daemon
selection through the real installer, and require the canonical daemon path in
both service metadata and the absolute LaunchAgent plist.

`AUT-T-OBS` is implemented by the compact-status protocol bounds and the
Dolgorae installed-service E2E flow. The latter rejects a different-socket
duplicate daemon, initializes through an explicit endpoint in a sanitized
environment, and requires a closed, bounded compact result whose queue sequence
matches the post-barrier workspace sequence with no pending or running work.

The Dolgorae fenced-lifecycle E2E flow validates a custom Procedure digest,
starts it through the installed service, removes the source, and resumes from
the immutable snapshot after daemon restart. It supplies fresh workspace,
session, revision, attempt, and item fences while populating every item shape,
then covers completion, reopen, replacement, and reset with the same explicit
consumer identity discipline.

The Dolgorae conflict and reconciliation E2E flows require the installed daemon
to reject a mismatched contract identity and require closed conflicts for stale
workspace, session, session-revision, and Procedure-digest observations. A zero-
duration synchronous wait proves durable admission through `JOB_WAIT_TIMEOUT`
and recovers the terminal receipt by idempotency-key lookup. A private socket
relay separately discards a real successful mutation response, requiring
`MUTATION_OUTCOME_UNKNOWN`, read-only terminal lookup, exact replay, and
different-request key-reuse rejection without a second admission.

Automation conformance scenarios are defined in the
[automation acceptance matrix](../interfaces/automation-client-contract.md#24-acceptance-matrix).
Their evidence may span implementation and integration tasks. A scenario enters
this implemented test inventory only when every task named by its roadmap mapping
has supplied the required executable evidence. The product acceptance matrix
remains the machine-readable binding for `PAC-*` criteria; it does not accept
`AUT-*` evidence rows.

The separate
[`podway.dolgorae-acceptance-matrix/v1`](../../../release/dolgorae-acceptance-matrix-v1.json)
binds every completed `DOLGI` task to its exact `AUT-T-*` evidence commands and
proof-source digests. The repository-local quality verifier rejects missing,
reordered, duplicated, stale, or non-exact DOLGI evidence independently of the
`PAC-*` matrix.

`AUT-T-CONTRACT` is implemented by the binary identity, daemon ingress, CLI
error-propagation, native service, and daemon-process E2E suites. Together they
cover matching peers, stale manifests across version combinations, pre-publication
installer rejection with bounded probing, replaced executable bytes, receipt refresh,
identity-aware readiness through a stale-daemon window, and restart identity.
Daemon ingress separately proves that a diagnostic version change with matching
product and manifest is accepted, while a different release version with the same
IPC ID and a different manifest is rejected before dispatch.

### CONID final review

The final identity review binds each requirement to an executable proof:

- `AUT-CONTRACT-001` uses the deterministic manifest check and its isolated
  tamper controls;
- `AUT-CONTRACT-002` and `AUT-CONTRACT-003` compare both binary identities and
  require the embedded source commit to equal the explicit build revision or Git
  `HEAD`, including rebuilds after a symbolic branch ref advances from packed to
  loose ref storage;
- `AUT-CONTRACT-004` proves daemon ingress and installation reject mismatches
  before dispatch, durable admission, service publication, or launchctl;
- `AUT-CONTRACT-005` probes a real daemon twice, then restarts it and requires a
  new process UUID with stable executable, endpoint, and manifest identity; and
- `AUT-T-CONTRACT` runs mismatch rejection followed by a verified matching
  install, rejects stale readiness on the service socket, accepts only the
  replacement daemon with a new process UUID, verifies live status, and proves a
  later restart creates another process UUID.

`AUT-T-ID` is implemented by the CLI route-surface, protocol slicing, production
dispatcher, execution/store race, reset-runtime, and guarded-read suites.

### CASID final review

The final identity-fence review binds each required scenario to executable proof:

- public flags, help, completions, applicability, and canonical IPC propagation
  are covered by the CLI route-surface and slice-contract tests;
- the reset-runtime test rejects both a stale mutation and a fresh-key stale
  reset after workspace replacement without admitting a job, consuming a
  sequence, publishing a marker, or replacing the target Store;
- claimed execution races replace a session before stale item, session-revision,
  and reopen mutations execute, including the case where both sessions have
  revision `1`, and retain exact terminal replay without changing the replacement;
- stale attempt and item revisions terminate as `ATTEMPT_NOT_CURRENT` and
  `ITEM_REVISION_CONFLICT` with immutable replay and no partial mutation; and
- guarded status/next reads reject immediate, idle-wait, and after-job session
  replacement using the authoritative Store observation made at return time.

## CLI and JSON tests

Every command requires:

- built-in `podway help` topic or route semantic assertions, plus an explicit test that the unsupported `--help` flag is rejected;
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

## Performance diagnostics

The project MAY measure the following without making network assumptions:

- daemon cold start;
- status and next latency on empty and maximum-size procedures;
- admission throughput across multiple worktrees;
- item update throughput;
- artifact hashing throughput separately;
- database growth and pruning;
- memory behavior under maximum IPC and queue limits.

Performance diagnostics must not weaken correctness pragmas. They inform
optimization work but are not a release-readiness gate.

## Local development and release gates

The repository-root `make test` command is the required development gate and runs these
stages sequentially:

- `test-prepare`: canonical-asset validation, formatting checks, lint,
  dependency checks, architecture guardrails, quality mappings, and contracts;
- `test-rust`: unit and architecture targets plus one integration suite per crate
  in one Cargo invocation, using four test workers by default;
- `test-e2e`: serial user journeys through real debug product binaries and shells,
  including a start/status/next smoke for all four presets.

The E2E layer builds the debug product binaries once. Preset-tool verification
runs afterward against that prepared CLI instead of initiating another build.
`make dist` always runs this gate, all-target Clippy, release helper sentinels,
fixed-run fuzzing, release builds, one distribution package, qualification, and
the handoff.

Focused `test-unit`, `test-int`, and `architecture` targets remain available while
iterating. Integration tests may execute one product component against controlled
collaborators; end-to-end tests are reserved for user-observable product journeys.
The suite registry preserves every layer source as a separately named Rust module
while avoiding one operating-system process per source file. Rust tests use the
bounded `TEST_THREADS` test-harness setting, which defaults to four; actual
daemon/process E2E remains serial. Make-driven Cargo gates set
`CARGO_INCREMENTAL=0` to bound codegen-object growth, while direct Cargo commands
retain the normal incremental default.

`test-fuzzing` uses `nightly-2026-07-17` and `cargo-fuzz 0.13.2` only for
sanitizer and coverage instrumentation. Product compilation and all non-fuzz test
targets remain pinned to Rust 1.97.1. The bounded release-gate campaigns do not
replace longer exploratory fuzzing during development; any discovered crash input
becomes a deterministic regression test or retained corpus seed.

All required crash, migration, protocol, service, and acceptance scenarios are
included in those targets. There is no hosted-CI or separate release lane
requirement. The product-acceptance verifier binds every mandatory bullet in the
acceptance source exactly once; adding an unmapped bullet fails `test-prepare`.
`make dist` packages the release-profile binaries once, verifies CLI, daemon,
manifest, source, target, binary digests, isolation capability, and provenance
identities, then extracts the
archive, selects a private absolute `PODWAY_DEV_HOME`, runs the packaged lifecycle,
conflict, timeout, response-loss, reconciliation, and identity scenarios through
the foreground dev daemon, terminates it through IPC, and requires socket cleanup.
