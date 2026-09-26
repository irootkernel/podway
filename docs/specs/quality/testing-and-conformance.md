# Testing and Conformance

Podway tests are organized around Procedure v2 semantic drift, durable mutation
ordering, crash ambiguity, filesystem boundaries, and public contract stability.

## Required layers

1. Pure graph, goal, item, decision, retry, and rework domain tests.
2. Procedure v2 parsing, validation, canonicalization, and preset identity tests.
3. SQLite schema-v13 initialization, migration, runtime-mode binding, rollback,
   and legacy-state rejection.
4. Daemon queue, idempotency, concurrency, restart, and crash-injection tests.
5. IPC framing, bounded decoding, schema validation, and fuzzing.
6. Git/worktree, service, CLI, and real-binary end-to-end tests.

Every valid graph fixture must prove a unique entry, reachable nodes, an acyclic
advance subgraph, and a finite terminal route when declared rework edges are
included. The executable reference-path fixture must retain the accepted clean,
failure, guarded, conditional, paging, goal-outcome, and manual-rework inventory
for all four shipped presets. Every shipped preset must validate and retain its
pinned canonical digest.

The Procedure scale contract accepts exactly 1,024 node definitions and graph
placements, rejects 1,025, exercises graph analysis across the first 64-bit word
and a full-length chain without recursion, and pins every current or reserved
public result ceiling that follows the domain envelope. Frozen historical result
families retain their released 64-counter bounds.

### Shipped reference-path evidence

[`reference-paths.json`](../../../tests/fixtures/v2/presets/reference-paths.json)
is the exact accepted path inventory for the four shipped presets. Each case is
bound to a checked Rust test or isolated real-binary Python dogfood function by
`runtime_proofs`; `tools/verify_quality_contracts.py` rejects a missing case,
extra case, invalid locator, or missing proof symbol.

The proof is intentionally layered:

- `analysis-v2` runtime tests exercise evidence-gap rework, separated source and
  analysis review ownership, conditional finding requirements, all goal outcomes,
  and every declared manual target against the shipped preset bytes.
- `small-change-v2` runtime tests exercise its clean path, review rework, retry,
  and every declared manual target without goal tracking.
- `bug-fix-v2` runtime tests exercise guarded failed verification, review-owned
  rework, all three goal outcomes, and every declared manual target against the
  shipped preset bytes.
- The isolated `sw-dev-v2` contributor-runtime dogfood exercises check-result
  failure, both conditional-required rejection paths, guarded verification
  retry, both phase-owner review routes, complete multi-page list readback,
  changed-value page-token rejection, daemon restart, immutable snapshot
  retention, achieved closeout, and every declared manual target. The generic
  goal outcome state-table test supplies the shared runtime proof for its
  not-achieved and superseded terminal routes; canonical graph validation binds
  those routes to the shipped preset.

The same contract verifier checks the exact inventory alongside graph semantics.
Preset validation, warnings-as-errors lint, embedding and manifest identity,
projection equivalence, guard dominance and satisfiability, option totality, and
the V2SCL/V2GRD response-budget known answers remain separate executable checks
inside `make test`. A focused runtime check is useful while iterating, but only a
successful `make test` establishes the complete development-gate claim.

## Persistence and compatibility

### Supervised delegation continuity baseline

`V2SDG-001` freezes the Direct control before delegated product behavior changes.
The source baseline is `73fd60d78d7117a6d8a3f65ad1cb1faae88b0067`; the
selected previous stable commit is `f66aa6798ca411748459a3b12a6149e3af51a8f6`
(`v0.2.10`). The prior Store generation is 10 and the source baseline is 11.
The earlier release has the 32-inactive-session cap; the source baseline does
not. These revisions identify comparisons, not a tested upgrade or an installed
runtime. The source and release contract-manifest file SHA-256 values are
`b590d08cfb32b9b72f5e1fc875bae3accfe53b2cd4e29dd9d35ce9043c7ea846`
and `daeeee098730448a56147b3882c842078cf54dbc362cae551256737bd1028b7e`.
The corresponding public result-family fixture SHA-256 values are
`7de5f4f024eb795ae7fb73594d4847b833016bea6be9f2737c567b524ae5466e`
and `23423b1060f6a0fedd1097ac458ca8a1cd8cb3c1a2a7e88aa964d514d01a248a`.
Later contract additions must retain old result shapes and stored terminal bytes;
the whole files need not retain these hashes.

