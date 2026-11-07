# Testing and Conformance

Podway conformance binds the Procedure v2 domain, direct in-process execution,
worktree-local persistence, filesystem exclusion, and closed public contracts.
A test locator identifies an executable proof surface; its presence does not
establish that the check passed for a particular candidate.

## Required layers

1. Pure graph, goal, item, decision, retry, rework, and delegation domain tests.
2. Procedure parsing, semantic validation, canonicalization, package digests,
   source containment, and shipped preset identity tests.
3. Generation-15 Store initialization, reconstruction, identity classification,
   atomic success outcomes, retention, corruption, and legacy refusal tests.
4. Typed Engine execution, preparation/revalidation races, coherent read
   snapshots, bounded lock contention, maintenance, and crash recovery tests.
5. Output/v4, error/v2, recovery/v2, result-family closure, aggregate decoding
   bounds, manifest/v2 identity, offline verification, and bounded fuzzing.
6. Native Git/worktree and actual single-CLI end-to-end tests, followed by exact
   packaged-binary qualification when distribution readiness is claimed.

Every valid graph proves a unique entry, reachable nodes, an acyclic advance
subgraph, and a finite terminal route with declared rework edges included. Tests
must preserve one current cursor and active attempt, prepared and terminal
states, required-item and blocker gates, conditional requirements, guarded
options, goal outcomes, and immutable stale attempt history.

The scale envelope accepts 1,024 node definitions and graph placements and
rejects 1,025. Graph analysis must cross the first 64-bit word and handle a
full-length chain iteratively. Unicode-scalar item limits, total recorded-content
limits, compact output ceilings, bounded evidence pages, exact collection totals,
and truncation flags remain cross-layer requirements. Maximum-size and escaped
JSON cases must exercise encoded response limits as well as domain counters.

### CLI usage and syntax guidance

The registered `podway-cli` `e2e_suite::e2e_v2dfc007_local_cli` cases
`calls002_missing_required_diagnostics_use_trusted_labels_and_keep_error_contracts`
and `calls002_help_usage_distinguishes_global_options_and_key_owners` exercise the
actual CLI. They cover declaration-order missing arguments, trusted labels,
redacted generic fallback, route and error fields, late JSON, value and literal
positions, retired precedence, local/workspace globals, mutation keys, stdin
ownership, dry runs and read-only lookup. Grammar unit coverage also rejects
untrusted or absent typed diagnostic context. Existing native mutation and
interactive tests remain the owners of admission and automatic key generation.

### Observation and recorded check guidance

Registered `podway-cli` E2E owners bind CALLS-003 guidance to real behavior:

- `e2e_v2dfc007_runtime_cli::calls003_help_and_human_observation_explain_reads_records_and_counters`
  covers help/human guidance and write-free observation.
- `e2e_v2dfc007_direct_cli::calls003_documented_check_result_decodes_and_records_with_observed_fences`
  extracts the existing nested JSON block from `docs/examples/v2-workflow.md`,
  decodes it canonically, fills observed fences, retains actual Procedure-check
  output bytes, hashes them, records the document and verifies original lookup,
  exact replay, separate session/item/init/success counters and unchanged running
  generation-1/2 bodies, templates and actions apart from their schema selector.
- `record_stdin_preserves_identity_applies_one_atomic_batch_and_replays_without_preflight`
  in the same direct CLI owner proves stale session/item refusal and whole-batch
  atomicity with no rejected outcome.
- `disposed_terminal_start_archives_atomically_and_purge_honors_the_archived_revision`
  in the runtime CLI owner checks the archive revision against the actual
  persisted graph/workspace producer and the separate terminal session revision.
- The frozen `e2e_guide006_generation1_baselines` observation cases and existing
  generation-2 lifecycle owners retain machine compatibility coverage.

These checks validate recording mechanics; they do not attest the truth of an
external check result or a caller's described input basis.

### Lifecycle, attribution and lost-output guidance

CALLS-004 uses registered `podway-cli` E2E owners:

- `e2e_v2dfc007_local_cli::calls004_help_explains_lifecycle_attribution_and_original_request_recovery`
  exercises actual help for begin/disposition fences, replacement variants,
  optional actors, citation eligibility/bounds and original-request recovery.
