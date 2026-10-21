# Testing

## Verification layers

```bash
make test-prepare   # formatting, lint, static checks, contracts, architecture
make test-rust      # unit, architecture, and integration tests
make test-unit      # focused library and binary tests
make test-int       # focused component integration tests
make test-fuzzing   # bounded deterministic protocol fuzzing
make test-e2e       # real user journeys through the debug CLI
make test-native-qualification NATIVE_EVIDENCE_PARENT=<absolute-owned-directory>
make test           # required development gate
make dist           # complete gate, qualification, handoff, and final verification
```

To run one registered native case through the same binary-building entry point:

```bash
make test-e2e-exact E2E_TEST=podway-cli::e2e_suite::e2e_v2sdg008_qualification::supervised_delegation_preserves_documented_example_hosts_and_recovery
```

Exact selection succeeds only when one test executes and passes; an empty or
multiple-test selection fails. Use `make test-e2e` for the complete E2E suite. The focused result does not
replace the full `make test` development gate.

The supervised-delegation case also executes the exact
[documented review Procedure](../examples/supervised-delegation.md), including
authoring checks, acceptance, stale receipts, and fresh-attempt correction.
Its executor is deterministic; neither host label proves a live integration.

Run the narrowest relevant layer while iterating and `make test` before sharing a
development revision. `make dist` always reruns that gate, adds bounded fuzzing,
all-target Clippy, and qualification of the release-profile distribution before
creating the handoff and independently verifying the final bundle.

Rust unit, architecture, and integration targets use four test workers by default;
set `TEST_THREADS=<n>` on a Make invocation to tune that bound. Real-binary E2E
remains serial. Make-driven Cargo gates disable incremental compilation to avoid
accumulating large numbers of codegen objects; ordinary direct `cargo build`
commands retain Cargo's incremental default.

Cargo integration files are modules of each crate's `int_suite`; run one exact
test with:

```bash
cargo test -p <package> --test int_suite <source>::<function> -- --exact
```

CLI E2E sources use `e2e_suite`. The test-layout verifier rejects sources omitted
from or duplicated across aggregate suites.

## Isolated direct CLI checks

The E2E runner builds the CLI once and uses disposable Git worktrees. Native
tests verify direct mutations, outcome lookup, contention, legacy refusal,
maintenance recovery, delegation, and goal flows. These fixtures never select
the user's active worktree or installed service as a test target.

The former contributor daemon helper and `dev-runtime-test` target are retired.
Use registered direct CLI cases through `make test-e2e-exact` or the complete
`make test-e2e` suite. A fixture's synthetic delegation provenance proves local
protocol behavior, not a live executor integration.

## Supported Store continuity

For a boundary refactor, preserve an accepted prechange CLI and create an owned
standalone Git fixture through that CLI: init, start, begin and record one required
item with explicit fences and a caller key. Retain the original stdout bytes,
status, Store generation, writer-contract row and canonical snapshot bytes/digest.
Use a fresh CLI process for every later observation or mutation.

The candidate must read the same recorded state, replay the original fully fenced
request byte-for-byte, and return that response through read-only outcome lookup.
Then complete the satisfied action with current explicit fences, verify the next
node, and replay the original request again with its original old attempt/item
fences. Both replays and lookups leave logical database state and sequence
unchanged; only continuation advances the sequence. Generation, writer-contract
and snapshot identity remain unchanged throughout, without reset or migration.

Verification binaries built from a reviewed dirty tree may embed the Git parent
that existed before the completion commit. Retain that actual build stamp and
binary hash; bind the reviewed source-file hashes to the accepted commit rather
than relabeling the artifact. Recheck replay/continuation after a later rebuild
when claiming behavior of that rebuilt CLI.

BOUND-006 exercises this scenario across the CLI decomposition against a
supported generation-15 Store. It complements registered persistence/replay tests
and does not establish legacy migration or installed-runtime compatibility.

STATE-005 exercises the same scenario across the snapshot and reconstruction
reuse boundary against a supported generation-15 Store created by a retained
pre-STATE CLI; it equally complements registered persistence/replay tests and
establishes no legacy migration or installed-runtime compatibility.

## Functional regression owners

