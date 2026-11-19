# Requirements Traceability

Accepted ADRs, canonical assets, and normative specifications own requirements.
Current source and registered tests establish implementation and proof locations.
The daemon-free current model follows ADR-0038: eight crates, one synchronous
CLI, direct Engine and Store execution, worktree-local state, and no daemon, IPC,
service, global product state, queues, jobs, or runtime modes.

A proof locator is not a passing result. The active roadmap owns task status;
`make test` is the combined development gate, and the adopted dossier owns its
additional native qualification criteria.

## Typed boundary ownership

| Requirement | Production owner | Executable proof |
| --- | --- | --- |
| TYPED integrated compatibility: independent source/protocol/persisted/native admission, both contract generations, success-only outcomes and exact replay | Existing owners below; no public, SQLite, writer or manifest contract change | [Twelve-feedback reconciliation and registered cross-boundary suites](../../implementation-tips/testing.md#typed-integrated-qualification); [fresh-process matching-writer Store continuation](../../implementation-tips/testing.md#supported-store-continuity), including exact old-fence replay before and after native completion |
| DF-002: seven stable item wire names have one owner | Core `ItemTypeV1::wire_name`; Config delegation evidence and Engine read projections | Core `procedure::tests::typed001_item_wire_names_preserve_all_seven_discriminators`; Config `int_v2sdg003_package`; Engine `int_v2run002_views` |
| DF-003: package reserves cover admitted source limits without changing admission or canonical identity | Config `procedure_v2_delegation_package`, retaining fixed/evidence/item reserves and the actual-byte check | Config `typed001_package_reserves_cover_source_boundaries`; `int_v2sdg003_package` admission and golden canonical package/digest checks |
| DF-011: shared delegation byte limits preserve decoder, SQL, schema and emission agreement | Core limits; Store receipt re-export; Protocol document/result validation; Engine package/receipt result projections | Protocol `typed001_delegation_schema_bounds_match_shared_domain_limits`; Store `typed001_receipt_ddl_enforces_shared_byte_boundary` and receipt codec suite; Config package and Engine delegation suites |
| DF-005: parsed delegation and persisted structural classification retain independent boundaries | Config `ParsedProcedureV2::placement_is_delegated`; Store admitted `GraphNodeSnapshotV2::has_delegation` | Config first-parse/preview suites; Engine `typed002_decision_named_delegation_rejects_package_and_acceptance_without_effects` with no receipt-table acquisition |
| DF-012/013: typed original acceptance and shared domain predicates preserve closed admission and identity | Protocol `DelegationAcceptanceInputV1` / borrowed `DelegationAcceptanceFieldsV1`; CLI fence extraction; Engine post-fence extraction; independent Store receipt codec and Core Role/round/disposition rules | Protocol `typed002_acceptance_view_preserves_original_document_and_closed_admission`; Store receipt domain/cold-corruption tests; Engine native roundtrip/fence/replay and existing semantic suites |
| DF-016/017: decision-current rejection and actual human CLI success are covered | Engine delegation ports; CLI generic success renderer | Engine `typed002_decision_named_delegation_rejects_package_and_acceptance_without_effects`; CLI `e2e_v2sdg008_qualification` / native qualifier's human package, acceptance and receipt cases |

## Retained domain and compatibility requirements

This table identifies current scoped checks and the historical evidence they
supplement. Generation-15 fixtures establish behavior of the previous writer.

| Requirement | Authority | Executable proof and applicability |
| --- | --- | --- |
| Procedure v3 and Execution Group v1 are the admitted model | ADR-0044; Procedure v3 and Execution Group v1 schemas | Config `int_execution_groups`; Engine scoped lifecycle and fence suites; CLI `e2e_adopt001_scoped_cli` |
| One active attempt and declared movement; required items, blockers, conditions and goals gate advancement | Domain/lifecycle/transition specs; ADR-0028 | Core model/property tests; current scoped Engine `int_group004_fences` and CLI `e2e_adopt001_scoped_cli`; historical v15 Engine `int_v2dfc006_runtime_ports`, `_guard_ports`, `_goal_ports`, `_decision_ports` |
| Prepared is persistent and cursor-free; Begin creates the first attempt atomically | ADR-0021; lifecycle spec | Current CLI start and begin controls; earlier Procedure v2 preparation `int_v2run001_start` and historical v15 Engine `int_v2dfc006_domain_ports` |
| Default deletion respects prepared/terminal disposition; force requires confirmation and progress summary | ADR-0021; lifecycle and automation specs | Current CLI destructive lifecycle in `e2e_v2dfc007_native_conformance`; historical v15 Engine domain and maintenance ports |
| Workspace UUID and lossless canonical Git identity remain bound; replacement witnesses are revalidated | Git resolver contract `ARC-005`, `SEC-002`, `INV-W01`–`INV-W05`; filesystem spec | Git `int_phase3_resolver`, `int_phase3_layout`, `int_phase4_init_layout`; current native CLI identity checks; historical v15 Engine identity/workspace cases, including native object replacement |
| Confirmed selected-worktree removal preserves Git and unrelated files and converges across interruption | ADR-0030 as superseded by ADR-0038; recovery and filesystem specs | Current native CLI reset/removal recovery and destructive lifecycle, including `D03` and `DE06`; historical v15 Engine maintenance/exclusion |
| One Unicode-scalar scale envelope binds authoring, runtime, persistence and output | ADR-0024; item spec | Core/config scale tests and protocol result budgets; historical v15 Engine `int_v2dfc006_readback_ports` and `_scale` |
| At most 1,024 nodes and graph placements; iterative bounded analysis and complete current counters | ADR-0037; Procedure schema | Core multiword/full-chain and config 1,024/1,025 boundary tests; historical v15 Engine maximum-scale ports |
| Current evidence is read one bounded page at a time without raising the aggregate bound | ADR-0024; JSON/automation specs | Current CLI runtime evidence-continuation and protocol encoded budgets; historical v15 Engine readback ports |
| Windowed collections report exact total and truncation state | ADR-0024; JSON response budgets | Current scoped CLI and protocol maximum-size tests; Store archive controls; historical v15 Engine view/readback/scale cases |
| Check results are structural attempt-local items, without verification or a second evidence ledger | ADR-0025; item and trust specs | Core satisfaction tests, protocol closed results and current CLI typed recipes; historical v15 Engine item/runtime ports |
| Unsatisfied stored check result has one closed first-failure reason; atomic record result remains kind-agnostic | ADR-0033; item/JSON specs | Core reason-order, protocol closure and CLI rendering; historical v15 Engine view/runtime ports |
| Archive has no inactive-session count cap; listing is newest-first with exact total, window, truncation, oldest summary | ADR-0034; lifecycle/SQLite/JSON specs | Store archive/reconstruction and current CLI archive controls; historical v15 Engine readback/domain ports |
| Typed conditions and guarded options remain bounded and freshness-sensitive | ADR-0028; Procedure/transition specs | Core/config guards, protocol budgets and current CLI guard cases in `e2e_adopt001_scoped_cli`; historical v15 Engine guard/decision ports and earlier preset reference paths |
| Four successor presets ship with pinned base and default group identities | Built-in preset spec; canonical preset assets | Presets `int_adopt001_presets`; CLI `e2e_adopt001_scoped_cli` and native conformance |
| Public assets, documentation, package identity and executable contracts agree | Manifest/v2; docs precedence | `make architecture-static`, documentation/quality checks, protocol identity and offline verifier tests |

### Historical requirements

The earlier traceability baseline included schema-v5/v10/v11 migrations, daemon
admission, FIFO queues, durable jobs, registry pruning, IPC frames, named runtime
coexistence (`V2DVC`), Aquarium service generation activation, managed-runtime/v3,
and account-runtime reset (`V2FRT`). ADR-0038 and the ordered daemon-free cutover
retire these as current product guarantees. Their accepted ADRs and archived task
records retain their historical meaning; current direct requirements do not
reuse those identifiers with new semantics. Legacy runtime state must remain
untouched unless its original generation's supported owner is separately
authorized to retire it.

## Direct execution requirements

The stable identifiers below are defined in the
[automation contract](../interfaces/automation-client-contract.md#direct-execution-requirements).
They connect normative behavior to current production boundaries and proof.

| Requirement | Production owner | Executable proof |
| --- | --- | --- |
| AUT-DIRECT-001: in-process execution; no resident/global product state | CLI dispatch; Engine typed API; eight-crate adjacency | CLI architecture tests, current contract inventory and native qualification |
| AUT-DIRECT-002: coherent write-free snapshots | Store direct connection and snapshot lease | Store direct Store/maintenance tests; Engine readback/CLI qualification; missing-SHM/WAL controls |
| AUT-DIRECT-003: one IMMEDIATE atomic mutation with validated success outcome | Store direct transaction; Engine mutation boundary | Store `int_v2dfc006_execution_boundary`; Engine direct execution; crash lookup/retry |
| AUT-DIRECT-004: prepare outside transaction, revalidate inside | Engine preparation and native descriptor adapters | Engine direct execution prepared-stamp checks; `int_v2dfc006_races` native rewrite/rename/mid-hash rejection, every-artifact batch and required-set checks, independent writer and multi-GiB timeout proofs; `typed_citation_bounds_reject_before_effects_and_preserve_fences_retry_and_replay` covers typed citation bounds, fence precedence, rejection without effects and exact retry/replay |
| AUT-DIRECT-005: one absolute wait budget across maintenance, connection-gate and SQLite read/write admission; one immediate attempt after expiry | Store lease/connection gate/busy handler; CLI original invocation deadline; Engine request deadline | Store zero-budget/slow-callback cases; Engine admitted reset/public boundaries; CLI zero-budget contention and real-PTY prompt cases |
| AUT-DIRECT-006: caller key and complete original resolved fences | Engine request identity; CLI grammar and stdin decoding | Protocol typed/stdin bounds; Engine errors/identity; CLI retained-command fences and stdin replay |
| AUT-DIRECT-007: exact success replay after UUID/writer classification | Store qualification and outcome lookup; identity resolution before digest/replay selection; execution callback bypass on replay | Store direct Store/legacy tests; CLI concurrent incompatible writer and old replay test |
| AUT-DIRECT-008: success-only retention; only COMMIT I/O yields unknown | Store commit classification; Engine error envelope | Store execution boundary; Engine errors/crashes; CLI post-COMMIT-before-stdout SIGKILL |
| AUT-DIRECT-009: read-only outcome.lookup | Store snapshot/outcome projection; Engine direct query | Store outcome lookup tests; Engine direct execution; CLI exact outcome reconciliation |
| AUT-DIRECT-010: session/workspace outcome retention | Store retention and reset boundaries | Store direct Store/relational/persistence; Engine domain and readback ports |
| AUT-DIRECT-011: early COMMAND_RETIRED syntax classification | CLI parser and retired command table | `e2e_v2dfc007_retirement::retired_surface_returns_valid_guidance_without_reading_open_stdin_or_worktree` |
| AUT-DIRECT-012: legacy Store/config/marker refusal | Config resolver; Engine workspace classification; Store identity | Store legacy suite; Engine workspace/errors; CLI legacy hot-WAL, marker and explicit reset cases |
| AUT-DIRECT-013: writer-contract identity and declared predecessor policy | Store compiled identity/empty predecessor registry | Store direct Store/legacy/predecessor framework tests; CLI incompatible writer test; no active Engine migration route |
| AUT-DIRECT-014: authorized reset/removal and exact recovery | Engine workspace maintenance; Store exclusive lease and durable markers | Engine maintenance/exclusion; D03/DE06; CLI marker SIGKILL and published recipe recovery |
| AUT-DIRECT-015: manifest/v2 and single-CLI identity; authoritative offline verifier | Protocol build identity and release verifier | Protocol `int_build_identity_git_refs`, `int_release_contract_verifier`; CLI version identity; release evidence/archive verification |
| AUT-DIRECT-016: output/v4, error/v2 and closed result families; no exit 3 | Protocol direct envelopes/result contract; Engine error; CLI renderer | Protocol `int_v2dfc006_direct_envelopes`, `int_v2dfc008_current_contract`, `int_v2_result_contract`; Engine errors and actual CLI envelopes |
| AUT-DIRECT-017: stdin and typed 1 MiB / depth-64 aggregate `item.record_many` and `goal.assess_criteria` bounds | CLI stdin (`record --stdin`, `goal assess-criteria --stdin`); Protocol `slice.rs` decoders; Engine `direct_items.rs` and `direct_procedure.rs` typed boundaries | Protocol `agent001_slice_tests`, `int_v2agt004_record_many`; Engine `int_v2dfc006_direct_execution`, `int_v2dfc006_goal_ports`; CLI `e2e_v2dfc007_direct_cli` record and batch assessment stdin/replay cases |
| AUT-DIRECT-018: immutable Procedure snapshot committed with Start; source-independent replay | Config preparation; Engine Start; Store snapshot and outcome | Engine Start/domain/reference ports; CLI source/conflict and start/enter/replay cases |
| AUT-DIRECT-019: closed recovery/v2; read-only or explicitly authorized recovery | Protocol recovery validator; Engine generated recipes | Protocol envelope negatives; Engine errors/maintenance; CLI generated recipes and real recovery |
| AUT-DIRECT-020: one native binary package; truthful disabled-hook development qualification | Release evidence/archive/qualifier and CLI release build | Python packaging self-tests; exact packaged scenario registry; `e2e_v2_native_qualification::release_binary_ignores_every_debug_maintenance_hook` |

The current predecessor registry is empty, and Engine has no predecessor migration
caller. Future declared migration support requires adopted activation and its own
compatibility qualification; framework coverage cannot establish it today.

## Direct Engine ownership and proof

The typed API executes against native Git worktrees and generation-16 Stores.
Engine tests are registered in `int_suite`. Current scoped CLI and Engine
proofs are listed in [the successor ownership table](#retained-domain-and-compatibility-requirements).
The rows below also retain explicitly labelled generation-15 proof.
CLI tests separately cover grammar, stdin, envelopes, deadlines and recovery.
Distribution and installation require their own evidence.

| Retained requirement | Production implementation | Executable proof |
| --- | --- | --- |
| Validated output/v4, error/v2, recovery/v2 | [Direct API](../../../crates/podway-engine/src/direct_api.rs), [error rendering](../../../crates/podway-engine/src/direct_error.rs), [envelope validator](../../../crates/podway-protocol/src/direct_envelope.rs) | Historical v15 Engine `int_v2dfc006_direct_execution`, `_errors`; current protocol envelopes and scoped CLI |
| Single-cursor items, blockers, conditions, goals, decisions and manual rework | Direct cursor/item/Procedure transaction boundaries | Historical v15 Engine runtime, guard, goal and decision ports; current scoped lifecycle and CLI above |
| All four presets retain declared paths and freshness | Preset preparation, Start/Begin and transaction transitions | v3 [actual default-group admission](../../../crates/podway-cli/tests/e2e_adopt001_scoped_cli.rs); historical v15 [reference ports](../../../crates/podway-engine/tests/int_v2dfc006_reference_ports.rs) and [inventory](../../../tests/fixtures/v2/presets/reference-paths.json) |
| DF-022 / AUT-ID-002, AUT-ID-007: shared UUID comparison with preserved ordinary/continuation error order | Engine `workspace::check_workspace_uuid_v1`; ordinary resolver and token-backed `execute_read_v1` retain their original call positions | Historical v15 Engine readback `typed003_workspace_uuid_comparison_preserves_read_order_and_public_details` covers both selectors and token/config/corruption priority on that writer. Current scoped CLI covers cursor/workspace fences. |
| DF-024, DF-027 / AUT-DIRECT-004: pure required-artifact facts, native stamps, no artifact reads under writer | Engine `execution::required_local_artifact_facts_v2`; native preparation/current-fact comparison and Git descriptor stamps; dev-only `OpenedArtifact::read` observer | Historical v15 Engine `typed004_` pure/native writer drift, four-route lower-read/concurrent-process/missing-artifact and sensitivity cases, plus stamp/replacement and caller-fence races. These bound the previous writer's native abstraction. |
| DF-025: one bounded combined-citation result with preserved single/batch admission order | Core `goal_record_v2::validate_criterion_citation_count_v2`; Core result/batch constructors and Engine early single-assessment preparation; distinct Protocol/Engine batch field guard | Core `typed005_citation_count_result_preserves_bounds_and_validation_order`; historical v15 Engine `typed005_single_and_batch_citation_bounds_preserve_fences_details_and_replay` and citation race/batch suites; current Protocol batch decoder and scoped CLI citation cases |
| Bounded immutable pageable evidence and goal history | Coherent Store snapshot projections | Current scoped history and CLI evidence cases; historical v15 [readback ports](../../../crates/podway-engine/tests/int_v2dfc006_readback_ports.rs) for prior limits and paging |
| Delegation packages, receipts, item currency and archived bindings | [Direct delegation](../../../crates/podway-engine/src/direct_delegation.rs) | Current scoped package and receipt CLI proofs; historical v15 [delegation ports](../../../crates/podway-engine/tests/int_v2dfc006_delegation_ports.rs) |
| Lifecycle identity, reset/replacement, archive/purge and truthful Doctor | Direct session and workspace boundaries | Current CLI lifecycle, identity and Doctor cases; historical v15 Engine domain and maintenance ports |
| Effect-once recovery across transaction/publication boundaries | Direct Store transactions and marker recovery | [Current Engine item SIGKILL](../../../crates/podway-engine/tests/int_v2dfc006_crashes.rs), DE01–DE04; D03/DE06 and current reset marker windows exercise the [native CLI](../../../crates/podway-cli/tests/e2e_v2dfc007_direct_cli.rs); historical v15 exact prepublication DE05, Store DS01–DS05 and separate 200 random kills |
| Transaction-current fences reject stale requests, preserve independent item writes and re-evaluate concurrent starts; changed local artifacts never commit | Store prepared-graph selection; Engine command fences and native revalidation | Historical v15 [preparation races](../../../crates/podway-engine/tests/int_v2dfc006_races.rs) for contention and native artifact stamps; current scoped CLI fences |
| Native writers and observers preserve serial equivalence | Bounded IMMEDIATE mutation API | Historical v15 [scale qualification](../../../crates/podway-engine/tests/int_v2dfc006_scale.rs), grown L and 1,024 placements |
| Maintenance excludes callers and reconciles acknowledged old commits | Exclusive lease, intent/operation markers, descriptor-safe removal | Current [native CLI removal](../../../crates/podway-cli/tests/e2e_v2dfc007_direct_cli.rs), DE06; historical v15 [100-cycle exclusion](../../../crates/podway-engine/tests/int_v2dfc006_exclusion.rs) |

The ignored native scale, SIGKILL, deadline, races, exclusion, and WAL cases
require explicit selected execution. G7/G8 use the
[direct CLI performance method](../../implementation-tips/direct-cli-performance-qualification.md).
Their existence or an older log is not qualification for the current candidate.

## Boundary refactoring ownership and proof

These owners implement existing domain, transaction and CLI requirements; the
BOUND refactoring introduces no new public or persisted contract.

| Responsibility | Production owner | Executable proof |
| --- | --- | --- |
| Pure aggregate state, workflow transitions and goal history | Core `graph_v2`, `workflow_v2`, `goal_state_v2`; Store `v2_state`, `v2_memory`, `v2_goal` retain admission/codecs/SQL | Core workflow/goal unit tests; retained Store workflow/goal round trips and relational corruption tests |
| Shared immutable runtime declarations with independent source/stored admission | Core `ProcedureRuntimeV2`; Config `ValidatedProcedureV2::runtime_v2`; Store snapshot reconstruction | Engine `int_v2dfc006_domain_ports` adapter parity and historical-default checks; Core placement/goal independence; Store relational admission |
| Deferred memory failure, competing revision priority and cold persisted refusal | Core snapshot/prepared domain boundaries; Store cold loading; Engine public mapping | BOUND-001 characterization in Store `int_v2dfc005_relational` and Engine `int_v2dfc006_domain_ports` |
| Typed callback failures without side-channel error slots | Store `DirectMutationFailureV1<E>` and one typed runner; all nine Engine sites | Store `bound004_typed_identity_and_execution_failures_roll_back_and_retry`; Engine 20-route revalidation; transaction COMMIT-only uncertainty unit test |
| One fence-preflight owner with separate completion/start observations | CLI `command/preflight.rs`, original `direct::Invocation`; private grammar/authoring/files/start/render modules | CLI original command unit tests and registered E2E: file safety, maintenance/completion budget, stdin precedence, previews, PTY deadlines and archive replay |
| Accepted request-scoped sharing and one unpublished batch draft | Engine borrowed read composition; Core `workflow_v2` immutable slot memo and unpublished draft; Store persistence consumer | Registered `int_optim001_*` and `int_optim003_draft` suites; actual record-stdin CLI replay |

The [testing guide](../../implementation-tips/testing.md#supported-store-continuity)
records the fresh-process supported-Store continuation scenario. The separately
selected [native matrix](../../implementation-tips/direct-cli-performance-qualification.md#native-scale-execution)
owns ignored crash/race/maintenance/scale/WAL qualification. Development, native
qualification and owner acceptance are separate completion facts.

## Supervised delegation ownership and proof

The [supervised-delegation specification](../interfaces/supervised-delegation.md)
owns SDG-001 through SDG-010. The following proof does not establish live adapter
support. Tests are registered in the owning crate's `int_suite`, except the named
CLI `e2e_suite` case.

| Requirement and durable owner | Production implementation | Executable proof |
| --- | --- | --- |
| SDG-001 responsibilities; [system](../../architecture/system.md#supervised-delegation) | [Core delegation values](../../../crates/podway-core/src/procedure_v2/delegation.rs); single-cursor model | [Native fixture](../../../tools/qualify_supervised_delegation.py) identical host-label semantics; Direct delegation routes |
| SDG-002 opt-in declaration | [Scoped parser](../../../crates/podway-config/src/execution_groups/parse.rs); canonical base and group schemas | Config `int_execution_groups`; protocol closed-contract negatives; native delegated fixture |
| SDG-003 execution identity; [adapter mapping](../../implementation-tips/delegation-adapter-contract-review.md), [trust](../operations/security-and-trust.md) | [Result validation](../../../crates/podway-protocol/src/result_contract/successor.rs); [scoped receipt admission](../../../crates/podway-store/src/successor_store/delegation.rs) | Protocol known answers; native role, provenance, round and disposition matrix |
| SDG-004 package and evidence | [Scoped package and read projections](../../../crates/podway-engine/src/successor_execution/native_delegation.rs) | Package domain separation, scope/goal/evidence identity and limits; native recorded-source routes |
| SDG-005 atomic structural acceptance; [transaction spec](../storage/transactions-concurrency-and-idempotency.md) | Direct acceptance and Store receipt/state transaction | Store receipt atomic rollback; [semantics](../../../crates/podway-store/tests/int_v2sdg005_semantics.rs); direct delegation ports |
| SDG-006 currency/correction; [guidance](../../implementation-tips/supervised-delegation.md#finish-recording-before-acceptance) | Accepted item-state digest; completion validation and views | Store no-op/rollback/edit/revert/blocker matrix; Engine `int_v2run002_views`; native correction matrix |
| SDG-007 lifecycle/recovery; [guidance](../../implementation-tips/supervised-delegation.md#recover-without-repeating-external-effects-automatically) | Original success outcomes; retry/rework/goals; historical receipts | Store lifecycle/receipt tests; native lost-response lookup/retry and controlled external effect counts |
| SDG-008 closed current results; [JSON](../interfaces/json-contract.md#scoped-direct-execution), [CLI](../interfaces/cli-specification.md#delegation-commands), [errors](../interfaces/errors-and-exit-codes.md#delegation-failure-behavior) | Direct envelopes; route admission; CLI rendering | Protocol direct envelopes; native route tests; Engine archived readback |
| SDG-009 persistence/isolation; [SQLite](../storage/sqlite-model.md) | [v16 DDL](../../../assets/specifications/sqlite-v16.sql); [direct transaction](../../../crates/podway-store/src/direct_store/transaction.rs); scoped receipt constraints | Direct Store reopen/corruption/rollback/replay tests; native legacy refusal and explicit reset |
| SDG-010 continuity/acceptance; [conformance](testing-and-conformance.md) | Direct single-cursor and atomic worktree-local writes | [Frozen Direct control](../../../tests/fixtures/v2/compatibility/supervised-delegation-direct-control.yaml); preset reference ports; native fixture cleanup; combined development gate and independent acceptance |

The actual CLI native case is
`e2e_v2sdg008_qualification::supervised_delegation_preserves_documented_example_hosts_and_recovery`.
It uses one CLI binary and deterministic external fixtures. It proves neither a
live executor/model nor arbitrary external effect settlement, and it does not
qualify a legacy-to-current release upgrade. Its native lost-response coverage
and deterministic transaction-interior failpoints establish separate boundaries.

## Contract generation ownership and proof

[ADR-0039](../../architecture-decision-records/0039-opt-in-contract-generations.md)
owns the contract-generation decisions; the
[automation contract](../interfaces/automation-client-contract.md#direct-execution-requirements)
defines `AUT-GEN-001`–`003`, `AUT-OBS-013`–`017`, `AUT-LIF-016` and
`AUT-ERR-006`–`007`. Generation 1 is deprecated and remains the default;
[ADR-0040](../../architecture-decision-records/0040-deprecation-in-changelog-changed-section.md)
records the CHANGELOG placement of the deprecation.

| Requirement | Production owner | Executable proof |
| --- | --- | --- |
| `AUT-GEN-001`–`003`: selection, presentation-only scope and declaration statuses | [Direct generation parsing](../../../crates/podway-cli/src/command/direct.rs); [ContractGenerationV1](../../../crates/podway-protocol/src/result_contract.rs); the canonical [`contract-generations.json`](../../../assets/specifications/contract-generations.json) declaration | CLI grammar unit tests; `e2e_guide009_generation2_lifecycle::generation2_lifecycle_conformance` (packaged scenario); `tools/verify_contracts.py --check/--sentinels` |
| `AUT-OBS-013`–`017`: generation-2 observation windows and templates | [Scoped v8 projection and lifecycle builders](../../../crates/podway-engine/src/successor_execution/presentation.rs) | `e2e_adopt001_scoped_cli`; the packaged `e2e_guide009_generation2_lifecycle::generation2_lifecycle_conformance` |
| `AUT-LIF-016`: paired plain-start fences in both generations | [Start fence verification](../../../crates/podway-engine/src/direct_start.rs) inside the start writer transaction | Engine `int_guide008_fenced_plain_start`; the packaged `e2e_guide009_generation2_lifecycle::generation2_lifecycle_conformance` |
| `AUT-ERR-006`–`007`: generation-2 error details | [Citable source admission](../../../crates/podway-core/src/graph_v2.rs); [RELEASED_CONTRACT_MANIFESTS_V1](../../../crates/podway-protocol/src/result_contract.rs); [error rendering](../../../crates/podway-engine/src/direct_error.rs); [CLI human hints](../../../crates/podway-cli/src/command/render.rs) | Core citation carrier tests; historical v15 Engine `int_guide010_generation2_error_details`; current scoped citation and writer-detail CLI cases |
| `AGENT-001`: reserved batch criterion assessment and commit-bound observation contracts ([ADR-0043](../../architecture-decision-records/0043-atomic-assessment-and-commit-bound-observations.md)) | [Slice wire types, staged pre-fence decoder, and canonical identity/digest helpers](../../../crates/podway-protocol/src/slice.rs); [reserved schema registry, partial-batch/wrapper correlations, and lookup headroom validator](../../../crates/podway-protocol/src/result_contract.rs); [`criterion-assessment-many-input-v1.schema.json`](../../../assets/schemas/criterion-assessment-many-input-v1.schema.json); [`criterion-assessment-many-result-v1.schema.json`](../../../assets/schemas/criterion-assessment-many-result-v1.schema.json); [`mutation-observation-result-v1.schema.json`](../../../assets/schemas/mutation-observation-result-v1.schema.json) | Protocol unit tests (`agent001_slice_tests`); `int_v2_result_contract`; `int_v2agt004_record_many`; `int_v2dfc006_direct_envelopes`; `int_v2dfc008_current_contract`; `int_guide007_contract_generations`; `int_release_contract_verifier` |
| `AGENT-002`: shared stdin-fenced route classification and preflight exclusion ([ADR-0043](../../architecture-decision-records/0043-atomic-assessment-and-commit-bound-observations.md)) | [`StdinFencedRoute`](../../../crates/podway-cli/src/command/grammar.rs); [flag conflict and preflight exclusion](../../../crates/podway-cli/src/command/direct.rs) | CLI grammar unit tests (`agent002_*`); `e2e_v2dfc007_direct_cli`; `e2e_v2dfc007_runtime_cli` |
| `AGENT-003` (`AUT-DIRECT-017`, `AUT-ID-008`): atomic batch criterion assessment `podway goal assess-criteria --stdin` ([ADR-0043](../../architecture-decision-records/0043-atomic-assessment-and-commit-bound-observations.md)) | [Core batch assessment & successor admission](../../../crates/podway-core/src/goal_state_v2.rs); [Graph outcome](../../../crates/podway-core/src/graph_v2.rs); [Protocol route & active result schema](../../../crates/podway-protocol/src/result_contract.rs); [Engine `goal.assess_criteria`](../../../crates/podway-engine/src/direct_procedure.rs); [CLI `goal assess-criteria --stdin`](../../../crates/podway-cli/src/command/direct.rs); [`command-catalog.yaml`](../../../assets/specifications/command-catalog.yaml); [`output-v4.schema.json`](../../../assets/schemas/output-v4.schema.json) | Core `goal_state_v2::tests::agent003_*`; Store `int_v2_goal_state` & `int_v2dfc006_execution_boundary`; Engine `int_v2dfc006_goal_ports`; CLI `e2e_v2dfc007_local_cli`, `e2e_v2dfc007_direct_cli`, & `e2e_v2dfc007_runtime_cli` |
| `AGENT-004`: coherent observation composition and `serialized_json_len_v1` budget reuse ([ADR-0043](../../architecture-decision-records/0043-atomic-assessment-and-commit-bound-observations.md)) | [Store `delegation_receipts_for_graph`](../../../crates/podway-store/src/direct_store/transaction.rs); [Engine observation submodule, shared Direct observation finisher, and `SuccessorObservationContextV1`](../../../crates/podway-engine/src/v2_read_service/observation.rs); [Engine read projections](../../../crates/podway-engine/src/v2_read_service.rs); [Direct read service](../../../crates/podway-engine/src/direct_read_service.rs); [Stage 5 preflight byte accounting](../../../crates/podway-engine/src/direct_execution.rs) | Engine `v2_read_service::tests::agent004_*`, `direct_execution::reads004_stage5_tests`, `int_guide006_generation1_baselines`, `int_guide009_generation2_observation`, `int_v2rel002_maximum_next`, `int_v2run002_views`, `int_v2dfc006_errors`; Store `int_v2sdg004_receipt`, `int_v2sdg005_semantics`; Protocol `json_len::tests` |
| `AGENT-005`: commit-bound `--with-observation` mutation response mode across plain `session.start`, `session.begin`, `item.record_many`, `session.complete`, `session.decide`, and `goal.assess_criteria` ([ADR-0043](../../architecture-decision-records/0043-atomic-assessment-and-commit-bound-observations.md)) | [Active result contract and `OutputEnvelopeV4` correlations](../../../crates/podway-protocol/src/result_contract.rs); [`command-catalog.yaml`](../../../assets/specifications/command-catalog.yaml); [`mutation-observation-result-v1.schema.json`](../../../assets/schemas/mutation-observation-result-v1.schema.json); [`output-v4.schema.json`](../../../assets/schemas/output-v4.schema.json); [Direct request contract](../../../crates/podway-engine/src/direct_api.rs); [Engine same-transaction observation wrapper and fault tests](../../../crates/podway-engine/src/direct_mutation.rs); [start, execution, items, and procedure mutation handlers](../../../crates/podway-engine/src/direct_start.rs); [CLI grammar, flags, and completions](../../../crates/podway-cli/src/command/grammar.rs) | Protocol `int_v2_result_contract`, `int_v2dfc006_direct_envelopes`, `int_v2dfc008_current_contract`; Engine `direct_mutation::tests::agent005_*`, `int_guide009_generation2_observation`; CLI `command::tests::agent005_*`, `e2e_guide009_generation2_lifecycle::agent005_with_observation_lifecycle_replay_lookup_and_conflicts` |
| `AGENT-006`: integrated caller workflows for `--with-observation` and `goal assess-criteria --stdin`, conditional first-submission progression, and mandatory `observe` recovery/fallback ([ADR-0043](../../architecture-decision-records/0043-atomic-assessment-and-commit-bound-observations.md)) | [Runtime skill](../../../skills/use-podway/SKILL.md) and references ([`goal.md`](../../../skills/use-podway/references/goal.md), [`recovery.md`](../../../skills/use-podway/references/recovery.md), [`lifecycle.md`](../../../skills/use-podway/references/lifecycle.md), [`delegation.md`](../../../skills/use-podway/references/delegation.md)); [workflow example](../../examples/v2-workflow.md); [user workflows](../product/user-workflows.md); [testing and conformance](testing-and-conformance.md); [testing guide](../../implementation-tips/testing.md) | CLI `e2e_guide009_generation2_lifecycle::agent006_small_change_v3_integrated_caller_workflow_and_call_counts`, `agent006_scoped_goal_assessment_and_closeout`, and `agent006_integrated_caller_recovery_contention_and_fallback_paths`; `e2e_v2dfc007_direct_cli::calls003_documented_check_result_decodes_and_records_with_observed_fences` |
