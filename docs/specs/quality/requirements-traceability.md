# Requirements Traceability

Accepted ADRs, canonical assets, and normative specifications own requirements.
Current source and registered tests establish implementation and proof locations.
The daemon-free current model follows ADR-0038: eight crates, one synchronous
CLI, direct Engine and Store execution, worktree-local state, and no daemon, IPC,
service, global product state, queues, jobs, or runtime modes.

A proof locator is not a passing result. The active roadmap owns task status;
`make test` is the combined development gate, and the adopted dossier owns its
additional native qualification criteria.

## Retained domain and compatibility requirements

| Requirement | Authority | Current executable proof |
| --- | --- | --- |
| Procedure v2 is the only admitted model | ADR-0019; Procedure schema | Config parsing/validation; Engine `int_v2run001_start`, `int_v2dfc006_domain_ports`; CLI retirement and admission tests |
| One active attempt and declared movement; required items, blockers, conditions and goals gate advancement | Domain/lifecycle/transition specs; ADR-0028 | Core model/property tests; Engine `int_v2dfc006_runtime_ports`, `_guard_ports`, `_goal_ports`, `_decision_ports` |
| Prepared is persistent and cursor-free; Begin creates the first attempt atomically | ADR-0021; lifecycle spec | Engine `int_v2run001_start`, `int_v2dfc006_domain_ports`; real CLI start/enter controls |
| Default deletion respects prepared/terminal disposition; force requires confirmation and progress summary | ADR-0021; lifecycle and automation specs | Engine domain/maintenance ports; CLI `e2e_v2dfc007_native_conformance` destructive lifecycle |
| Workspace UUID and lossless canonical Git identity remain bound; replacement witnesses are revalidated | Git resolver contract `ARC-005`, `SEC-002`, `INV-W01`–`INV-W05`; filesystem spec | Git `int_phase3_resolver`, `int_phase3_layout`, `int_phase4_init_layout`; Engine identity/workspace tests; native object-replacement test |
| Confirmed selected-worktree removal preserves Git and unrelated files and converges across interruption | ADR-0030 as superseded by ADR-0038; recovery and filesystem specs | Engine maintenance/exclusion; `D03`, `DE06`; CLI reset/removal recovery and destructive lifecycle |
| One Unicode-scalar scale envelope binds authoring, runtime, persistence and output | ADR-0024; item spec | Core/config scale tests; Engine `int_v2dfc006_readback_ports`, `_scale`; protocol result budget tests |
| At most 1,024 nodes and graph placements; iterative bounded analysis and complete current counters | ADR-0037; Procedure schema | Core multiword/full-chain tests; config 1,024 acceptance and 1,025 rejection; Engine maximum-scale ports |
| Current evidence is read one bounded page at a time without raising the aggregate bound | ADR-0024; JSON/automation specs | Engine readback ports; CLI runtime evidence-continuation test; protocol encoded-budget tests |
| Windowed collections report exact total and truncation state | ADR-0024; JSON response budgets | Engine view/readback/scale tests; Store archive controls; protocol maximum-size tests |
| Check results are structural attempt-local items, without verification or a second evidence ledger | ADR-0025; item and trust specs | Core satisfaction tests; protocol closed results; Engine item/runtime ports; CLI typed recipes |
| Unsatisfied stored check result has one closed first-failure reason; atomic record result remains kind-agnostic | ADR-0033; item/JSON specs | Core reason-order; Engine view/runtime ports; protocol closure; CLI rendering |
| Archive has no inactive-session count cap; listing is newest-first with exact total, window, truncation, oldest summary | ADR-0034; lifecycle/SQLite/JSON specs | Store archive/reconstruction tests; Engine readback/domain ports; CLI archive list and bare terminal archive |
| Typed conditions and guarded options remain bounded and freshness-sensitive | ADR-0028; Procedure/transition specs | Core/config guard tests; Engine guard/decision ports; protocol budgets; shipped reference paths |
| Exactly four distinct v2 presets ship with pinned identity and exact reference-path inventory | Built-in preset spec; canonical preset assets | Preset embedding/digest tests; Engine `int_v2dog003_embedded_presets`, `int_v2dfc006_reference_ports`; executable fixture verifier |
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
| AUT-DIRECT-017: stdin and typed 1 MiB aggregate record bound | CLI stdin; Protocol record decoder; Engine typed boundary | Protocol `int_v2agt004_record_many`; Engine direct execution; CLI record stdin atomic/replay case |
| AUT-DIRECT-018: immutable Procedure snapshot committed with Start; source-independent replay | Config preparation; Engine Start; Store snapshot and outcome | Engine Start/domain/reference ports; CLI source/conflict and start/enter/replay cases |
| AUT-DIRECT-019: closed recovery/v2; read-only or explicitly authorized recovery | Protocol recovery validator; Engine generated recipes | Protocol envelope negatives; Engine errors/maintenance; CLI generated recipes and real recovery |
| AUT-DIRECT-020: one native binary package; truthful disabled-hook development qualification | Release evidence/archive/qualifier and CLI release build | Python packaging self-tests; exact packaged scenario registry; `e2e_v2_native_qualification::release_binary_ignores_every_debug_maintenance_hook` |