Pure workflow and goal tests live beside their Core implementations in
[`workflow_v2/tests.rs`](../../crates/podway-core/src/workflow_v2/tests.rs) and
[`goal_state_v2/tests.rs`](../../crates/podway-core/src/goal_state_v2/tests.rs).
BOUND-002 moved 20 workflow and 11 goal cases from the Store suites with their
original names; they run as library unit tests without SQLite or filesystem
fixtures. The registered Store workflow/goal suites retain the 9 and 7 native
round-trip, successor, row-codec and corruption cases respectively.

The BOUND-001 Store/Engine characterization in `int_v2dfc005_relational` and
`int_v2dfc006_domain_ports` preserves snapshot-success/later-declaration-error
timing, competing revision priority and distinct cold persisted refusal. Keep
those checks when adapting either runtime declarations or stored admission.

The fixture's schema-valid text declaration has `min_length: 2` and
`max_length: 1`. Snapshot construction succeeds; convenience `prepared` fails,
but explicit empty-workflow Prepared construction succeeds. In `begin`, a stale
revision wins over the retained declaration error; a matching revision exposes
it without changing the aggregate. Persisting the explicit fixture succeeds,
while an independent cold load rejects it as snapshot corruption before Engine
domain fences, clock access or outcome recording. This deliberately malformed
fixture is not a supported Store continuity case.

BOUND-003 adds registered Engine checks for all four shipped presets and both
equivalent source formats through the source and persisted adapters, comparing
complete runtime declarations and pinned identity. A historical unmaterialized
list fixture checks equal runtime defaults with unchanged stored bytes/digest.
Core checks keep goal interpretation independent of deferred memory errors and
preserve local placement queries for absent, null, malformed and contradictory
fields. These pure malformed examples do not authorize Store schema bypass.

BOUND-004's registered Store test
`int_v2dfc005_direct_store::bound004_typed_identity_and_execution_failures_roll_back_and_retry`
checks typed identity refusal, execution refusal after actual graph-row writes,
unchanged binding/sequence and absent failed outcomes, same-key success and
byte-identical replay, identity-before-replay failure, resolved-key mismatch and
changed-digest reuse. The Engine's named 20-route in-transaction revalidation
case covers all nine callback sites and exact public mapping/retry behavior.
`direct_store::transaction::tests::uncertain_outcome_is_only_the_commit_io_error_mapping`
separately owns the COMMIT-only unknown-outcome classification; callback errors
must not acquire that classification.

BOUND-005 keeps the existing `command::tests` and registered `e2e_suite` owners.
New local CLI cases check successful format replacement with original
permissions, canonical no-op inode/mtime/bytes, symlink refusal and offline
fresh-start preview. A private file-module unit case forces rename failure after
staging and checks preserved parked bytes/destination plus temporary cleanup.
Direct/runtime cases check completion's independent 500 ms budget under
maintenance, fresh versus fully fenced stateful preview admission, and stdin
identity/flag/shape error priority. The existing terminal archive case now also
replays with only workspace UUID omitted while no current session exists,
making an accidental status fetch observable. Existing PTY, retirement-before-
stdin, prepared-fence, rendering and plain-start replay cases remain required.

These tests own direct identity, attempt-content overflow, and one-draft atomic
batch behavior:

- [Direct identity](../../crates/podway-engine/tests/int_optim001_direct_identity.rs):
  `optim001_direct_identity_preserves_four_boundaries`,
  `optim001_direct_prepared_identity_keeps_stored_bytes`
- [Engine overflow](../../crates/podway-engine/tests/int_optim001_engine_overflow.rs):
  `optim001_engine_record_many_rejects_attempt_content_without_outcome`
- [Store batch](../../crates/podway-store/tests/int_optim001_batch.rs):
  `optim001_preloaded_attempt_rejects_earlier_overflow_before_later_reduction`,
  `optim001_persisted_store_rejects_attempt_content_without_outcome`
- [Store draft](../../crates/podway-store/tests/int_optim003_draft.rs):
  `optim003_batch_shapes_publish_one_revision_for_mixed_noops`,
  `optim003_sorted_position_failures_publish_nothing`,
  `optim003_supported_forms_follow_sorted_item_order`
- [Direct CLI](../../crates/podway-cli/tests/e2e_v2dfc007_direct_cli.rs):
  `record_stdin_preserves_identity_applies_one_atomic_batch_and_replays_without_preflight`

The preloaded overflow oracle is
`optim001_preloaded_attempt_rejects_earlier_overflow_before_later_reduction`
in the Store batch file above. It seeds a real attempt to within one legal list
addition of the attempt-content ceiling, then submits a batch whose
sorted-earlier item crosses that ceiling while a later item would reduce the
final total. The expected result is the existing content-limit rejection,
unchanged item and graph revisions, no outcome, and the seeded preimage.

