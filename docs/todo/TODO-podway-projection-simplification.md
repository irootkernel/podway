# Podway Projection Simplification

## Status and authority

Status: Adopted. Owning epic: `VIEWS`. Design registered on 2026-10-19. The
[roadmap](../roadmap/README.md#views-projection-simplification) alone owns task
order and status. This is the first epic of `VIEWS -> PARSE -> STATE -> ADMIT`,
following accepted READS completion. Accept this epic before PARSE starts.

This dossier defines unfinished work. Existing ADRs, canonical assets and
specifications retain authority over implemented behavior. No CLI/JSON interface,
storage identity, release or external-consumer change is adopted here.

## Verified context and evidence

The source baseline is `98796e6a20467d1d072bd84f377fd9ddccccab93` on
2026-10-19, after accepted OPTIM, BOUND and READS. In
[`v2_read_service.rs`](../../crates/podway-engine/src/v2_read_service.rs):

- `ReadScope` already borrows the graph and memoizes validated Procedure access.
  That completed work is not a new optimization target.
- `project_observation_item_value` serializes each growing prefix of the active
  item's list value in `session.observe`; this is not evidence-page budgeting.
  `history_window_measure_bytes` likewise serializes successive prefixes and
  derives trace bounds from them.
- `item_values` constructs display values before the byte budget admits its
  prefix. `stale_attempt_value` formats all recorded values before selecting three.
- Evidence preview/page helpers allocate full scalar vectors for bounded text.
  Some responses still require a full logical scalar count.
- `CurrentProjection` derives selected evidence; readback and
  `delegation_work_package_in` repeat selection.
  Observation constructs next-style evidence previews and then strips them.

Existing in-module byte-budget tests, the registered
[view suite](../../crates/podway-engine/tests/int_v2run002_views.rs), and
[readback corruption suite](../../crates/podway-engine/tests/int_reads001_readback_ports.rs)
own relevant behavior. Their existence alone is not evidence of a future change.

## Goals, non-goals, and scope

Remove repeated prefix serialization, unnecessary display construction and
repeated evidence selection. Keep one clear owner for each calculation, using
existing Engine projection boundaries.

Elapsed-time tests, p95/p99 or other latency percentiles, throughput benchmarks,
performance baselines and performance thresholds are excluded. No general
instrumentation framework, persistent cache, lazy Store validation, new read
endpoint or public output change is included. No duplicate work already removed
by OPTIM or READS is to be reintroduced for comparison.

## Accepted design decisions and interfaces

### Exact byte accounting

Apply this work to `project_observation_item_value` and history-window budgeting.
Use request-local serialized lengths to admit each candidate prefix once. Count
each candidate's exact JSON representation without repeatedly serializing the
already admitted prefix. Give exact serde JSON length calculation one small
Protocol-owned counting helper, used by Engine here and PARSE's response bounds.
It returns a checked length with the existing serialization error semantics,
without retaining an output buffer. Do not introduce independent counting
implementations, telemetry or new byte-budget policy.

Account for array separators, escaped strings, nested values, wrapper fields,
numeric digit widths, trace bounds and truncation flags. Trace bounds are the
minimum and maximum trace positions present in admitted entries, not assumed
first/last positions; entries can have no trace. Reserve the longer `false`
spelling wherever the existing budget calculation does. Keep exact-fit and
one-byte-over behavior and existing final serialization/error mapping.

Retain the history iterator's current lookahead/error policy when a full window
has already been admitted. Do not accidentally surface a previously ignored
trailing error or hide an error within the admitted window. CLI and JSON schemas
stay unchanged; the length helper is a workspace-internal Rust interface.
Preserve whether another filtered source entry exists, without requiring full
materialization of that discarded lookahead solely to answer `is_some()`.

### Bounded construction

Traverse text by Unicode scalar, preserving scalar offsets and continuation
meaning. Allocate only requested output text rather than a full `Vec<char>`.
When the response includes a full logical length, count the full input without
retaining every scalar. Reuse a logical size already calculated from the same
admitted value rather than counting it again inside the page/preview helper.
Keep empty-page and end-of-content behavior.

Delay expensive item formatting until its display position can be considered.
Keep total counts independent of displayed prefixes. For the stale-attempt
summary, count only slots with a recorded value and format the first three of
those valued slots, preserving their order. Empty slots contribute neither an
item nor a count; do not take three slots before filtering them.
For byte-bounded lists, the first rejected candidate may need construction to
establish fit; later undisplayed candidates do not. Required validation still
occurs for all stored values before projection, including hidden historical data.

### Projection reuse

Use one selected-evidence calculation for consumers of the same admitted
`CurrentProjection`, including next/readback and `delegation_work_package_in`.
Preserve each consumer's existing error mapping and selection order. Separate
observation's display needs from next/readback display needs through the existing
private projection owner. Observation should
avoid constructing preview text that it immediately removes, while preserving
its exact fields, selected evidence, bounds and failures. Inventory the
preview-gated source-snapshot, active-consumer and page-token constructibility
checks in `readback_value` before suppressing materialization. Retain those
checks at their original logical stage and skip only preview materialization.
Keep their original explicit-selector, remaining-preview-slot, preview-existence
and truncation gates, including preview-slot consumption order. Compute those
gates without building discarded display payloads; evaluate guards for exactly
the same items as before, without front-loading checks for other items.
Omitting their output fields does not remove the checks. Metadata, digest and
source-slot checks remain independent.

Keep source/Store validation, projection and envelope checks independently
owned. A purpose-specific projection is not permission to return unvalidated
state or remove Standard preflight projection/schema/envelope-equivalent checks.
Avoid a generic projection framework or a second public response model.

## Failure handling and compatibility boundaries

Preserve JSON fields, array order, canonical bytes where required, public error
codes/details/exit/retryability, output limits and truncation rules. Preserve
eager stored-value admission outside displayed windows; archive-list retains its
existing summary-only exception. Do not change CLI grammar, Store layout,
writer identity, request identity or exact stored replay responses.

Keep clock/ID and validation ordering, success-only outcomes and write-free
snapshot reads. Sharing is limited to one coherent request snapshot. Separate
start/reset observations and external CLI call sequences remain unchanged.

## Roadmap ownership, dependencies, and traceability

The roadmap is the sole status authority. These are work and acceptance mappings:

| Task | Deliverable and focused verification |
|---|---|
| `VIEWS-001` | Inventory each targeted output builder, byte/scalar budget and validation stage; identify existing exact-output, overflow and hidden-value refusal checks before editing behavior. |
| `VIEWS-002` | Implement exact prefix accounting; verify escaped/multibyte values, trace gaps, digit boundaries, exact fit, one byte over and trailing iterator errors against current output behavior. |
| `VIEWS-003` | Bound display construction; verify scalar paging, empty/end pages, full logical totals, empty slots interleaved with stale valued slots, first-three valued selection and item byte limits without narrowing persisted validation. |
| `VIEWS-004` | Share evidence selection across readback and delegation packages and observation-specific construction; compare complete results and preserve preview-gated errors, including Standard preflight failures. |
| `VIEWS-005` | Integrate behavior equivalence and corruption checks, required development gate and independent review; promote lasting ownership and verification guidance before dossier retirement. |

Task-local verification accompanies every implementation task. Material changes
to these boundaries require revising the design before implementation; ordinary
private helper naming does not require a new product decision.

## Verification and release acceptance

Use existing functional suites first. Add only missing behavior checks needed by
the changed algorithm. Verify exact byte accounting against real serialization
and public limits; tests must not depend on TODO prose or incidental helper
structure. Local deterministic assertions may establish that one selected value
is reused or undisplayed text is not constructed where that is material. They do
not justify broad probes, telemetry or a benchmark project.

Run focused checks while implementing, and `make test` before sharing executable
changes. Closeout requires complete outputs, failure ordering and hidden-value
refusal to remain equivalent, plus source evidence that the targeted repeated
work is gone. No latency or throughput result is acceptance evidence.

This is development work without a selected release. Historical native-scale
qualification does not become a new gate. Promote durable conclusions and
retire this dossier only under the [TODO lifecycle](README.md#lifecycle).

## References

- [Shared program boundaries](../roadmap/README.md#shared-preservation-boundaries)
- [System architecture](../architecture/system.md)
- [JSON contract](../specs/interfaces/json-contract.md)
- [Structural verification](../implementation-tips/testing.md#structural-simplification-verification)

## VIEWS-001 remove/retain ledger

First-task deliverable registered on 2026-10-20. The roadmap alone owns status;
this ledger is the task's checkable inventory, not another status source. Line
numbers refer to `crates/podway-engine/src/v2_read_service.rs` at the VIEWS-001
candidate unless another file is named; unqualified test names live in its
in-module `tests` module or the registered view suite
(`crates/podway-engine/tests/int_v2run002_views.rs`).

### Site ledger

1. Observation list projection — `project_observation_item_value` (list arm,
   :2907-2927), consumed by `active_item_descriptors` (:2059-2064) for the
   observation active-item window. Remove: per-candidate reserialization of the
   whole admitted prefix. Retain: per-entry scalar truncation, the serialized
   byte budget's exact-fit admission and one-byte-over refusal, entry and window
   truncation flags, and the non-list arms unchanged. Registered checks:
   `reads003_observation_list_projection_measures_in_place` plus the new
   `views001_observation_list_exact_fit_admits_at_the_byte_budget` and
   `views001_observation_list_one_byte_over_the_budget_refuses`, which pin the
   exact 1024-byte boundary for ASCII, escaped and multibyte bytes against real
   serde serialization of unprojected candidates.
2. History windows — `history_window` (:3734) with
   `history_window_measure_bytes` (:3765) across the six `add_histories`
   families (:3599-3695). Remove: per-candidate serialization of the whole
   admitted window. Retain: the longer `false` spelling reserve, min/max trace
   bounds (:3703), the trailing-iterator error swallow (:3759) with in-window
   error propagation, family maxima, newest-first order, and the no-extra
   lookahead after a byte-overflow break. Registered checks: in-module
   `reads003_borrowed_history_measurement_matches_owned_construction_bytes`,
   `reads003_history_window_exact_fit_admits_and_one_byte_over_truncates`,
   `reads003_history_window_swallows_trailing_error_after_a_full_window`,
   `reads003_owned_members_move_rather_than_reserialize`; view-suite
   `v2rel002_every_verbose_history_family_marks_actual_omissions_and_exact_window`
   and `v2run002_valid_and_stale_attempt_histories_are_disjoint_and_cursor_bound`;
   production history pagination stays owned by
   `v2drw005_production_readback_is_bounded_immutable_pageable_and_cold_stable`
   in `crates/podway-engine/tests/int_v2dfc006_readback_ports.rs`.
3. Status values window — `CurrentProjection::item_values` (:1973-2013). Remove:
   display formatting of values the byte budget never admits. Retain: the local
   two-byte array charge and separator accounting, the first-rejected-candidate
   stop, the `STATUS_VALUES_BYTES_MAX` serialization-failure fallback (:2003),
   and the exact total and truncation flag. The always-admit-first policy of
   evidence pages is not imported here. Registered check:
   `v2rel002_standard_item_values_stop_before_the_first_whole_value_over_budget`.
4. Stale-attempt summary — `stale_attempt_value` (:3843-3890). Remove:
   formatting of valued slots beyond the first three. Retain: valued-only
   counting, first-three-valued order, `items_truncated`, metadata, timestamps
   and reference projection with their error mapping. Registered checks:
   `v2run002_valid_and_stale_attempt_histories_are_disjoint_and_cursor_bound`
   plus the new `views001_stale_attempt_summary_skips_interleaved_empty_slots`
   (interleaved empty slots, valued-only total, first-three-valued order).
5. Evidence preview/page helpers — `evidence_preview_v1` (:3486),
   `evidence_page_v1` (:979), `projected_preview_bytes_v1` (:3549),
   `evidence_logical_size_v1` (:962). Remove: full scalar materialization and
   repeated logical-size counting inside the helpers. Retain: scalar-boundary
   paging, the page path's first-entry admission, empty and terminal page
   behavior, whole-entry preview admission, indivisible values, stale-token and
   second-reference behavior. Registered checks: the view-suite `v2scl004`
   family — paging determinism and exact failures, exact totals when truncating,
   slot budget and count, empty terminal page, second reference, stale reference
   versus stale token, multi-page text, indivisible values, whole-entry preview
   admission.
6. Selected-evidence duplication — `CurrentProjection::derive` (:1841),
   `readback` (:3327), `delegation_work_package_in` (:196/:204), and the single
   observation strip call (:1264, helper :1521). Remove: repeated
   `selected_evidence_readback` computation for the same admitted projection and
   observation preview text that is immediately stripped. Retain: per-consumer
   error mapping and selection order; in `readback_value` the unconditional
   source-slot, revision and digest checks before the explicit-selector,
   remaining-slot, preview-existence and truncation gates, with the original
   preview-slot consumption order. Registered checks:
   `v2run002_fresh_action_projects_closed_compact_standard_verbose_and_next_views`,
   the `v2sdg006` delegation cases,
   `v2sdg007_running_delegated_projection_fits_observation_allocation`, and
   `v2agt002_observation_projects_typed_items_and_fenced_templates`.

### Deliberately split paths

- `ReadScope` memoized validated access is completed READS work, not a target.
- `admit_within_bytes_v1`, `admit_members_within_bytes_v1` and
  `serialized_object_bytes_v1` member admission, including the observation
  shared-member fail-closed budget, stay unchanged.
- ADR-0024 window policy (`missing_ids`, `missing_items`, template windows)
  stays unchanged.
- The unreachable stale readback arm (:3364) retains its guard and recorded
  invariant; no unsupported fixture is invented.
- Eager hidden historical-value admission in Store before projection, corruption
  precedence (`crates/podway-engine/tests/int_reads001_readback_ports.rs`),
  archive-list's summary-only exception, and the Standard preflight Stage 4/5
  projection, schema and envelope-equivalent checks (`preflight_facts_v1` in
  `crates/podway-engine/src/direct_execution.rs`) remain independently owned; no
  trust-boundary validation is merged.
- The mixed-width Unicode paging regression is a required VIEWS-003 check and is
  deliberately not added here.

### Task mapping

Ledger rows 1-2 belong to VIEWS-002, rows 3-5 to VIEWS-003, and row 6 to
VIEWS-004. VIEWS-002 must preserve the trailing-iterator policy while avoiding
construction of the discarded lookahead payload; VIEWS-003 must keep eager
persisted validation of hidden values before projection.