The four shipped preset identities are pinned in
`podway-presets::ANALYSIS_V2_SHIPPED_DIGEST`, `BUG_FIX_V2_SHIPPED_DIGEST`,
`SMALL_CHANGE_V2_SHIPPED_DIGEST`, and `SW_DEV_V2_SHIPPED_DIGEST`, with exact
source/digest proof in `int_v2dog003_preset_identity`. The 45 accepted clean,
failure, rework, paging, and goal paths remain bound by
`tests/fixtures/v2/presets/reference-paths.json`. The YAML/JSON canonical
known answer remains bound by `int_v2_procedure_canonical`; the additional
`supervised-delegation-direct-control.yaml` pins a non-delegated digest for
subsequent comparison. The isolated Store control in
`int_v2_graph_start_terminal::v2sdg001_populated_direct_store_preserves_current_and_inactive_snapshots`
reopens a populated current and inactive session and compares their persisted
Procedure bytes. Named-mode, manifest, admission, and unrelated-worktree
controls remain covered by the existing V2DVC and daemon/CLI suites. No fixture
uses an owner's account or active worktree.

The following matrix names the acceptance proof to add in the owning task. Each
affected mutation path must retain a baseline Direct control, either beside the
delegated test or in an existing registered suite cited by the owning task. A
successful fixture receipt proves protocol behavior, not real executor work.

| Case | Required result | Proof owner |
| --- | --- | --- |
| Unknown, mistyped, over-bound, secret-shaped, duplicate, or mismatched contract fields | Reject before mutation; keep old closed branches | `V2SDG-002` schema/decoder negatives |
| Delegation on a decision or skip-enabled action; partial implementation | Reject authoring or runtime admission | `V2SDG-003`, `V2SDG-007` admission tests |
| Package reread or final output recording | Preserve input digest; changed attempt-bound input changes it | `V2SDG-003` package tests |
| Receipt commit, failure, and response loss | One atomic receipt and replayable job result, or no effect | `V2SDG-004`, `V2SDG-008` Store/crash tests |
| Incorrect declaration, package, Role, task, rounds, disposition, item readiness, or blocker state | Reject without changing the cursor or receipt | `V2SDG-005` transaction tests |
| True item no-op; read; unrelated session revision | Preserve accepted receipt applicability | `V2SDG-005`, `V2SDG-008` matrix tests |
| Actual required, optional, or cosmetic item edit; edit then restore text | Preserve receipt as stale; block completion and require a fresh attempt | `V2SDG-005`, `V2SDG-008` matrix tests |
| Failed or rolled-back edit; block/unblock without item change | Preserve receipt applicability; open blocker independently gates completion | `V2SDG-005`, `V2SDG-008` matrix tests |
| Acceptance-key replay after an edit; changed-key duplicate | Return the historic result or reject the duplicate; current view stays stale | `V2SDG-005`, `V2SDG-007`, `V2SDG-008` replay tests |
| Retry, rework, goal revision, cancellation, reset, or archive | Keep old receipt historical; never relabel it to a fresh attempt | `V2SDG-005`, `V2SDG-008` lifecycle tests |
| Prepared, running, terminal, and inactive observations | Use only the declared bounded result generation and truthful readiness | `V2SDG-006` projection tests |
| External and managed Primary restart, late result, unknown worker state | Require exact identity and explicit recovery; no automatic redispatch | `V2SDG-008` controlled-host fixtures |
| Populated v11 upgrade, interruption, newer-state refusal, independent Direct worktree | Preserve bytes and progress in isolated named runtimes | `V2SDG-008` native migration and mixed-worktree tests |

