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

Unix-socket fixtures must keep the bound path below the 104-byte macOS
`sun_path` capacity. The two socket tests in `podway-git`
(`int_phase3_resolver` and `int_phase4_init_layout`) create their fixture root
through a local helper: it creates the root under the canonical spelling of the
inherited temporary directory — so the measured path is exactly the path the
test binds — keeps that root when the bound socket path still fits, and
otherwise falls back to a short root under the canonical form of `/tmp`,
asserting the fallback fits. The registered regression
`int_guide001_socket_roots::socket_fixtures_pass_with_a_long_tmpdir`
re-executes both socket tests in bounded child processes with a roughly
120-byte TMPDIR, once with a genuinely long directory and once with a long
symlink alias of a short directory, so the fixtures stay valid regardless of
the ambient temporary directory's length or spelling.

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

ADMIT-004 exercises the same supported generation-15 Store with a retained
pre-ADMIT CLI from `a5e77bb14d1958b71518b43fd118f97cf108e5a8` (binary SHA-256
`bede76bac373643930f82d29c82737cfd5778a394ec5da8d78092c4e34383b1a`) and
an ADMIT candidate CLI (SHA-256
`aee4588dc163f787427d1c28706af82ec9a37d62dd4a7fc82cdfef2cf0e008f0`).
The candidate's actual embedded source commit is
`cac21dd2260bd42bfc015a99472e6ae2aec9878a`, with build identity
`sha256:df10d6d88de063b00c71176869de677a72f7aeaf1630a80c6f38c0ae1809257a`;
it was built from the dirty ADMIT-004 test/documentation candidate, not from a
future completion commit. The captured build-source diff SHA-256 is
`caca8d246b9fa7903420076ef3abd35051e0a92f5ccdd1b44b5c3278ca270bf8`.
Later changes to test-only child cleanup and documentation do not change CLI
inputs; executable input hashes remain bound to that artifact.

Two standalone Git fixtures use fresh processes for every CLI step. Run A uses
fully explicit current completion fences; Run B omits only session revision to
exercise automatic preflight and matching context reuse. All 29 comparisons in
each run pass: candidate reads preserve recorded state; initial and old-attempt
replays after continuation preserve original stdout bytes; both lookups retain
the original raw canonical response; reads, replays and lookups change no logical
state or sequence. Only completion advances sequence 4 to 5 and enters
`implement` with a new attempt. The complete writer-contract row, generation,
workspace and canonical snapshot bytes/digest remain unchanged, without reset or
migration. Existing debug-only counters report no hint for fully fenced
completion and one retained/consumed/reused hint for automatic preflight.
This functional evidence supports the owner-accepted ADMIT closeout recorded by
the roadmap; it establishes no release or installed-runtime compatibility.

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

ADMIT request admission and invocation-local reuse checks use these registered owners:

- Engine's [error suite](../../crates/podway-engine/tests/int_v2dfc006_errors.rs)
  `admit001_` and `admit002` cases pin native/binding/UUID order, typed resolver
  versus operation failures, binding and operation witnesses, resolver early
  return, deadlines and write-free missing-SHM/no-sidecar reads. The cold-read
  matrix includes the context-bearing preflight facade. `reads004_equivalence`
  checks full Standard projection/schema/envelope-equivalent success through the
  stateless facade and failure through both stateless and context-bearing facades.
  Context success parity is owned by
  `int_v2dfc006_races::admit003_matching_context_adds_no_preflight_queries_and_is_single_use`;
  `admit003_failed_final_preflight_witness_cannot_publish_or_keep_a_hint`
  prevents publication after a failed witness.
- Engine's [race suite](../../crates/podway-engine/tests/int_v2dfc006_races.rs)
  `admit003_cold_context_releases_all_guards_before_a_competing_commit` and
  `admit003_shared_context_releases_all_guards_before_a_competing_commit` pin
  the preflight-to-preparation competing-writer window. The existing
  `unrelated_item_commit_preserves_exact_item_fences_and_replay` and
  `same_item_race_reports_the_current_revision_without_effects` run stateless
  and context-bearing paths, admitting the competing child only at `BeforeBegin`
  after preparation: unrelated items succeed after reload; stale same-item
  fences refuse without effects. `admit003_context_releases_guards_during_actual_hashing`
  independently pins lease-free hashing with a real competing process.
- The same race suite's `admit003_matching_context_adds_no_preflight_queries_and_is_single_use`,
  seeded-context cleanup, reset dry-run/read neutrality and generation-change
  cases pin retention, consumption, cold fallback and preparation-stage restamping.
  The [identity suite](../../crates/podway-engine/tests/int_v2dfc006_identity_ports.rs)
  `admit003_context_database_replacement_before_begin_keeps_writer_refusal` and
  `admit003_changed_session_and_procedure_snapshot_discard_the_hint_before_writer_fences`
  keep later file refusal distinct from earlier hint discard.