These cases own symlinked temporary-parent fixtures under the ambient temp root:

- [Engine races](../../crates/podway-engine/tests/int_v2dfc006_races.rs):
  `competing_child_resolves_a_symlinked_temporary_parent`
- [Store direct fixtures](../../crates/podway-store/tests/int_v2dfc005_direct_store.rs):
  `store_fixture_resolves_a_symlinked_temporary_parent`

Store lock isolation is owned by
`a_real_controller_write_lock_returns_busy_within_deadline_without_effects` in
that Store file. The case runs in its own process and still acquires the
connection gate inside the request. Unrelated tests no longer contend for that
process-global gate during the SQLite busy assertion.

VIEWS projection equivalence is owned by the registered view suite, its
in-module oracles and the Protocol length helper's unit checks:

- Exact serde length ownership:
  [`json_len.rs`](../../crates/podway-protocol/src/json_len.rs) unit checks
  `views002_serialized_len_matches_to_vec_across_value_shapes`,
  `views002_serialized_len_reports_serialization_failures` and
  `views002_serialized_len_agrees_at_history_window_scale`.
- Incremental admission and lookahead policy, in-module:
  `reads003_borrowed_history_measurement_matches_owned_construction_bytes`,
  `reads003_history_window_exact_fit_admits_and_one_byte_over_truncates`,
  `reads003_history_window_swallows_trailing_error_after_a_full_window`,
  `reads003_owned_members_move_rather_than_reserialize`,
  `views001_observation_list_exact_fit_admits_at_the_byte_budget`,
  `views001_observation_list_one_byte_over_the_budget_refuses`,
  `views002_history_window_incremental_total_matches_serialized_windows`,
  `views002_history_window_full_window_lookahead_does_not_render`,
  `views002_history_window_byte_overflow_stops_all_lookahead` and
  `views002_observation_list_incremental_admission_matches_serialized_prefixes`.
- Bounded construction and valued-only summaries, in the
  [view suite](../../crates/podway-engine/tests/int_v2run002_views.rs):
  `views003_mixed_width_text_pages_rejoin_exactly_across_scalar_boundaries`,
  `v2rel002_standard_item_values_stop_before_the_first_whole_value_over_budget`
  and `views001_stale_attempt_summary_skips_interleaved_empty_slots`.
- Shared selection and preview plans, in-module:
  `views004_readback_value_modes_share_slots_and_stripped_output`,
  `views004_preview_plan_and_payload_match_legacy_construction` and
  `views004_indivisible_charge_matches_the_legacy_value_charger`; in the same
  view suite `views004_observation_guidance_equals_stripped_next_for_preview_rich_evidence`,
  `views004_absent_preview_consumes_no_slot_at_the_boundary` and
  `views004_artifact_preview_boundary_fits_then_refuses_by_one_scalar`.
- Public integration and corruption: the view suite's
  `v2run002_fresh_action_projects_closed_compact_standard_verbose_and_next_views`
  and the readback corruption precedence owners in
  [`int_reads001_readback_ports.rs`](../../crates/podway-engine/tests/int_reads001_readback_ports.rs).

PARSE document admission and ownership checks use these registered owners:

- Core [canonical unit tests](../../crates/podway-core/src/canonical.rs), including
  `value_verifier_compares_original_bytes_instead_of_normalized_values`, reject
  duplicate keys, alternate ordering/escaping and unsupported numeric forms while
  preserving integer edges. Store's
  [persistence suite](../../crates/podway-store/tests/int_v2dfc008_persistence.rs)
  `parse001_canonical_execution_admission_preserves_failure_precedence_and_bytes`
  pins byte/depth/syntax/canonical precedence and retained original bytes.
- Store's [direct suite](../../crates/podway-store/tests/int_v2dfc005_direct_store.rs)
  `persisted_outcomes_fail_closed_on_invalid_headers_or_future_sequences` preserves
  independent persisted checks, oversized SQL classification and write-free
  refusal; `success_replay_bypasses_callback_clock_and_new_response_uuid` checks
  the original canonical response. Engine's
  [direct execution suite](../../crates/podway-engine/tests/int_v2dfc006_direct_execution.rs)
  owns original-response lookup without writes, old-attempt replay, clock/ID
  ordering and replay after artifact file loss.