- `e2e_v2dfc007_direct_cli::sigkill_after_commit_before_stdout_preserves_the_original_outcome_and_exact_replay`
  proves committed mutation output was never delivered, then original lookup and
  exact replay with one effect; changed input under the key and a fresh key with
  stale item fences are rejected without another effect.
- `calls004_workspace_retention_miss_preserves_known_effect_and_read_only_lookup`
  in that direct owner evicts a workspace outcome through 100 later public CLI
  repairs, proves generation-1/2 lookup misses while known effects remain, and
  verifies read-only lookup leaves the Store unchanged.
- `calls004_citations_preserve_optional_actor_eligibility_duplicates_and_total_bound`
  in that direct owner covers duplicate/over-bound/ineligible/no-citation-mode
  rejections without outcomes, mixed eligible citations and optional actor use.
- Existing `goal_begin_define_assess_decide_and_completed_revision_preserve_fences_and_replay`,
  `guide010_generation2_citation_details_list_citable_sources`, runtime lifecycle
  owners and `e2e_guide009_generation2_lifecycle::generation2_lifecycle_conformance`
  retain actor, stale-fence, optional grammar, generation and template coverage.
- `e2e_guide009_generation2_lifecycle::agent006_small_change_v2_integrated_caller_workflow_and_call_counts`,
  `agent006_bug_fix_v2_integrated_caller_workflow_and_batch_goal_assessment`, and
  `agent006_integrated_caller_recovery_contention_and_fallback_paths` qualify the
  integrated `--with-observation` and `goal assess-criteria --stdin` caller
  workflows across unchanged `small-change-v2` and `bug-fix-v2` presets, proving
  two-call action recording+completion (`record --stdin --with-observation` +
  `complete --with-observation`), two-call three-criterion batch assessment+decision
  (`goal assess-criteria --stdin --with-observation` + `decide --with-observation`)
  with single session-revision/workspace-sequence advancement and stable goal
  definition revision, observed `check_result` operation-contract binding with
  hashed bounded fixture check facts, generation-1 `--with-observation` embedded
  `podway.observation-result/v6` consumption on known first submission,
  partitioned normal versus excluded call traces, and mandatory
  fresh `observe` recovery after replay, `outcome lookup`, lost stdout,
  `WORKSPACE_BUSY` SQLite write-lock contention, competing-advancement
  stale-fence refusal, or unwrapped/unsupported responses (`AGENT-003`
  owners in `goal_state_v2::tests`, `int_v2_goal_state`,
  `int_v2dfc006_goal_ports`, `e2e_v2dfc007_direct_cli`, and
  `e2e_v2dfc007_runtime_cli` cover `not_achieved`/`not_applicable` outcome and
  zero-citation rules, combined four-citation bounds, and duplicate/already-assessed
  `REQUEST_INVALID` batch rejections; `AGENT-005`
  `agent005_with_observation_lifecycle_replay_lookup_and_conflicts` covers the
  six-route `--with-observation` admission and mode-conflict matrices).

No result above makes Podway an external effect executor or attestor. Lookup is
bounded by retention; exact replay uses the original canonical request and key.

### Shipped reference-path evidence

`e2e_v2dfc007_local_cli::calls005_preset_explain_reports_admitted_goal_tracking_only_in_human_output`
checks all four admitted presets through the real CLI, including enabled and
disabled policy and identical existing generation-1/2 JSON result fields.
`int_v2dog002_bug_fix_preset::bug_fix_v2_is_canonical_clean_and_covers_the_full_defect_workflow`
pins version `4`, the exact reproduction-record prompt and unchanged item bounds,
graph and later verification contract. Preset embedding/digest tests remain the
identity owner. The existing actual CLI
`incompatible_writer_identity_refuses_concurrent_reads_mutations_and_old_replays`
owns fail-closed writer classification; authentic previous-manifest Store checks
also require unchanged refusal for an unaffected preset and a fresh candidate
fixture, without migration, reset or an installed-runtime change.

