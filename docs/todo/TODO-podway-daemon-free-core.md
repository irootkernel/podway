# Podway Daemon-Free Core

## 1. Status and authority

- Document state: `Adopted`
- Owning roadmap epic: `V2DFC`
- Adoption date: October 5, 2026
- Scope: Podway repository only; Aquarium and Dolgorae changes are external installation prerequisites
- Contract target: a new public contract generation without backward compatibility
- Acceptance target: the full `make test` gate for every task; `make dist` at the release commit
- Product release: conditional `v0.2.13` target after `V2DFC-001` through `V2DFC-010` and a separately authorized stable-release qualification
- Decision authority: [ADR-0038](../architecture-decision-records/0038-remove-the-daemon.md), Proposed until `V2DFC-002` accepts it after the feasibility gate
- Execution order and status: [V2DFC roadmap](../roadmap/README.md#v2dfc-daemon-free-core)

This is the decision-complete implementation dossier for removing `podwayd`.
No capability is implemented, released, installed, or removed merely by this
adoption. The roadmap is the only task-status owner. This dossier adds no
competing status table, automatic task start, or deployment authority.

The dossier replaces an untracked proposal draft written on the same day and
incorporates an independent review of that draft. Section 2.5 records how each
review finding is resolved. Neither draft is an authority.

Until `V2DFC-002` accepts ADR-0038, ADR-0003 and the other daemon decisions
remain the accepted authority for shipped behavior. `V2DFC-001` changes no
product behavior, so it needs no new authority.

## 2. Verified context and evidence

### 2.1 Baseline

The inspected checkout was committed at `656a51e`: `v0.2.12` is released and
`v0.2.13` is open with an empty changelog section. The Store schema generation
is 13. Take a fresh baseline before each task; this section is planning
context, not permanent authority over a later checkout.

### 2.2 What the daemon owns today

[ADR-0003](../architecture-decision-records/0003-daemon-single-writer.md) was
accepted before the first commit. Its entire context is:

> Humans, scripts, and AI agents may issue Podway commands concurrently. Direct
> CLI writes would require complex file locking and still make durable
> admission, retries, and service-level observation inconsistent.

It makes `podwayd` the sole normal writer of every worktree database. The CLI
submits mutations as durable jobs, the daemon keeps one FIFO queue per worktree,
and reads go through the daemon so they can report pending queue state. The
daemon also owns admission, restart recovery, maintenance coordination, the
workspace registry, readiness, logs, runtime modes, and the LaunchAgent service
path. The ADR named only direct CLI writes as an alternative; it did not
evaluate SQLite's own locking.

### 2.3 Verified implementation facts

| Evidence | Consequence |
|---|---|
| `apply_connection_pragmas_v1` in `crates/podway-store/src/schema.rs` | Every connection sets WAL, `synchronous = FULL`, `foreign_keys`, `trusted_schema = OFF`, and a busy timeout of at most 5,000 ms from options. SQLite already serializes writers. |
| `admit`, `claim_next`, `commit_terminal`, and `commit_graph_mutation_terminal_with_receipt_v1` in `crates/podway-store/src/sqlite_store.rs` | Every write path uses an IMMEDIATE transaction. One mutation crosses at least three write transactions plus socket round trips. |
| `open_guarded` and `open_existing_guarded` | Every guarded open recovers interrupted publications on the filesystem, updates the validated root in an IMMEDIATE transaction, runs the full Fast integrity pass, and calls `recover_running_connection`, which requeues running jobs and prunes history in another IMMEDIATE transaction. Many CLI processes cannot reuse it unchanged. |
| `IntegrityModeV1` | `Fast` is the only production mode. It checks schema version, object set, migration checksums, pragmas, workspace identity, `quick_check`, `foreign_key_check`, full graph-state reconstruction, the job queue, internal codecs, and idempotency receipts. `Deep` adds `integrity_check` and has no caller. |
| Read resolution in `crates/podway-daemon/src/workspace.rs` | Routed reads byte-copy the database and WAL to a temporary directory and run a Fast pass on the copy. |
| `WorkspaceMaintenanceCoordinatorV1` in `crates/podway-daemon/src/runtime_workspace.rs` | Maintenance exclusion is process-global and in memory. Exclusion across processes exists only through the daemon singleton lock. |
| Store identity columns | The Store records `user_version`, migration checksums, workspace identity, the last validated root, and the runtime mode. It records no writer, binary, or contract-manifest identity. |
| Failure persistence | Fence, lifecycle, identity, and request rejections found at admission write nothing and are re-evaluated on retry. Domain failures found after admission persist a failed job and terminal response that a same-key retry replays. |
| Request digests | The daemon computes them with `podway-protocol` canonicalizers. The CLI computes none and may not depend on `podway-store`. |
| Idempotency retention | Session-scoped records are deleted only when that session's reset commits. Workspace-scoped orphan receipts are deleted beyond rank 100 or after 30 days. |
| [Reset-all protocol](../specs/storage/recovery-retention-and-maintenance.md#destructive-reset-all-protocol) | Reset-all publishes `reset.marker`, unlinks the database, WAL, and SHM files, seeds a new Store, and is resumed by daemon startup or an exact retry. It is not one transaction. |
| Workspace removal | Removal publishes a marker and deletes `.podway` with the marker last; any later request on that worktree resumes it. |

### 2.4 Cost of the daemon

Production line counts at the baseline exclude test directories, test-only
files, and `#[cfg(test)]` regions. Categories were assigned by file purpose and
function name, so they size the problem; they do not measure removable
complexity.

| Area | Approximate production lines |
|---|---:|
| Pure domain core (`podway-core`) | 6,600 |
| Daemon-only infrastructure in `podway-daemon` | 18,000 |
| Domain logic that lives in `podway-daemon` and must move | 18,000 |
| Daemon-shaped code in `podway-store` (queue, job types, codecs, receipt checks) | 6,200 |
| `podway-service` (LaunchAgent, runtime reset, managed runtime) | 8,300 |

Each mutation command today needs an admission step, an encoded execution
document, and a worker-side execution of the claimed job: about 1,386 lines of
`admit_*` functions, 2,069 lines of execution-document encoding and admission
helpers, and 1,301 lines of `execute_*_claimed` functions. Every new mutation
pays this cost on the daemon path.

Eight ADRs exist to operate the daemon and its per-user runtime: ADR-0012,
ADR-0020, ADR-0022, ADR-0023, ADR-0029, ADR-0030, ADR-0031, and ADR-0032.

Recorded incidents concentrate in the same area. Their current status matters:

- Startup with registered worktrees took about 47 seconds, so installation could
  report `DAEMON_UNAVAILABLE`
  ([ADR-0029](../architecture-decision-records/0029-serve-phase-aware-daemon-readiness.md)).
  Phase-aware readiness resolved the misreport.
- `daemon status` exceeded the CLI's five-second read timeout with 22 active
  schedulers and about 133 MB of databases. Epic `V2DHP` resolved it; status now
  returns in about 13 ms at that scale.
- Cold recovery with 22 worktrees and about 164 MB of Stores stayed in
  `recovering` beyond 180 seconds. Later work reached 47.31 seconds in a trial
  and 7.76 seconds on two worktrees; the 22-worktree fixture was not rerun and
  no latency target exists
  ([candidate](TODO-podway-startup-recovery-performance.md)).

[ADR-0013](../architecture-decision-records/0013-native-apple-silicon-macos-only.md)
limits releases to native Apple Silicon macOS because Podway's "service, release
builder, qualification environment, and distribution format are all specific to
a native macOS LaunchAgent on Apple Silicon." Removing the daemon removes the
service part of that coupling. It does not by itself make another platform
supported, and this epic preserves ADR-0013.

Aggregated daemon logs from one maintainer host over 35.6 hours showed 7,957
requests across 25 sessions in 9 worktrees, with peaks of 7 requests and 2
mutations in one second and 172 requests in one minute. 105 requests (1.3%)
were rejected and none failed internally. This is context only: it shows a
stable current system and a low write rate, and it does not establish
contention, fairness, or latency for future parallel callers.

### 2.5 Review findings and dispositions

| Finding | Disposition |
|---|---|
| Write serialization is not durable admission; retiring it is a product decision. | Accepted. Section 3.3 lists every retired guarantee, including execution after client termination. No Aquarium path uses `--detach`; internal tools, tests, and `delegation accept --detach` are retired by `V2DFC-007` and `V2DFC-009`. |
| The guarded open recovers jobs and validates far more than `quick_check`. | Accepted. Section 4.3 separates classification, read-only, mutation, and maintenance opens; no ordinary open writes; gate check G8 measures the real open path. |
| Reset-all and removal cross filesystem boundaries; SQLite locking cannot coordinate file replacement. | Accepted. Section 4.6 defines a process-independent maintenance lock and keeps marker recovery and `WORKSPACE_MAINTENANCE`. |
| A schema version does not prove writer agreement. | Accepted. Section 4.7 records a writer contract identity in the Store and allows only declared predecessor migration. |
| Failure replay and the transition of existing jobs are unspecified. | Resolved by owner decision and design. Daemon-era state is not migrated (section 4.9). Rejections are not retained, and fences prevent a second state effect (section 4.5). The retention boundary is stated. |
| Snapshot consistency does not preserve waiting semantics. | Resolved by removal. `--wait-for-idle` and `--after-job` are retired with guidance; reads document snapshot semantics (section 4.4). |
| The feasibility spike is too narrow. | Accepted. Section 7.1 adds separate processes, crash boundaries, maintenance and preparation races, bounds, full-invocation latency, and legacy classification, and separates feasibility from adoption requirements. |
| Evidence and wording need correction. | Accepted. Incidents carry their resolution status, load is context only, line counts separate moved from deleted code, and section 5.6 describes Aquarium's routine and setup paths separately. Global catalog and diagnostic ownership disappear because no global state remains (section 4.8). |

### 2.6 Specification and implementation discrepancies

Inspection found five places where specifications and code already disagree.
They are resolved by deletion or rewrite in `V2DFC-005` through `V2DFC-008` and
must not be copied into new specifications:

- The SQLite model specification says application ID and page size are verified
  on open; no code verifies either.
- Barrier and queue-limit conditions are specified as `WORKSPACE_MAINTENANCE`
  and `WORKSPACE_QUEUE_FULL`, but the Store returns a busy storage error that
  maps to `DAEMON_UNAVAILABLE`.
- Barrier cleanup of old-session idempotency records is specified for reset and
  replacement; only reset performs it.
- The artifact digest comparison is specified inside the transaction; it runs
  immediately before it.
- An unexpected execution failure is specified to mark the job failed with
  `INTERNAL_ERROR`; the implementation fails the drain closed instead.

## 3. Goals, non-goals, and scope

### 3.1 Owner decisions

The owner decided on October 5, 2026:

1. Remove the daemon.
2. Target release `v0.2.13`.
3. Publish a new public contract generation with no backward compatibility.
4. Delete daemon-era runtime state by reset, without backup or migration.
5. Make removed commands and flags fail immediately with a stable guidance
   error.

### 3.2 Required outcome

Every Podway command runs to completion in the invoking process. A mutation
opens the owning worktree's Store and commits in one SQLite transaction with its
fences and idempotency record. Reads observe one committed snapshot without
writing. No resident process, socket, LaunchAgent, or per-user state exists.

### 3.3 Retired guarantees

- Durable admission and execution after the client terminates (`--detach`).
- FIFO ordering among concurrent writers; SQLite lock order and fences replace
  it.
- Queue introspection and waiting: `job list`, `job status`, `job wait`,
  `job cancel`, `--wait-for-idle`, `--after-job`, and the `queue`, `admission`,
  and `job` result members.
- Runtime modes, named and managed development runtimes, and `workspace mode`.
- Account runtime reset (`runtime reset`).
- Daemon lifecycle (`daemon *` and `terminate`), daemon and bootstrap logs, and
  readiness.
- The global workspace registry.
- Every daemon-era runtime state: Store generations 1 to 13, `~/.podway`, and
  the LaunchAgent.

### 3.4 Preserved guarantees

- One active attempt and single-cursor graph semantics.
- Fences on every mutation, exact-key idempotency for successful mutations, and
  fail-closed handling of stale or unsupported state.
- Worktree-local authority and canonical Git-path workspace identity.
- Same-user local trust, bounded input and output, and no command execution,
  Git mutation, or network I/O.

### 3.5 Non-goals

- Linux or any other platform; ADR-0013 is preserved.
- The Web console, graph engineering, continuation generations, and live
  executor integration.
- Changes in the Aquarium or Dolgorae repositories; they are installation
  prerequisites only.
- Measuring Podway's effect on agent work or reducing procedure ceremony.
- Any resident or login-start process.

## 4. Accepted design decisions and interfaces

### 4.1 Process model

No resident process remains. Every command runs in the invoking process and
exits. A future process that must stay alive, such as a Web console, runs only
while in use, executes through the same engine, and needs its own accepted
decision. A resident or login-start process requires a new ADR.

### 4.2 Crate structure

- A new `podway-engine` crate owns host-independent domain services: read
  projections, preparation, canonical request digests, workspace resolution,
  maintenance markers, and command execution against the Store.
- `podway-cli` depends on `podway-engine`. The adjacency contract allows the
  CLI to reach the Store and Git layers only through the engine.
- `podway-daemon` and `podway-service` are deleted, and `podwayd` is not
  shipped.
- `podway-protocol` keeps public envelopes, the error catalog, canonicalization,
  and bounded decoding. IPC framing and the socket envelope are deleted.

### 4.3 Store open paths

| Path | Writes | Checks | Used by |
|---|---|---|---|
| Classification | Never | Database header (`user_version`) and legacy markers; legacy files are never opened read-write, because hot-WAL recovery writes | Every invocation, before any other open |
| Read-only snapshot | Never | Pragmas, binding identity, schema-object digest, writer identity | Reads |
| Mutation | Only inside the command transaction | Read-only checks plus the shared maintenance lock | Mutations |
| Maintenance | Under the exclusive lock | The checks the operation needs | Store creation, reset-all, removal, predecessor migration, `doctor` |

`quick_check`, `foreign_key_check`, and full graph reconstruction run in
`doctor` and in maintenance, not on every invocation. A moved worktree root is
rebound only inside a write transaction.

### 4.4 Write and read paths

A mutation:

1. classifies the Store and fails closed on legacy, newer, or foreign-writer
   state;
2. performs expensive, side-effect-free preparation, such as Procedure
   validation and artifact hashing, before taking any lock;
3. takes the shared maintenance lock and begins an IMMEDIATE transaction within
   the command deadline;
4. looks up the idempotency key and returns the stored success for an identical
   request, or fails with `IDEMPOTENCY_KEY_REUSED` for a different one;
5. validates the fences the command requires;
6. revalidates prepared inputs, such as the Procedure digest and artifact
   digests, against the state it will commit;
7. reads the clock once and applies the pure `podway-core` transition;
8. writes state, the success outcome, the operational journal entry, and any
   pruning in the same transaction; and
9. commits and prints the response.

A rejection rolls back and leaves no record. A read opens one WAL read
transaction, observes one committed snapshot, and never waits: an acknowledged
mutation is always committed, so no pending state exists to wait for.

### 4.5 Idempotency and outcome lookup

- Only successful mutations record an outcome, in the same transaction as their
  state change.
- Rejections are not retained. Every cursor-changing command is fenced by
  session revision and every item command by item revision, so re-evaluating a
  rejected request cannot apply a second state effect; the rejected request had
  none.
- `podway outcome lookup --idempotency-key <key>` (route `outcome.lookup`) is
  read-only. It returns the recorded success or not-found; not-found means no
  state effect within the retention boundary.
- Session-scoped outcomes live until that session is reset or the worktree is
  reset. Workspace-scoped outcomes keep today's bound of 100 entries or 30 days.
- Automation must pass `--idempotency-key`. A key generated by the CLI cannot be
  recovered after a crash.
- `MUTATION_OUTCOME_UNKNOWN` is limited to an I/O failure during COMMIT, when the
  process cannot tell whether the commit succeeded. Its recovery is outcome
  lookup.

### 4.6 Maintenance exclusion and markers

- `podway-store` owns an advisory `flock` lock file under `.podway/runtime/`.
  Every ordinary open takes it shared. Store creation, reset-all, workspace
  removal, and predecessor migration take it exclusive.
- After acquiring the lock, the holder revalidates the lock file's path and
  inode and retries if the file was replaced or deleted.
- An intent marker prevents exclusive requests from starving behind readers,
  because macOS `flock` has no writer preference.
- Contention beyond the command deadline returns the retryable
  `WORKSPACE_MAINTENANCE` error with a recovery recipe.
- `podway-engine` owns the reset-all and removal marker protocols. An exact
  retry with the same idempotency key and digest resumes an interrupted
  operation. Any other command on a marked worktree fails with
  `WORKSPACE_MAINTENANCE` and a recipe naming the resuming command. Reads never
  resume maintenance.
- Session reset and start replacement are ordinary single-transaction
  mutations.

### 4.7 Writer contract identity

- The Store records the contract-manifest digest of the binary that created or
  last migrated it.
- A binary with a different digest refuses to read or write the Store with
  `WORKSPACE_CONTRACT_MISMATCH`, unless its build declares that digest as a
  forward-migratable predecessor. It then migrates under the exclusive lock and
  records its own digest.
- Predecessor declarations are added only in a release commit or the following
  open-development commit, never in feature work.
- `v0.2.13` declares no predecessor; daemon-era Stores are legacy (section 4.9).
  Predecessor migration starts with the release after `v0.2.13`.
- The Store DDL becomes a contract-manifest asset, so a schema change moves the
  digest.
- A development build whose contract assets differ refuses Stores written by
  other builds; development uses scratch worktrees. This replaces runtime modes
  as the isolation mechanism.

### 4.8 No global state

- Podway neither creates nor reads `~/.podway`. The workspace registry, daemon
  and bootstrap logs, service metadata, sockets, locks, and runtime-reset
  records are removed.
- Workspace identity remains worktree-local through canonical Git paths
  ([ADR-0036](../architecture-decision-records/0036-use-canonical-git-paths-for-workspace-identity.md));
  a copied or moved worktree is detected from the Store's recorded identity.
- Diagnostics use command error envelopes and the Store's bounded operational
  journal.

### 4.9 Legacy runtime state

- Classification detects Store generations 1 to 13 and daemon-era markers
  (`reset.marker`, the workspace removal marker, `mode-switch.json`, and
  `development-v2.marker`). It fails closed without writing, using
  `LEGACY_RUNTIME_STATE_UNSUPPORTED`, which replaces
  `LEGACY_PROCEDURE_STATE_UNSUPPORTED`. The recipe is `podway reset --all --yes`.
- `reset --all` on a legacy worktree deletes the runtime directory contents and
  creates a fresh Store. It makes no backup and performs no conversion.

### 4.10 Public contract generation

- One new generation per still-emitted family that carries `queue`,
  `admission`, or `job`. Today 47 schemas carry `admission` and 13 status, next,
  and observation schemas require `queue`. The success envelope becomes
  `podway.output/v4`, failures `podway.error/v2`, and recovery recipes
  `podway.recovery-recipe/v2`. Direct and delegated results share one
  generation.
- Retired families: daemon status, readiness, endpoint, IPC request, detached
  admission, job result, job lookup, job wait timeout, runtime reset, workspace
  mode, managed runtime, Aquarium development service, and registry.
- New families: the outcome lookup result; error details for busy, maintenance,
  legacy state, writer mismatch, and retired commands; Store marker formats; the
  writer-predecessor declaration; `podway.version-result/v2`; and
  `podway.contract-manifest/v2` without `supported_ipc_ids`.
- Input and output bounds keep their values. The 1 MiB request and response
  budgets become named input and output bounds, preserving the semantics of
  ADR-0024 and ADR-0037.

### 4.11 Retired command surface

The commands `daemon *`, `terminate`, `runtime reset *`, `job list`,
`job status`, `job wait`, `job cancel`, `job lookup`, and `workspace mode *` and
the flags `--mode`, `--dev`, `--socket`, `--detach`, `--wait-for-idle`, and
`--after-job` fail before any Store access with `COMMAND_RETIRED`. The error
names the replacement or states that none exists. A closed retired-command table
is a canonical asset.

### 4.12 Configuration

`podway.workspace/v1` remains the configuration format. Its `job_queue` member
is accepted, ignored, and no longer written. A `podway.workspace/v2` document is
accepted only when `mode` is absent or `prod`; any other mode fails with
`WORKSPACE_CONFIG_INVALID` and guidance to remove `mode` and reset.

### 4.13 Error catalog

- Retire exit class 3 (`daemon`) and these codes: every `DAEMON_*` code,
  `PROTOCOL_VERSION_UNSUPPORTED`, `SOCKET_ENDPOINT_INVALID`,
  `WORKSPACE_QUEUE_FULL`, every `WORKSPACE_MODE_*` code, every `JOB_*` code,
  every `RUNTIME_RESET_*` code, and `UNSUPPORTED_V2_CAPABILITY`.
- Keep `WORKSPACE_MAINTENANCE`, now process-independent;
  `IDEMPOTENCY_KEY_REUSED`; and `MUTATION_OUTCOME_UNKNOWN`, re-scoped to COMMIT
  I/O failure.
- Add `WORKSPACE_BUSY` (retryable; the write lock or busy wait exceeded the
  deadline), `LEGACY_RUNTIME_STATE_UNSUPPORTED`, `WORKSPACE_CONTRACT_MISMATCH`,
  and `COMMAND_RETIRED`.
- `REQUEST_TOO_LARGE` becomes an input-bound error without IPC framing.

### 4.14 Deadlines

`--timeout` bounds the total wait for the maintenance lock and the SQLite write
lock. The default is 5 seconds. Exceeding it returns `WORKSPACE_BUSY` or
`WORKSPACE_MAINTENANCE` with no partial effect.

### 4.15 Tooling and qualification

- `make test` remains the development gate. End-to-end tests run the single
  binary against scratch worktrees.
- `make dist` packages one binary and qualifies it against scratch worktrees,
  with no daemon process or socket to clean up.
- The Aquarium managed development service, its controller, and its schemas are
  retired. Any replacement belongs to Aquarium under its own contract.
- The Dolgorae adapter contract is regenerated as a new version.
- Crash failpoints for the real binary are debug-only and reuse the existing
  proof that release builds ignore test hooks.

## 5. Failure handling and compatibility boundaries

### 5.1 Contention and deadlines

A writer that cannot obtain the maintenance lock or the SQLite write lock within
the deadline fails with `WORKSPACE_BUSY` or `WORKSPACE_MAINTENANCE`. No partial
effect and no outcome record remain. The error is retryable and carries a
read-only recovery recipe.

### 5.2 Crash boundaries

| Termination point | Required result |
|---|---|
| Before BEGIN | No effect; a same-key retry applies once. |
| After BEGIN, before state is written | Rollback; no effect; a same-key retry applies once. |
| After state, before the outcome is written | Rollback; no effect, because state and outcome commit together. |
| After COMMIT, before output | The effect and outcome exist; outcome lookup and a same-key retry return the stored success. |
| During reset-all or removal | The marker records progress; an exact retry completes the operation exactly once. |

### 5.3 Legacy state and cutover

Daemon-era state fails closed and is never converted. The supported cutover is:

1. Finish or abandon active sessions.
2. Run `podway daemon uninstall` with the old binary, removing the LaunchAgent.
3. Retire Aquarium's managed development service through its old controller, if
   installed.
4. Install the new binary and remove `~/.podway`.
5. Run `podway reset --all --yes` in each worktree that holds legacy state.

In-flight sessions are lost at step 5. A still-running old daemon could write to
unlinked files during a reset, which is why step 2 precedes it.

### 5.4 No backward compatibility

Clients built for `podway.output/v3`, `podway.observation-result/v3`, or the
daemon error codes stop at their own schema checks when they meet the new
generation. No compatibility shim, alias, or dual emission is provided.

### 5.5 Delivery constraints

- No patch release is cut from the main branch between `V2DFC-007` and
  `V2DFC-009`, while the release tooling still expects a daemon. A `v0.2.12`
  hotfix branches from its tag.
- Every task commit passes `make test`. Link and locator repair lands in the
  same commit that moves or deletes the target.

### 5.6 External consumers

- Aquarium's routine path uses `observe --wait-for-idle`, `podway.output/v3`,
  `podway.observation-result/v3`, `job lookup`, and routing by `DAEMON_*` codes.
  Its setup and diagnosis path uses `daemon wait-ready`, and its development
  contract pins a managed service started with `--dev`. Aquarium must ship an
  update before `v0.2.13` is installed.
- Dolgorae must re-pin to the regenerated adapter contract.
- `V2DFC-003` records the exact integration delta for both repositories.

## 6. Roadmap ownership, dependencies, and traceability

The roadmap owns order and status. Tasks execute in numeric order with one
active task. `V2DFC-004` and `V2DFC-005` change independent layers, but both
precede `V2DFC-006`. Each task is one logical implementation-and-verification
commit that passes `make test`, subject to separate commit authorization.

### V2DFC-001: Prove the direct-write model against pre-registered thresholds

Purpose: produce feasibility evidence for in-process writes before any authority
or product change. A failed check blocks the epic.

- Baseline: release-profile `podway` and `podwayd` in the repository-owned
  isolated release-qualification runtime, never the installed LaunchAgent.
  Fixtures are S (small), M (the `V2DHP` scale: six batches of 16 items with
  55,000 characters each), and L (the largest single Store observed on the
  maintainer host), each also with a hot WAL. This baseline is the only daemon
  reference left after `V2DFC-007`.
- Prototype, test-only: one IMMEDIATE commit for `set`, `complete`, and
  `record --stdin` with lookup-first success records over the existing
  relational helpers; a cheap open that includes the lock, the marker check, and
  the writer-identity read; a child-process re-exec harness; and a
  release-profile example binary for full-invocation timing.
- Location: ignored integration tests and an example inside
  `crates/podway-daemon`, deleted with that crate in `V2DFC-007`. Never in an
  `e2e_*` target, because the end-to-end runner includes ignored tests.

Done when every check in section 7.1 has its method, host, build profile, commit,
sample size, percentiles, and verdict recorded in this dossier; raw logs stay
under the ignored `artifacts/` directory; and `make test` passes. If every check
passes, the task completes. If any check fails, the task becomes Blocked and the
epic returns to the owner.

### V2DFC-002: Accept the daemon-free architecture authority

Purpose: turn the gate evidence into accepted architecture before any contract
or product change.

- Mark ADR-0038 Accepted with a summary of the measured gate results.
- Change ADR-0003, ADR-0012, ADR-0020, ADR-0022, ADR-0023, ADR-0029, ADR-0031,
  and ADR-0032 to `Superseded by` ADR-0038. Add `Superseded in part by`
  ADR-0038 to ADR-0004, ADR-0005, ADR-0006, ADR-0024, ADR-0027, ADR-0030,
  ADR-0035, and ADR-0036. Update the ADR index.
- Add AGENTS.md transition wording: the daemon invariants, crate ownership, and
  global-state rules describe shipped behavior until `V2DFC-007`, and `V2DFC`
  tasks change them only as this dossier specifies.
- Update this dossier where the gate evidence changes a design decision.
- Change no specification, contract, or code.

Done when `tools/verify_docs.py`, `git diff --check`, and `make test` pass.

### V2DFC-003: Reserve the daemon-free contract generation

Purpose: add every new contract as a reserved, unemitted asset, so later
behavior changes have stable targets.

- Schemas: `podway.output/v4`, `podway.error/v2`, `podway.recovery-recipe/v2`,
  new generations of the still-emitted families listed in section 4.10, the
  outcome lookup result, the new error details, marker formats, the
  writer-predecessor declaration, `podway.version-result/v2`, and
  `podway.contract-manifest/v2`.
- Catalogs: the new error codes; a reserved `outcome.lookup` route in the
  command catalog and `contracts/command-routes.json` under a new `direct`
  execution class; the retired-command table; known answers under
  `docs/examples/json`.
- Store DDL: `assets/specifications/sqlite-v14.sql` as a complete baseline,
  registered as a contract-manifest asset.
- Rust: `podway-protocol` validators, `ERROR_CODE_CATALOG_V1` entries,
  fixtures, and result-family registrations.
- Tooling: counts and sets in `tools/verify_contracts.py` and
  `tools/create_dolgorae_handoff.py`, a regenerated Dolgorae adapter contract,
  and the contract manifest.
- Specifications: reserved entries in the JSON contract, error, and SQLite model
  specifications. Record the Aquarium and Dolgorae integration delta in this
  dossier.

Done when `tools/verify_contracts.py --check`, `--sentinels`,
`tools/contract_manifest.py --check`, the Dolgorae handoff self-test, the
protocol family tests, and `make test` pass, and runtime output is unchanged.

### V2DFC-004: Extract host-independent domain services into podway-engine

Purpose: move domain logic out of the daemon crate without changing behavior.

- Create `crates/podway-engine`, depending on core, protocol, config, store,
  Git, and presets. The daemon depends on it.
- Move read projections (`v2_read_service.rs`); start, snapshot, and item
  preparation and canonical request digests from `execution.rs`;
  `native_execution.rs`; and the resolver, binding inspector, validated runtime
  and removal directories, and marker codecs from `workspace.rs`.
- Keep admission, claimed execution, dispatch, envelope building, error
  mapping, the runtime-workspace protocols, the registry, scheduler, worker,
  server, and observability in the daemon.
- Move the pure tests with their code, including `int_v2run002_views` and
  `int_v2rel002_maximum_next`.
- In the same commit, update `contracts/cargo-adjacency.json` and
  `tools/verify_contracts.py` for ten crates, the product package list in
  `tools/verify_patch_release.py`, the repository-structure and Rust-codebase
  architecture documents, AGENTS.md crate ownership, crash-boundary and proof
  locators, traceability links, and the contract manifest.

Done when `make test` passes, the `cargo test -- --list` inventory is identical
before and after, and end-to-end output is unchanged.

### V2DFC-005: Build the direct Store and maintenance primitives beside the legacy Store

Purpose: implement the daemon-free persistence layer next to the existing
Store, so the daemon keeps working until the switch.

- Add a Store type over schema generation 14 that coexists with
  `SqliteStoreV1`. The legacy type rejects generation 14 as newer; the new type
  classifies generations 1 to 13 as legacy without opening them read-write.
- Implement the open paths in section 4.3, rebinding a moved root only inside a
  write transaction.
- Implement the maintenance lock and marker primitives in section 4.6 and the
  writer identity and predecessor-migration framework in section 4.7.
- Implement the outcome table and the single-transaction commit in sections
  4.4 and 4.5, with the clock read inside the transaction and a deadline-derived
  busy error.
- Port the relational-state Store tests to the new type. Freeze legacy fixtures
  (generation 13 with jobs, receipts, a hot WAL, and every marker kind) as text
  recipes produced by the legacy code. Add multi-process contention,
  deterministic `SIGKILL` failpoints, byte-identity of classified legacy files,
  identity mismatch, and lock and marker race tests.
- Add the new crash windows to `quality/crash-boundaries-v1.json`, keeping its
  filename because the roadmap links it.

Done when `make test` passes with these tests and the daemon and legacy tests
are unchanged.

### V2DFC-006: Execute every retained command through the direct engine

Purpose: complete the in-process execution API before the CLI switches.

- Engine API: a typed request (command, selector, fences, idempotency key,
  deadline, injected clock and identifiers) that yields `podway.output/v4` or
  `podway.error/v2` with `podway.recovery-recipe/v2`.
- Cover all reads; all mutations, including `start` with dry-run and state
  policies, archive and purge, `record --stdin`, and delegation acceptance;
  `init`, `doctor` with its new checks, reset-all, removal, outcome lookup, and
  resumption of interrupted maintenance.
- Prepare outside the transaction and revalidate inside it (section 4.4).
- Validate every emitted envelope. Compile result validators lazily per schema
  if evidence item E1 shows that startup cost matters.
- Port about 95 daemon-dispatch domain tests to an engine harness; the daemon
  copies remain until `V2DFC-007`. Re-point reference-path and payload proofs,
  crash windows, and traceability links, and add engine-level crash, race, and
  maintenance tests.

Done when `make test` passes, every emitted envelope validates in tests, and the
task commit records each ported and retired test with a reason for each
retirement.

### V2DFC-007: Switch the CLI to the direct engine and delete the daemon host

Purpose: make shipped behavior daemon-free in one commit.

- The CLI depends on `podway-engine` and drops `podway-service` and the IPC
  client. Command routes become local or direct, the new contract generation is
  emitted, and `podway outcome lookup` is added.
- Retired commands and flags return `COMMAND_RETIRED` (section 4.11). This
  replaces the CLI rule that removed commands fail argument parsing. Help text
  and shell completion change with it.
- Contracts: route registry version 2 with only local and direct routes,
  explicitly superseding the rule that the route delta never shrinks; catalog
  availability; transition-matrix result identifiers; the regenerated Dolgorae
  adapter contract; and `tools/verify_contracts.py` rules.
- Delete `crates/podway-daemon` and the `podwayd` binary from workspace members,
  adjacency, crate order, and product packages. `tools/run_e2e.py` builds one
  binary, and `tools/verify_test_layout.py` drops its daemon rules.
- Move CLI integration tests from the scripted fake socket to scratch worktrees,
  delete the IPC client tests, and rewrite the architecture test. Add
  single-binary end-to-end tests for contention, `SIGKILL` after commit before
  output followed by outcome lookup, reset-all and removal crash resumption,
  legacy fail-closed handling and reset, writer refusal, retired-command
  guidance, and delegation and goal flows.
- Replace or retire `tools/dev_runtime.py` and its `dev-runtime-test` target,
  retarget or retire `tools/qualify_supervised_delegation.py`, delete
  `docs/implementation-tips/dev-runtime.md` with link repair, and update the
  `use-podway` skill grammar.

Done when `make test` passes, no `podwayd`, socket, or IPC use remains outside
retirement guidance, and checks G7 and G8 are re-measured with the real binary
against the `V2DFC-001` baseline without regression.

### V2DFC-008: Retire residual daemon-era infrastructure and contracts

Purpose: remove the remaining daemon-shaped code and contract surface.

- Delete `crates/podway-service` and the LaunchAgent template; IPC framing,
  codecs, and envelopes; the IPC protocol specification and the protocol test
  that embeds it, after moving its evidence-paging and response-budget sections
  to surviving specifications under identical headings; `SqliteStoreV1`, the
  generation 1 to 13 DDL and migrations, jobs, codecs, receipts, and their
  integrity checks; `RuntimeModeV1`, configuration mode rewriting, and the Git
  registry types.
- Retire daemon-only and superseded schemas and error codes, exit class 3, the
  reservation fixtures, daemon known answers, the store, service-manager, and
  runtime-reset interface contracts, and the daemon crash windows.
- Retarget fuzz targets to CLI input decoders and `podway.output/v4`.
- Update verifier counts, schema paths, and `deny.toml`.
- Repair every link to deleted targets, including the macOS service and runtime
  reset documents, ADR-0032, requirements traceability, the graph-engineering
  candidate, the roadmap, and the archive.

Done when `make test`, `make test-fuzzing`, and
`tools/verify_contracts.py --sentinels` pass and crate adjacency lists eight
crates.

### V2DFC-009: Rebuild release and development tooling around one binary

Purpose: restore the release gate for a single binary.

- Identity: `podway.contract-manifest/v2` without `supported_ipc_ids`,
  `podway.version-result/v2`, the build identity, a single-binary contract
  verifier with the matching release-archive call and verifier string check,
  and the `dist`, `dist-patch`, and `release-version-check` targets.
- Packaging and qualification: the archive layout, release evidence,
  packaged-binary qualification on scratch worktrees without daemon cleanup,
  bundle verification that keeps the disabled-test-hook probe, Dolgorae handoff
  creation, and patch-release verification with predecessor declarations and
  the manifest path.
- Aquarium: retire the managed development service controller, bundle, and
  schemas, and retire the Aquarium development targets and tests unless Aquarium
  adopts a replacement contract. Delete the Aquarium development service
  specification with link repair.
- Release documentation: the release workflow, release admission QA, and the
  AGENTS.md release-QA and gate-selection rules. Release upgrade QA for
  `v0.2.12` to `v0.2.13` becomes legacy fail-closed handling plus an explicit
  reset.

Done when `make test`, `make release-prepare`, and `make test-fuzzing` pass and
a native dry run (package with a non-distribution class, qualify, hand off, and
verify the bundle) passes. The full `make dist` runs at the release commit.

### V2DFC-010: Promote durable documentation and close the epic

Purpose: make every lasting decision durable and retire this dossier.

- Specifications: CLI, automation client, JSON contract, errors and exit codes,
  observability (journal only), release and packaging, security and trust,
  recovery, transactions and idempotency, SQLite model, supervised delegation,
  testing and conformance, and requirements traceability.
- Architecture: system, Rust codebase, repository structure, risk register, and
  Git worktree and filesystem. Delete the daemon and write-queue architecture
  document with link repair.
- README: installation without a daemon and the cutover in section 5.3, keeping
  the install loops and the custom-Procedure block that `tools/verify_docs.py`
  requires.
- The `v0.2.13` changelog, final AGENTS.md wording, and the skills.
- Update the graph-engineering and Web-console candidates to the daemon-free
  model, delete the startup-recovery candidate with link repair, resolve
  deferred feedback DF-001, DF-009, DF-010, and DF-015, and retire
  `release/dolgorae-acceptance-matrix-v1.json`.
- Replace every link to this dossier with durable authorities, remove its index
  entry, and delete it.

Done when `make test` passes, the independent review is resolved, and the epic
is Completed. Release, installation, and activation remain separately
authorized.

## 7. Verification and release acceptance

### 7.1 Feasibility gate

Thresholds are pre-registered at adoption. The owner may adjust them before
`V2DFC-001` starts; they must not change after measurement begins.

Measurement protocol: native Apple Silicon on AC power, release profile for both
paths, and matched fixtures. Each latency check uses 20 warm-up runs and at
least 200 measured runs and reports p50, p95, p99, and maximum. The prototype's
open path includes the lock, the marker check, and the writer check.

| ID | Check | Method | Pass threshold |
|---|---|---|---|
| G1 | Multi-process correctness | 8 OS processes × 1,000 fenced mutations plus 2 observers; same-key retries; 5 s busy timeout | 0 lost or duplicated effects (final state equals serial replay of the outcome log); 0 stale-fence commits; 0 `SQLITE_BUSY` reaching a caller; observers never decode an invariant violation |
| G2 | Wait and fairness | Instrument the IMMEDIATE wait and the lock hold during G1 | Wait p99 ≤ 250 ms and maximum ≤ 1 s; hold p99 ≤ 50 ms; the slowest process finishes within 2× the fastest |
| G3 | Over-deadline behavior | A controller holds the write lock past a 500 ms deadline | A structured busy error within the deadline plus 100 ms; no partial effect and no outcome record |
| G4 | Crash boundaries | `SIGKILL` at the five boundaries in section 5.2, at least 20 each, plus 200 random kills; then lookup and same-key retry | The cheap open succeeds after every kill; each key has either one effect with a record and replay, or no effect with not-found and exactly one apply on retry; a final `integrity_check` passes |
| G5 | Maintenance exclusion | 100 reset-all and removal cycles with a kill at each marker step, against 4 writers and 4 readers | 0 acknowledged commits into an unlinked or superseded file; contenders see only success or the maintenance error; interrupted maintenance completes exactly once; exclusive acquisition p99 ≤ 2 s |
| G6 | Preparation races | 1,000 state or artifact changes between preparation and commit | 100% rejected; 0 stale commits |
| G7 | Full-invocation latency | `observe`, `set`, `complete`, and `record --stdin` with 16 items on S, M, and L, daemon path versus direct path | Direct p50 and p99 no worse than the daemon's for every command and fixture |
| G8 | Open cost | Read-only and mutation opens on L, warm (SHM present) and cold (hot WAL, no SHM) | Warm p99 ≤ 100 ms; cold p99 ≤ 1 s |
| G9 | WAL bound | 10-minute soak with 8 writers and 4 readers | WAL ≤ 64 MiB throughout and ≤ 4 MiB within 5 s after load stops |
| G10 | Write-free legacy classification | Generation 1, 4, 10, and 13 fixtures with a hot WAL and markers | A structured legacy result; the SHA-256 of every file is unchanged |

Evidence recorded without a threshold:

- E1: a per-phase cost breakdown of one invocation, including result-validator
  compilation.
- E2: an inventory of tests to port, retire, or rewrite.
- E3: an audit confirming that no retained behavior needs a resident process.
- E4: the cost of `quick_check` and `integrity_check` on fixture L, justifying
  their move to `doctor` and maintenance.

### 7.2 Stop rule

If any gate check fails, `V2DFC-001` becomes Blocked with its data recorded in
this dossier and the epic returns to the owner. There is no automatic fallback,
and rerunning the gate requires an owner decision.

### 7.3 Adoption requirements

Passing the gate proves feasibility only. These requirements are proven later by
permanent tests:

| Requirement | Owning tasks |
|---|---|
| Writer identity and predecessor migration, including concurrent incompatible opens | `V2DFC-005`, `V2DFC-007` |
| Legacy fail-closed handling and explicit reset | `V2DFC-005` to `V2DFC-007` |
| Marker recovery for reset-all and removal | `V2DFC-005` to `V2DFC-007` |
| Success-only outcome lookup and its retention boundary | `V2DFC-003`, `V2DFC-006` |
| Retired-command guidance | `V2DFC-003`, `V2DFC-007` |
| New contract generation conformance and the regenerated adapter contract | `V2DFC-003`, `V2DFC-007`, `V2DFC-008` |
| Input and output bounds | `V2DFC-006`, `V2DFC-007` |
| Latency re-measured against the gate baseline | `V2DFC-007` |
| Single-binary release with test hooks proven disabled | `V2DFC-009` |
| Aquarium and Dolgorae integration deltas delivered | `V2DFC-003`, `V2DFC-010` |
| Documentation promotion and dossier retirement | `V2DFC-010` |

### 7.4 Release acceptance

The `v0.2.13` release commit runs the full `make dist`. Its upgrade
qualification proves that `v0.2.12` state fails closed and that an explicit
reset produces a working worktree. Development acceptance does not authorize a
release, installation, removal of an installed daemon, or Aquarium runtime
changes. Installation waits for Aquarium and Dolgorae releases that support the
new contract generation.

## 8. References

- [ADR-0003: Make the Daemon the Sole Normal Writer](../architecture-decision-records/0003-daemon-single-writer.md)
- [ADR-0004: Store Task State Inside the Worktree](../architecture-decision-records/0004-worktree-local-state.md)
- [ADR-0006: Use Same-User Local Trust Without an Access Key](../architecture-decision-records/0006-same-user-local-trust.md)
- [ADR-0013: Release and Support Only Native Apple Silicon macOS](../architecture-decision-records/0013-native-apple-silicon-macos-only.md)
- [ADR-0024: Bounded Evidence Scale and Paged Read-back](../architecture-decision-records/0024-bounded-evidence-scale-and-paged-read-back.md)
- [ADR-0029: Serve Phase-Aware Daemon Readiness](../architecture-decision-records/0029-serve-phase-aware-daemon-readiness.md)
- [ADR-0036: Use Canonical Git Paths for Workspace Identity](../architecture-decision-records/0036-use-canonical-git-paths-for-workspace-identity.md)
- [ADR-0037: Bound Procedure Graphs at 1,024 Nodes](../architecture-decision-records/0037-bound-procedure-graphs-at-1024-nodes.md)
- [ADR-0038: Remove the Daemon and Commit Mutations In-Process](../architecture-decision-records/0038-remove-the-daemon.md)
- [Recovery, retention, and maintenance](../specs/storage/recovery-retention-and-maintenance.md)
- [Transactions, concurrency, and idempotency](../specs/storage/transactions-concurrency-and-idempotency.md)
- [JSON contract](../specs/interfaces/json-contract.md)
- [Errors and exit codes](../specs/interfaces/errors-and-exit-codes.md)
- [Automation client contract](../specs/interfaces/automation-client-contract.md)
- [Podway Web Operations Console candidate](TODO-podway-web-operations-console.md)
- [Podway Graph Engineering Evolution candidate](TODO-podway-graph-engineering-evolution.md)
- [Startup Recovery Performance candidate](TODO-podway-startup-recovery-performance.md)
- [V2DFC roadmap](../roadmap/README.md#v2dfc-daemon-free-core)
