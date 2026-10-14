# Podway Execution Path Optimization

## 1. Status and authority

- Status: Adopted
- Adoption date: 2026-10-14
- Owning epic: [OPTIM](../roadmap/README.md#optim-execution-path-optimization)
- Review baseline: v0.2.14, `fa29dba929777b020377bc9803cb0a3954d219b1`
- Delivery: internal improvements, not a release program or runtime activation

The owner approved three sequential epics under the
[internal-improvement scope](../roadmap/README.md#current-internal-improvements).
This dossier owns the unfinished OPTIM design, not task status or implemented
product behavior. Accepted ADRs, canonical contracts and specifications retain
precedence. No owner-level product decision remains open within this scope.
An incompatible change must return for separate authorization rather than expand
this epic. Registration does not mean its baseline measurement or tests ran.

## 2. Verified context and evidence

Source inspection at the review baseline established these structural costs;
it did not measure their contribution to invocation latency:

| Finding | Source owner and entry point | Consequence |
|---|---|---|
| Running observation rehydrates and derives current state, then next repeats both; prepared observation and prepared next repeat rehydration only | [Engine projections](../../crates/podway-engine/src/v2_read_service.rs), `project_graph_observation_v1`, `project_graph_next_v2`, `project_prepared_next_v1`, `rehydrate_snapshot` | Completed/cancelled observation does not call next; `CurrentProjection::derive` returns no active projection outside running lifecycle |
| `projection_view` clones the owned graph view; direct status/next/observation wrappers independently reselect schemas | [Direct projections](../../crates/podway-engine/src/direct_read_service.rs), `projection_view`, `project_direct_status_v1`, `project_direct_next_v1`, `project_direct_observation_v1` | Archive-show constructs the owned view twice through its nested status; `validate_projection` also clones the result object for schema validation |
| Delegation work-package construction validates a rehydrated Procedure again | [Package projection](../../crates/podway-engine/src/v2_read_service.rs), `delegation_work_package_from_view_v1` | Rehydration discards the validated wrapper and returns parsed data, forcing a second validation for the package builder |
| A batch repeatedly reconstructs `WorkflowMemoryStateV2`, not the outer graph successor | [Workflow memory](../../crates/podway-store/src/v2_memory.rs), `mutate_active_items_v2`, `mutate_active_item_v2`; [graph wrapper](../../crates/podway-store/src/v2_state.rs), `GraphSessionStateV2::mutate_active_items_v2` | The graph wrapper already applies the batch once; the inner memory path clones history containers per operation, including no-op paths |

Recorded payloads already use immutable shared values, and the snapshot already
retains its parsed memory model. This is not a claim that each batch operation
copies all payload bytes or reparses that memory model. Preserve those existing
optimizations. [DF-007](../deferred-feedback/README.md#resolved) is promoted to
`OPTIM-002`; promotion does not claim implementation.

## 3. Goals, non-goals, and scope

Remove repeated computation and graph-container construction inside one command.
Keep the existing aggregate load, SQL query shape, crate ownership and public
API behavior while doing so. OPTIM must finish without a dependency on BOUND or
READS. Downstream work consumes its accepted implementation and measurements.
For OPTIM, preserve the data acquisition and validation behavior of
`load_graph_session_connection_v2`, `load_present_graph_session_with_cache_v2`,
`load_workflow_memory_v2`, and `use_prepared_graph`. Do not change their SQL or
loaded-value scope as part of projection/batch optimization. This is an OPTIM
scope boundary, not a permanent SQL-shape rule: READS owns later query changes.

No daemon, cross-invocation cache, new crate, new user command, public schema
change, Store generation/index change, threshold relaxation, or increased data
limit is included. Optimizing artifact-bearing replay before transaction-time
outcome lookup is not part of this epic; DF-026 remains independent work.

## 4. Accepted design decisions and interfaces

### Request-scoped projection context

Use a borrowed context over the existing coherent Store view. Retain the existing
Config-owned `ValidatedProcedureV2`, not only a cloned `ParsedProcedureV2`, for
each distinct immutable snapshot actually needed by the read. Consumers borrow
its parsed representation or its validated wrapper as appropriate. Rehydrate and
semantically validate it at most once at the same admission point as today;
sharing must not force earlier validation on branches that previously bypassed it.

The complete inventory of Direct projection entries covered by this task is:

| Direct entry in `direct_read_service.rs` | Inner consumer in `v2_read_service.rs` |
|---|---|
| `project_direct_status_v1` | `project_graph_status_v2` |
| `project_direct_next_v1` | `project_graph_next_v2`, `project_prepared_next_v1` |
| `project_direct_observation_v1` | `project_graph_observation_v1` and its nested next |
| `project_direct_archive_show_v1` | status plus `project_graph_archive_show_result_v2` |
| `project_direct_evidence_v1` | `project_evidence_read_v1` on branches requiring Procedure declarations |
| `project_direct_receipt_v1` | `project_delegation_receipt_read_result_v1` |
| `project_direct_delegation_package_v1` | `delegation_work_package_from_view_v1`, `project_delegation_package_result_v1` |

All nested consumers reuse the same context rather than another top-level read
that rehydrates it again. The package builder consumes the retained validated
wrapper and does not call `validate_procedure_v2` a second time. This also covers
standalone status, evidence and receipt entry points, not only observation/next.

Construction may use a two-phase scoped owner/borrow pattern; do not introduce a
self-referential Rust structure or global cache. The relevant identity is the
same read snapshot, session, Procedure snapshot identifier and stored digest.
Reuse neither current state nor parsed declarations across unrelated snapshots.
Do not attach Config's parsed/validated Procedure to Store's
`ProcedureSnapshotV2` or session cache. Store already retains a distinct
`SnapshotMemoryModelV2`, and `from_snapshot` returns that model without reparsing.

#### Procedure identity sources

Retaining `ValidatedProcedureV2` shares computation, not identity authority.
Revalidation can materialize defaults, so the wrapper's canonical bytes/digest
can differ from the immutable stored snapshot. Preserve each boundary below;
do not rewrite one identity to make the other match.

| Boundary | Preserved identity source and check |
|---|---|
| Status, next, observation and archive Procedure identity | Keep `state.snapshot().digest()` and the original stored canonical bytes as the snapshot identity; do not substitute the wrapper digest or rewrite stored bytes. |
| Delegation work-package construction | `project_delegation_work_package_v1` continues to use `ValidatedProcedureV2::digest()` for its `procedure_digest`; retaining the wrapper removes the second semantic validation, not this source. |
| Delegation package result admission | `project_delegation_package_result_v1` still compares that package `procedure_digest` with `state.snapshot().digest()` and refuses mismatch with its existing error. Keep its workspace/session/attempt/placement checks and package digest semantics too. A constructed package is not yet an admitted public response. |
| Execution rehydration | `rehydrate_procedure_v2_execution_v1` keeps rejecting when the wrapper digest or canonical bytes differ from the stored snapshot. This execution function is outside the seven Direct read entries and is not replaced by the read context. |

The read context does not import the execution mismatch check into reads that
currently accept historical snapshots. Conversely, it cannot remove the existing
package-result or execution mismatch checks to make those paths accept more
states. The sources are visible in [Engine reads](../../crates/podway-engine/src/v2_read_service.rs),
[package construction](../../crates/podway-config/src/procedure_v2_delegation_package.rs)
and [execution rehydration](../../crates/podway-engine/src/execution.rs).

Keep existing top-level projection signatures where a small delegating adapter
preserves callers. Replace `projection_view` ownership conversion with borrowing
or explicit shared ownership only where it removes a measured container copy.
Give the wrappers' schema selection one owner separately from the owned-view
conversion. Reduce the clone in `validate_projection` only through equivalent
borrowing/ownership, not by omitting schema validation. Retain canonical
projection/output validation at the established trust boundaries. This work does
not remove the protocol's lazy schema compilation or response admission checks.

The same context must cover running, prepared and archived nested projections.
Terminal observation still has its existing null guidance. Preserve all counter
copies, ordering, exact totals, truncation flags, byte budgets, page-token
identity, evidence freshness and delegation readiness. Reusing data does not
permit dropping response members to obtain a faster result.

### Batch-specific draft

Extract a single-item primitive that validates and updates an unpublished active
attempt draft. Single-item and batch paths share the value-mutation rules, not a
loop around an owned `WorkflowMemoryStateV2` successor constructor. The outer
`GraphSessionStateV2` batch wrapper already builds one graph successor; changing
only that wrapper does not satisfy this task. Measure the memory-successor
ceiling inside `WorkflowMemoryStateV2::mutate_active_items_v2`, including its
nested item calls and clones, not at the outer graph wrapper. Track Store cache
copies and whole-invocation container costs separately so their constant copies
are not mislabeled as per-item successors or hidden from resource evidence.

The draft is a pure in-memory value, with no connection, transaction or Store
error embedded in it. Keep its validation logic separable from existing error
mapping so BOUND-002 can move it with the memory model. OPTIM need not redesign
the existing public/internal error families before that move.

A batch must retain this sequence:

1. Apply existing input bounds, item-ID ordering, duplicate detection and fences.
2. Create one unpublished draft for the affected attempt and retain shared
   immutable payloads/history. Generate no externally visible partial state.
3. Apply each sorted operation with its original item revision, value rules,
   timestamp rules and content-bound checks at the same point as today.
4. Construct and validate one final workflow/graph successor, then use the
   existing changed-row persistence and atomic success outcome boundary.

Do not change intermediate content-limit failures into a final-state-only check.
An earlier sorted operation can exceed the bound even when a later one would
reduce the final content. Preserve that rejection and its priority. Preserve
changed/no-op results, per-item revision increments, the single session/workspace
revision effect of a changed batch, and successful no-op outcome semantics.
On any failure, discard the draft and retain no partial state or success outcome.

The write-path content-limit regression requires a preloaded attempt. Seed it
through multiple supported requests, each within the 1 MiB input cap and every
item bound, until it is within one legal list addition of
`MAX_ATTEMPT_CONTENT_SCALARS_V2`. Submit a small batch whose lexicographically
earlier item exceeds that scalar ceiling, while its later item would reduce the
final total back below it. Assert the existing content-limit code/details, no
changes to either item or graph revisions, and no outcome for the rejected key.
Compare against the seeded preimage, not an empty attempt. Permute input order
and retain the same sorted-item failure. This is separate from the 1/16/128
operation wire-bound matrix; one empty-state request cannot reach the cumulative
ceiling under the input cap.

## 5. Failure handling and compatibility boundaries

Apply the [shared preservation rules](../roadmap/README.md#preservation-rules).
Original-key lookup, changed-request key rejection, stale session/attempt/item
fences, validation order, clock/ID ordering and rollback behavior must not move
merely to simplify code. Artifact preparation stays outside the writer and its
stamps remain revalidated inside it.

The replacement does not authorize migration or reset of existing current
Stores. Preserve public canonical assets, manifest inputs, DDL, Procedure digests
and stored envelope bytes. Version/build provenance can differ between candidate
binaries; compare behavior with deterministic clocks/IDs, not by hiding field
changes. Exact stored-outcome replay must remain byte-identical without
normalization. Never replace the frozen comparison executable during a run.

## 6. Roadmap ownership, dependencies, and traceability

The [roadmap](../roadmap/README.md#optim-execution-path-optimization) is the sole
source for order and status. Each task includes its own focused tests; the final
task composes, rather than substitutes for, those checks.

### OPTIM-001: Freeze the current Direct baseline and regression checks

- Inventory all affected read compositions and batch paths, existing tests,
  fixtures and already shared payload/model allocations. Record the exact source
  revision, diff and native release-profile executable identities.
- Reuse the [comparison protocol](../implementation-tips/direct-cli-performance-qualification.md#internal-improvement-comparisons)
  and its closed mandatory matrix: 12 G7 cells, four L G8 cells, and OPTIM's
  separate projection/batch work-count and required memory probes. SQL/facts
  probes belong to READS-001, not this task. Register matched host/OS/power,
  exact fixtures and an odd block count of at least three before measurement.
- Extend existing qualification tooling with a small current-Direct report
  aggregator/comparator and self-tests. The current `qualify_direct_cli.py`
  compares one block to historical daemon numbers; it does not yet implement
  this new multi-block baseline or tolerance gate. Missing/mismatched reports,
  count misses, noisy baselines and candidate regressions must be rejected by
  the comparator rather than judged from a prose summary.
- Add test/test-support-only parser, semantic-validator, current-derive,
  workflow-successor and history-container probes on the real paths, plus
  characterization of no-op and failure ordering. Do not instrument the release
  CLI timed by G7/G8 or add product flags/environment switches. Keep unit tests
  beside code and integration tests registered in aggregate suites.
- Before retaining the validated wrapper, characterize both equal-identity and
  admitted historical/default-materialization fixtures whose revalidated bytes
  and digest actually differ from the stored snapshot. Assert that precondition,
  rather than forge a stored digest or only test equal identities. Record the
  existing read results/errors, package construction versus result admission,
  and execution-consumer refusal at their respective boundaries. Freeze the
  route/lifecycle-specific derive ceilings in the shared comparison protocol;
  a guarded call returning `None` is not a zero-call observation.
- Acceptance: every mandatory latency cell has valid matched blocks and frozen
  B/T, deterministic probes capture the real pre-optimization counts, and
  required shared-payload/container memory evidence is present. No-op graphs
  cannot substitute for memory-layer counts. Historical Direct columns, V2DFC
  verdicts and the review commit are not these measurement records. Unknown or
  noisy evidence remains incomplete; the current-Direct comparator passes its
  positive, boundary and rejection self-tests before OPTIM-002 begins.

### OPTIM-002: Share validated read projections

- Implement the request-scoped validated context across every entry in the
  inventory, including archive-show and prepared observation from DF-007.
- Extend view/readback/maximum-response tests and the actual archive-show owners
  in `int_v2dfc006_direct_execution.rs` and `int_v2dfc006_domain_ports.rs`.
  Add package/receipt/evidence probes; use real parser, semantic-validator and
  derive counters in test-only builds, not assertions about source text.
- Acceptance: each distinct snapshot that needs rehydration is parsed and
  semantically validated at most once per composed read, including package
  construction. Enforce the [route-specific work-count ceilings](../implementation-tips/direct-cli-performance-qualification.md#projection-work-count-ceilings):
  prepared/completed/cancelled observation may call the guarded derive path once
  and must produce no active projection; prepared next, evidence and receipt
  keep a zero derive ceiling. A second discovering call is a miss. Rehydration
  and semantic validation have separate counters; declaration-bypassing branches
  remain zero for those counters, regardless of derive behavior. Terminal
  guidance remains null. Other routes follow their own table rows, not a blanket
  allowance of one derive for every non-running read.
- Identity acceptance uses the OPTIM-001 characterization: successful ordinary
  read identities remain stored identities, package construction retains the
  wrapper digest, and package-result mismatch is still rejected. Existing
  execution consumers retain both canonical-byte and digest mismatch refusal.
  Do not merge these sources or normalize differences in comparison tests.
  Output, errors, guard/delegation/evidence semantics and budgets otherwise match
  across prepared, running, terminal and archived fixtures.

### OPTIM-003: Apply atomic batches through one draft

- Implement the shared draft-level item primitive and batch-specific path without
  moving domain ownership or rewriting persistence.
- Test 1, 16 and 128 operations within the wire bound, changed/no-op mixtures,
  duplicate items, stale fences and value failures at first/middle/last positions
  after item-ID sorting (where that position exists). Cover supported mutation
  forms, input-order permutations, and separate request-size rejection.
- Implement the preloaded-list intermediate-overflow scenario specified above
  through real Store/CLI requests. A constructor-only limit test or an empty
  attempt under the wire cap does not prove write-path ordering.
- Acceptance: a successful batch constructs at most one new
  `WorkflowMemoryStateV2` successor, changed or no-op; early failure need not
  construct one. History-container copying does not scale with operation count
  at fixed history. The outer graph already has one batch successor and must
  remain so. Real Store/CLI tests prove rollback, revision behavior, absent failed
  outcomes and exact replay independently of the test-only count probes.

### OPTIM-004: Qualify and close execution-path optimization

- Run the complete development gate and the affected native contention, crash,
  replay and scale checks; compare the frozen real CLI against the baseline.
  Reuse G7/G8 and run the
  [native qualification entry point](../implementation-tips/direct-cli-performance-qualification.md#native-scale-execution)
  for the three ignored Store/Engine scale cases. `make test` alone does not run
  them and cannot satisfy scale acceptance. Never execute a removed daemon.
- Acceptance: original applicable thresholds, the current-Direct comparator and
  the OPTIM-002/003 count ceilings all pass independently. A count miss fails
  even with faster latency; a resource regression fails even with lower counts.
  Report required memory, latency and writer hold separately. SQL/facts count
  optimization is owned by READS, and no speedup is claimed from counts alone.
- Promote lasting design to architecture and implementation guidance, link final
  accepted measurement summaries to their exact source identity, and complete the
  independent Epic audit/review before owner acceptance. No release is implied.

## 7. Verification and release acceptance

Existing verification owners to extend, not blindly duplicate:

| Concern | Current source |
|---|---|
| Projection behavior and bounds | [view tests](../../crates/podway-engine/tests/int_v2run002_views.rs), [readback tests](../../crates/podway-engine/tests/int_v2dfc006_readback_ports.rs), [maximum next](../../crates/podway-engine/tests/int_v2rel002_maximum_next.rs) |
| Archive-show behavior | [direct execution tests](../../crates/podway-engine/tests/int_v2dfc006_direct_execution.rs), [domain ports](../../crates/podway-engine/tests/int_v2dfc006_domain_ports.rs), including delegated archives |
| Package, receipt and evidence reuse | [delegation tests](../../crates/podway-engine/tests/int_v2dfc006_delegation_ports.rs), [reference tests](../../crates/podway-engine/tests/int_v2dfc006_reference_ports.rs), plus new actual-path count probes |
| Historical/default-materialized Procedure identity | Extend read/domain/delegation characterization and [Engine execution tests](../../crates/podway-engine/tests/int_v2dfc006_direct_execution.rs) with differing-identity fixtures; separately assert stored read identity, wrapper-backed package construction, final package mismatch refusal and existing execution mismatch guards. These are new required cases, not claims about already passing coverage. |
| Conditional items and option guards | [guard tests](../../crates/podway-engine/tests/int_v2dfc006_guard_ports.rs) |
| Workflow memory and changed-row persistence | [memory tests](../../crates/podway-store/tests/int_v2_workflow_memory.rs), [relational tests](../../crates/podway-store/tests/int_v2dfc005_relational.rs) |
| Real stdin batch and exact replay | [direct CLI tests](../../crates/podway-cli/tests/e2e_v2dfc007_direct_cli.rs), `record_stdin_preserves_identity_applies_one_atomic_batch_and_replays_without_preflight` |
| Native scale and lock costs | [Store scale](../../crates/podway-store/tests/int_v2dfc005_scale.rs), [Engine scale](../../crates/podway-engine/tests/int_v2dfc006_scale.rs) |

Follow [AGENTS.md](../../AGENTS.md) for focused commands, Gaori routing and
`make test` on executable changes, plus the explicitly required native scale
route above. The existing stdin test proves state/fences/replay, not a measured
absence of preflight; READS-004 adds process-local observation for that claim.
Keep raw timings, profiler output and frozen
fixtures in ignored, owned locations, not in this dossier or canonical assets.
Use the [dossier lifecycle](README.md#lifecycle): after exact owner acceptance,
replace links with durable authorities and remove this dossier. Promote any
cross-epic references before removal. Release, installation and external-consumer
activation remain separately authorized.

## 8. References

- [Direct execution decision](../architecture-decision-records/0038-remove-the-daemon.md)
- [System architecture](../architecture/system.md)
- [Transactions and request-owned value reuse](../specs/storage/transactions-concurrency-and-idempotency.md)
- [JSON contract and response bounds](../specs/interfaces/json-contract.md)
- [Direct performance qualification](../implementation-tips/direct-cli-performance-qualification.md)
- [Testing and conformance](../specs/quality/testing-and-conformance.md)
