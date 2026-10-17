# Podway Domain and Application Boundaries

## 1. Status and authority

- Status: Adopted
- Adoption date: 2026-10-14
- Owning epic: [BOUND](../roadmap/README.md#bound-domain-and-application-boundaries)
- Dependency: accepted OPTIM completion
- Review baseline: v0.2.14, `fa29dba929777b020377bc9803cb0a3954d219b1`
- Implementation baseline: the actual accepted OPTIM source and evidence

The owner approved responsibility separation under the
[internal-improvement scope](../roadmap/README.md#current-internal-improvements).
This dossier describes unfinished work. It neither asserts that the target
separation exists nor supersedes an accepted ADR or public contract. Task order
and status belong only to the roadmap. No owner-level product decision remains
open within the approved scope; incompatible changes require separate approval.
BOUND must be independently complete without requiring READS implementation.

## 2. Verified context and evidence

The crate graph already keeps Core free of infrastructure, but semantic ownership
is not fully aligned with the documented target:

| Finding | Current implementation |
|---|---|
| Pure state transitions and their errors coexist with SQL persistence | [Store graph state](../../crates/podway-store/src/v2_state.rs), `GraphSessionStateV2`; [workflow memory](../../crates/podway-store/src/v2_memory.rs), `WorkflowMemoryStateV2`, `GraphMutationErrorV2`; [goal state](../../crates/podway-store/src/v2_goal.rs) |
| Persisted snapshot admission combines structural validation and derived runtime declarations | `ProcedureSnapshotV2` and `SnapshotMemoryModelV2` in those Store modules |
| Shared fence/success helpers exist but some callers implement equivalents independently | [mutation helpers](../../crates/podway-engine/src/direct_mutation.rs), [cursor/begin execution](../../crates/podway-engine/src/direct_execution.rs), [item execution](../../crates/podway-engine/src/direct_items.rs) |
| Callbacks transport typed Engine errors using paired side-channel capture/return | Engine mutation handlers; [DF-023](../deferred-feedback/README.md#resolved) |
| Grammar, authoring, interactive start, safe file handling and rendering still share `command.rs`; fence fetching/filling already lives in `command/direct.rs` | [CLI command](../../crates/podway-cli/src/command.rs), [Direct adapter](../../crates/podway-cli/src/command/direct.rs); completion and retirement are already separate and must not move back |

These are responsibility and change-risk findings, not evidence of a current
lost mutation or incorrect public result. DF-023 is promoted to BOUND-004 without
claiming its code has been corrected. Other deferred entries are not implicitly
adopted just because they mention a touched module.

### Paired callback transport inventory

The reviewed baseline has these nine sites. Refresh their locations against
accepted OPTIM in BOUND-001; searching only for `execution_error` is insufficient.

| Site | Source | Side-channel state to account for |
|---|---|---|
| `execute_cursor_v1` | [cursor/begin](../../crates/podway-engine/src/direct_execution.rs) | `execution_error` |
| `begin_session_v1` | [cursor/begin](../../crates/podway-engine/src/direct_execution.rs) | `execution_error` |
| `record_many_v1` | [items](../../crates/podway-engine/src/direct_items.rs) | `execution_error` |
| `execute_item_v1` | [items](../../crates/podway-engine/src/direct_items.rs) | `execution_error` |
| `accept_delegation_v1` | [delegation](../../crates/podway-engine/src/direct_delegation.rs) | `execution_error` |
| `execute_procedure_v1` | [procedure](../../crates/podway-engine/src/direct_procedure.rs) | `execution_error` |
| `execute_session_v1` | [sessions](../../crates/podway-engine/src/direct_sessions.rs) | `execution_error` |
| `start_session_v1` | [start](../../crates/podway-engine/src/direct_start.rs) | `failure` and `preparation_failed` |
| `existing_mutation` | [workspace](../../crates/podway-engine/src/direct_workspace.rs) | `resolution_error` and `refused_plain_init` |

Bootstrap callbacks that return Store errors directly remain direct; do not
invent side-channel machinery for them. A changed name or helper extraction
does not remove the obligation to test each underlying operation.

### Fence and success-helper groups

| Group | Reviewed callers | Refactoring constraint |
|---|---|---|
| Already share helpers | record-many, procedure execution, delegation accept | Preserve `check_session_fences`, `check_attempt_fence`, `graph_success` and their call order |
| Inline matching parts | cursor, begin, single-item execution | Share only equivalent checks/output; single-item operations use item-local revision, not a new session-revision fence |
| Deliberately different session results | purge, reset, archive | Keep their distinct envelope/result and dry-run/retention semantics |

## 3. Goals, non-goals, and scope

Make domain transitions independently testable without SQLite, keep persistence
validation at the actual storage boundary, and make each Engine/CLI operation's
role explicit. Preserve OPTIM's request-scoped sharing and batch draft work.

Use the existing eight crates and allowed production adjacency. Do not add a
Repository framework, event bus, event sourcing, plugin/executor abstraction,
new daemon, parallel cursor, generic workflow engine or new schema generation.
No new public commands, capability claims, graph/continuation implementation,
release or deployment belong to BOUND. Do not absorb unrelated maintenance,
checkpoint, recovery-policy or prompt-rendering changes into module cleanup.

## 4. Accepted design decisions and interfaces

### Ownership after refactoring

| Owner | Required responsibility | Explicit exclusion |
|---|---|---|
| Core | Pure graph/session/workflow/goal state, transition inputs/results, invariants and domain failures | SQL, filesystem, clock reads, native identity discovery, public envelopes |
| Config | Source YAML/JSON parsing, authoring semantics, canonicalization and source diagnostics | Store reads or Engine dependencies |
| Store | Row decoding, persisted schema/canonical digest checks, relational consistency, transactions, success outcomes, leases and maintenance | Application orchestration or duplicated domain transition policy |
| Engine | Command-specific validation order, preparation, revalidation, domain invocation and public result/error composition | SQL access through leaked connections, alternate state writer |
| Protocol | Existing bounded public carriers, envelopes, schema validation and serialization | Storage or Engine dependencies |
| CLI | Command grammar, prompting, local authoring adapters and rendering through Engine | Direct Store/Git production access |

### Move pure rules without flattening trust boundaries

First separate pure state and transition code from row mapping within Store; then
move the pure portion into cohesive Core modules. Include graph/session state,
workflow-memory state, goal state, mutation inputs/outcomes and domain errors
where their implementation is infrastructure-free. Preserve the existing
invariant-enforcing constructors and immutable payload ownership.

Storage failures remain Store failures. A malformed SQL row, invalid persisted
canonical document, writer mismatch and a valid command rejected by a domain
rule must remain distinguishable. Split mixed error enums at that seam and map
them back to the existing public errors in Engine. Do not leak SQLite or Store
error types into Core to make a move compile.

The state/error structures do not themselves hold `rusqlite` connections, even
though their files contain SQL load/insert functions. The concrete obstacles are
`GraphMutationErrorV2::InvalidState(StoreValueErrorV1)`, snapshot schema admission,
and the stored derived-model error. Split domain invalid-state values from
Store row/codec failures before moving transitions; leave load/insert functions
and `schema_matches_v2` in Store. Core already depends on `serde_json`, so guard
detail values using `Vec<serde_json::Value>` are not a new reverse dependency or
an automatic reason to invent another wire model.

Store successor validation still verifies legal persistence changes and relational
consistency. It may invoke shared Core predicates but must not blindly trust a
newly constructed aggregate or remove independent stored-data validation. Keep
changed-row writes and transactional outcome recording intact.

### Procedure representation and validation

Config remains the authoring parser. Store retains independent validation of
persisted canonical bytes, stored digest, schema shape and relational rows.
Neither Store-to-Config nor Core-to-Config dependency is allowed.

The shared pure runtime representation belongs in Core: bounded graph/item/goal
and delegation declarations required by transitions and projections, with
invariant-enforcing constructors. Config's validated source adapter and Store's
validated snapshot adapter construct that representation. Stored snapshot ID,
canonical bytes and digest remain authoritative and immutable; derived defaults
or indexes must not rotate the historical identity.

Share domain predicates through Core constructors, not a general JSON Schema
interpreter in Core. Keep storage-only wire/row checks at the Store boundary and
source diagnostics in Config. Do not delete Store's current structural schema
checker unless its complete acceptance/rejection behavior is preserved by the
replacement at the same boundary. Prefer retaining it to silently weakening
validation. This epic does not add schema keywords or alter authoring semantics.

Preserve the observed deferred-admission seam: today `ProcedureSnapshotV2::new`
can succeed while `Arc<Result<SnapshotMemoryModelV2, StoreValueErrorV1>>` retains a
derived-model error, which `memory_model()` returns at later workflow admission.
A pure replacement may use a domain-owned error plus a Store mapping, but must
not turn this into an earlier constructor failure, suppress it, or change which
competing error is returned. Characterize both the successful snapshot creation
and subsequent failure before moving this code, including malformed persisted
input. This requirement also applies to the minimum types moved in BOUND-002;
it cannot be postponed until BOUND-003 repairs a changed admission order.

The Engine projection context from OPTIM may borrow the pure model but must not
become a persistent cache or force Store to import an authoring representation.
Maintain already validated immutable value/declaration reuse under the exact
identity rules in the transaction specification.

### Typed mutation callback failure transport

Replace the paired outer error slots and temporary Store errors across the
complete inventory, including start/workspace sites with other variable names,
with one typed callback-result path. Store owns transaction setup/rollback/commit;
Engine owns the callback failure and its public mapping. Use a small generic
internal runner or an explicit typed callback-failure wrapper, with the Store
error branch separate from the caller's failure. The mechanism must have no
Store dependency on Engine or Protocol. Choose concrete names locally; the
ownership and observable error behavior are fixed by this design.

Keep any existing Store-only entry point as a thin delegate to the same runner
where current callers need it. Do not keep two implementations of commit,
outcome lookup or rollback. Replays must bypass the mutation callback exactly
as before, and callback failure must roll back without retaining an outcome.
COMMIT I/O ambiguity alone keeps its existing unknown-outcome mapping.

Reuse the existing shared session/attempt fence and success-construction helpers
where contracts agree. Preserve deliberate command differences: item-local
revision fences, session-fenced batches, prepared lifecycle, goal revision,
delegation, dry-run and maintenance rules. This is not permission to invent one
universal validation order or to move all checks into a generic middleware chain.

### CLI decomposition

Split command grammar, authoring, safe Procedure file operations, interactive
start, preflight orchestration and rendering into internal modules. Existing
`direct`, `completion` and `retirement` modules remain authoritative for their
roles. Share only genuinely common classification and formatting helpers.

Give the fence-preflight boundary one owner: move `needs_facts`, `fill_facts`
and the gated standard-status fetch from `command/direct.rs` together with
`StatusPreflight::from_direct` from `command.rs` into one internal preflight
module. Keep `Command::needs_preflight` classification coordinated with it;
BOUND-001 records its exact home and callers. Moving only the parser or only the
fetch is not the intended handoff. The fetch still uses public status in BOUND.

READS-004 replaces only this fence-preflight module's public status projection
with Engine facts. Dynamic completion's status query, start/reset dry-run and
terminal-disposition handling, and the later status read following
`SESSION_START_DECISION_REQUIRED` remain outside that replacement. They may
continue using shared parsing/fill helpers without changing their behavior.
Keep those caller distinctions explicit when moving helpers so the later task
does not accidentally replace every status read in the CLI.

Keep CLI output, flag precedence, late JSON handling, stdin-owned identity,
non-TTY behavior, help/completion and the original invocation deadline unchanged.
Do not include READS' typed facts feature in this move; retain the current
preflight behavior behind its extracted module until READS owns its replacement.
Module names are implementation details, not new public contracts.

## 5. Failure handling and compatibility boundaries

Apply the [shared preservation rules](../roadmap/README.md#preservation-rules).
Current supported Stores must remain usable without reset, reinitialization,
migration or canonical-asset changes. Preserve existing unsupported/newer/foreign
writer refusal. Internal Rust changes must update all known repository consumers;
retain the shared Direct API facade and do not assume external consumers agreed
to changed command/result contracts.

Capture the mapping from each moved domain/storage error and command-specific
validation stage to the existing public code, details, retryability and exit
status. Test competing errors to preserve which one wins. Preserve error
attribution to the requested mutation when a preflight fails.

No optimization may move artifact hashing into the writer, remove descriptor
checks, bypass transaction-current fence validation, or retain failed outcomes.
No domain test fixture becomes authority to mutate the user's active Store.

## 6. Roadmap ownership, dependencies, and traceability

The [BOUND roadmap](../roadmap/README.md#bound-domain-and-application-boundaries)
owns sequence and status. Start after accepted OPTIM completion; READS starts
after BOUND, not in the middle of a type move.

### BOUND-001: Freeze responsibility and invariant ownership

- Capture the accepted OPTIM baseline and inventory every move candidate, public
  Rust consumer, SQL/codec dependency, immutable cache and relevant test owner.
- Map each invariant and public failure to its pre/post-refactor owner, including
  goal revisions, delegation acceptance/completion, terminal dispositions and
  deferred memory-model errors. Reconcile all nine callback sites and the three
  helper groups above, not only matches for one error variable.
- Register a failure/replay test case per callback site and concrete old/new
  ownership of the fence-preflight fetch, parser and callers. Preserve the
  accepted OPTIM request-scoped sharing and single-draft structure before moving
  production code. OPTIM supplies accepted implementation and functional
  evidence, no performance baseline, comparator, or count ceilings. Any
  BOUND-owned future comparison must be registered independently.
- Record the concrete module/type moves and callback-failure mechanism before
  editing those production paths. This is engineering inventory within the
  approved ownership model, not reopening product or compatibility policy.
- Acceptance: no shared invariant has two competing normative owners, no moved
  Core type requires infrastructure, and focused characterization tests cover
  the public behavior affected by each move.

#### BOUND-001 implementation inventory

The accepted source baseline is local commit
`eb332d280b8b3f0fdf1169d565b748234626d2d7`. It preserves accepted OPTIM;
the unrelated `.mulgae/config.yaml` change is excluded. The following is the
concrete engineering handoff, not a second authority for Task status.

| Current owner / move candidates | Target and retained boundary | Known production consumers |
|---|---|---|
| Store `v2_state`: `GraphSessionStateV2`, transition inputs/outcomes, `GraphNodeCounterV2`, `AttemptMetadataV2`, graph/member/terminal invariants | Core graph module; caller-supplied time and IDs. SQL loaders/writers, archive queries, `GraphWorkspaceViewV2` and durable worktree identity stay Store. | Store connection/transaction, delegation and test-support adapters; Engine execution/native execution, all graph mutation handlers and read projections |
| Store `v2_memory`: `WorkflowMemoryStateV2`, attempt/item/blocker/evidence state, mutations/results, recorded-value digest helpers, pure history validation and `GraphMutationErrorV2` | Core workflow module. Preserve `RecordedSlotValueV2` immutable memo and `UnpublishedAttemptDraftV2`; row decoding, stored recorded JSON, changed-row writes and SQL successor checks stay Store. | Graph state, goal state, Store row adapters; Engine items, delegation, cursor and `v2_read_service` |
| Store `v2_goal`: `GoalStateV2`, criterion/attempt assessment state, pure goal/history validation and goal runtime declarations | Core goal module. SQL loads, writes, record digests and persistence successor admission stay Store. Goal interpretation remains independently demanded, not eagerly merged with memory-model admission. | Graph/workflow state, Store row adapters; Engine goal commands/read projections |
| Store `ProcedureSnapshotV2`, `GraphNodeSnapshotV2`, `SnapshotMemoryModelV2` and bounded node/item/evidence declarations | Minimum immutable runtime carrier and declaration interpretation move with BOUND-002. Store independently checks canonical bytes/digest/schema and persisted rows before constructing Core values; no schema assets or interpreter move to Core. | Engine start/source adapter, native execution and projections; Store snapshot loading and relational verification |
| Config parsed Procedure/graph/delegation declarations and Engine placement readers | BOUND-003 aligns adapters on the Core runtime representation. Config retains YAML/JSON diagnostics, graph vetting, canonical/default emission and source admission. Store retains independent persisted admission. | Config authoring/canonical/DOT adapters; Engine preparation, cursor, delegation and read projections |

Store's existing root reexports may temporarily expose the same Core types to
known consumers; they must not conceal a second transition implementation.
Update direct consumers to the actual owner where useful. Core receives no
Store, Config, Protocol, filesystem or SQLite dependency. Existing dependency
adjacency remains the executable constraint.

| Invariant / failure seam | Owner after move and observable preservation |
|---|---|
| Cursor, lifecycle, attempt identity, revisions, counters and transition legality | Core; Engine preserves each command's validation order and public mapping. Item-local revision fences remain distinct from session-fenced batches. |
| Goal revisions, citations, assessments and reactivation; delegation readiness; terminal eligibility | Core pure predicates/transitions; Store independently validates persisted histories and accepted receipts/dispositions. Engine retains goal fences, receipt checks and command-specific ordering. |
| Structural invalid graph/runtime declaration | A Core structural error carrying the existing bounded reason replaces the mixed `StoreValueErrorV1` dependency. Explicit Store/Engine conversions preserve `GraphMutationErrorV2::InvalidState` as `INTERNAL_ERROR`, exit 6, nonretryable, internal diagnostic details. `DomainError::InvalidState` remains `REQUEST_INVALID`, exit 2. |
| Invalid persisted canonical/schema/digest or relational row | Store; cold admission maps corruption to `WORKSPACE_STATE_UNREADABLE`, exit 5, nonretryable, workspace recovery details. Source admission keeps Config diagnostics and `PROCEDURE_INVALID`. |
| Stale session/attempt/item/goal fences and competing errors | Core/Engine retain current priority and conflict details. Session revision conflict remains exit 4, retryable. Store never records a rejection outcome. |
| Replay, clock/ID allocation, writer admission, rollback and COMMIT ambiguity | One Store transaction runner; Engine callback errors remain typed. Identity resolution receives the recorded outcome before digest comparison and replay selection; start replay recovers the pinned digest without fresh source preparation. Exact replay then bypasses the execution callback, clock reads and ID allocation. Only existing COMMIT I/O ambiguity gets unknown-outcome treatment. |
| Validated reads and batch application | Preserve OPTIM's borrowed request `ReadScope`, immutable recorded payload memo, and one unpublished attempt draft. No persistent cache or READS query/facts redesign. |

The deferred-model fixture is schema-shape-valid canonical JSON with a text item
whose `min_length` is 2 and `max_length` is 1. Actual observed boundaries are:

1. `ProcedureSnapshotV2::new` succeeds and retains its derived-model error.
2. Convenience `GraphSessionStateV2::prepared` fails through `empty_for_trace`.
3. Explicit `new_with_workflow_memory` with an empty workflow and Prepared trace
   succeeds. A stale revision wins over the retained error in `begin_v2`; a
   matching revision exposes `InvalidState` without changing the aggregate.
4. Persisting that explicit fixture succeeds, but an independent cold load
   rejects it as `CorruptStateV1 { record: Snapshot }` through
   `load_workflow_memory_v2`. The Engine refuses it before domain fences, without
   reading the mutation clock, changing sequence or recording an outcome.

These distinctions are characterized in
`int_v2dfc005_relational::bound001_deferred_memory_admission_and_cold_read_remain_distinct`
and
`int_v2dfc006_domain_ports::bound001_deferred_memory_error_keeps_public_mapping_and_fence_priority`.
Do not turn snapshot admission into eager derived-model admission, or describe
the malformed fixture as a supported Store that must continue successfully.

The existing registered
`int_v2dfc006_races::in_transaction_workspace_revalidation_keeps_typed_errors_and_exact_retry`
covers all nine callback sites through ten routes: item, repair, record-many,
cursor, begin, procedure, reset, archive, start and delegation. Both in-transaction
config invalidation and unsafe Git replacement preserve exact public details,
exit 5/nonretryable behavior, unchanged graph/binding/sequence, no failed-key
outcome and same-key success/replay without clock or ID access. BOUND-004 must
retain this per-site proof and the existing start identity-preparation,
changed-input replay, competing-fence and COMMIT ambiguity tests.

The chosen callback mechanism is one Store-owned generic runner with separate
Store and caller-error branches. Both identity and execution closures return
typed caller failures; current Store-only methods delegate to that runner and
flatten its Store-only branch. Start's source handoff between closures may keep
its legitimate ownership cell; error slots and preparation/refusal flags go.
Bootstrap remains direct. The three helper groups above remain distinct.

| CLI responsibility now | Concrete BOUND-005 destination / callers |
|---|---|
| `command.rs` Clap grammar and parse diagnostics | `command/grammar.rs`; shared entry/dispatch stays `command.rs` |
| Local authoring handlers and descriptor-safe Procedure file operations | Separate `command/authoring.rs` and `command/procedure_files.rs`; preserve atomic replacement, mode/no-op/symlink and cleanup behavior |
| Interactive start interviews and dry-run handling | `command/start.rs`; preserve non-TTY refusal and execution-first plain start |
| `command.rs` `StatusFacts`, `StatusPreflight::from_direct/from_parts`; `direct.rs` `needs_facts`, `fill_facts`, gated standard-status fetch/prepared rejection | One `command/preflight.rs` owner. `Command::needs_preflight` remains a coordinated grammar classification, also used by explicit flag validation. The gate consumes the original `Invocation` deadline and attributes errors to the requested mutation. |
| Rendering helpers | `command/render.rs`; existing `direct`, `completion` and `retirement` keep their existing roles |

Four status paths remain distinguishable: gated missing-fence preflight;
dynamic completion's separate best-effort 500 ms query; local fresh-start
dry-run versus stateful replacement/reset disposition checks; and the later
interactive read only after `SESSION_START_DECISION_REQUIRED`. READS replaces
only the first. Missing workspace UUID alone does not trigger a fetch;
stdin-owned record/delegation identities remain independent. Plain start executes
first, preserving exact replay, and any retry reuses the key/deadline. Preserve
retirement/late-JSON handling before stdin and current input/flag precedence.

BOUND registers the complete nine-case `tools/run_native_qualification.py::CASES`
matrix for BOUND-006: Store G1/G2 L/maximum, Engine G1/G2 at 300 and 1024 nodes,
busy deadlines, five-boundary SIGKILL, random SIGKILL, 100 G5 reset/remove cycles,
500 state plus 500 artifact G6 races, and ten-minute Store G9. Use the existing
native Make entry point on frozen source, AC-powered Apple Silicon, without
diagnostic overrides. Retain current blocking/informational thresholds; this
registers no OPTIM metric baseline or new comparator. Fresh-process supported
Store continuation, canonical assets/DDL/manifest identity and exact replay are
separate final acceptance checks.

### BOUND-002: Separate domain state and transitions from persistence

- Move pure graph/workflow/goal state and transition code into Core with explicit
  inputs, clocks and IDs supplied by callers. Move the minimum pure snapshot and
  declaration types required by those transitions in this same task; adapt the
  existing Store construction path and Engine consumers so the result is fully
  functional without waiting for BOUND-003. Keep the transaction model intact.
- Move pure tests beside the moved Core implementation as unit tests, preserving
  their behavioral coverage and historical traceability. Core `tests/int_suite.rs`
  currently contains only a comment, not executable domain proof. A new Core
  integration module must be explicitly registered there; Store relational
  round-trip/changed-row tests remain in Store `int_suite`.
- Split Store-specific errors and schema/SQL code before the move, and prove the
  snapshot-success/later-model-error sequence remains unchanged in this task.
- Acceptance: transitions can be exercised without SQLite or filesystem setup;
  stored states reconstruct and transition identically; no reverse production
  dependency or duplicated transition implementation remains.

### BOUND-003: Clarify Procedure runtime representation and admission

- Build on the pure snapshot/declaration types already delivered by BOUND-002.
  Align Config/Store adapters on that representation and consolidate remaining
  duplicated domain interpretation, while retaining independent source and
  persistence admission checks. Do not introduce a second model or retroactive
  prerequisite for BOUND-002.
- Reuse domain constructors/predicates without changing schema acceptance,
  error priority, canonical snapshots or materialized-default behavior.
- Acceptance: valid/invalid source and stored-document fixtures have the same
  results and error timing, including successful snapshot construction with a
  retained derived-model error later returned by workflow admission. Old snapshot
  digests remain stable; no Store-to-Config dependency or schema infrastructure
  in Core is introduced.

### BOUND-004: Unify typed mutation failure transport and shared helpers

- Implement one transaction callback failure path and migrate every affected
  callback inventoried in BOUND-001, including all paired transports in DF-023.
- Reuse fence and envelope helpers according to the three groups above, not a
  universal session fence or success envelope.
- Add or extend a named test for each of the nine sites, selecting an actual
  resolver/preparation/domain failure from that site's branches. Assert the
  original public code, details, retryability and mapped exit status, plus no
  outcome for the failed key and unchanged graph/binding rows as applicable.
  Existing tests may supply these assertions; shared-runner tests alone cannot.
- Acceptance: every inventoried operation is covered and no paired error
  transport remains there. Exact same-key replay bypasses the callback and clock;
  changed-input key reuse and competing fences retain their ordering. Test COMMIT
  I/O ambiguity separately from callback failure, using its existing classification.
  Store must not depend on Engine or gain a second transaction implementation.

### BOUND-005: Decompose CLI responsibilities without changing behavior

- Extract the modules described above. Move the fence-preflight fetch and parser
  together under one owner, with a caller inventory distinguishing completion,
  start dry-run and interactive-start retry. Keep all preflight behavior unchanged
  and hand off the named module to READS-004 through durable architecture.
- Acceptance: actual CLI tests cover help/completion, authoring and file safety,
  human/JSON output, stdin identity, prepared-state rejection, interactive start,
  non-TTY refusal and deadline consumption. A smaller file alone is not proof.

### BOUND-006: Qualify and close boundary refactoring

- Run the full development gate, applicable native crash/race/maintenance and
  Store/Engine scale checks through the
  [native qualification entry point](../implementation-tips/direct-cli-performance-qualification.md#native-scale-execution),
  and preserve accepted OPTIM request-scoped sharing and single-draft structure.
  The three ignored scale cases require that separate route; a successful
  `make test` is insufficient.
- Verify canonical assets, manifest inputs, DDL and stored replay envelopes did
  not change; prove fresh-process continuation of current Stores without reset.
- Acceptance: invariant/error mapping is complete, all applicable tests and
  frozen comparison criteria pass, and the final implementation matches the
  ownership table. Promote durable architecture/traceability before independent
  Epic review and exact owner acceptance. Performance gains are not required
  for a boundary-only move, but regressions cannot be hidden by file relocation.
  Preserve accepted OPTIM request-scoped sharing and single-draft structure.
  OPTIM supplies accepted implementation and functional evidence, no performance
  baseline, comparator, or count ceilings. Any BOUND-owned future comparison
  must be registered independently.

## 7. Verification and release acceptance

| Concern | Existing verification owner to retain or extend |
|---|---|
| Pure transitions | Unit tests beside moved Core code, following [existing session unit tests](../../crates/podway-core/src/session_v2.rs); migrate relevant pure cases from [workflow tests](../../crates/podway-store/tests/int_v2_workflow_memory.rs) and [goal tests](../../crates/podway-store/tests/int_v2_goal_state.rs). Core `int_suite` is not itself existing domain coverage. |
| Stored state and corruption | [relational tests](../../crates/podway-store/tests/int_v2dfc005_relational.rs), [persistence tests](../../crates/podway-store/tests/int_v2dfc008_persistence.rs) |
| Errors, domain rules and delegation | [error tests](../../crates/podway-engine/tests/int_v2dfc006_errors.rs), [domain tests](../../crates/podway-engine/tests/int_v2dfc006_domain_ports.rs), [delegation tests](../../crates/podway-engine/tests/int_v2dfc006_delegation_ports.rs) |
| Callback rollback/replay and races | [direct execution](../../crates/podway-engine/tests/int_v2dfc006_direct_execution.rs), [race tests](../../crates/podway-engine/tests/int_v2dfc006_races.rs), [crash tests](../../crates/podway-engine/tests/int_v2dfc006_crashes.rs) |
| CLI surfaces | [local CLI](../../crates/podway-cli/tests/e2e_v2dfc007_local_cli.rs), [direct CLI](../../crates/podway-cli/tests/e2e_v2dfc007_direct_cli.rs), [native conformance](../../crates/podway-cli/tests/e2e_v2dfc007_native_conformance.rs) |

Unit tests remain connected to their implementation modules. Moved integration
and end-to-end tests remain registered in `int_suite`/`e2e_suite`; an unregistered
file is invisible with `autotests = false`. Update durable traceability when an
owner changes. Retain explicit Store round trips after moving pure tests. For executable
changes, focused checks precede the required `make test`, under
[AGENTS.md](../../AGENTS.md). Keep measurement evidence private/ignored according
to the [comparison protocol](../implementation-tips/direct-cli-performance-qualification.md#internal-improvement-comparisons).

The [dossier lifecycle](README.md#lifecycle) governs promotion and removal after
acceptance. No stable release, source commit, push, installed runtime operation
or external consumer change is authorized by this planning dossier.

## 8. References

- [Rust codebase architecture](../architecture/rust-codebase.md)
- [Allowed production adjacency](../../contracts/cargo-adjacency.json)
- [Direct execution decision](../architecture-decision-records/0038-remove-the-daemon.md)
- [Transactions and idempotency](../specs/storage/transactions-concurrency-and-idempotency.md)
- [Current SQLite model](../specs/storage/sqlite-model.md)
- [Public errors](../specs/interfaces/errors-and-exit-codes.md)
- [Testing and conformance](../specs/quality/testing-and-conformance.md)