The registered CLI E2E case
`e2e_v2sdg008_qualification::v2sdg008_native_lifecycle_migration_and_host_qualification`
runs `tools/qualify_supervised_delegation.py` against actual debug CLI and daemon
binaries. Its controlled external runtime persists assignment identities, feedback,
and effect counts across disposable Primary processes. Both host labels use the
same Procedure semantics. The native matrix covers receipt provenance and worker
disposition, unchanged reads/no-ops, failed atomic recording, blockers, required
and optional edits, edit/revert, immutable lookup/replay, and fresh retry/rework
and goal attempts. A named-runtime socket relay discards acceptance and completion
responses; daemon SIGKILL/restart and exact job lookup reconcile their outcomes.
A separate SIGKILL after durable acceptance admission reconciles the same job
to one receipt and one session-revision increment. This timing case permits
either pending or already committed execution at the kill boundary; it does
not claim a deterministic interruption inside the receipt transaction.
The fixture also exercises populated v11 migration, SIGKILL during startup
recovery while its migration writer is locked, transaction failure after v12
DDL, newer schema refusal, and backup restore. It compares the preexisting v2
table rows and permits another Direct worktree to finish while migration fails.
It never uses an installed runtime.

This qualification asserts explicit runtime cleanup. Its JSON result records
`live_adapter` and `live_model` as `not_performed`. Assignment/effect counters prove
only the controlled recovery pattern; they do not attest to external artifacts,
model quality, real adapter behavior, or arbitrary same-user execution. Store
transaction failpoints in the registered integration suites also cover
uncommitted receipt and item writes.

The offline adapter review inputs are pinned in
`docs/implementation-tips/delegation-adapter-contract-review.md` at Dolgorae
commit `5d93f43023ebf15e26b9c33b1ad032a7eeff6bbe`. That review closes generic
field/digest meanings and explicitly classifies unavailable real paths; its
source comparison and local known answers do not qualify either adapter.

The store suite creates canonical schema-v13 from an empty database, migrates each
supported empty or Procedure v2-only predecessor transactionally, binds the
workspace singleton to its effective runtime mode, preserves v2 domain and
receipt state, rejects newer schemas, and rejects every nonempty or mixed
Procedure v1 predecessor without mutation. Reset-all recovery is tested against
isolated fixtures rather than user state.

The protocol suite binds every command to its result schema and stable error
details, validates known answers through production decoders, checks the exact
262,144-byte compact-envelope boundary, and fails closed on missing, unknown,
mistyped, mismatched, or cross-command fields. Successful responses use
`podway.output/v3` only.

## Crash and concurrency coverage

The canonical [crash registry](../../../quality/crash-boundaries-v1.json) binds
each current boundary to an executable test and implementation locator. Restart
must converge to no admission, one queued retry, one terminal result, or an exact
idempotent replay; no domain effect may occur twice.

Concurrency tests cover FIFO worktree ordering, parallel independent worktrees,
bounded queues, stale identity fences, same-key replay and conflict, item races,
and deletion of a worktree with pending jobs. Explicit workspace-removal tests
abort child processes after durable marker publication, exact registry removal,
and complete `.podway` deletion; cold replay must converge at every boundary
without deleting the Git worktree. Separate tests cover stale authority and
renamed-ancestor rejection, bounded success and rejection observability, exact
confirmation, response-loss replay, empty and non-empty residuals, paths outside
`.podway`, and reinitialization with a new workspace UUID.

Aquarium managed-service tests validate the external producer description and
artifact manifest, immutable bundle and executable digests, read-only status and
planning, fail-closed activity observation, busy deferral, stale-token rejection,
exact target activation, and exact prior-generation rollback. These tests use an
isolated controller harness and never inspect or mutate the installed production
service or a shared Aquarium runtime.

### Named runtime conformance

