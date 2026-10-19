# Podway Snapshot State Reuse

## Status and authority

Status: Adopted. Owning epic: `STATE`. Design registered on 2026-10-19. The
[roadmap](../roadmap/README.md#state-snapshot-state-reuse) alone owns order and
status. Accepted PARSE completion is required before this epic starts; accept
STATE before ADMIT starts. Current ADRs, canonical assets and specifications
retain authority. This design selects no release or storage change.

## Verified context and evidence

The source baseline is `98796e6a20467d1d072bd84f377fd9ddccccab93` on
2026-10-19. Same-snapshot opportunities remain after READS:

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