The current predecessor registry is empty, and Engine has no predecessor migration
caller. Future declared migration support requires adopted activation and its own
compatibility qualification; framework coverage cannot establish it today.

## Direct Engine ownership and proof

The typed API executes against native Git worktrees and generation-15 Stores.
Engine tests below are registered in `int_suite`. Actual CLI tests separately
prove grammar, stdin, envelopes, deadlines and recovery. Neither layer alone
establishes packaged-byte identity, distribution readiness, or installation.

| Retained requirement | Production implementation | Executable proof |
| --- | --- | --- |
| Validated output/v4, error/v2, recovery/v2 | [Direct API](../../../crates/podway-engine/src/direct_api.rs), [error rendering](../../../crates/podway-engine/src/direct_error.rs), [envelope validator](../../../crates/podway-protocol/src/direct_envelope.rs) | Engine `int_v2dfc006_direct_execution`, `_errors`; protocol direct envelopes |
| Single-cursor items, blockers, conditions, goals, decisions and manual rework | Direct cursor/item/Procedure transaction boundaries | Engine runtime/guard/goal/decision ports |
| All four presets retain declared paths and freshness | Preset preparation, Start/Begin and transaction transitions | [Reference ports](../../../crates/podway-engine/tests/int_v2dfc006_reference_ports.rs); [exact inventory](../../../tests/fixtures/v2/presets/reference-paths.json) |
| Bounded immutable pageable evidence and goal history | Coherent Store snapshot projections | [Readback ports](../../../crates/podway-engine/tests/int_v2dfc006_readback_ports.rs), maximum escaped content and stale tokens |
| Delegation packages, receipts, item currency and archived bindings | [Direct delegation](../../../crates/podway-engine/src/direct_delegation.rs) | [Authored Start routes](../../../crates/podway-engine/tests/int_v2dfc006_delegation_ports.rs); no job lookup/wait |
| Lifecycle identity, reset/replacement, archive/purge and truthful Doctor | Direct session and workspace boundaries | Engine domain/identity/maintenance ports; Doctor is a generic query result, not a named schema family |
| Effect-once recovery across transaction/publication boundaries | Direct Store transactions and marker recovery | [Engine crash qualification](../../../crates/podway-engine/tests/int_v2dfc006_crashes.rs), DE01–DE05; Store DS01–DS05; separate 200 random kills |
| Transaction-current fences reject stale requests, preserve independent item writes and re-evaluate concurrent starts; changed local artifacts never commit | Store prepared-graph selection; Engine command fences and native revalidation | [Preparation races](../../../crates/podway-engine/tests/int_v2dfc006_races.rs), including actual independent-item and plain-start processes; native artifact-stamp races for attach, record, complete and decide; multi-GiB default-timeout and independent-writer-during-hash regressions |
| Native writers and observers preserve serial equivalence | Bounded IMMEDIATE mutation API | [Scale qualification](../../../crates/podway-engine/tests/int_v2dfc006_scale.rs), grown L and 1,024 placements |
| Maintenance excludes callers and reconciles acknowledged old commits | Exclusive lease, intent/operation markers, descriptor-safe removal | [Maintenance](../../../crates/podway-engine/tests/int_v2dfc006_maintenance.rs), DE06; [100-cycle exclusion](../../../crates/podway-engine/tests/int_v2dfc006_exclusion.rs) |

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
| SDG-002 opt-in declaration | [Closed parser](../../../crates/podway-config/src/procedure_v2_wire.rs); canonical schema | [Package tests](../../../crates/podway-config/tests/int_v2sdg003_package.rs); protocol closed-contract negatives |
| SDG-003 execution identity; [adapter mapping](../../implementation-tips/delegation-adapter-contract-review.md), [trust](../operations/security-and-trust.md) | [Result validation](../../../crates/podway-protocol/src/result_contract.rs); [receipt validation](../../../crates/podway-store/src/v2_delegation.rs) | Protocol known answers; [receipt negatives](../../../crates/podway-store/tests/int_v2sdg004_receipt.rs); native provenance/Role/round/disposition matrix |
| SDG-004 package and evidence | [Package builder](../../../crates/podway-config/src/procedure_v2_delegation_package.rs); [read projections](../../../crates/podway-engine/src/v2_read_service.rs) | Package domain separation, attempt/goal/frozen-evidence identity and limits; native recorded-source routes |
| SDG-005 atomic structural acceptance; [transaction spec](../storage/transactions-concurrency-and-idempotency.md) | Direct acceptance and Store receipt/state transaction | Store receipt atomic rollback; [semantics](../../../crates/podway-store/tests/int_v2sdg005_semantics.rs); direct delegation ports |
| SDG-006 currency/correction; [guidance](../../implementation-tips/supervised-delegation.md#finish-recording-before-acceptance) | Accepted item-state digest; completion validation and views | Store no-op/rollback/edit/revert/blocker matrix; Engine `int_v2run002_views`; native correction matrix |
| SDG-007 lifecycle/recovery; [guidance](../../implementation-tips/supervised-delegation.md#recover-without-repeating-external-effects-automatically) | Original success outcomes; retry/rework/goals; historical receipts | Store lifecycle/receipt tests; native lost-response lookup/retry and controlled external effect counts |
| SDG-008 closed current results; [JSON](../interfaces/json-contract.md#direct-cli-generation), [CLI](../interfaces/cli-specification.md#delegation-commands), [errors](../interfaces/errors-and-exit-codes.md#delegation-failure-behavior) | Direct envelopes; route admission; CLI rendering | Protocol direct envelopes; native route tests; Engine archived readback |
| SDG-009 persistence/isolation; [SQLite](../storage/sqlite-model.md) | [v15 DDL](../../../assets/specifications/sqlite-v15.sql); [direct transaction](../../../crates/podway-store/src/direct_store/transaction.rs); receipt constraints | Direct Store reopen/corruption/rollback/replay tests; native legacy refusal and explicit reset |
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
| `AUT-OBS-013`–`017`: generation-2 observation windows and templates | [Observation projection](../../../crates/podway-engine/src/direct_read_service.rs) and the v6 lifecycle builders of the Engine read service | Engine `int_guide009_generation2_observation`; the packaged `e2e_guide009_generation2_lifecycle::generation2_lifecycle_conformance` |
| `AUT-LIF-016`: paired plain-start fences in both generations | [Start fence verification](../../../crates/podway-engine/src/direct_start.rs) inside the start writer transaction | Engine `int_guide008_fenced_plain_start`; the packaged `e2e_guide009_generation2_lifecycle::generation2_lifecycle_conformance` |
| `AUT-ERR-006`–`007`: generation-2 error details | [Citable source admission](../../../crates/podway-core/src/graph_v2.rs); [RELEASED_CONTRACT_MANIFESTS_V1](../../../crates/podway-protocol/src/result_contract.rs); [error rendering](../../../crates/podway-engine/src/direct_error.rs); [CLI human hints](../../../crates/podway-cli/src/command/render.rs) | Core citation carrier tests; Engine `int_guide010_generation2_error_details`; CLI `guide010_*` end-to-end cases |