[The reference-path fixture](../../../tests/fixtures/v2/presets/reference-paths.json)
owns the accepted inventory for the four shipped presets. Every `runtime_proofs`
entry currently resolves to a registered test in
[the direct Engine reference ports](../../../crates/podway-engine/tests/int_v2dfc006_reference_ports.rs).
`tools/verify_quality_contracts.py` checks exact case inventory and proof locators.

- `analysis-v2` covers evidence-gap rework, source and analysis review ownership,
  conditional findings, every goal outcome, and every declared manual target.
- `small-change-v2` covers a goal-free clean path, review rework, retry, and every
  declared manual target.
- `bug-fix-v2` covers guarded verification failure, review rework, every goal
  outcome, and every declared manual target.
- `sw-dev-v2` covers check-result failure, conditional required evidence, guarded
  retry, both review-owner routes, complete multi-page evidence, stale page-token
  rejection, every goal outcome, and every declared manual target.

Canonical preset bytes and shipped digest constants remain independently checked
by preset identity and embedding tests. Graph validity, guard dominance and
satisfiability, option totality, canonical known answers, projection equivalence,
and response-budget known answers are separate checks. The historical contributor
daemon dogfood is retired; it is not a current proof locator.

## Persistence and compatibility

Current Store tests are registered in `podway-store`'s `int_suite`, including
`int_v2dfc005_direct_store`, `_legacy`, `_maintenance`, `_relational`, `_scale`,
`int_v2dfc006_execution_boundary`, and `int_v2dfc008_persistence`. They bind
canonical [generation-15 DDL](../../../assets/specifications/sqlite-v15.sql),
workspace and Git binding, schema object identity, writer-contract identity,
current and archived reconstruction, rollback, and success-only outcome lookup.

Ordinary domain mutations must preserve a single IMMEDIATE transaction containing
their state, validated original success envelope, outcome identity, sequence,
journal entry, and required pruning. Same-key retry returns the original success;
changed request identity conflicts. Rejected or rolled-back work creates no outcome.
Reset-all and removal use separately specified durable marker protocols. Only a
COMMIT I/O failure can report unknown outcome. A missing lookup is not
permission to invent new fences or repeat an external effect.

Tests must cover session outcomes surviving archive purge until session or
workspace reset, bounded workspace outcomes, and bounded journal retention.
Read-only status and lookup tests must prove coherent snapshots and no filesystem
writes, including missing-SHM and uncheckpointed-WAL cases. Logical replay no-op
must not be confused with a physically write-free connection open.

The compiled writer predecessor registry is currently empty. Foreign writer
identities and older daemon Store generations are refused before writable open;
there is no active Engine migration route. Store predecessor-framework tests
prove a future declaration mechanism, not current upgrade support. A later
activated predecessor requires explicit release/open-development adoption and
separate compatibility evidence. Confirmed legacy reset is destructive recovery,
not migration or receipt preservation.

Protocol integration tests bind commands to their result families, stable error
details, closed variants, and production validators. They cover missing, unknown,
mistyped, mismatched, and cross-command fields, canonical numbers, selected-schema
offline closure, drift, and exact compact/aggregate ceilings. The Rust identity
suite also covers Git-ref rerun inputs, manifest preimage identity, source and
packaged verification, version-result/v2, and manifest/v2 without IPC fields.
Output/v3, version-result/v1, manifest/v1, daemon IPC, named modes, queues, jobs,
service controllers, and account-runtime reset are retired current surfaces.
Historical tests and accepted ADRs retain their original meaning.

### Supervised delegation continuity baseline

`V2SDG-001` froze a non-delegated control at source commit
`73fd60d78d7117a6d8a3f65ad1cb1faae88b0067`, compared with previous stable
`f66aa6798ca411748459a3b12a6149e3af51a8f6` (`v0.2.10`). Those Store generations
11 and 10 describe that historical baseline; they do not establish current
daemon-free migration or installed-runtime support. Its historical source/release
manifest file hashes are respectively
`b590d08cfb32b9b72f5e1fc875bae3accfe53b2cd4e29dd9d35ce9043c7ea846` and
`daeeee098730448a56147b3882c842078cf54dbc362cae551256737bd1028b7e`. The
corresponding result-family fixture hashes are
`7de5f4f024eb795ae7fb73594d4847b833016bea6be9f2737c567b524ae5466e` and
`23423b1060f6a0fedd1097ac458ca8a1cd8cb3c1a2a7e88aa964d514d01a248a`.
These identify historical comparisons, not current manifest bytes or a tested
upgrade. The earlier release had the 32-inactive-session cap; the source baseline
did not. Current direct persistence does not import those daemon-era Stores.
The [frozen Direct control](../../../tests/fixtures/v2/compatibility/supervised-delegation-direct-control.yaml),
canonical Procedure known answers, shipped preset digests, and current/inactive
snapshot preservation remain continuity controls.