- Protocol's [envelope suite](../../crates/podway-protocol/tests/int_v2dfc006_direct_envelopes.rs)
  `parse003_output_constructor_moves_result_and_warning_trees` checks nested
  result/warning allocation identity, complete value/byte equality, optional
  omission and warning order. Its complete-envelope admission checks and the
  [result suite](../../crates/podway-protocol/tests/int_v2_result_contract.rs)
  retain registered routes, deep/closed failures, identity and delegation
  size/correlations. The existing `json_len.rs` unit owners above verify exact
  escaped-byte accounting and serialization failures.
- Config's [preview suite](../../crates/podway-config/tests/int_v2_procedure_preview.rs)
  `parse004_delegated_facts_survive_admission_and_vet_failures` checks authored
  order, direct/malformed empty facts, delegated success and semantic/vet failure
  across YAML/JSON. Its existing Mermaid-overflow case also retains delegated
  facts without success details. The
  [check suite](../../crates/podway-config/tests/int_v2_procedure_check.rs) owns
  unchanged invalid/unsupported/clean and format/drift/lint ordering behavior.
- Real CLI [authoring checks](../../crates/podway-cli/tests/e2e_v2dfc007_local_cli.rs)
  `v2sdg006_delegated_preview_and_graph_select_v2_without_changing_direct_results`
  preserve direct/delegated generations, semantic/vet failures, malformed fallback
  and preview/graph authored order. The
  [direct CLI suite](../../crates/podway-cli/tests/e2e_v2dfc007_direct_cli.rs)
  retained-command replay/lookup and postcommit-before-stdout SIGKILL cases own
  original-response recovery through the real binary.

STATE snapshot state reuse checks use these registered owners:

- Store's [query-inventory suite](../../crates/podway-store/tests/int_reads001_query_inventory.rs)
  `state002_view_admits_graph_and_dispositions_once` and
  `state002_archived_view_admits_dispositions_once` pin one graph admission
  and one disposition load per current and archived view, with the retained
  dispositions reaching the view.
- Store's [persistence suite](../../crates/podway-store/tests/int_v2dfc008_persistence.rs)
  `state001_disposition_corruption_refuses_view_and_verification` refuses
  corrupt dispositions in the current view and the integrity verifier with
  intact schema checks and unchanged bytes, and its healthy control pins the
  non-delegated empty receipt snapshot;
  `state002_verifier_admits_archived_dispositions_once` pins the verifier's
  per-archived-session disposition admission.
- Store's [negative-boundary suite](../../crates/podway-store/tests/int_reads001_negative_boundaries.rs)
  `state001_disposition_admission_precedes_receipt_and_cursor_checks` pins
  with probe evidence that the disposition admission SQL runs before the
  receipt window, and that its execution precedes the loader's in-memory
  cursor comparison.
- Store's [receipt suite](../../crates/podway-store/tests/int_v2sdg004_receipt.rs)
  `state001_truncated_history_reports_point_readiness_outcome_for_absent_active_receipt`
  pins the truncated-window readiness outcome for an absent active receipt:
  `ReceiptMissing`, no current receipt, a 32-of-33 truncated window;
  `state001_older_receipt_window_corruption_refuses_with_valid_active_receipt`
  pins whole-window history validation before readiness;
  `state001_completing_delegated_attempt_without_admitted_receipt_fails_write_time_guard`
  and
  `state001_prepared_arc_write_path_receipt_snapshot_refuses_unjoinable_active_receipt`
  pin the write-time guard and the prepared-Arc write-path point-query
  boundary;
  `state004_readiness_acquisition_pins_window_derivation_absence_fallback_and_write_path`
  pins zero readiness point queries for a same-snapshot admitted receipt and
  for a complete-window proven absence, one for the truncated-window fallback
  and one on the prepared-Arc write path.
- Store's [memory unit checks](../../crates/podway-store/src/v2_memory.rs)
  `v2_memory::reads_tests::state003_attempt_index_keeps_first_match_for_duplicate_ids`
  pin the reconstruction index's duplicate-id first match and pointer identity
  to the first attempt. Positional matching is excluded by design, not proven
  unreachable by this test: in reachable states cached attempts are a
  same-order prefix of the current trace, so id-keyed and positional lookups
  agree there, and matching by identity is required because prefix identity
  is not established before admission.
