# Podway Read Models and Data Access

## 1. Status and authority

- Status: Adopted
- Adoption date: 2026-10-14
- Owning epic: [READS](../roadmap/README.md#reads-read-models-and-data-access)
- Dependency: accepted BOUND completion, including inherited OPTIM improvements
- Review baseline: v0.2.14, `fa29dba929777b020377bc9803cb0a3954d219b1`
- Implementation baseline: accepted BOUND `036e86ed922a4e153acce02ae3e267b357a217a1`
  and its [delivery record](../roadmap/README.md#bound-delivery-record)

The owner selected preservation of both existing external/storage contracts and
existing read validation guarantees. This narrows the earlier read-optimization
proposal: reduced-cost data access is required, but lazy decoding that weakens
cold-read validation is not. No owner-level product decision remains open within
this scope. Status and execution order belong only to the
[roadmap](../roadmap/README.md#current-internal-improvements).

This dossier describes unfinished work, not a new implemented read contract.
Accepted ADRs, canonical assets and existing specifications take precedence.
A storage migration or changed validation policy requires separate authorization
and is not an automatic fallback if an optimization proves infeasible.

## 2. Verified context and evidence

| Finding | Current implementation | Scope implication |
|---|---|---|
| Full reconstruction serves current views, mutation preparation and archive-show | [Store read API](../../crates/podway-store/src/direct_store/connection.rs), `view`, `prepare_graph`; [graph loader](../../crates/podway-store/src/v2_state.rs), `load_present_graph_session_with_cache_v2`, `load_archived_session_connection_v2` | Preserve validation outside the displayed window; archive-list is a different, summary-only path |
| Slots, blockers and evidence are queried per attempt, with extra value point-queries for mismatched cached slots | [workflow loader](../../crates/podway-store/src/v2_memory.rs), `load_workflow_memory_v2`, `load_attempt_memory_v2`, `load_blockers_v2`, `load_evidence_v2` | Bulk cold acquisition and selective prepared reload need separate probes; preserve header-only reuse of unchanged payloads |
| Goal criteria are queried per revision and citations per criterion assessment | [goal loader](../../crates/podway-store/src/v2_goal.rs), `load_goal_revisions_v2`, `load_criterion_assessments_v2`, `load_citations_v2` | Include these loops in READS-002 rather than leaving a second source of history-dependent SQL |
| CLI obtains missing fences from a standard-status envelope whose projection/validation can also fail | [CLI preflight](../../crates/podway-cli/src/command/preflight.rs), `needs_facts`, `fill_facts`, `prepare`, `StatusPreflight::from_direct`; [implemented boundary](../architecture/system.md#podway-cli) | BOUND-005 extracted this fetch/parser together; READS-004 replaces that boundary, not every CLI status read |

Attempt, counter, decision and rework acquisition are already session-scoped
statements. Record their counts but do not claim they are per-attempt loops.
OPTIM's SQL-shape freeze ends with that epic; READS owns the query changes above.

The CLI already skips status preflight when the required explicit fences are
present. Stdin-owned record/delegation identities must not be replaced by an
automatic read. Store already reuses unchanged immutable values when validating
a request-owned prepared graph. Preserve both optimizations.

The [transaction specification](../specs/storage/transactions-concurrency-and-idempotency.md#request-owned-prepared-value-reuse)
requires normal cold reads and integrity checks to decode persistent values.
This epic does not replace that with a doctor-only policy. The measured share of
these costs remains unknown until the accepted comparison baseline is captured.

## 3. Goals, non-goals, and scope

Reduce repeated query preparation/execution, graph/container reconstruction and
internal JSON projection while preserving current validation coverage. Expose
small typed facts and read views through the existing Engine/Store boundary.

Do not promise constant-time cold reads independent of all retained data: the
preserved validation guarantee may require reading that data. Distinguish SQL
statement count, decoded row/byte count, allocated objects, public projection
cost, memory peak and invocation latency when reporting improvements.

No new Store table/index/column, schema or writer-generation change, lazy
corruption detection, persisted cache, new public read route, daemon, pooled
cross-process service, weakened read-only sidecar rule or new concurrency model
is included. Graph engineering and continuation remain unadopted feature work.

## 4. Accepted design decisions and interfaces

### Required read and validation scope

| Internal purpose | Data needed for the returned view | Validation boundary that remains |
|---|---|---|
| Mutation facts | Workspace/session/attempt identities, lifecycle and needed item/goal revisions | Current preflight failures through Store admission, status projection, schema validation and envelope validation |
| Start fence facts | Current-session inputs needed by the existing fence preflight | Preserve lifecycle ordering; separate reset dry-run/disposition and later interactive-start observations stay on their existing paths |
| Current execution | Active attempt, required evidence, goal/delegation readiness and existing summaries | Full required cold decode and coherent reference/freshness validation |
| Status history | Existing history window | Full current-session decode, including values outside the window |
| Archive-show | One archived session and requested history | Full selected-session decode, including values outside the window |
| Archive-list | Existing archive summaries | Summary-row validation only; do not add archived graph reconstruction |
| Doctor/maintenance | Existing explicit integrity and filesystem checks | Unchanged deeper scans, exclusion and recovery authority |

A smaller returned carrier is not permission to validate fewer persisted values.
The caller can avoid rendering the full status while Store still validates the
same state. Optional reductions in loaded fields are allowed only when they
preserve every required check and error meaning; otherwise retain eager loading.
Ordinary Store opens retain their current pragma/schema/binding/writer checks,
not newly added `quick_check`, `foreign_key_check`, full graph reconstruction or
all-receipt scans. Full graph decode remains at the existing graph-reading
commands, not at every open or summary-list command.

### Bulk row acquisition first

Replace the per-attempt slots/blockers/evidence, per-revision goal-criteria and
per-assessment citation loops with session-scoped acquisition using the existing
schema/indexes. Group by attempt/item and revision/criterion identity, retaining
reference ordinals and existing reconstruction order. Cold queries should be
bounded by the selected table set rather than the number of history entries.
Do not create an unbounded `IN` list, temporary write table or multi-table
cross-product join. Statement reuse alone does not remove repeated execution.

For stale prepared state, keep the initial header-only slot checks and reuse
unchanged immutable values only under their existing identity/type/revision/
presence stamps. Batch only the changed or newly seen payload reads in bounded
parameter groups instead of one point query per mismatched slot. Register the
chunk bound before measuring and report these counts separately from cold loads;
count may depend on bounded changed-slot chunks, not one execution per slot.
Do not obtain a lower statement count by rereading every unchanged payload.

Preserve detection of missing, duplicate, mismatched and invalid records,
unknown variants, timestamps, cross-references and illegal history. Do not
silently discard rows during grouping, and do not introduce unrelated validation
policy changes. Materializing rows must not advance an error ahead of its
current validation stage. Characterize multi-fault precedence before choosing
the decoding order.

Reuse decoded immutable payloads and borrowed/shared containers rather than
retaining both a complete raw payload copy and a second full aggregate. Any
staging required for grouping must obey the existing per-row/collection bounds
and the frozen memory comparison. In test/test-support builds, count both
prepare and execute at the real production connection boundary, excluding
fixture setup. Observers must not add, drop, reorder or rewrite SQL/results/errors,
or replace SQLite with a test double. Keep observers out of the release CLI
used for timing; follow the shared instrumentation protocol. Required
shared-payload/copied-container evidence is separate from optional peak RSS.

### Typed views over validated state

Store may expose narrowly scoped read carriers built only after the required
admission and stored-state checks. Their fields and constructors prevent callers
from manufacturing a validated-state witness. Engine derives the public result
or internal application facts from that state.

Use borrowing or existing immutable sharing so a current-state view, an archive
view and a history window do not each clone the full graph. Keep one committed
snapshot per read; do not query session identity on one connection and evidence
or counters on another. A private query-purpose enum or narrow methods are
sufficient. Do not add a generic query framework or a second persistence model.

Keep the legacy eager path until equivalence is established, then remove unused
implementations. A final fallback may reuse the same eager validation core for
checks that cannot be narrowed; it must not maintain two competing validators.
Reuse OPTIM's projection context and BOUND's domain/persistence ownership.

### Engine-owned typed preflight

Add an internal typed facts operation to the shared Direct Engine facade rather
than making CLI depend on Store. The CLI requests the facts its existing
`needs_facts` classification requires, then constructs the same mutation with
the original caller-supplied key and fences. The replacement boundary is the
fence-preflight module delivered by BOUND-005: `needs_facts`, `fill_facts`, the
gated standard-status fetch, and `StatusPreflight::from_direct` under one owner.
Do not replace dynamic-completion status, start/reset dry-run/disposition handling
or the later status read after `SESSION_START_DECISION_REQUIRED`. Shared parser
helpers can continue serving those consumers without changing their semantics.

The facts operation returns typed Engine facts/errors, not a public standard
status envelope. The CLI must not call `session.status` and decode JSON as an
intermediate implementation of that operation. Reuse existing public error
rendering to attribute failure to the requested command, with the same code,
details, retryability and exit status. No public command/schema is added.

For every affected mutation, preserve failures that currently arise only while
projecting, schema-validating or envelope-validating standard status, not merely
failures of Store or facts acquisition. READS-001 inventories those conditions;
READS-004 tests the reachable failure cases and proves why any removed check is
redundant for the admitted state. An internal helper fault injected solely into
the old implementation is not evidence of a user-visible input condition.
Equivalent typed/bounded checks may replace JSON construction, but failures must
retain their existing priority and public mapping. If equivalence cannot be
established without constructing the public envelope, stop that replacement and
return for authorization; do not silently weaken the contract or declare the
no-envelope task complete with the old status path left inside it.

Preserve explicit-fence bypass, stdin-owned record/delegation fences, missing-item
behavior, prepared lifecycle rejection and start policy decisions. Preserve the
existing order of validation, key requirements, prompts and destructive
confirmation. Do not merge or reorder start/reset dry-run observations merely
to save a query if that changes its error or concurrency behavior.

All reads consume the original invocation wait deadline. Facts reserve neither
a revision nor a lock across a human prompt. Mutation admission must revalidate
current identity/revisions under the existing transaction even after a successful
facts read. A concurrent edit between facts and mutation must still be rejected
or accepted only under the existing command-specific fence policy.

Prove the gap with an in-process Engine test: a test-only barrier pauses after
the facts function returns successfully, once its read transaction, process-local
connection gate and actual `DirectOpenLease` have all been released, but before
mutation admission. Return owned facts or immutable data that holds none of those
guards; do not leave the barrier inside the Store read callback.

Ordinary reads acquire `DirectOpenLease::Shared(SharedMaintenanceLeaseV1)`;
the existing cold path can instead acquire
`DirectOpenLease::Cold(ExclusiveMaintenanceLeaseV1)` when required WAL/SHM sidecars
are absent. Preserve both paths in [Store read admission](../../crates/podway-store/src/direct_store/connection.rs)
and [lease acquisition](../../crates/podway-store/src/direct_store/maintenance.rs).
The barrier itself acquires, retains or waits on no additional maintenance lease;
it neither replaces nor bypasses the lease previously used by the facts read.

A second real writer must finish committing a change invalidating the relevant
fence while the first caller is paused. Only then resume with the original fences
and assert the existing conflict code/details, no outcome for the rejected key
and no effect beyond the competing writer's commit. Use an explicit rendezvous,
not sleeps or a test that holds a read guard and prevents that competing commit.
The barrier is absent from the release CLI. CLI integration tests separately
prove the extracted module is the one the real command path uses.

For `record --stdin` and delegation acceptance, extend the existing CLI cases
rather than duplicate their stale-fence assertions. Compose the body first, make
a competing change, then send the unchanged body with a fresh key and verify the
original document fences govern rejection. A separate successful/replay case
retains exact outcome behavior. In an explicitly instrumented test executable,
count status-envelope/facts-preflight activity for that command process only and
require zero; setup status calls in the parent fixture are outside the counter.
Preserving fences alone does not prove absence of a redundant status read.

## 5. Failure handling and compatibility boundaries

Apply the [shared preservation rules](../roadmap/README.md#preservation-rules).
Normal reads remain write-free, including their existing WAL/SHM and maintenance
lease behavior. Leave the process-local connection gate unchanged. Do not add
cache files, write summaries during reads or treat a missing sidecar as permission
to ignore committed WAL state.

Demonstrate unchanged cold-read refusal on invalid values in active and historical
attempts for the commands that decode those graphs. The existing relational
counter/digest/slot-deletion/orphan test is not the hidden-value oracle. Add a
fixture that changes only `value_json` of a stale historical attempt. After fresh
uncached opens, compact status and a history window omitting that value must
return the baseline cold-read error without effects. Prepare a valid archived
fixture before injecting the same fault and assert archive-show refusal too;
do not try to archive an already corrupt current session to manufacture it.

Use that same post-injection archived fixture for both archive-list and
archive-show, through fresh uncached reads. Choose a list window containing the
corrupted session and assert that its exact session ID and unchanged valid
summary are returned successfully despite its corrupted `value_json`.
Archive-show of that same session must still refuse with the baseline cold-read
error. Success on another healthy archive, or a page omitting the corrupted
session, does not satisfy the summary-only proof. Keep a separate fully healthy
archive/list case as a control, not a replacement for the corrupted-fixture pair.

Assert DB/WAL/state/outcome preservation and the existing no-sidecar rules after
each read against the same post-injection preimage, without assuming pre-existing
volatile SHM bytes are immutable. Do not repair the payload, change the fixture
or discard the affected summary between the list and show assertions. Keep
doctor's orphan/integrity scans separate and retain unsupported-generation/
foreign-writer, Busy, I/O and actual corruption distinctions.

No changes to canonical public assets, DDL, stored request/outcome bytes,
Procedure identity or writer manifest inputs are authorized. Existing current
Stores must continue without reset or migration. Non-UTF-8 internal paths,
lossless identities, all bounds and exact replay remain unchanged.

## 6. Roadmap ownership, dependencies, and traceability

The [READS roadmap](../roadmap/README.md#reads-read-models-and-data-access) owns
order and status. Each task includes focused regression tests and updates its
changed source/test ownership references.

### READS-001: Freeze data requirements and validation equivalence

- Use the accepted BOUND candidate as the comparison baseline. Map every affected
  read/preflight to queries, values decoded, validation stages and returned facts.
- Capture command-specific error precedence for hidden historical-value
  corruption, cold committed WAL, explicit/stdin fences, and status-projection,
  schema and envelope failures. Distinguish graph reads from archive-list and
  ordinary opens before specifying load reductions. Use BOUND's actual preflight
  module/caller inventory rather than recreating it from function names.
- Freeze SQL-count, row/byte, memory and latency comparisons using the
  [internal comparison protocol](../implementation-tips/direct-cli-performance-qualification.md#internal-improvement-comparisons).
- Acceptance: every proposed omitted load has an equivalent validation proof or
  is excluded; normal cold-read validation has not been reassigned to doctor.
  The five-table query families, selective cache-miss reload, goal/revision and
  citation histories, typed-facts probes and negative fixtures are registered
  before optimization. Register the paired list-success/show-refusal oracle for
  the same corrupted archived session, including its visibility in the list
  window, and the post-facts release of transaction/gate/DirectOpenLease before
  the competing commit. OPTIM recorded no performance baseline or comparator
  for READS to extend. Do not present later measurements as an OPTIM baseline.

READS-001 registration record (2026-10-17, instrumented unchanged-query
candidate at `052f8625`):

- Scoped observers and baseline live in `podway-store` `query_accounting`
  (`test-support` feature) with the registered call-site scope, uncovered
  reads, units and actual per-recipe numbers in the
  [READS registered baseline](../implementation-tips/direct-cli-performance-qualification.md#reads-registered-unchanged-query-baseline).
  Store characterization: `int_reads001_query_inventory` (S/M/grown-L/1,024
  recipes, empty/single controls, paired observed/unobserved neutrality,
  unchanged/sparse/dense prepared reloads with pointer-identity reuse proofs,
  failed-decode release accounting) and `int_reads001_negative_boundaries`
  (stale `value_json` refusal with byte-identical preimage and no sidecar,
  exact typed re-encoding rejection of noncanonical-but-valid values, the
  same-corrupted-archive list/summary success versus graph refusal with a
  healthy control, multi-fault precedence pinned by real probe order through
  the scoped observers because the public error is identical).
- Projected-command oracles: `podway-engine` `int_reads001_readback_ports`
  (compact and window-omitting standard status refuse with
  `WORKSPACE_STATE_UNREADABLE`; archive-list of the same corrupted fixture
  returns the exact session id and unchanged summary while archive-show
  refuses; the four-way ordering current-corruption → archived-corruption →
  `SESSION_ARCHIVE_NOT_FOUND` → `SESSION_ID_MISMATCH` is pinned as the
  unchanged implementation behaves; healthy archive control decodes).
- CLI preflight boundary: `podway-cli` preflight parser-surface unit tests
  (`reads001_parser_surface_tests`) characterize the crafted-map rejection
  surface separately from input-reachable failures; the input-reachable
  unreadable mapping through `preflight_error` and the Busy/deadline
  attribution reuse the existing
  `e2e_v2dfc007_direct_cli` cases (`v2dfc015_invalid_current_stores_require_unfenced_confirmed_recovery`,
  the SQLite-writer contention and maintenance-deadline cases). `Command::Record`'s
  structural exclusion from `needs_facts` remains the recorded basis for the
  future zero-preflight counters.
- Registration-only for READS-004 (no implementation here): process-local
  zero status/facts-preflight counters for `record --stdin` and delegation
  acceptance, and the test-only post-facts barrier that proves release of the
  read transaction, process-local connection gate and actual
  shared/cold-exclusive `DirectOpenLease` before a confirmed second-writer
  commit rejects the original fences.
- Proof obligations: resolved rows above. Established gate facts from final
  source review: the first byte/schema gate on standard status is the
  projection validator (depth plus 1 MiB result bound), which maps to
  `WORKSPACE_STATE_UNREADABLE` (exit 5, non-retryable); the envelope
  `Protocol` gate maps to `REQUEST_INVALID` (exit 2, non-retryable) and can
  fire only in the narrow band where a result fits the bound while the
  enveloped output does not. Evidence-reference integrity is validated at
  full graph admission — Core runs `validate_attempt_evidence_v2` on every
  attempt, rejecting missing source trace/memory and mismatched
  node/number/trace-order/lifecycle/resolved-at/digest before the graph is
  constructed — and the evidence family persists in the relational
  `v2_resolved_evidence_references` table, so missing-source corruption
  classifies as a Store admission failure (`WORKSPACE_STATE_UNREADABLE`),
  not a projection failure. Open rows carried to READS-004 as equivalence
  obligations: extreme persisted `UnixMillis` values, schema-size arithmetic
  for the standard-status path (a drift guard built on exact field overhead
  or a normal-ASCII identifier invariant is the form such a proof would
  take), and the selected-readback fallback's precise invariant trace if it
  is excluded. None of these rows is claimed proven unreachable here.
- Non-scope inventory (unchanged by READS): `WorkspaceShow`, doctor deep
  scans, outcome lookup and the non-preflight CLI status consumers retain
  their existing paths; omissions from the scoped counters are not
  optimization targets.

Read and preflight inventory (unchanged implementation, from
`execute_read_v1` and the BOUND-005 preflight module). Every read variant
first resolves the workspace, admits the Store and eagerly builds the full
current-session view (complete cold decode including values outside any
window); per-variant data and validation beyond that boundary:

| Variant | Additional data | Validation/returned facts beyond the eager current view |
|---|---|---|
| `session.status` | none | Projects the requested tier/history window from the fully decoded state; schema-validated `podway.status-result/v5` family output |
| `session.next` | none | Projects next-step readiness from decoded state |
| `session.observe` | none | Projects the observation document from decoded state |
| `evidence.read` | token binding check | Evidence page from decoded resolutions; page-token session binding; optional consumer-attempt fence |
| `session.archive_list` | inactive summary rows only | Summary-row validation only; no archived graph reconstruction; returns exact ids, unchanged summaries, count/truncation/oldest |
| `session.archive_show` | archived graph + archived summary | Full selected-session decode including values outside the window; summary joins the projected tier/history output |
| `delegation.receipt` | one receipt row (current or archived view) | Receipt decode and identity/workspace match against its graph |
| `delegation.package` | none beyond the eager view | Projects the delegation package from decoded state |

The single CLI preflight read is `session.status` at Standard tier with
`history_before: None` through `invocation.execute` in `preflight::prepare`;
its reachable failures arrive as engine `Failure` envelopes mapped by
`preflight_error` (corrupt state, Busy/maintenance/deadline), followed by
the crafted-map-only parser surface recorded above, then the
prepared-lifecycle rejection ordering (key validation, automation
`--idempotency-key` requirement, revision conflict, `SESSION_NOT_RUNNING`).

### READS-002: Bulk-load attempt data without reducing validation

- Batch slots, blockers, evidence references, goal criteria and criterion citations
  by session and preserve ordered reconstruction/corruption classification.
  Replace mismatched-slot point reloads with bounded changed-slot batches while
  retaining header-only reuse of unchanged payloads.
- Acceptance: cold selected-table statement counts are bounded by table families,
  not attempts/revisions/assessments. On preregistered fixtures with repeated
  baseline queries, execution counts are strictly lower than accepted BOUND;
  small/empty/single-entry controls require no unnecessary count increase, not an
  impossible strict reduction. Assert prepare counts separately. Cover grown L,
  1,024 nodes, repeated goal revisions/assessments and sparse/dense cache misses;
  report node, attempt, revision, assessment and changed-slot dimensions.
- Real Store tests must preserve cold relational history, all item variants,
  goals, rework, delegation, invalid rows and multi-fault precedence. Avoid a
  memory regression from keeping raw and decoded payload copies together.

### READS-003: Build purpose-specific views over validated state

- Introduce narrow typed current/history/archive/facts views using the shared
  validation core and request-scoped projection context, with eager decode where
  required. Eliminate unnecessary owned whole-graph projection copies.
- Acceptance: outputs, freshness, exact totals, ordering, page tokens and
  truncation match the baseline; the new stale-attempt `value_json` oracle fails
  at compact status/history/archive-show. Archive-list of the same corrupted
  archived fixture must successfully include that session's unchanged summary,
  while archive-show of that session refuses. A separate healthy-summary control
  or an empty list does not establish this boundary. Required cold decode stays
  eager. Reduced query scope is optional where it
  would weaken the guarantee. Remove any remaining duplicate construction found
  at READS-001; do not invent extra copies to claim reductions already delivered
  by OPTIM. Views must use one shared validation core, not competing validators.

### READS-004: Replace CLI status-JSON preflight with typed facts

- Add the Engine facts facade and replace only BOUND-005's fence-preflight
  fetch/parser boundary, without exposing Store. Leave completion, start dry-run
  and later interactive-start status paths unchanged.
- Acceptance: that automatic fence preflight no longer builds/parses a public
  status envelope, while all original Store/projection/schema/envelope failure
  conditions retain code, details, retryability, mapped exit and priority.
  Explicit/stdin-fenced paths still bypass it. Prompts, deadline, start policies,
  missing-item and prepared-state behavior remain unchanged.
- Implement the test-only Engine barrier after releasing the facts transaction,
  connection gate and actual shared/cold-exclusive DirectOpenLease. Prove the
  second writer commits before resuming the original-fence mutation, as described
  above. Add process-local zero-preflight assertions and original-fence races
  for both stdin commands. Keep separate native CLI behavior tests against the
  uninstrumented candidate; neither a sleep nor only a helper unit test suffices.

### READS-005: Qualify and close validated data-access improvements

- Run full development, applicable native read/write/maintenance/crash checks,
  Store/Engine L and 1,024-node scale checks and matched current-Direct G7/G8
  comparisons against BOUND. Execute the
  [native qualification route](../implementation-tips/direct-cli-performance-qualification.md#native-scale-execution)
  for the three ignored scale cases; `make test` does not execute them. Include
  explicit versus automatic-fence diagnostic cells because G7's explicit fences
  alone cannot prove preflight gains.
- Acceptance: original thresholds, matched latency/memory criteria and
  deterministic SQL/facts count rules all pass independently. Repeated-query
  fixtures must improve; small controls need not improve an already minimal
  count. No cold-read, status-failure, archive-list, storage-contract or
  side-effect guarantee changes.
- Promote actual design, query requirements and measurement summaries to durable
  authorities. Complete independent Epic audit/review and owner acceptance before
  marking completion or retiring the dossier. No release is implied.

## 7. Verification and release acceptance

| Concern | Existing verification owner to retain or extend |
|---|---|
| Cold reconstruction and corruption | [relational tests](../../crates/podway-store/tests/int_v2dfc005_relational.rs), including `every_item_type_retry_and_manual_rework_preserve_cold_relational_history` and `corrupted_graph_rows_fail_closed_and_doctor_detects_an_orphan_snapshot` |
| Hidden historical payload and summary-only listing | New stale-attempt `value_json` corruption fixtures in Store relational tests with compact/history/archive-show assertions in Engine readback/domain-port tests; on the same corrupted archived fixture, archive-list must include the affected session's unchanged summary while archive-show refuses. Keep a separate healthy control. |
| Coherent read and no-sidecar behavior | [direct Store](../../crates/podway-store/tests/int_v2dfc005_direct_store.rs), [persistence](../../crates/podway-store/tests/int_v2dfc008_persistence.rs), [Engine readback](../../crates/podway-engine/tests/int_v2dfc006_readback_ports.rs) |
| Prepared-value reuse and contention | [Store scale](../../crates/podway-store/tests/int_v2dfc005_scale.rs), [Engine races](../../crates/podway-engine/tests/int_v2dfc006_races.rs) |
| Evidence, guards and delegation | [reference tests](../../crates/podway-engine/tests/int_v2dfc006_reference_ports.rs), [guard tests](../../crates/podway-engine/tests/int_v2dfc006_guard_ports.rs), [delegation tests](../../crates/podway-engine/tests/int_v2dfc006_delegation_ports.rs) |
| Native preflight and deadlines | [direct CLI tests](../../crates/podway-cli/tests/e2e_v2dfc007_direct_cli.rs), extending `record_stdin_preserves_identity_applies_one_atomic_batch_and_replays_without_preflight` and `delegation_stdin_acceptance_receipt_and_completion_preserve_the_real_attempt` with actual process-local observations, not reliance on test names |
| Facts-to-mutation gap and status-only failures | New test-only post-facts/pre-admission barrier in [Engine race tests](../../crates/podway-engine/tests/int_v2dfc006_races.rs), after transaction/gate/DirectOpenLease release and before a confirmed competing commit; baseline/replacement failure cases in [Engine error tests](../../crates/podway-engine/tests/int_v2dfc006_errors.rs) and CLI integration |

Use test-only query/allocation observers, registered aggregate suites and
[AGENTS.md](../../AGENTS.md) gates. Source regexes and mocks alone cannot establish
cold-read validation, rollback or sidecar safety. Retain raw evidence in ignored
owned locations; put only source-bound summaries in durable documentation.

Follow the [dossier lifecycle](README.md#lifecycle) after exact acceptance.
Release, installation, migration and external-consumer adoption remain separately
authorized. No feature candidate becomes adopted through READS completion.

## 8. References

- [System architecture](../architecture/system.md)
- [Transaction and prepared-value reuse contract](../specs/storage/transactions-concurrency-and-idempotency.md)
- [SQLite model](../specs/storage/sqlite-model.md)
- [Recovery and maintenance](../specs/storage/recovery-retention-and-maintenance.md)
- [CLI contract](../specs/interfaces/cli-specification.md)
- [Automation contract](../specs/interfaces/automation-client-contract.md)
- [Testing and conformance](../specs/quality/testing-and-conformance.md)