Named-runtime coverage binds the complete model rather than testing selectors in
isolation:

- config v1, config v2 without `mode`, and explicit `prod` retain production
  compatibility, while valid named modes and every invalid boundary are covered;
- `--dev` and `--mode dev` select the same complete keyed namespace, and
  production, `dev`, arbitrary named, contributor, and parallel `release-qa`
  roots do not share locks, sockets, registries, logs, metadata, or recovery
  state;
- daemon endpoint, workspace config, registry generation, and SQLite Store mode
  must agree before Store access, so one worktree cannot be admitted by two modes;
- workspace mode planning and apply cover no-change, busy and unavailable
  targets, stale identities, source-to-target combinations, preservation of Git
  and non-runtime workspace content, destructive-boundary interruption, and
  convergent replay;
- Aquarium producer and controller coverage proves pending-only publication,
  exact-generation activation, busy deferral, rollback, recovery debt, and
  production coexistence; and
- contributor and packaged qualification coverage proves purpose-specific
  admission, exact executable identity, parallel private roots, success cleanup,
  and bounded failure preservation without using production or Aquarium state.

Focused tests establish only their named boundary. The integrated development
claim requires the contract and documentation verifiers, all Rust and Python
layers, real-binary E2E coverage, Aquarium controller tests, and the isolated
contributor self-test to pass together through `make test`.

### User runtime reset conformance

Account-runtime reset coverage binds its complete destructive boundary. Isolated
real CLI and daemon tests exercise read-only planning and confirmed live or
offline single-mode and all-mode apply, stable legacy command meanings, exact
selectors and tokens, bounds, partial results, and reservation races. The Store
inside every retained worktree remains byte-owned by that worktree: after reset,
an explicitly started fresh daemon must accept public `workspace repair` and
`status`, restore the registry entry, and expose the same workspace and session
identity.

Service integration tests use only private account roots and the fake launchctl
or native-service harness. The `D04` crash proof aborts a child process after
each durable reset checkpoint and after a live shutdown effect but before its
`stopped` publication; cold exact-token replay must recognize the exact stopped
postcondition and converge. Separate all-mode replay and real-daemon E2E prove
recreated-namespace preservation and retained worktree Store, job, workspace,
and session identity. Store cold-open and daemon worker-recovery tests separately
prove stale-running-to-queued recovery and subsequent claim and terminalization,
so reset conformance does not need to inspect or mutate preserved Store rows. No
reset test may use the installed production LaunchAgent, user-global state,
Aquarium's development runtime, or an active contributor worktree.

## Development and distribution gates

`make test` is the required development gate. It runs preparation and static
contracts, Rust tests, the feature-gated contract verifier, serial real-binary
E2E tests, preset tooling checks, and the isolated contributor runtime self-test.
The E2E inventory covers normal public Procedure v2 admission, restart,
goal-closeout, decision rework, idempotency reconciliation, service operation,
the shipped `sw-dev-v2` preset identity, and confirmed selected-worktree removal
through production dispatch. One isolated destructive-lifecycle scenario also
chains initialization, prepared-session reset, reset-all, missing-registry
repair, workspace removal and reinitialization, purge-logs uninstall, reinstall,
and cold recovery so boundary interactions cannot rely only on independent
tests. Preset-specific integration tests cover the other shipped identities and
their accepted paths.

Post-admission reconstruction integration coverage uses process-isolated renderer
injection with real Store commits and the production dispatcher. It verifies
session-start failure responses, truthful admitted identity, idempotent replay,
a normal reset control, and durable receipts after the dispatcher closes. Independent release QA follows
the [admission failure procedure](../../implementation-tips/release-admission-qa.md)
rather than treating these automated tests as scenario evidence.

`make dist` is the release gate. It reruns `make test`, bounded fuzzing,
release-profile builds, native qualification, packaging, Dolgorae handoff, and
final bundle verification. Optional diagnostics and advisory evidence never
replace either gate.