- Engine's [domain-port suite](../../crates/podway-engine/tests/int_v2dfc006_domain_ports.rs)
  `state001_reset_dry_run_fence_rejection_precedes_clock_and_receipt_admission`
  and
  `state002_reset_dry_run_reuses_admitted_state_and_keeps_admission_before_fences`
  pin reset dry-run order — graph admission, first fences, clock, then
  receipt/view — including the multi-fault case where a corrupt disposition
  beats a stale revision fence while the clock is never read.
- Engine's [workspace suite](../../crates/podway-engine/tests/int_v2dfc006_workspace.rs)
  `state002_doctor_reuses_verifier_admission_deep_and_non_deep` pins doctor
  reuse of the verifier-seeded admission in both modes with unchanged
  outputs.
- Engine's [direct execution suite](../../crates/podway-engine/tests/int_v2dfc006_direct_execution.rs)
  `state001_changed_items_report_items_changed_through_view_with_in_window_receipt`
  pins the read path's in-window `ItemsChanged`.

These owners sit beside the unchanged count and reload owners in the
[query-inventory suite](../../crates/podway-store/tests/int_reads001_query_inventory.rs)
(`prepared_reload_counts_unchanged_sparse_and_dense_selective_misses`) and the
[reload baseline suite](../../crates/podway-store/tests/int_reads002_baseline.rs),
whose accounting targets the reuse work keeps.