SDG-001 through SDG-010 retain the following acceptance obligations:

| Boundary | Required result | Current proof surface |
| --- | --- | --- |
| Declaration and result fields | Reject unknown, mistyped, oversized, secret-shaped, duplicate, or mismatched fields; keep variants closed | Config `int_v2sdg003_package`; protocol `int_v2_result_contract` |
| Unsupported placement or partial declaration | Refuse decision/skip-enabled delegation and incomplete admission | Config package tests; Engine `int_v2dfc006_delegation_ports` |
| Package reread and final recording | Preserve attempt-bound input identity; changed frozen input changes the digest | Config package tests; Engine delegation ports |
| Receipt acceptance, rollback, response loss | Commit receipt and original success atomically, or commit neither | Store `int_v2sdg004_receipt`; direct outcome and native recovery tests |
| Declaration, package, Role, task, rounds, disposition, item readiness, blockers | Reject invalid acceptance/completion without cursor or receipt change | Store `int_v2sdg005_semantics`; native fixture |
| Reads, true no-op, unrelated revision, failed edit, block/unblock | Preserve receipt applicability; an open blocker still gates completion | Store semantics; Engine delegation ports |
| Required/optional/cosmetic edit and edit/revert | Keep historic receipt stale and require a fresh attempt | Store semantics; native correction matrix |
| Replay and changed-key duplicate | Return historic original success or reject duplicate; never relabel current currency | Store semantics; CLI stdin/replay and native fixture |
| Retry, rework, goal revision, cancellation, reset, archive/purge | Preserve historical attempt ownership and defined retention | Store semantics; Engine lifecycle/readback ports |
| Prepared, running, terminal, archived observation | Emit bounded truthful current result generations | Engine `int_v2run002_views` and readback ports |
| External/managed Primary restart and uncertain worker state | Require exact identity and explicit recovery; no automatic redispatch | Deterministic native external-host fixture |

The registered native CLI case is
`e2e_v2sdg008_qualification::supervised_delegation_preserves_documented_example_hosts_and_recovery`.
It runs `tools/qualify_supervised_delegation.py --podway <binary>` against the
actual single CLI. Its `podway.direct-delegation-qualification/v2` result checks
the documented example, external and managed host labels, decision evidence,
recovery, and runtime cleanup. Controlled external ledgers preserve assignment
identities, feedback, and effect counts across disposable Primary processes.
Exact outcome lookup/retry reconciles lost acceptance and completion responses.
There is no daemon relay, job reconciliation, named runtime, or legacy Store
migration in this current qualification.

`live_adapter` and `live_model` are `not_performed`. Deterministic receipt and
effect counters prove protocol recovery, not external artifact truth, model
quality, actual adapter behavior, or arbitrary same-user writer settlement.
The [adapter contract review](../../implementation-tips/delegation-adapter-contract-review.md)
is an offline mapping reference, not live adapter qualification.

## Crash and concurrency coverage

The [crash registry](../../../quality/crash-boundaries-v1.json) binds current
implementation and proof locators for `D03`, `DS01`–`DS05`, and `DE01`–`DE06`.
No retired service or daemon boundary counts as current execution evidence.
`D03` exercises interrupted removal at its actual process-exit boundary; the
Store and Engine SIGKILL qualifications prove their separately named boundaries.

Cold lookup and exact retry must converge to either no committed effect followed
by one application, or the original recorded success without a second effect.
Maintenance tests cover durable intent, reset/removal publication, resumable
original identity, final marker retirement, descriptor/ancestor replacement,
preservation of unrelated files, and a fresh UUID after reinitialization.
Concurrent writers and readers must remain coherent across acknowledged
old-generation commits. Native SQLite contention must terminate within the one
command deadline without inventing a failed outcome or changing request identity.
There is no FIFO admission promise or background job recovery.