- Store's [connection unit checks](../../crates/podway-store/src/direct_store/connection.rs)
  `direct_store::connection::admit003_tests` pin graph-body identity, missing
  metadata, single consumption, every identity/stamp mismatch and optional
  comparison failure versus mandatory admission/cold-loader failure. The private
  comparison-result injection and non-STRICT decoder fixture are bounded test
  seams, not supported malformed production state. Source inspection must also
  establish no added retention SQL/decode/validation: query accounting excludes
  some qualification SQL, so equal counters alone do not establish that claim.
- Store's [direct suite](../../crates/podway-store/tests/int_v2dfc005_direct_store.rs)
  `admit002_typed_read_callbacks_are_once_only_and_not_store_normalized` preserves
  callback transport. `hot_wal_read_snapshot_is_write_free_with_an_open_writer_process`
  checks ordinary and composed reads against a live writer with unchanged file
  bytes and once-only admission/operation callbacks. Prepared-graph refresh,
  sequence-only replay, independent identity-component refusal and root-isolation
  cases remain alongside the [reload accounting](../../crates/podway-store/tests/int_reads001_query_inventory.rs)
  owner. Receipt readiness and integrity verification retain their independent
  owners listed under STATE above.
- Real CLI [direct checks](../../crates/podway-cli/tests/e2e_v2dfc007_direct_cli.rs)
  `every_legacy_generation_refuses_without_new_files_then_explicitly_resets`,
  `v2dfc015_newer_generations_refuse_every_store_admission_without_new_files` and
  `incompatible_writer_identity_refuses_concurrent_reads_mutations_and_old_replays`
  exercise unsupported-state refusal through reads and automatic preflight.
  `admit003_real_cli_wires_reuse_and_preserves_previews_key_errors_and_later_invocations`
  pins actual wiring, input/key ordering, separate observations and invocation
  lifetime. The [Invocation unit check](../../crates/podway-cli/src/command/direct.rs)
  pins distinct request IDs and one original deadline. Stdin record/delegation,
  retained-command replay/lookup and postcommit-before-stdout SIGKILL owners
  remain required; the PARSE owners above retain old-attempt and file-loss replay.

ADMIT characterization also records invariants instead of fabricating fixtures:
SHA-256 formatting produces a valid digest; typed request IDs and fixed command
names satisfy `DirectMutationRequestV1::new`; reset's later missing-graph guard
follows successful same-snapshot admission. Rebuilding a public Git value loses
native transient evidence and cannot serve as a successful native control.
The stateless cold-preparation corruption case does not require matching-hint
reuse to detect arbitrary unchanged-stamp row changes; that bounded limitation
lives in the [prepared-value contract](../specs/storage/transactions-concurrency-and-idempotency.md#request-owned-prepared-value-reuse).
Existing private-token debug counters support CLI wiring evidence only and are
absent from release builds and ordinary output. Query/decode counts still need
the source inspection described above.

## Structural simplification verification

The [VIEWS, PARSE, STATE and ADMIT program](../roadmap/README.md#structural-simplification-program)
removes duplicated projection construction, document admission, same-snapshot
reconstruction and request admission. Implemented boundaries live in the
[system architecture](../architecture/system.md); the registered
[functional regression owners](#functional-regression-owners) above own their
maintenance checks. All four epics are completed and owner-accepted, as recorded
by the roadmap.

Preserve exact byte fit/overflow, Unicode scalar pages, logical totals,
preview-gated failures and hidden-value corruption refusal; independent document
schema/relational admission and original bytes; graph/fence/clock/receipt order
and bounded reloads; native/Store identity, full Standard preflight, both
competing-writer windows, write-free reads and exact replay. Use
[supported Store continuity](#supported-store-continuity) across these boundaries
without reset or migration. Retention adds no preflight SQL/decode/validation;
optional comparison failure runs the existing cold path once, while mandatory
admission and cold-loader failures retain their errors and precedence.

Do not add or run elapsed-time comparison tests, latency percentiles including
p95/p99, throughput benchmarks, performance baselines or performance thresholds
for this program. Historical native-scale and release qualification records do
not become inherited acceptance gates. Functional deadlines remain behavior to
preserve, not speedup targets.

Use focused success and failure checks before changing these owners and add tests
only for material gaps. Keep unreachable guards with their admitted invariants
rather than fabricating unsupported state. Source-level ownership evidence and
bounded deterministic reuse assertions complement behavioral checks; fewer
operations cannot justify reduced validation or changed failure order. Reuse
Store's existing `query_accounting` and registered query-inventory fixtures;
do not create a general metrics framework, probes or telemetry, bind tests to
retired TODO prose, or delete correctness accounting as a historical benchmark.

Mutation artifact preparation remains outside read leases and writer transactions.
Deep doctor's exclusive maintenance rehash retains its distinct scope. Run focused
checks and the required `make test` development gate for executable or contract
changes, plus `make lint-all` for test changes. Integration review and durable
promotion complement task-local verification. The four accepted dossiers are
retired under the [TODO lifecycle](../todo/README.md#lifecycle); completed behavior
remains independently documented. CALLS remains unadopted and
creates no interface-change verification obligation.

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