Recorded invariants, not fixtures, cover these guards by design: the reset
`MissingGraphSession` check after graph admission; duplicate cached attempt
IDs, which `WorkflowMemoryStateV2::new` rejects; cached/current attempt-order
divergence; the legacy pre-generation-5 disposition branch, unreachable under
generation-15 direct reads; and a read-path out-of-window active receipt. The
[staged snapshot state reuse](../architecture/system.md#staged-snapshot-state-reuse)
architecture records why that receipt is unreachable on the read path.

## Structural simplification verification

The [VIEWS, PARSE, STATE and ADMIT program](../roadmap/README.md#structural-simplification-program)
targets structural, logical and conceptual duplication. Its registration is
recorded in the roadmap, which owns current status. The following rules guide
the adopted work without asserting that planned reuse is already implemented.

Do not add or run elapsed-time comparison tests, latency percentiles including
p95/p99, throughput benchmarks, performance baselines or performance thresholds
for this program. Existing historical native-scale and release qualification
records remain unchanged and do not become inherited acceptance gates. Existing
functional deadlines remain behavior to preserve; they are not speedup targets.

Define focused success and failure checks before each implementation task. Use
the existing functional owners and add checks only for material behavioral gaps:

- VIEWS: exact byte fit/overflow, escaped JSON, trace bounds and truncation,
  Unicode scalar pages and logical totals, first-three valued-slot counts,
  observation/readback/package equivalence, retained preview-gated failures and
  corruption outside displayed windows.
- PARSE: malformed/deep/noncanonical documents, independent schema/relational
  checks, original canonical/stored bytes, preview fallback and parsed-but-invalid
  delegated facts, exact lookup/replay identity.
- STATE: graph/fence/clock/receipt/disposition error order, changed/unchanged
  memory paths, bounded changed-row fallback, matching receipt reuse and history
  refusal, supported Store continuity without reset or migration.
- ADMIT: native and Store identity, matching/stale hint handling, full Standard
  preflight failures, real competing writers after guard release, file replacement,
  preparation-stage hint discard versus writer reload/refusal, separate Engine
  request IDs,
  read-only missing-SHM/hot-WAL behavior and exact replay without new effects.
  Retention adds no preflight SQL/decode/validation stage; failed optional stamp
  comparison falls back to the original cold-path error order, while mandatory
  admission failures still propagate.

Mutation artifact preparation remains outside read leases and writer transactions.
Deep doctor's existing exclusive maintenance rehash is a distinct path; preserve
its scope rather than imposing mutation preparation rules on it.

Each `*-001` task produces an update to its adopted dossier, rather than a new
permanent catalog. Include a bounded remove/retain ledger for the named target
sites: current owner, duplicated work to remove, validators and failure order to
retain, eligible paths, deliberately split paths and registered correctness
checks. Identify any material characterization gap and cover it before changing
the affected behavior. For an unreachable guard, retain it and record the
admitted invariant instead of inventing an unsupported fixture. Acceptance of
the first task requires this ledger and its verification mapping; the next task
then follows normal roadmap order, without a separate approval ceremony.

Use source-level ownership evidence to show that the named duplicate work has
been removed. A narrow deterministic assertion of parse reuse, bounded display
construction or shared admitted state is appropriate when it proves a material
property. Do not create a general metrics framework, probes or telemetry merely
to count work, and do not bind tests to TODO prose or incidental helper layout.
Fewer operations alone cannot justify reduced validation or changed failure order.
Reuse Store's existing `query_accounting` correctness fixtures and registered
`int_reads001_query_inventory` checks when a changed path affects them. Update
their intended assertions deliberately; do not delete them as historical timing
baselines or create new instrumentation solely to replace them.

Run focused checks during each task and `make test` before sharing future
executable behavior or contract changes. Final epic tasks integrate evidence,
independent review and durable documentation; they do not defer earlier checks.
Accept each epic before starting its successor. CALLS is unadopted and creates no
interface-change verification obligation.

Promote each epic's lasting verification guidance at closeout. ADMIT-004 also
retires or rewrites the program-level planning text in this section and the two
documentation indexes; the completed behavior must not depend on retired TODOs.
VIEWS lasting verification guidance is registered under
[functional regression owners](#functional-regression-owners).

## Optional diagnostics

- The pinned fuzz targets cover canonical JSON, selectors, response decoding,
  Procedure v2 parsing, atomic record input and delegation acceptance. Every
  Cargo fuzz binary runs in the bounded gate; inventory drift fails before
  execution.

Optional diagnostics provide investigation support; they do not replace
`make test` as the development gate or `make dist` as the release gate. See the normative [testing and conformance
specification](../specs/quality/testing-and-conformance.md).

## Native runtime qualification

Run `make test-native-qualification NATIVE_EVIDENCE_PARENT=<absolute-owned-directory>`
on native Apple Silicon with AC power. The existing writable evidence parent must
be agent-owned; set `TMPDIR` to an owned scratch directory as well.
`tools/run_native_qualification.py` owns the executable matrix: Store G1/G2 at
300 and 1,024 nodes, Engine G1/G2 at both sizes, Engine G3, DE01–DE05 and 200
random kills, G5, G6, and Store G9. Each exact ignored test runs once through an
immutable copy of the release-profile Cargo artifact, with bounded process-group
cleanup. Source is captured before building and checked before and after every
case. A failure stops the entry point; remaining cases are incomplete.

The runner refuses `PODWAY_DIRECT_SCALE_COUNT`, `PODWAY_DIRECT_SCALE_SECONDS`,
`PODWAY_ENGINE_SCALE_DIAGNOSTIC_COUNT` and `PODWAY_DIRECT_G9_OBSERVER_DELAY_MS`;
reduced diagnostics or injected cleanup delays cannot qualify the
required matrix. `PODWAY_DIRECT_TEST_EVIDENCE_ROOT`,
`PODWAY_ENGINE_SCALE_EVIDENCE` and `PODWAY_ENGINE_G5_EVIDENCE_ROOT` are assigned
inside its unique evidence directory. Registration and the report bind source,
host, power, exact binary SHA-256 values, selectors and completed cases. Retain
raw child evidence locally; summarize candidate-specific results in the roadmap.
The quality verifier rejects ignored proof locators absent from this matrix and
proof functions outside registered Cargo test sources.

Engine G1/G2 keeps the API-grown active session; G5 separately retains its
archived-grown-L/fresh-active-session fixture. Store uses 78% set, 20% atomic
record and 2% complete, with preparation-time caller fences, fresh-key retries
after stale-fence rejection, all-attempt observers and serial outcome replay.
Its 50 completed history nodes must produce a main database of at least
43,384,832 bytes; G9 caps completions at 31 per writer to keep the same L history
and workload inside the 300-node Procedure. G1
applies at both sizes; G2 thresholds apply only at 300 nodes. Latency samples
exclude each writer's first 20 acknowledged operations and their retries/replays
as warm-up; correctness still checks all 1,000 acknowledged operations. G7/G8 use the
[separate matched CLI method](direct-cli-performance-qualification.md).

G9 measures its drain deadline from stop-marker publication, a conservative
upper bound on time since the final in-flight write. Worker cleanup and report
collection do not reset that clock. A permanent one-second negative fixture
delays observer reports by six seconds and must reject the late observation;
it is failure-path coverage, not the required ten-minute qualification.

Store's `test_support` module is disabled in production builds. Standard
integration Make gates select `podway-store/test-support`; a standalone Store
`int_suite` invocation must pass `--features test-support`. Engine tests enable
the same feature only through their development dependency.