The dossier's long G1–G6/G9 cases are registered ignored tests and require explicit
execution through `make test-native-qualification` (see the
[native runner and environment](../../implementation-tips/testing.md#native-runtime-qualification)),
including:

- Engine `int_v2dfc006_scale::engine_g1_g2_grown_l_300` and
  `engine_g1_g2_grown_l_1024` for native multi-process serial equivalence and
  grown/maximum scale; maximum-scale latency is informational.
- Engine `int_v2dfc006_crashes::sigkill_at_all_five_engine_boundaries_preserves_effect_once_recovery`,
  `random_sigkills_preserve_success_only_lookup_and_exact_retry`, and
  `engine_write_deadlines_return_busy_without_effect_or_outcome`.
- Engine `int_v2dfc006_races::g6_rejects_500_state_and_500_artifact_preparation_races_without_stale_commits`
  and `int_v2dfc006_exclusion::g5_100_reset_remove_cycles_preserve_all_acknowledged_namespaces_with_four_writers_and_readers`.
- Store `int_v2dfc005_scale::direct_multi_process_g1_g2_l_and_maximum` and
  `direct_ten_minute_g9_wal_bound`.

Repeated deterministic boundary kills, 200 live random kills, 500 state plus 500
artifact races, 100 maintenance cycles, and the ten-minute WAL bound establish
different claims. One cannot substitute for another. G7 full-invocation latency
and G8 open-cost qualification follow the
[direct CLI performance method](../../implementation-tips/direct-cli-performance-qualification.md)
and its baseline identity and exclusions. The E3 resident-behavior audit is
retained in [ADR-0038](../../architecture-decision-records/0038-remove-the-daemon.md)
and [System Architecture](../../architecture/system.md). CLI architecture tests,
the current contract inventory and native qualification support the direct
process model. Test registration and retained logs are not fresh candidate
qualification.

## Development and distribution gates

`make test` is the required development gate. It runs preparation/static
contracts, Rust suites, the feature-gated offline contract verifier, serial
actual-binary E2E, and preset tooling checks. Current E2E covers direct lifecycle,
item and delegation stdin, exact replay/lookup, stale fences, legacy refusal,
writer mismatch, bounded SQLite contention, maintenance recovery, artifact
paging, native worktree identity, early retired-command classification,
generation selection with the executable observation-v6 lifecycle templates
(`e2e_guide009_generation2_lifecycle::generation2_lifecycle_conformance`, also
a registered packaged conformance scenario), and commit-bound `--with-observation`
plus `goal assess-criteria --stdin` caller journeys and recovery
(`e2e_guide009_generation2_lifecycle::agent005_with_observation_lifecycle_replay_lookup_and_conflicts`
and `agent006_*`).

The release-hook case
`e2e_v2_native_qualification::release_binary_ignores_every_debug_maintenance_hook`
requires an actual release-profile binary supplied through
`PODWAY_RELEASE_TEST_BINARY`. The packaged qualifier explicitly selects ignored
cases against its immutable binary snapshot. A development invocation that skips
for missing release bytes is not hook-absence evidence. Debug-hook crash fixtures
and release-hook absence prove separate properties.

`make dist` is the full distribution gate: development checks, release
preparation, bounded fuzzing, one CLI release build, archive creation, native
packaged qualification, Dolgorae handoff, and final bundle verification. Its
packaged scenarios must execute the exact frozen binary bytes in owned temporary
worktrees with controlled environment, explicit worktree targets, digest checks,
and bounded cleanup. No installed runtime or unrelated worktree is a fixture.

`make dist-patch` has its separately confirmed clean same-code baseline and exact
patch-metadata conditions. It does not run omitted full-gate fuzzing or packaged
native qualification. A development-qualification dry run uses truthful dirty
provenance and disabled release hooks but is nonpublishable. Gate success, native
qualification, independent acceptance, commit, release publication, installation,
and activation are distinct claims. Documentation-only changes require source and
authority readback, reference verification, and `git diff --check`; executable
commands or normative behavior changes require the affected executable gates.
