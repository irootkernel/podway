# Podway Snapshot State Reuse

## Status and authority

Status: Adopted. Owning epic: `STATE`. Design registered on 2026-10-19. The
[roadmap](../roadmap/README.md#state-snapshot-state-reuse) alone owns order and
status. Accepted PARSE completion is required before this epic starts; accept
STATE before ADMIT starts. Current ADRs, canonical assets and specifications
retain authority. This design selects no release or storage change.

## Verified context and evidence

The source baseline is `98796e6a20467d1d072bd84f377fd9ddccccab93` on
2026-10-19; the STATE-001 characterization baseline is
`d5be4f7971af89e33e1b7c7dc495397373e175f9` on 2026-10-21, after accepted
PARSE completion. Same-snapshot opportunities remain after READS:

- Store's [graph loader](../../crates/podway-store/src/v2_state.rs) admits terminal
  dispositions while loading a graph but discards that vector. The
  [view owner](../../crates/podway-store/src/direct_store/connection.rs) loads
  dispositions again after the receipt stage.
- [Reset dry-run](../../crates/podway-engine/src/direct_sessions.rs) loads a graph,
  checks fences and reads the clock before acquiring its view. Deep
  [doctor](../../crates/podway-engine/src/direct_workspace.rs) loads the graph for
  its artifact checks and then acquires a view. Its Store integrity verifier
  also reconstructs graph state before invoking the callback and checks archived
  dispositions. Include those stages in the inventory; they are not permission
  to skip integrity checks or merge distinct snapshot observations. Moving the
  entire view earlier would change validation order.
- [Memory reconstruction](../../crates/podway-store/src/v2_memory.rs) searches the
  cached attempts repeatedly during slot-owner discovery and reconstruction.
  The existing whole-unchanged prepared fast path already avoids reconstruction.
- [Receipt snapshot](../../crates/podway-store/src/v2_delegation.rs) admits bounded
  history and finds the active receipt; readiness then queries and decodes that
  receipt again.

These are request-local opportunities, not grounds for reducing required
persisted-state admission or merging observations made at different times.

### STATE-001 responsibility and removal ledger

This ledger records the prechange owners for the later implementation tasks;
it does not claim that reuse is implemented. Function names are the primary
anchors; line references reflect the characterization baseline.

| Owner and path | Eligible duplicate work | Retained responsibility |
|---|---|---|
| Store loader `v2_state.rs::load_present_graph_session_with_cache_v2` with `load_terminal_dispositions_connection_v2` and `validate_terminal_disposition_history_v2` | The admitted disposition vector is discarded at the `schema_version >= 5` call; STATE-002 replaces only that discard with retention. The disposition SQL and history validation stay in place, before the loader's cursor/latest-trace comparison. | Session/Workspace count admission, cached identity filter plus digest re-read or `load_snapshot_v2`, lifecycle/attempt/trace/counter/metadata/memory/goal admission, `new_with_goal_state`, the `PRAGMA user_version` gate, disposition decode and history rules, cursor check after disposition admission, `GraphLoadScope` commit (`v2_state.rs:1381-1382`, `1462-1482`). |
| View owners `connection.rs::DirectReadSnapshotV1::view` and `archived_view` | The second disposition SQL call in `view` and in `archived_view` (STATE-002 removals), and the graph reconstruction repeated when callers call `view` after `current_graph` in one snapshot. | Graph → receipts → dispositions → sequence order; the empty snapshot without receipt or disposition SQL when no current graph exists; the archived graph → receipts → dispositions → sequence order. |
| Reset dry-run `direct_sessions.rs` (dry-run arm) | Nothing yet; STATE-002 may carry the admitted graph into the later view only inside the existing stage order. | S0 binding resolution, S1 revalidation, graph admission including dispositions, missing graph ⇒ `SessionIdentityMismatch{actual: None}`, first fences (session id, then revision), clock, view, fence recheck, revision-only disposition match, `reset_result` invalid-input errors last. |
| Doctor `direct_workspace.rs::doctor_workspace_v1` with `connection.rs::verify_integrity_under_exclusive_lease` and `v2_state.rs::verify_v2_graph_state_connection_v2` | The per-archived-session disposition reload after `load_archived_session_connection_v2` inside the verifier loop (same deferred transaction) is a STATE-002-eligible duplicate; the verifier discards both vectors and keeps only the validation. | Verifier stages in order (lease, gate, classify, connection validation, `quick_check`, `foreign_key_check`, orphan snapshot, current graph with dispositions, inactive/slot checks, all-receipt verification, archived loop, outcome loop); any verifier-seeded carrier is consumed only after every verifier check finishes; callback deep rehash before clock and view; verifier-stage routing to the fallback read; callback errors returned raw. |
| Mixed reconstruction `transaction.rs::use_prepared_graph` with the cached arm of the loader and `v2_memory.rs::load_workflow_memory_v2` | The two attempt-keyed linear lookups (discovery and the per-trace-attempt main loop) become one reconstruction-local borrowed index (STATE-003). | Whole-unchanged prepared fast path untouched; cached identity and digest re-checks; first-match lookup semantics; no eager validation of later rows; header/chunk/point SQL shapes; 128-pair chunk bound; TooBig truncation and fallback; pointer reuse with the query-accounting ledger; mixed declaration-before-discovery error order. Item-level lookups in `classify_owner` and `validate_attempt_slots` are not attempt-keyed and stay split. |
| Receipts `v2_delegation.rs::receipt_snapshot_connection_v1` with `completion_readiness_connection_v1` and `DelegationReceiptSnapshotV1::new` | The readiness stage re-acquires the active receipt through the unaccounted point query after the window already admitted it (STATE-004 removes the re-acquisition for a state admitted in the same snapshot as the window). | The no-delegation early-empty predicate, the accounted count, whole-window validation before readiness, the placement/`has_delegation` gate, `matches_state` and the accepted-items digest evaluated at the readiness stage, fallback acquisition when the window cannot prove absence, and `new` coherence. |

Window admission proves identity only: it covers the `matches_state` fields,
so `PackageChanged` is excluded for an in-window active receipt, while
`ItemsChanged` remains the normal in-window outcome when items change after
acceptance because only the readiness stage compares the accepted-items
digest. A receipt-requiring readiness with no admitted receipt resolves to
`InvalidStateV1` through `new` coherence; snapshot-path `PackageChanged` is
constructible only hand-built or through the write-time guard's direct
readiness call in `v2_state.rs::replace_direct_graph_session_transaction_v2`.

Deliberately split from the reuse tasks, each with its owner: the write-side
reload pair `DirectTransactionV1::terminal_dispositions`
(`transaction.rs:192-200`; engine callers in `direct_sessions.rs` and
`direct_start.rs`) and the double load in
`record_terminal_disposition_transaction_v2` (both run inside one write
transaction, so the split exists because the STATE-002 carrier binds to read
snapshots, not because they are distinct observations);
`archive_current_session_transaction_v2` genuinely observes an earlier
prepared read; the write-time receipt guard in
`replace_direct_graph_session_transaction_v2`; the receipt/view cross-check
against `read_receipt_connection_v1`; `verify_receipts_connection_v1`; the
DF-005 early-empty predicate; and `read_under_exclusive_lease` in general
(the doctor fallback and `workspace.remove`'s current-graph observation),
which never receives a carrier. The planned STATE-004 boundary keeps the
existing point-query acquisition on the prepared-Arc write path of
`DirectTransactionV1::delegation_receipts`, because the stamp check does not
cover `v2_attempts`/`v2_delegation_receipts` row identity and derived
absence would convert today's coherence refusal into a silent
`ReceiptMissing`.

### STATE-001 stage and failure-order baseline

- Loader and view: the disposition check sits between the `PRAGMA user_version`
  gate and the cursor comparison, so a disposition fault precedes receipt
  admission and the cursor check; the view then loads receipts before
  dispositions and closes with the committed sequence.
- Reset dry-run keeps graph admission → first fences → clock → view, with
  the fence recheck and disposition match after the view. Building the full
  view before the fences would let receipt corruption beat
  `SESSION_REVISION_CONFLICT`/`SESSION_ID_MISMATCH` and read the clock early.
- Doctor: verifier-stage integrity failures route to the fallback
  `storage_integrity: fail` read; the deep artifact rehash precedes clock and
  view inside the callback.
- Receipts: the whole window validates before readiness; readiness evaluates
  `matches_state` and the accepted-items digest; absence is provable only
  from a complete window (`summaries.len() == total`), never from a
  last-element assumption, because window rows carry no trace sequence.

Recorded invariants instead of fixtures: the reset `MissingGraphSession`
check is unreachable after S1 graph admission; duplicate cached attempt IDs
are unreachable because `WorkflowMemoryStateV2::new` rejects them; cached and
current attempt order cannot diverge because the cached graph requires
session and snapshot identity and traces only append, so STATE-003 must still
avoid positional zipping; direct reads require schema generation 15, so the
legacy disposition branch is unreachable retained code; and on the read path
an out-of-window active receipt is unreachable: receipts attach only to the
attempt active at acceptance, attempts append in trace order, and the loader's
cursor check binds the persisted active cursor to the trace's active attempt,
so an active attempt's receipt is always the newest receipt-bearing row.

### STATE-001 baseline regression owners

- Disposition corruption and loader order: new
  `int_v2dfc008_persistence::state001_disposition_corruption_refuses_view_and_verification`
  (current view plus verifier refusal with intact schema checks and unchanged
  bytes; its healthy control also pins the non-delegated empty receipt
  snapshot that owns the early-empty predicate's observable contract) and new
  `int_reads001_negative_boundaries::state001_disposition_admission_precedes_receipt_and_cursor_checks`
  (typed Session corruption with probe evidence that the disposition
  statement ran and the receipt window did not).
- Reset fence priority and clock order: new
  `int_v2dfc006_domain_ports::state001_reset_dry_run_fence_rejection_precedes_clock_and_receipt_admission`
  with the panicking clock; existing domain-port reset dry-run cases and
  `v2run007_preadmission_fences_reject_without_jobs_and_exact_replay_wins`;
  deferred admission boundaries stay owned by
  `int_v2dfc005_relational::bound001_deferred_memory_admission_and_cold_read_remain_distinct`
  and
  `int_v2dfc006_domain_ports::bound001_deferred_memory_error_keeps_public_mapping_and_fence_priority`.
- Mixed reconstruction owners: `int_reads001_query_inventory::prepared_reload_counts_unchanged_sparse_and_dense_selective_misses`
  owns the unchanged sparse/dense selective-reload accounting;
  `int_reads002_baseline::reload_baseline_three_waves_128_plus_128` and the
  rest of the reload-baseline family own the chunked changed-row bound and
  peak accounting;
  `int_reads002_baseline::deferred_scratch_survives_earlier_scope_exits_and_books_unwind`
  owns the mixed per-owner error order;
  `int_reads002_negative::mixed_chunk_late_too_big_preserves_cached_and_new_collection_modes`
  owns the TooBig fallback between cached and new collection modes;
  `int_reads002_negative::mixed_missing_earlier_spec_wins_before_later_oversized_payload`
  owns spec-lookup-before-payload ordering; and
  `int_v2dfc005_direct_store::stale_prepared_history_matches_cold_state_after_set_clear_and_complete`
  owns mixed/cold equivalence after set, clear and complete. Duplicate cached
  attempt IDs stay unreachable as recorded above, and the reconstruction
  index helper's first-match unit check belongs to STATE-003.
- Receipt window, readiness and guards: existing
  `int_v2sdg004_receipt::v2sdg006_receipt_history_reports_exact_total_and_newest_bounded_window`,
  `v2sdg006_receipt_snapshot_rejects_incoherent_window_and_readiness` (extended
  with the `PackageChanged`/`ItemsChanged` with no receipt arms),
  `v2sdg006_live_receipt_reads_reject_row_identity_mismatch`,
  `v2sdg004_cold_reads_reject_well_formed_mismatched_and_noncanonical_receipts`,
  `v2sdg004_orphan_receipt_is_rejected_by_cold_doctor`; new
  `state001_truncated_history_reports_point_readiness_outcome_for_absent_active_receipt`,
  `state001_older_receipt_window_corruption_refuses_with_valid_active_receipt`,
  `state001_completing_delegated_attempt_without_admitted_receipt_fails_write_time_guard`,
  and `state001_prepared_arc_write_path_receipt_snapshot_refuses_unjoinable_active_receipt`
  (prepared-Arc write-path boundary).
- ItemsChanged on the read path: new
  `int_v2dfc006_direct_execution::state001_changed_items_report_items_changed_through_view_with_in_window_receipt`,
  alongside the existing write-path
  `delegated_acceptance_recomputes_package_and_blocks_changed_items_before_clock`;
  the receipt/view cross-check is owned by
  `int_v2run002_views::v2sdg006_current_and_stale_receipts_gate_completion_across_views`
  and the delegation-package cross-check by `int_optim001_direct_identity`.
- Doctor and split boundaries: `int_v2dfc006_workspace` doctor cases
  including
  `deep_doctor_detects_changed_required_artifact_without_writing_or_reading_clock`,
  the domain-port doctor fallback envelope owners, CLI doctor e2e, and the
  prepared-graph tests in `int_v2dfc005_direct_store`
  (`prepared_graph_refreshes_before_fences_and_replay_bypasses_preparation`,
  `stale_prepared_history_matches_cold_state_after_set_clear_and_complete`);
  `int_v2dfc005_relational::delegation_disposition_archive_purge_and_session_reset_obey_retention`
  owns the write-side disposition/archive retention paths. Supported Store
  continuity execution belongs to STATE-005.

## Goals, non-goals, and scope

Give admitted graph/disposition data, cached attempt lookup and active receipt
facts one owner within their coherent SQLite snapshot. Remove repeated
reconstruction and lookup without moving their checks across fences or clocks.

No lazy corruption validation, persistent/global cache, schema/index/table,
migration, writer identity change or CLI/JSON interface change is adopted. Preserve
archive-list's existing summary-only archived rows and its current-session
admission; do not skip the current view on archive reads. No elapsed-time tests,
latency percentiles, throughput benchmarks, performance baselines or thresholds are
allowed. Do not turn request-local sharing into a generalized caching framework.

## Accepted design decisions and interfaces

### Staged snapshot reuse

Use a private Store-owned carrier for an admitted graph and already obtained
dispositions. It belongs to one read snapshot and cannot outlive the associated
identity/transaction scope. Core remains pure; persistence carriers stay in Store.

Avoid loading the same graph again when constructing the later view. Preserve
the existing graph, receipt and disposition validation stages. If the graph
loader already admits a disposition value, retain it for use at its original
view stage; do not advance a later error or skip an earlier check.

Reset dry-run retains graph admission, first fence checks, clock access and then
receipt/view admission, including its later fence recheck. It must not construct
the full view before the first fences merely to share data. Doctor reuses state
only within its existing maintenance scope, including deep artifact rehash under
the exclusive maintenance lease. Keep the integrity verifier's independent
validation responsibilities and snapshot boundaries; a later projection cannot
stand in for an earlier integrity check. This work does not extend the lease or
apply deep scans to ordinary opens.

STATE-002 implementation record: `AdmittedGraphStateV2`
(`v2_state.rs`) carries the admitted graph plus the dispositions its loader
stage already validated; `load_present_graph_session_with_cache_v2` keeps the
`schema_version >= 5` disposition call at its exact stage (the legacy branch
yields an empty vector) and returns both values. `DirectReadSnapshotV1` stages
that admission once in a private one-shot carrier: `view` takes it with
`borrow_mut` (fail-closed; no silent fallback admission), and
`with_admitted_current_graph` admits first without holding a borrow across the
admission I/O, then lends the graph under a shared borrow. Its re-entrancy
contract is documented on the method: calling `view` on the same snapshot from
inside `body` is a programming error and panics on the staged carrier's borrow,
while a nested `with_admitted_current_graph` only re-lends the same admitted
graph. The verifier seeds the carrier only when it constructs the read
snapshot, after every verifier check; `read_under_exclusive_lease` snapshots
are never seeded, so the doctor fallback and `workspace.remove` keep their own
cold work. One retention consequence: the doctor's seeded current admission
stays alive through the verifier's archived and outcome loops, holding the
current graph plus one archived graph instead of the previous maximum of one;
the test-support retention ledger does not observe caller-side drops, so no
accountant metric changes. Deterministic counts (registered SQL executions,
no timing): a view or archived view goes from two disposition loads to one and
from three to two `PRAGMA user_version` reads; a reset dry run from two graph
admissions and three disposition loads to one and one; doctor from two or
three admissions (non-deep, deep) and three or four disposition loads to one
and one; per healthy archived session the verifier's archived loop goes from
two disposition loads to one and from three `PRAGMA user_version` reads to two
(3A -> 2A and 2A -> A for A archived sessions; failing admissions never reach
the removed reload). Count bases: the ordinary-view and reset counts apply
when a current graph exists — an empty store performs no receipt or
disposition SQL before or after — and the doctor counts assume no archived
sessions, with A archived sessions adding the per-session verifier counts
already given. `int_reads001_query_inventory` targets are unchanged
because `measure_cold` observes only `current_graph()`, whose wrapper still
performs the identical loader admission and then discards the retained
dispositions. `state002_` tests in `int_reads001_query_inventory`,
`int_v2dfc008_persistence`, `int_v2dfc006_domain_ports` and
`int_v2dfc006_workspace` pin these counts and the unchanged outputs.

### Borrowed attempt index

Build one reconstruction-local index from borrowed cached attempts, keyed by
attempt identity, and share it between slot-owner discovery and reconstruction.
A `BTreeMap` over borrowed IDs/values fits existing ownership without cloning the
attempt bodies. Preserve current first-match semantics while building the index,
for example with entry insertion that keeps the first admitted value.

Do not eagerly validate a later current row while building the cache index or
change per-owner SQL/error order. Do not zip by position: matching prefix
identity has not been proven before admission. Retain changed-chunk bounds of
at most 128 owner/item pairs, TooBig fallback and existing pointer reuse. The
whole-unchanged fast path remains unchanged; this optimization applies when
reconstruction is actually necessary.

STATE-003 implementation record: `v2_memory.rs::index_cached_attempts` builds
one reconstruction-local `BTreeMap` from borrowed cached attempts (first-match
`entry().or_insert`; no attempt-body clone, no SQL, no validation of later
rows) once per `load_workflow_memory_v2` mixed reconstruction, before header
discovery; `discover_slot_owners` and the per-trace-attempt loop share it by
reference, replacing exactly the two attempt-keyed linear scans. Item-level
lookups in `classify_owner` and `validate_attempt_slots`, the 128-pair chunk
bound, TooBig fallback, pointer reuse and the whole-unchanged prepared fast
path are unchanged, and no SQL is added or removed, so every mixed-path count
owner and pointer-reuse assertion keeps its existing targets. The first-match
unit check is owned by
`v2_memory::reads_tests::state003_attempt_index_keeps_first_match_for_duplicate_ids`.
Id-keyed matching (not positional) is required because matching prefix
identity is not proven before admission; in reachable states cached attempts
are always a same-order prefix of the current trace (session and snapshot
identity plus digest re-check, append-only attempts), so positional and
id-keyed lookups are observationally equal there and no reachable-state test
distinguishes them.

### Receipt readiness

Extract pure readiness derivation from receipt acquisition. Supply an already
admitted active receipt when the snapshot proves that the matching receipt is
present in the acquired history. Include identity and absence semantics, not
merely a last-element assumption. Retain fallback acquisition if coverage cannot
be proven and preserve the existing admission stage for any fallback error.

Keep all history count/window and receipt checks. Do not narrow history
validation to the active receipt or bypass a malformed older entry. This task
does not adopt the separate cross-layer predicate consolidation in DF-005.

## Failure handling and compatibility boundaries

Preserve corrupt hidden/history values and competing-revision error priority,
deferred declaration errors, graph/digest identity and native Store/root/writer
checks. Reuse is valid only within the same admitted snapshot; changed identity
or a later observation requires authoritative acquisition again.

Keep current output/error semantics, canonical bytes, exact old-attempt replay,
no outcome for rejection, atomic success outcomes, clock/ID order and bounded
collections. Store generation and writer-contract identity remain unchanged;
no reset or migration is part of acceptance. Preserve write-free snapshots,
missing-SHM and hot-WAL read behavior and existing maintenance deadlines.

## Roadmap ownership, dependencies, and traceability

| Task | Deliverable and focused verification |
|---|---|
| `STATE-001` | Inventory graph/disposition, reconstruction and receipt owners, including Store integrity verification before the doctor callback, with stage/fence/clock ordering; identify corruption, deferred-admission and snapshot checks. |
| `STATE-002` | Introduce bounded Store-owned staged reuse; verify ordinary views, reset and doctor outputs and multi-fault error precedence without widening maintenance scope. |
| `STATE-003` | Share one borrowed attempt index; verify unchanged/changed paths, first-match behavior, owner order, changed-chunk bounds, TooBig fallback and borrowed identity reuse. |
| `STATE-004` | Derive readiness from matching admitted receipt; verify absent/stale/current receipts, truncated history coverage, fallback acquisition and corrupt-history refusal. |
| `STATE-005` | Integrate corruption priority, supported Store continuity and exact replay checks, required development gate and independent review; promote lasting documentation. |

The roadmap alone records task status. Focused verification accompanies each
task; no task is accepted solely because a new carrier compiles.

## Verification and release acceptance

Use registered Store relational and prepared-memory tests, Engine view,
[readback refusal](../../crates/podway-engine/tests/int_reads001_readback_ports.rs)
and [domain-port](../../crates/podway-engine/tests/int_v2dfc006_domain_ports.rs)
checks. Retain the established snapshot-success/later-declaration-error boundary
and stale-revision priority. Add targeted multi-fault checks only where reordered
acquisition creates a material verification gap.

Future executable changes require focused checks and `make test` before sharing.
Use the existing [supported Store continuity](../implementation-tips/testing.md#supported-store-continuity)
scenario for the snapshot/reconstruction boundary: preserve original response
bytes, writer/snapshot identity, continuation and old-attempt replay in isolated
fixtures. No timing, percentile or historical scale benchmark qualifies this
epic. Structural evidence is the removal of the named duplicate acquisition and
lookup while all admission responsibilities remain owned.

No release or installed-runtime qualification is selected. Promote durable
architecture and verification guidance before
retirement under the [TODO lifecycle](README.md#lifecycle).

## References

- [Shared program boundaries](../roadmap/README.md#shared-preservation-boundaries)
- [System architecture](../architecture/system.md)
- [SQLite model](../specs/storage/sqlite-model.md)
- [Transaction contract](../specs/storage/transactions-concurrency-and-idempotency.md)
- [Structural verification](../implementation-tips/testing.md#structural-simplification-verification)
