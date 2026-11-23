# Testing

## TYPED source feedback and revisit conditions

TYPED adopts the twelve findings below. Their original source, feedback,
deferral reason and revisit condition are retained verbatim from the former
candidate. They describe the historical reasons for postponement, not current
implementation or acceptance status. Roadmap promotion closes the deferrals
only; the [roadmap](../roadmap/README.md#typed-typed-boundary-ownership) owns
delivery and acceptance. The delivered ownership and verification response is
recorded below; this detailed register preserves the original history after
retirement of the execution dossier.

| ID | Source | Feedback | Deferral reason | Revisit condition |
|---|---|---|---|---|
| DF-002 | V2SDG-003 maintainability review | The `ItemTypeV1` wire-name mapping is repeated in the delegation package projector and Engine read projection. | Exhaustive Rust matches and current schema checks keep the seven names aligned; consolidating both consumers crosses this task's reviewed scope. | Before adding or renaming an item type, give its wire name one owner and verify both projections and schemas. |
| DF-003 | V2SDG-003 maintainability review | Delegation package static-budget reserves depend on bounded goal and evidence values owned elsewhere. | Current limits fit the reserves and the projection has an actual-byte backstop; exporting all bounds solely for this task would widen the public core API. | Before changing goal, selector, identifier, or item bounds, derive or assert the worst-case reserve against those limits and verify admission-time rejection. |
| DF-005 | V2SDG-006 and V2SDG validation maintainability reviews | Delegated-placement detection is repeated across Config, Engine, and Store; the Store receipt fast path searches canonical placement JSON. PARSE-004 already relocated CLI authoring detection to Config-owned first-parse facts. | Current typed predicates and fail-closed Store checks preserve emitted results; a Direct decision option named `delegation` only causes redundant receipt queries. Consolidation is independent drift prevention. | Before changing placement variants or delegation detection, give parsed-Procedure detection one config-owned API and keep Store detection structural behind its persisted-state boundary, without adding a Store-to-config dependency; verify Direct and delegated result generations together. |
| DF-011 | V2SDG final epic maintainability review | Receipt byte limits are repeated in Store and protocol validation, and delegation result limits are repeated in protocol and Engine projection. | Current constants, schemas, Store DDL, and emitted bounds agree; consolidating ownership is future drift prevention rather than a current correctness fix. | Before changing receipt or delegation result byte limits, give shared bounds a core owner and verify decoder, Store DDL, schema, and emission agreement. |
| DF-012 | V2SDG final epic maintainability review | Store receipt validation repeats core RoleRef, round-count, and after-acceptance disposition predicates. | Current core, Store, and protocol validators agree; sharing their implementation is independent maintainability work. | Before changing role references, round bounds, or after-acceptance dispositions, reuse core constructors or named bounds while preserving closed JSON shapes and verify authoring, receipts, protocol, and schemas together. |
| DF-013 | V2SDG validation maintainability review | Delegation acceptance passes validated JSON between protocol, CLI, Engine, and Store through repeated string-keyed field access. | Closed protocol validation and transaction-time input-to-receipt checks currently reject malformed input; a shared typed carrier would prevent future field drift rather than repair current behavior. | Before changing acceptance-input fields, consolidate their typed extraction at the existing protocol boundary while retaining independent Store validation and the closed wire shape; verify malformed inputs and replay identity. |
| DF-016 | V2SDG validation testing review | Native delegation route tests do not separately assert package and acceptance rejection while a decision placement is current. | Both routes use the same explicit non-action rejection in the package projector; Direct rejection and valid decision-source evidence are covered, and no current result defect is known. This is additional branch coverage. | Before changing active-placement classification or delegation error mapping, add a decision-current route case asserting the error code and reason for package and acceptance, plus unchanged session and attempt state. |
| DF-017 | V2SDG validation testing review | Delegation command success paths lack dedicated human-output smoke cases. | All three use the existing generic v2 success renderer, whose complete result rendering is already tested; there is no missing delegation-specific rendering branch. Dedicated cases are future coverage. | Before changing delegation success rendering or adding schema-specific output, assert successful non-JSON package, acceptance, and receipt output through the CLI, including meaningful result content and absence of error output. |
| DF-022 | V2DFC-013 maintainability review | The expected workspace UUID comparison is duplicated in the workspace resolver and the token-backed read path. | Both paths currently preserve the same typed error and details, and public ordering tests cover their intentional placement. Consolidating them is independent maintenance work with no current behavior defect. | Before changing workspace identity fence rules or their public error context, consolidate the comparison under one owner and verify both token-backed and ordinary reads retain their ordering and typed details. |
| DF-024 | V2DFC-014 maintainability review | Required-artifact preparation in the native Engine adapter maps orchestration errors and traverses workflow-memory slots. | Current required-set, active-attempt and item-revision checks agree with pure preparation and fail closed; relocating their ownership is independent maintenance work. | Before changing required-artifact slot or attempt structure, give the pure item/revision projection one Engine owner and keep native stamp checks at the filesystem boundary; preserve typed errors and replay ordering. |
| DF-025 | V2DFC-014 maintainability review | The Engine's bounded citation precheck repeats the domain's diagnostic text. | The shared numeric bound agrees today; early rejection prevents quadratic duplicate checks and cloning oversized input, so removing it would weaken resource bounds. Diagnostic ownership is independent future work. | Before changing citation bounds or their diagnostics, expose one domain-owned bounded validation result and verify Engine and domain errors together without moving preparation into the writer. |
| DF-027 | V2DFC-014 testing review | The hash observer sees preparation streams but does not instrument every potential future read inside stamp verification. | The current descriptor-stamp resolver reads metadata only, and same-byte rename tests reject a digest-only regression. Broader instrumentation is independent future regression protection, not an unverified current byte-reading path. | Before changing stamp verification or introducing another artifact read there, instrument the actual in-writer file-read boundary and prove all four routes read no artifact bytes while a separate writer can progress during preparation. |

### Verification and durable owners

The mapping records delivered regression owners. Each owning Task promoted
actual behavior and regression ownership here and into architecture,
specifications and requirements traceability. A proof locator alone does not
establish a passing run or Task acceptance.

| Task | Feedback | Regression ownership |
|---|---|---|
| TYPED-001 | DF-002, DF-003, DF-011 | Core `procedure::tests::typed001_item_wire_names_preserve_all_seven_discriminators`; Config `procedure_v2_delegation_package::tests::typed001_package_reserves_cover_source_boundaries` and `int_v2sdg003_package`; Protocol `int_v2_result_contract::typed001_delegation_schema_bounds_match_shared_domain_limits`; Store `int_v2sdg004_receipt::typed001_receipt_ddl_enforces_shared_byte_boundary` and existing receipt codec checks |
| TYPED-002 | DF-005, DF-012, DF-013, DF-016, DF-017 | Protocol `int_v2_result_contract::typed002_acceptance_view_preserves_original_document_and_closed_admission`; Store `int_v2sdg004_receipt` domain and cold corruption checks; Engine `int_v2dfc006_delegation_ports::typed002_decision_named_delegation_rejects_package_and_acceptance_without_effects`; CLI `e2e_v2sdg008_qualification` and its native qualifier's `Fixture.human` assertions |
| TYPED-003 | DF-022 | Engine `int_v2dfc006_readback_ports::decisions::typed003_workspace_uuid_comparison_preserves_read_order_and_public_details`; existing `v2scl004_production_dispatch_serves_and_fences_evidence_read` and workspace resolver/native-binding regressions |
| TYPED-004 | DF-024, DF-027 | Engine `execution::tests::typed004_required_artifact_facts_follow_required_sets_revisions_and_attempts`; `native_execution::tests::typed004_writer_rejects_required_set_revision_and_attempt_drift_before_native_stamps`; registered `int_v2dfc006_races::typed004_` route tests and direct-execution lower-read sensitivity control |
| TYPED-005 | DF-025 | Core `goal_record_v2::tests::typed005_citation_count_result_preserves_bounds_and_validation_order`; Engine `int_v2dfc006_goal_ports::typed005_single_and_batch_citation_bounds_preserve_fences_details_and_replay`; retained typed citation race, batch ordered-error and exact-replay suites; Protocol `int_v2agt004_record_many` batch decoder |
| TYPED-006 | All twelve | Cross-Task compatibility, supported Store continuation, complete development gate and durable-document/reference reconciliation |

TYPED-001 gives wire names and shared delegation bounds one Core owner. Config's
reserve proof projects actual packages with maximum escaped goal/criterion
values, maximum identifiers and role references, selected and full evidence
sets, and maximum revision/size fields. It checks the private source limits at
and beyond their boundaries, each fixed/evidence/item reserve, and the retained
actual-byte backstop. The package identity regression pins canonical bytes and
the domain-separated digest observed before the refactor. Existing package
admission tests retain below/over-limit rejection; Protocol ties schema limits
to Core, and Store executes the canonical DDL at the exact receipt byte ceiling,
one byte over and a multibyte overrun. The DDL test isolates the SQL byte guard;
closed receipt shape and relational admission remain independently verified by
the codec and transaction suites.

TYPED-002 verifies the Config predicate through existing first-parse/preview and
native Direct/delegated projection suites. A Direct decision option named
`delegation` causes no receipt-table queries; both package and acceptance routes
reject with `DELEGATION_NOT_APPLICABLE` / `direct_attempt` and unchanged session,
attempt, sequence and outcomes. The same native case preserves stale-fence
priority over malformed raw input. Protocol tests pin raw-document and canonical
request identity through the immutable carrier, borrowing of execution facts,
closed fields, numeric representation and round/disposition limits. Store tests
retain closed codec admission and cold refusal of invalid Role, round,
disposition and unknown-field corruption independently of Protocol.

The deterministic supervised-delegation qualifier executes actual non-JSON CLI
package, acceptance and receipt successes for both host labels. It asserts
meaningful schema/digest/receipt/summary content, empty stderr and no error
output, then reconciles the acceptance through exact JSON replay. Existing
rollback, retry/conflict, archived receipt and Direct/delegated generation
checks remain in their original suites. This uses the generic renderer and
establishes no live adapter or model qualification.

TYPED-003 pins matching and mismatched UUIDs through actual native ordinary and
continuation reads for both contract generations. It compares the complete public
mismatch details, exit and retryability; verifies wrong-session token refusal,
UUID priority over a bound stale token, and stale-token refusal with a matching
UUID; and preserves session state, sequence and outcomes on healthy reads.
Competing invalid configuration and corrupt persisted identity are rejected before
UUID comparison or token binding. Existing token tests retain wrong-item and
stale caller-attempt order, while native resolver tests retain preliminary retry,
binding and revalidation behavior. The shared pure helper performs no Store open
and changes no replay or generation boundary.

TYPED-004 observes actual descriptor-backed reads at Git `OpenedArtifact::read`,
not only the preparation hash observer. Its nondefault test-support feature uses
scoped thread-local callbacks, restored on return/unwind, without product flags.
A deliberate in-writer native-read control proves sensitivity and normal return
restores the observer. Actual attach, record, complete and decide each produce
positive preparation byte counts and no reads from the conservative `BeforeBegin`
boundary through completion and exact replay after the file is removed. Separate
owned native children commit during lower-level preparation reads on every route;
item-local attach preserves the independent write, while cursor-fenced routes
retain the session-revision conflict and absent rejected outcome.

Pure projection cases cover conditional required-set changes, identical recorded
metadata with changed slot revision, and a new active attempt. A native verifier
test uses those current graphs in admitted Store writer callbacks, rejects each
drift as typed `ArtifactChanged`, checks an unchanged native-stamp control, and
rolls back fixture graph writes without outcomes. These are native comparison
cases, separate from public-route caller-fence races. Missing-artifact cases pin
`ARTIFACT_NOT_FOUND` for attach/record and `ARTIFACT_CHANGED` for complete/decide.
Existing same-byte rename, same-size rewrite, mid-hash, every-stamp and caller-fence
suites retain failure, retry, replay and rollback coverage. Children are bounded
and reaped by the existing fixture helper; temporary fixtures retain RAII cleanup.
The observer covers the current thread and native artifact read abstraction,
including stamp-path regressions using it; it is not universal OS interception,
a benchmark, packaged qualification or evidence that ignored release stress ran.

TYPED-005 checks the shared Core count result at zero, maximum, one over and
`usize::MAX`, preserving the exact domain diagnostic and its priority over
status, duplicate, reason and not-applicable failures at the original Core
stages. Both immutable and raw batch conversions retain bounded success/failure.
The registered native Engine single/batch matrix exercises both generations:
zero and four combined citations succeed; over-combined and individual-array
bounds, duplicates, invalid status/reason/targets and competing stale fences
retain their distinct stages, typed codes/details and unchanged graph/sequence.
Every rejected key has no outcome. Exact successes replay after cold reopen,
and changed input with the same key fails without effects.

The existing typed citation race proves single early count failures remain
behind caller fences and allow corrected-key retry/replay. The batch ordered
error suite retains post-fence semantics and the separate pre-fence per-array
guard; the Protocol decoder suite admits four plus four raw entries for deferred
semantics and refuses five in either array. Batch commit/replay tests retain
atomic rows, request order and key-conflict identity. Source inspection preserves
the early constant-size count call before citation cloning or duplicate scans;
these are bounded functional regressions, not allocation or timing benchmarks.
Public `REQUEST_INVALID` code/empty-detail equality alone does not distinguish
count from status or reason priority; exact Core domain-error assertions and
unchanged Engine call placement establish that internal ordering.
### TYPED integrated qualification

The following reconciliation connects every original revisit condition above
to its delivered ownership response. The Task regression table and detailed
coverage paragraphs retain the executable owners and their proof limits; the
historical feedback rows remain unchanged.

| Feedback / Task | Response to the original revisit condition | Durable production and contract owners |
|---|---|---|
| DF-002 / TYPED-001 | One exhaustive seven-name mapping feeds both projections; schema and projection regressions retain agreement. | Core `ItemTypeV1::wire_name`; [shared names and bounds](../architecture/system.md#shared-item-names-and-delegation-bounds), [Rust ownership](../architecture/rust-codebase.md) |
| DF-003 / TYPED-001 | Existing fixed reserves are checked against actual maximum admitted source values and emitted metadata, with boundary admission and the actual-byte backstop retained. | Config package projector; [shared names and bounds](../architecture/system.md#shared-item-names-and-delegation-bounds) |
| DF-005 / TYPED-002 | Parsed classification has one Config API; Store uses structural admitted facts with no Config dependency. Direct and delegated generations remain covered together. | Config `ParsedProcedureV2::placement_is_delegated`, Store snapshot `has_delegation`; [delegation ownership](../architecture/system.md#supervised-delegation) |
| DF-011 / TYPED-001 | Core owns shared receipt/result byte limits; decoder, canonical DDL, schemas and actual emission retain their unchanged limits. | Core shared bounds; [shared names and bounds](../architecture/system.md#shared-item-names-and-delegation-bounds), [delegation contract](../specs/interfaces/supervised-delegation.md) |
| DF-012 / TYPED-002 | Receipt admission reuses Role, round and disposition predicates while independently checking closed shapes, persisted values and relational bindings. | Core domain constructors and Store receipt codec; [delegation ownership](../architecture/system.md#supervised-delegation) |
| DF-013 / TYPED-002 | Protocol extracts borrowed typed fields from one original admitted document; CLI fences, post-fence Engine semantics, canonical identity and independent Store checks retain their stages. | Protocol `DelegationAcceptanceInputV1` / `DelegationAcceptanceFieldsV1`; [delegation contract](../specs/interfaces/supervised-delegation.md), [transactions](../specs/storage/transactions-concurrency-and-idempotency.md) |
| DF-016 / TYPED-002 | Decision-current package and acceptance rejection assert exact code/reason and unchanged session, attempt, sequence and outcomes. | Engine delegation ports; [delegation contract](../specs/interfaces/supervised-delegation.md) |
| DF-017 / TYPED-002 | Native non-JSON package, acceptance and receipt success assert meaningful content and empty error output; exact JSON replay reconciles acceptance. | CLI generic renderer and deterministic qualifier; [delegation implementation guidance](supervised-delegation.md) |
| DF-022 / TYPED-003 | One UUID comparison constructs the typed mismatch at both original call positions; both generations retain ordinary/token ordering and competing configuration/corruption priority. | Engine `workspace::check_workspace_uuid_v1`; [read path](../architecture/system.md#read-path), [automation contract](../specs/interfaces/automation-client-contract.md) |
| DF-024 / TYPED-004 | One pure projection supplies required item, revision, attempt and metadata; native file/stamp checks remain independent and revalidate under the writer. | Engine `execution::required_local_artifact_facts_v2`; [artifact ownership](../architecture/system.md#required-artifacts-and-native-stamps), [transactions](../specs/storage/transactions-concurrency-and-idempotency.md) |
| DF-025 / TYPED-005 | Core owns the bounded count result and diagnostic; early single preparation, separate batch field admission and post-fence semantics retain their distinct order. | Core `validate_criterion_citation_count_v2`; [citation ownership](../architecture/system.md#bounded-criterion-citations), [automation contract](../specs/interfaces/automation-client-contract.md) |
| DF-027 / TYPED-004 | The actual native lower-read observer has an in-writer sensitivity control; attach, record, complete and decide prove zero writer-admitted reads and separate-writer progress during preparation. | Git `OpenedArtifact::read` test-support hook and Engine native/race suites; [artifact ownership and limits](../architecture/system.md#required-artifacts-and-native-stamps) |

TYPED-006 reuses the passed TYPED-005 development gate for exactly unchanged
executable, test and contract inputs. That `make test` run includes the
registered cross-boundary Core/Config/Protocol/Store/Engine suites, both
contract generations, native CLI supervised-delegation qualification, and
success-only rollback and exact-replay coverage. Reuse does not mean that the
earlier gate executed later TYPED-006 documentation. New documentation receives
its own reference, authority and verifier checks. No executable or normative
contract change is introduced by this reconciliation.

The remaining native gap is covered by the fresh-process
[supported Store continuation](#supported-store-continuity) scenario below.
It independently checks original response domains and exact stored canonical
success, preserving source, protocol, persistence and native admission rather
than replacing them with one shared trust boundary. The [Rust codebase guide](../architecture/rust-codebase.md)
and [System Architecture](../architecture/system.md) already describe each
implemented owner; their audit found no additional ownership change to promote.
[Requirements traceability](../specs/quality/requirements-traceability.md)
connects these delivered boundaries to their regression owners.

The twelve original historical feedback rows remain intact above. The six Task
IDs, completed scope and canonical references remain in the roadmap. Whole-Epic
review and independent validation preceded Master's acceptance of the exact
closeout candidate; the temporary execution dossier and its references are
retired. TYPED-006 qualification supplied the reference/deletion inventory and
did not itself grant either acceptance.

DF-014 retains its [AGENT owner](#agent-source-feedback-and-revisit-conditions).
PARSE-004's first-parse authoring facts remain delivered; TYPED-002 covers only
the remaining parsed-Procedure and independent persisted-state boundaries.
No performance measurement, provider, installed-runtime or release qualification
is introduced by this Epic.

## Verification layers

`make test-prepare` also checks exact Rust and Python toolchain selection and refusal with
isolated fixtures; it does not install or upgrade either interpreter.

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

The `bug-fix-v2` version `4` prompt intentionally changes the manifest. It is
outside the matching-writer continuity scenario below: an authentic
previous-manifest Store must refuse candidate reads, fresh writes and original
request replay with `WORKSPACE_CONTRACT_MISMATCH`, including an unchanged-preset
session, while preserving filesystem bytes, metadata and session state. Create
such owned fixtures with the verified previous CLI, retain original requests and
responses, and separately prove a freshly initialized candidate workflow. Do not
edit a user Store, add a predecessor, migrate or reset to make the check pass.
These isolated compatibility checks establish no release or installation claim.

For a boundary refactor, preserve an accepted prechange CLI and create an owned
standalone Git fixture through that CLI: init, start, begin and record one required
item with explicit fences and a caller key. Retain the original stdout bytes,
status, Store generation, writer-contract row and canonical snapshot bytes/digest.
Use a fresh CLI process for every later observation or mutation.

The candidate must read the same recorded state and replay the original fully
fenced request byte-for-byte. Check the owning response domain independently from
printing/correlation metadata, and require read-only outcome lookup to return the
exact stored canonical success.
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

TYPED-006 exercises this scenario for both contract generations with a retained
prechange stable v0.2.17 CLI from `31cbd110573271ea3376cb07eb961f3dc257a05d`
(SHA-256 `c9923c6c1fee8c31e595c8163863051ebc5d62ae651c3a947b8662cd3aa927a7`)
and the reviewed candidate CLI
(`cfec044864c385fdeb3ac5dd8dc4a39be81dcddbbe7fd5fab04154d1a334abf9`).
The candidate actually embeds source parent
`e441ffe9d95a098044c3497468091ea76f880efd`, built during the dirty
TYPED-005 gate; unchanged executable/test/contract source inputs bind that gate
to accepted commit `8a17cde3f06ddf7b0d3468edbeabaa6bcaed35b6`.
This is not a clean build stamp for that later commit. The retained stable is a
verified local tagged ancestor of the initial Epic base, with matching manifest;
it is not an exact initial-base build or downloaded-archive qualification.

Two owned standalone Git fixtures retain old-CLI init/start/begin and fully
fenced batch recording of required text and a native artifact. Every CLI
observation and mutation uses a new process. Fifty checks cover same recorded
state and status, both byte-identical old-request replays, independent response
domains, read-only exact canonical outcome lookup, key conflict and stale-fence
rejection without outcomes, native completion to the next node/new attempt,
and unchanged generation-15 writer rows and canonical snapshot bytes/digests.
Only explicit current-fenced completion advances sequence 4 to 5; later replay
retains the original old attempt and item fences. Read-only logical inspection
uses quiescent copies and preserves authentic runtime bytes and witnesses.
All children are reaped and exactly the owned fixtures are removed.

The executed harness retained full snapshot rows at all twelve checkpoints.
Its auxiliary display mislabeled the digest column as canonical JSON; a separate
schema-column reconciliation extracts the actual retained canonical bytes and
verifies their SHA-256 against the stored digest at every checkpoint. Original
receipts remain unchanged. Full-row identity and actual replay/continuation
checks are independent of that display defect. This evidence establishes
supported matching-writer continuation, with no reset, migration, release,
installation or installed-runtime qualification.

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

## Native destructive generation replacements

Engine's [maintenance ports suite](../../crates/podway-engine/tests/int_v2dfc006_maintenance_ports.rs)
`destructive_generation_guards_refuse_physical_replacement_at_each_interval`
owns eighteen fresh reset/removal cases: physical newer database, committed newer
WAL and indeterminate committed page-one WAL replacements after initial preflight
before lease acquisition, under the exclusive lease, and before runtime clearing.
`owned_maintenance_retry_preserves_generation_refusal_and_exact_resume` adds six
authentic prepared-marker retry cases. Bounded worker acknowledgments bind atomic
renames and actual dev/inode changes to the selected observer interval. Rejection
preserves the post-injection phase namespace/replacement and publishes no success;
late callbacks retain already-owned markers and allowed earlier config effects.
Restoring supported input in the owned fixture permits exact original-request
recovery without a new clock/UUID or changed response/time. It is not operator
advice for unsupported state.

Store's [classification unit tests](../../crates/podway-store/src/direct_store/classification.rs)
`destructive_generation_probe_retains_captured_database_and_wal_witnesses`
own four held-probe/fresh-control cases. DB/WAL witnesses live inside each probe;
the exclusive lease owns directory/lock witnesses, not an old DB/WAL identity
across probes. Supported replacements after capture refuse as changed witnesses;
stable supported replacements before capture remain valid generation inputs.
No universal earlier inode refusal is implied. Existing marked/unmarked unknown
WAL, scan-budget, admission/fence and confirmation regressions remain required.
Use registered exact integration/library commands through Gaori and `make test`;
no guard placement, public error or persistence protocol changes are introduced.

## Native migration checkpoint faults

Store's private [predecessor unit tests](../../crates/podway-store/src/direct_store/predecessor.rs)
own declared-migration checkpoint durability. Run exact library cases with
`cargo test --locked -p podway-store --lib <case> -- --exact` through Gaori.
`declared_migration_mid_checkpoint_replacement_refuses_changed_witnesses` replaces
the actual database or maintenance-lock inode from an acknowledged native Busy
callback inside `PRAGMA wal_checkpoint(TRUNCATE)`. It requires callback/swap/return
ordering, successful native TRUNCATE, unchanged replacement bytes and post-call
witness refusal before durability sync. Entry/exit-only faults do not prove this
window.

`declared_migration_sync_failure_retries_both_syncs_with_zero_wal` injects an IO
failure after successful native TRUNCATE. Its committed zero-WAL retry observes
successful native database and runtime-directory sync results in order, without
another clock, migration journal entry or changed identity/time/outcome state.
The existing `declared_migration_checkpoint_busy_retains_main_and_retry_converges`
and `empty_production_registry_and_declared_schema_migration_are_atomic` retain
real reader contention, unchanged-main refusal, atomic schema admission and
unknown-writer rejection. All fault state is unit-test-only; production and
`test-support` builds gain no fault API, environment switch or predecessor.
These fixtures establish no released migration route. Normal executable changes
still require `make test` after focused checks.

## CNTRT feedback traceability

The following table preserves the original review observations, deferral reasons
and revisit conditions adopted by CNTRT. Its wording and source line references
are historical: the missing check and incorrect example described here were
addressed by the implemented outcomes below. This table does not describe the
current example or reopen the resolved deferrals.

| ID | Source | Feedback | Deferral reason | Revisit condition |
|---|---|---|---|---|
| DF-004 | V2GSC documentation review | The catalog defines `SOURCE_PROJECTION_BUDGET_EXCEEDED` for canonical JSON exceeding 1,048,576 characters or 1 MiB encoded, while `procedure format` also reports it when the emitted authoring document exceeds the character limit. | The format-emitter behavior predates the 1,024-node correction. Resolving the difference in which document is measured requires a separate decision across the diagnostic catalog, schema, release adapter, and CLI contract rather than widening this correction. | Before changing source-projection diagnostics or the emitted-document bound, assign the format overflow its own contract or explicitly broaden the existing code and update every published consumer. |
| DF-008 | V2SDG-006 testing review | The command catalog's `result_schemas` entries have no exhaustive automated match against registered result schemas. | Current catalog, registry, schemas, and emitted generations agree under the development gate; an exhaustive cross-check is separate contract-tooling work. | Before adding or renaming a result generation, extend contract verification to compare catalog command memberships with registered schemas and add a known-answer check. |
| DF-046 | GUIDE-012 integration review, recorded from the GUIDE-004 r2 observation | The `error-workspace_contract_mismatch` known answer (`tests/fixtures/v2/protocol/current-contracts.json:2112-2141`) carries the shared template message "The write-lock deadline elapsed without a state effect." and a `diagnose_workspace`/`workspace.doctor` recovery, while the catalog (`assets/specifications/error-codes.json:588-592`) and the runtime emit "The Store writer contract is not supported by this binary." with the `diagnose_writer` `version --identity` recipe. | The example is schema-valid (`workspace-contract-mismatch-details/v1` admits both recoveries), five other error examples share the same template message, no verifier rule requires example text to equal the catalog, and automation never depends on human-readable output; correcting a manifest-enrolled known answer forces the manifest, adapter and manifest regeneration for a non-authoritative example field. | Before the fixture examples are next regenerated for a contract change, align this example's message and recovery with the catalog summary and the `diagnose_writer` recipe, then run the full contract checks. |

### Implemented outcomes and retained boundaries

| Feedback | Current outcome and authoritative owner | Regression owner |
|---|---|---|
| DF-004 | The [diagnostic catalog](../../assets/specifications/authoring-diagnostics.json), [procedure commands](../specs/interfaces/cli-specification.md#procedure-commands) and [authoring diagnostics](../specs/interfaces/errors-and-exit-codes.md#typed-conditions-and-authoring-diagnostics) distinguish complete canonical JSON's scalar and UTF-8 byte limits from emitted canonical authoring text's scalar-only limit. Each limit remains 1,048,576; equality is admitted by that check and independent envelope budgets remain in force. Existing code, severity, runtime text and wire shape are unchanged. | Config's [canonical suite](../../crates/podway-config/tests/int_v2_procedure_canonical.rs) and [formatter unit tests](../../crates/podway-config/src/procedure_v2_format.rs), plus the isolated [CLI format-write failure](../../crates/podway-cli/tests/e2e_v2dfc007_local_cli.rs), own exact boundaries, multibyte input, post-admission emission overflow and original-byte preservation. |
| DF-008 | The [JSON contract](../specs/interfaces/json-contract.md#direct-cli-generation) defines exhaustive equality of every `(command, closed result family)` pair across the [command catalog](../../assets/specifications/command-catalog.yaml), public registration tables and Output v4 selections. Generic-object branches and non-command documents contribute no pair; generic command inventory agreement is not asserted. Deprecated generation 1 and supported generation 2 retain their existing selection rules. | Protocol's [result-contract suite](../../crates/podway-protocol/tests/int_v2_result_contract.rs) owns the real three-way comparison and shared-helper negative controls; the [generation suite](../../crates/podway-protocol/tests/int_guide007_contract_generations.rs) retains generation-declaration coverage. |
| DF-046 | Only `error-workspace_contract_mismatch` in the [current known answers](../../tests/fixtures/v2/protocol/current-contracts.json) now uses the [error catalog](../../assets/specifications/error-codes.json) summary and existing read-only `diagnose_writer` / `version --identity` [recovery recipe](../specs/interfaces/errors-and-exit-codes.md#structured-recovery-recipes). Its error/detail families, remaining fields and the distinct generation-2 detail family are retained. Other template examples were not changed. | Protocol's `cntrt002_writer_mismatch_example_matches_catalog_and_writer_recovery` in the [result-contract suite](../../crates/podway-protocol/tests/int_v2_result_contract.rs) checks the selected catalog message, recipe and closed schemas. This scoped assertion does not make every human-readable error message a compatibility field. |

Canonical asset changes use the [manifest/adapter generation workflow](release.md)
and preserve exact-manifest writer refusal. CNTRT declares no predecessor, Store
conversion or transparent migration; frozen GUIDE compatibility baselines remain
unchanged. The independent future YAML-source pipeline regression remains
[DF-052](../deferred-feedback/README.md#open); current direct boundary,
canonical-admission/JSON and real-CLI coverage does not claim that additional
YAML-specific regression executes.

### Integrated public contract verification

Run the named [functional regression owners](#functional-regression-owners) first,
including both membership tests, canonical scalar/byte and emitted-text bounds,
the selected writer example and retained generation tests. Use the registered
`int_suite` paths and exact E2E selection described above; verify that the intended
tests actually execute. The local emitter boundary is a library unit test.

Core's [canonical unit tests](../../crates/podway-core/src/canonical.rs),
Protocol's [build-identity suite](../../crates/podway-protocol/tests/int_build_identity_git_refs.rs),
the [shared known-answer fixture](../../tests/fixtures/contract/canonicalization-v1.json)
and `./tools/python tools/contract_manifest.py --self-test` own the
`[-9223372036854775808, 18446744073709551615]` integer boundary, boolean
preservation and recursive float/out-of-range rejection. Protocol's
[`int_v2agt004_record_many`](../../crates/podway-protocol/tests/int_v2agt004_record_many.rs)
and Config's [`int_v2_procedure_canonical`](../../crates/podway-config/tests/int_v2_procedure_canonical.rs)
verify that signed `i64` item, recorded-value and predicate bounds remain
separate from unsigned `u64` revision and size fields.
Run the Python `--self-test` negative controls separately; `make test` checks the
known-answer fixture through the manifest check but does not run these sentinels.

Protocol's [`int_v2dfc006_direct_envelopes`](../../crates/podway-protocol/tests/int_v2dfc006_direct_envelopes.rs)
and [`int_v2_result_contract`](../../crates/podway-protocol/tests/int_v2_result_contract.rs),
Engine's `direct_mutation::tests::agent005_*` unit tests and
[`int_guide009_generation2_observation`](../../crates/podway-engine/tests/int_guide009_generation2_observation.rs),
and CLI's [`e2e_guide009_generation2_lifecycle`](../../crates/podway-cli/tests/e2e_guide009_generation2_lifecycle.rs)
own the `AGENT-005` commit-bound `--with-observation` wrapper contract,
worst-case `outcome.lookup` headroom and real-transaction rollback across all
six routes, bidirectional `IDEMPOTENCY_KEY_REUSED` conflicts, and route grammar
and completion parity.

Through Gaori with the pinned repository toolchain, run `make architecture`,
`make contract-manifest` and `make test-int`. Architecture's static prerequisites
include documentation/link checks, contract/schema integrity and the prepared
adapter self-test; `./tools/python tools/verify_contracts.py --sentinels` supplements
these checks. Then run the configured `full` command (`make test`) on the frozen
final candidate. Inspect repository status and the complete diff after checks.
Focused results and compilation do not replace the complete development gate.
These checks establish development evidence, without release, packaged
qualification, installed-runtime or external-consumer coverage.

The roadmap alone owns member and Epic status. Integrated qualification and a
prepared closeout do not constitute whole-Epic acceptance: the normal CNTRT-003
completion commit, independent whole-Epic validation and explicit owner acceptance
precede final reference relinking, adopted-index removal and dossier retirement.

## MAINT source feedback and revisit conditions

These twelve historical records preserve their original source, feedback,
deferral rationale and revisit condition. Promotion closes the original deferral;
the roadmap owns task implementation and acceptance, with whole-Epic acceptance
remaining a separate boundary. Current qualifications and lasting owners follow
the unchanged historical records.

| ID | Source | Feedback | Deferral reason | Revisit condition |
|---|---|---|---|---|
| DF-029 | V2DFC-015 maintainability review | Workspace UUID and durable-selector fences are enumerated in both the general guard and invalid-Store recovery admission. | Both current predicates include the same two fences, and native copied-Store reset/removal regressions refuse durable selectors without effects. Consolidation is future drift prevention, not current recovery correctness. | Before adding or changing an identity fence, give the complete fence set one Engine owner and verify both valid-Store guards and invalid-Store recovery admission. |
| DF-030 | V2DFC-015 security review | The existing interactive removal prompt renders control characters in a worktree path without escaping them. | Exact-byte path confirmation binds deletion to the descriptor-validated target and cannot redirect it; clearer display is independent prompt hardening. | Before changing removal confirmation rendering, escape control characters visibly while preserving exact target identity and test unusual native path bytes. |
| DF-031 | V2DFC-015 security review | An unmarked checksum-valid WAL with a committed page-one image lacking a SQLite header is refused as unreadable, without destructive recovery advice. | That image does not provide a trustworthy effective generation. Markers remain diagnostically visible but do not authorize deletion of an indeterminate generation; destructive admission refuses marked and unmarked unknown committed page-one images. Defining recovery for those images requires a separate policy, rather than treating a possibly unsupported Store as current. | Before broadening unreadable-State recovery, define a generation-safe policy for unmarked corrupt WAL images and prove that newer Stores cannot be deleted or opened. |
| DF-032 | V2DFC-015 testing review | Checkpoint tests do not inject a database or lease replacement during the native checkpoint, or an fsync failure immediately after TRUNCATE. | Native external-reader Busy, unchanged main bytes, committed retry, no duplicate journal or clock, and exclusive failed-read witness tests cover current acceptance. Pre/post descriptor guards and zero-WAL retry sync preserve the suffix; precise mid-call fault injection is independent regression hardening. | Before changing checkpoint or durability suffix logic, add deterministic private fault injection at these boundaries and verify changed witnesses fail closed and post-TRUNCATE retry completes sync without duplicate effects. |
| DF-033 | V2DFC-015 ordinal-two maintainability review | Classification precedence after invalid configuration is invoked in both workspace init and existing-workspace resolution. | Both refuse a known newer Store; their Maintenance-arm difference preserves each command's existing admission policy. There is no current divergent newer-generation behavior. | Before adding another precedence variant or caller, consolidate the resolver-owned probe while preserving caller-specific Maintenance policy and test init/status/reset/removal on invalid configuration. |
| DF-034 | V2DFC-015 ordinal-two maintainability review | Invalid-current read errors are classified by Store while the Engine separately enumerates errors eligible for unfenced recovery. | Narrow corruption mapping, generation admission, identity fences and native invalid-data regressions satisfy current behavior; a missed future error category would fail closed. | Before adding another recovery-eligible error kind, expose the Store-owned corruption classification explicitly and verify that IO, Busy, changed witnesses and unknown generations remain excluded. |
| DF-036 | V2DFC-015 ordinal-three maintainability review | CLI confirmation and Engine recovery separately identify an unfenced request. | The CLI supplies no durable selector identity, so both predicates agree today and the native prompt tests enforce current behavior. | Before adding a CLI identity fence or a durable selector, give the complete fence predicate one Engine owner and verify both confirmation and invalid-Store recovery. |
| DF-039 | V2DFC-015 ordinal-three product review | Interactive removal requests exact-path confirmation before reporting that a newer Store cannot be removed. | Newer state is refused before effects with compatible-version guidance; avoiding the unnecessary prompt is independent workflow refinement. | Before changing destructive confirmation order, consider an Engine-owned read-only compatibility precheck while retaining authoritative post-confirmation admission and unchanged-file refusal. |
| DF-040 | V2DFC-015 ordinal-three testing review | Deterministic replacement injection does not cover every interval between destructive-generation guards. | Held file witnesses, checks before and under the lease and before destruction, and native newer-WAL refusal cover current acceptance; no missed guard or current race failure is established. | Before changing guard placement, inject database and WAL replacement at the existing maintenance boundaries and verify unsupported-generation or changed-witness refusal without deletion. |
| DF-041 | V2DFC-016 ordinal-one logic review | Concurrent empty-residual removals after the runtime child is already absent can report a disappearing-path failure before an exact retry converges. | Marked-runtime cleanup holds the runtime inode anchor through final unlink; missing-parent races fail closed and do not delete a fresh successor. Root-only residual convergence is an independent refinement, outside the reproduced named-lock race. | Before changing runtime-absent residual admission, deterministically contend two residual removals and fresh init, retaining descriptor identity checks and successor protection while defining absence convergence. |
| DF-042 | V2DFC-016 ordinal-two maintainability M1 | Lone-removal-marker restoration repeats closed intent fields rather than reusing the canonical intent constructor. | Current exact-retry conversion validates both schemas and preserves the original response identity and time; native interruption/resume gates pass. Consolidation is independent future schema-maintenance work. | Before changing intent fields or schema identity, consolidate marker construction while retaining exact response/time, request digest and writer/root fences, and run lone-marker retry regressions. |
| DF-043 | V2DFC-016 ordinal-two product P1 | Human unforced removal of a prepared/running session reports the safe `REQUEST_INVALID` refusal without a specific force hint. | Refusal occurs before intent or filesystem effects; session-less admitted unforced removal and recovery preserve their original authority. The closed empty error details remain compatible. | Before refining human destructive-refusal presentation, provide an explicit authorized-force hint without widening JSON authority or bypassing confirmation, and test prepared/running no-effect refusal. |

### Current qualifications and delivered owners

DF-033's historical wording remains unchanged. With invalid configuration, both
`Ok(Maintenance)` observations select Maintenance. Initialization propagates
classification `Err(e)`; existing-workspace resolution retains the original
configuration error unless a successful probe observes Newer or Maintenance.
`Err(DirectStoreErrorV1::Maintenance)` is distinct from `Ok(Maintenance)`.
The resolver-owned probe creates no SQLite connection, SHM, lease or marker;
reset/removal retain their command-specific authority. Native representative
classification failures and unchanged caller ordering jointly establish this
boundary; the source guard is not labeled a separate native Err(Maintenance) case.

The [native CLI owner](../../crates/podway-cli/tests/e2e_v2dfc007_direct_cli.rs)
`maint002_indeterminate_committed_wal_refuses_owned_and_unmarked_destruction`
checks marked/unmarked reset and removal in the default and explicit contract
generations 1 and 2. Human unreadable-state output places preservation and
Store-independent diagnosis limits before the unchanged recovery reason, and
qualifies its discard advice on supported generation, original authority and
stable witnesses. Each invocation preserves the namespace and creates no SHM.
Normalized machine bytes agree across generations, and the recovery object matches
the frozen GUIDE-004 baseline. GUIDE-004 and GUIDE-010 separately retain exact
writer-mismatch JSON and human-output baselines.

DF-042's historical wording also remains unchanged. Lone-removal restoration
already validated the removal value, changed schema, removed phase and validated
the intent schema; it did not reconstruct every identity field. The delivered
private helper shares only canonical encoding, bounded document admission and
Store marker construction. Callers retain schema/order/error policy and original
fields, digest, root/writer/UUID/force, response and time. The authentic SIGKILL
owner guards the clock and observes a different caller-supplied retry request ID
returning the original response identity; reset's exact retry separately guards
both clock and ID source. Their [registered owners](#exact-maintenance-marker-construction)
preserve the complete membership, values and canonical bytes.

| Historical record | Member | Lasting behavior and verification owner |
|---|---|---|
| DF-029 | MAINT-001 | [Current owner](../architecture/system.md#direct-engine) |
| DF-030 | MAINT-003 | [Current owner](../specs/interfaces/cli-specification.md#workspace-lifecycle) |
| DF-031 | MAINT-002 | [Current owner](../specs/storage/recovery-retention-and-maintenance.md#newer-store-generations) |
| DF-032 | MAINT-004 | [Current owner](#native-migration-checkpoint-faults) |
| DF-033 | MAINT-002 | [Current owner](../specs/storage/recovery-retention-and-maintenance.md#invalid-configuration-and-compatibility) |
| DF-034 | MAINT-001 | [Current owner](../architecture/system.md#direct-engine) |
| DF-036 | MAINT-001 | [Current owner](../architecture/system.md#direct-engine) |
| DF-039 | MAINT-003 | [Current owner](../specs/storage/recovery-retention-and-maintenance.md#workspace-removal) |
| DF-040 | MAINT-005 | [Current owner](#native-destructive-generation-replacements) |
| DF-041 | MAINT-006 | [Current owner](#native-empty-residual-convergence) |
| DF-042 | MAINT-007 | [Current owner](#exact-maintenance-marker-construction) |
| DF-043 | MAINT-003 | [Current owner](../specs/interfaces/cli-specification.md#workspace-lifecycle) |

## MAINT integrated qualification

Qualification of an assembled revision uses the existing registered Engine
maintenance/maintenance-ports suites, Store classification/predecessor units and
binary-backed native CLI owners. Start with a focused exact admission check,
then bounded maintenance module groups and the native indeterminate-WAL and seven
confirmation/guidance cases, retaining existing invalid-current recovery owners.
The [checkpoint](#native-migration-checkpoint-faults),
[destructive-generation](#native-destructive-generation-replacements),
[residual](#native-empty-residual-convergence) and
[marker](#exact-maintenance-marker-construction) sections own their concrete
fault, exclusion, retry and preservation checks. Complete development qualification
requires the repository `make test` gate on the applicable frozen source.

After that final build, compare the actual final debug CLI with the retained
prechange binary in one unique owned standalone Git fixture. Verify both hashes,
embedded identities and matching manifests before creating the fixture. Baseline
build inputs remain unknown beyond the actual bytes/embedded stamp. Record a
required item with explicit fences and a caller key, then require candidate reads,
original byte-identical replay and original-response lookup without logical or
sequence effects. Continue once with current fences, verify the next node and
exactly one sequence increment, then repeat original old-attempt replay/lookup.
Generation, writer-contract row and Procedure snapshot remain unchanged. This is
the [supported Store continuity](#supported-store-continuity) scenario; mismatch
must remain refusal, without reset, migration or edited writer identity.

Evidence stays within its native limits. Non-UTF8 qualification combines the
observed fixture-filesystem creation refusal, real raw-byte argv/no-lossy-alias
refusal and constructed lossless-target confirmation unit; it does not claim an
existing non-UTF8 worktree reaches confirmation or generalize one filesystem's
EILSEQ to every platform. Missing unprovable child ancestry remains strict while
its parent survives; whole-target absence uses both fresh Git and retained-root
proof. The original-plan verification is bounded, with no continuously atomic
filesystem snapshot claim. DB/WAL witnesses are per probe, not permanent lease
witnesses, and the production predecessor registry remains empty.

Bound/reap native children, close SQLite handles on success and failure, persist
owned fixture allocation before initialization, and remove only that exact owned
fixture after its consumers finish. Retain actual source/binary/check identities
and result fields; host-local logs/receipts are not durable documentation owners.
This qualification selects no release, installation or external-consumer change.

The adopted dossier and current links remain until whole-Epic validation and
actual final owner acceptance of a concrete closeout patch. Prepare consumer and
replacement/deletion scope before that decision. Final relinking, adopted-index
removal, dossier deletion and Epic completion follow the
[TODO lifecycle](../todo/README.md#lifecycle); they are not qualification side effects.

## Exact maintenance marker construction

Engine's [registered maintenance suite](../../crates/podway-engine/tests/int_v2dfc006_maintenance.rs)
`sigkill_between_final_removal_markers_preserves_exact_retry_and_original_response`
uses the authentic first-marker SIGKILL window and the existing final-removal
callback to observe restored intent. It compares complete preserved membership,
values and canonical bytes, excluding only schema/phase distinctions, and retains
the original response/time with a NoEffects clock guard. A different caller-supplied
retry request ID still returns the original response identity. Wrong-key/writer
refusal leaves the original lone marker bytes unchanged. These assertions
establish baseline behavior and verify constructor consolidation.

The same suite's `exact_removal_retry_resumes_markers_then_absence_destroys_retention`,
`interrupted_removal_refuses_changed_explicit_and_selector_workspace_fences`,
`removal_force_and_original_workspace_fence_are_required_for_exact_marker_retry`
and `final_removal_contenders_cannot_recreate_unlinked_locks_or_replace_markers`
retain exact retry, fence/force refusal and live directory-lease exclusion.
`exact_reset_retry_resumes_all_owned_marker_and_private_publication_windows`
covers shared fresh/reset/phase construction and passes NoEffects as both clock
and ID source during exact recovery. The [maintenance ports suite](../../crates/podway-engine/tests/int_v2dfc006_maintenance_ports.rs)
`invalid_unfenced_removal_marker_survives_crash_and_requires_exact_retry` owns
nullable identity and unfenced invalid-current recovery. Caller-specific schema
validation/order and Store identity-error propagation stay with their existing
owners; no new production seam or mirror encoding-helper test is required.

## Native empty-residual convergence

Engine's [maintenance ports suite](../../crates/podway-engine/tests/int_v2dfc006_maintenance_ports.rs)
`concurrent_runtime_absent_residual_removals_converge_without_successor` uses two
real removers and bounded channel acknowledgments at classification/open-use,
retained-root inspection, final unlink and nested whole-target disappearance.
The winner reports its actual unlink; the loser confirms full absence with null
UUID and does not claim the winner's removal. Its active-child case deliberately
retains refusal while the original parent survives: native metadata on the
qualified fixture filesystem does not establish that a missing held child was
unlinked rather than moved. No link-count or last-path predicate grants authority.

`residual_removal_preserves_fresh_init_and_changed_ancestry` owns eleven native
subcases: completed fresh initialization in a new parent, initialization paused
under its real lease before publication in the retained parent, a new empty
parent, root substitution, Git administration replacement, moved child, empty
child replacement, child symlink, late unknown file, late configuration and new
empty runtime. Post-injection snapshots compare names, device/inode, modes and
bytes, including moved-outside data. Refusals retain public codes, exit and
retryability; actual fresh Stores remain integral and reject old-UUID retries.
Workers are released and joined before assertions and fixture teardown.

`residual_repopulation_before_child_removal_preserves_complete_namespace` injects
unknown content and configuration after empty inspection in a parent with an
original empty child. Its final expectation compares the entire post-injection
namespace and reproduces deletion of that child before the fix. One bounded
verification pass over the recorded original plan now refuses before any deletion,
without opening new entries or rescanning the full tree for every child. Later
intervening changes remain subject to original-identity checks and atomic empty
unlink; the test does not claim continuously atomic filesystem capture.

Workspace's `residual_plan_preserves_native_refusal_and_traversal_bounds` uses
native descriptors for foreign-owner/device validation, depth 128/129, an
exhausted/exact entry budget, unknown content and symlink refusal. Owner/device
cases vary the expected identity against actual metadata; they do not claim
privileged ownership changes or a mounted foreign filesystem. Existing
`markerless_residual_converges_only_when_descriptor_verified_empty` and
`markerless_residual_exclusion_expires_without_recreating_names_and_then_converges`
retain nonempty refusal, original runtime exclusion and deadline behavior.
The strict renamed-subdirectory unit and marked-flow fresh-init/old-UUID
regressions remain separate owners. Traversal keeps bounded original-identity
metadata and active DFS descriptors, with no descriptor per entry, repeated
whole-tree scan, new lock protocol or restarted deadline.

## Functional regression owners

Overflow measurement coverage belongs to Config's existing
`int_v2_procedure_canonical` suite: the canonical character/UTF-8 boundary test
`v2gsc004_canonical_projection_enforces_exact_character_and_byte_boundaries`
uses both ASCII and multibyte input, and
`cntrt002_emitted_overflow_follows_successful_canonical_admission` proves that an
admitted canonical document can exceed the separate emitted-text limit. The
formatter's local `cntrt002_emitted_projection_counts_scalars_at_the_exact_boundary`
unit test pins exact and one-over scalar boundaries without imposing a byte cap
on authoring text. The real CLI case
`e2e_v2dfc007_local_cli::cntrt002_format_write_emission_overflow_preserves_original_bytes`
uses an owned fixture to prove failed `format --write` preserves source bytes and
emits diagnostics rather than a partial successful source result. Protocol's
`cntrt002_writer_mismatch_example_matches_catalog_and_writer_recovery` pins only
the selected known answer's catalog message and read-only writer recipe; it
does not make every human-readable error message a compatibility field.

Public result memberships are owned by Protocol
[`int_v2_result_contract`](../../crates/podway-protocol/tests/int_v2_result_contract.rs).
`cntrt001_catalog_registry_and_output_have_exhaustive_result_memberships` compares
the command catalog, the three public registration tables and Output v4 selections,
including both observation generations, status tiers, prepared/running guidance
and legacy graph/preview variants. Generic objects and non-command documents do
not contribute closed result memberships; `__complete` remains internal.
`cntrt001_membership_comparison_rejects_drift_and_duplicate_declarations` uses the
same comparison helper with mutated inputs to reject missing, extra, duplicate,
misassociated and unresolved memberships before set conversion can hide defects.

`e2e_phase5_user_environment::calls001_help_completion_executes_contextually_without_workspace_queries`
executes generated bash, zsh and fish completion functions for root, partial,
multi-word and dotted help topics, option placement and values, flag-selected
topics, retired help families and retained executable routes. A sentinel CLI
rejects workspace queries. Bash and fish exercise prefix filtering; zsh captures
the generated function's candidates because native `compadd` requires an
interactive completion context.

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
  `record_stdin_preserves_identity_applies_one_atomic_batch_and_replays_without_preflight`,
  `agent002_stdin_fenced_mutations_share_conflict_rejection_and_preserve_set_stdin_preflight`

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
  `v2sdg006_delegated_group_preview_and_graph_keep_exact_scope_and_declared_order`
  covers successor group preview and graph output with declared scope and order.
  The [direct CLI suite](../../crates/podway-cli/tests/e2e_v2dfc007_direct_cli.rs)
  tests original-response recovery through retained-command replay, outcome
  lookup and SIGKILL after commit and before stdout.

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
  pins distinct request IDs and one original deadline. Stdin record/delegation
  (`record_stdin_preserves_identity_applies_one_atomic_batch_and_replays_without_preflight`,
  `delegation_stdin_acceptance_receipt_and_completion_preserve_the_real_attempt`,
  and `agent002_stdin_fenced_mutations_share_conflict_rejection_and_preserve_set_stdin_preflight`),
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

GUIDE agent-guidance and contract-generation checks use these registered owners:

- Socket-fixture independence from the ambient temporary directory is owned by
  the [long-TMPDIR regression](../../crates/podway-git/tests/int_guide001_socket_roots.rs)
  `socket_fixtures_pass_with_a_long_tmpdir` with the per-file `socket_fixture_root`
  helpers of the two podway-git socket tests; the fallback design is recorded
  above. The helpers never change the process working directory, because tests
  run concurrently, and keep every socket inside the worktree fixture, because
  the resolver validates containment.
- Local-command option rejection and command-word help topics are owned by the
  [local CLI checks](../../crates/podway-cli/tests/e2e_v2dfc007_local_cli.rs)
  `guide002_local_commands_reject_every_worktree_request_option` and
  `guide003_help_topics_resolve_command_words_and_groups`.
- Record-field help and human recovery hints with byte-identical JSON are owned
  by the [direct CLI checks](../../crates/podway-cli/tests/e2e_v2dfc007_direct_cli.rs)
  `guide004_human_writer_mismatch_hint_names_digests_and_recovery` and
  `guide004_human_citation_hint_states_the_admission_rule`, whose
  `GUIDE004_WRITER_MISMATCH_JSON_BASELINE` and
  `GUIDE004_CRITERION_CITATION_JSON_BASELINE` constants pin the generation-1
  error envelopes.
- The historical GUIDE-006 Engine baselines guard the previous writer's
  default generation. The Engine [baseline suite](../../crates/podway-engine/tests/int_guide006_generation1_baselines.rs)
  `int_guide006_generation1_baselines` and the CLI [baseline suite](../../crates/podway-cli/tests/e2e_guide006_generation1_baselines.rs)
  `e2e_guide006_generation1_baselines`. The current CLI baseline uses the
  v7 expected bytes under `tests/guide006_scoped_baselines/`. The historical
  v5 fixtures under `tests/guide006_baselines/` preserve former expected bytes. The two suites normalize
  differently: the Engine suite injects deterministic identifiers and
  timestamps and replaces only the worktree root, while the CLI suite applies
  first-appearance normalization to the request id, generation timestamp,
  worktree root and every remaining generated UUID and timestamp literal.
- Generation reservation and selection are owned by the protocol
  [contract suite](../../crates/podway-protocol/tests/int_guide007_contract_generations.rs)
  (`guide007_*`), the `guide007_released_manifest_table_pins_published_digests`
  unit pin in `crates/podway-protocol/src/result_contract.rs`, and
  `tools/verify_contracts.py --check/--sentinels`.
- Fenced plain start is owned by the Engine
  [start suite](../../crates/podway-engine/tests/int_guide008_fenced_plain_start.rs)
  (`guide008_*`, five cases) and the five `guide008_cli_*` cases of the
  [runtime CLI checks](../../crates/podway-cli/tests/e2e_v2dfc007_runtime_cli.rs).
- Observation v6 and generation selection through the real CLI are owned by the
  Engine [observation suite](../../crates/podway-engine/tests/int_guide009_generation2_observation.rs)
  (`generation2_*`), which also pins an explicit generation 1 equal to the
  frozen GUIDE-006 baselines, and the packaged scenario
  `e2e_guide009_generation2_lifecycle::generation2_lifecycle_conformance`.
- Generation-2 error details are owned by the core citation carrier tests in
  [`graph_v2.rs`](../../crates/podway-core/src/graph_v2.rs), the Engine
  [details suite](../../crates/podway-engine/tests/int_guide010_generation2_error_details.rs)
  (`generation2_*`) and the direct CLI `guide010_generation2_*` cases.
- The deprecation help text is owned by the grammar unit test
  `guide011_contract_generation_deprecation_topic_and_overview`.

### Integrated CLI guidance qualification

The existing registered owners cover separate boundaries; qualify the combined
caller workflow through the actual CLI in owned standalone Git fixtures as well.
Run `small-change-v2` and `bug-fix-v2` through terminal completion and disposition.
The bug-fix flow records reproduction commands actually run and observed results,
or a reproduction limitation, then records the later complete nested
`check_result` with a digest of retained exact output. Exercise enabled Goal
assessment with eligible source and recorded decision-local item citations.
Preserve fresh observed fences, original request/key replay and read-only lookup.

Execute both preset and file variants actually advertised as
`session.start_replace`, using an appropriate prepared or replacement-eligible
state. A disposed terminal's plain-start template is named `session.start`;
that distinct auto-archive action does not prove either replacement variant.
Preserve the generation-1/2 machine templates and allowed actions while filling
only declared placeholders and supported optional arguments.

Use the existing exact Make E2E entry point and registered owners before adding
coverage. The [real-shell owner](#functional-regression-owners) executes bash, zsh and fish
functions; the
[usage/diagnostic](../specs/quality/testing-and-conformance.md#cli-usage-and-syntax-guidance),
[documented record/counter](../specs/quality/testing-and-conformance.md#observation-and-recorded-check-guidance),
[lifecycle/recovery](../specs/quality/testing-and-conformance.md#lifecycle-attribution-and-lost-output-guidance)
and [preset/writer](../specs/quality/testing-and-conformance.md#shipped-reference-path-evidence)
owners cover the remaining boundaries. The
[workflow example](../examples/v2-workflow.md) supplies the canonical nested input
that the real CLI regression decodes and submits. Engine-only preset paths do
not replace the two actual CLI journeys. Keep whole-batch rejection, stale
session/item fences, lost-output recovery, distinct counters, unchanged preset
bytes and incompatible-writer refusal in the verification basis.

Bind every claimed result to its actual source, binary, manifest and check
inputs. Reuse a prior full gate only where those inputs remain applicable; record
changed build stamps and run current checks for changed documentation or inputs.
Executable or contract changes still require `make test`, and test changes
require `make lint-all`. These are functional correctness checks, with bounded
reaped children and owned-fixture cleanup. They establish no agent-usability,
VM, distribution, installation, release or elapsed-time qualification.

The [roadmap](../roadmap/README.md#calls-agent-workflow-guidance) owns the ordinary
qualification, later internal whole-Epic validation and final owner-acceptance
split. Keep the adopted dossier and its links until the exact closeout is
accepted and the final reference/consumer check permits retirement. A private
prepared patch is a proposal and does not complete those lifecycle steps.

### Guidance source feedback and revisit conditions

The following historical findings, deferral reasons and revisit conditions are
preserved verbatim from GUIDE's design review and UTM reruns. They describe the
original gaps before CALLS implementation. Promotion closed their deferrals;
the roadmap alone records implementation and acceptance. Current behavior belongs
to the CLI, JSON, built-in preset and writer specifications and the linked
regression owners. DF-050 retains its independent owner.

| ID | Source | Feedback | Deferral reason | Revisit condition |
|---|---|---|---|---|
| DF-044 | GUIDE design review | `podway help` topics have no shell completion: the help route completes no values, and multi-word command topics would need contextual completion. | GUIDE-003 resolves topics without changing completion route accumulation; completing multi-word positionals is an independent refinement. | Before changing help grammar or completion route accumulation, complete help topics from the same command tree and verify the bash, zsh and fish scripts. |
| DF-047 | GUIDE-012 UTM agent reruns | Per-command help usage lines omit `--idempotency-key` and global options such as `--contract-generation`, so agents learn the key requirement only from a `REQUEST_INVALID`, and "invalid command syntax" does not name a missing required option such as `--task`. | Help keeps its established per-route usage format and the closed JSON errors remain actionable; enriching usage lines and usage failures is a guidance improvement, not a contract defect. | Before changing help usage rendering or usage-failure messages, consider appending the always-available global options to per-command usage lines and naming the first missing required option in the usage failure. |
| DF-048 | GUIDE-012 UTM agent reruns | The nested `check_result` shape (`input_basis` with `descriptor` and `digest`, `executor` with `name` and `version`) appears only in the observation's `stdin_template` and not in the inspected agent guidance (help, the runtime skill and observation prose); `item.set` results call the session revision just `revision` beside item revisions; and nothing guides agents toward reproducible check-output digests. | The shapes are schema-valid and the observation carries the executable input. The runtime skill now names the required nested fields, uses stdin_template when present, distinguishes session/item revisions and recommends hashing retained check output. The remaining CLI-help and observation-prose guidance is independent. | When record help or observation prose is next revised, explain the nested check-result shape, session-revision field name and reproducible output digests; preserve the runtime guidance already added. |
| DF-049 | GUIDE-012 UTM agent reruns | Help and the runtime skill do not say when to choose `disposition handed-off` versus `not-required`, or what `--reference` holds; `allowed_actions` lists `session.start_replace` once while two prepared templates exist; and the templates omit the optional `--actor` and the `goal assess-criterion` `--evidence`/`--item` options. | The template shapes remain valid by design. Runtime lifecycle guidance now explains disposition choice and reference; shared execution rules permit supported optional/repeatable arguments, and goal guidance names citation flags. CLI-help explanation remains independent. | When lifecycle or goal help is next revised, explain disposition choice and reference, the shared start_replace action covering two variants, and supported optional actor/citation arguments. |
| DF-051 | GUIDE-012 UTM agent reruns on the final candidate build | The `begin` and `disposition` help say "Omitted fences are filled from a fresh status preflight" and their usage lines bracket the fences as optional, while the runtime skill says never to omit a required mutation fence (`774954a:skills/use-podway/SKILL.md:75`), so both agents read the two as conflicting; the envelope's `latest_workspace_sequence`, the init result's `revision` and the archive list's `archived_workspace_revision` are not related anywhere in the inspected guidance; `preset explain` (text and JSON) does not show that `bug-fix-v2` tracks a goal (`assets/presets/bug-fix-v2.yaml:7`); and the `bug-fix-v2` `regression-check` prompt (`:40`) can be read as the check to run later or as its result. | The runtime skill now distinguishes CLI preflight auto-fill from automation preserving observed fences. Both agents had preserved all fences, and the counters, preset explain output and prompt remain valid; their presentation improvements remain independent. | When command help or presets are next revised, explain automation fence practice, relate workspace revision and sequence, show goal tracking in human preset explain, and clarify the regression-check prompt. |

| Source feedback | Durable behavior and regression ownership |
| --- | --- |
| DF-044 | [Help grammar and completion](../specs/interfaces/cli-specification.md#grammar-and-global-options), with the [real-shell owner](#functional-regression-owners). |
| DF-047 | [Usage and diagnostics](../specs/interfaces/cli-specification.md#output-and-exits), with [registered diagnostic cases](../specs/quality/testing-and-conformance.md#cli-usage-and-syntax-guidance). |
| DF-048 | [Recording and counters](../examples/v2-workflow.md#keep-counters-distinct), with the [actual documented record owner](../specs/quality/testing-and-conformance.md#observation-and-recorded-check-guidance). |
| DF-049 | [Lifecycle and recovery](../specs/interfaces/cli-specification.md#mutation-rules), with [attribution/recovery cases](../specs/quality/testing-and-conformance.md#lifecycle-attribution-and-lost-output-guidance). |
| DF-051 | [Automation fences](../specs/interfaces/automation-client-contract.md), [distinct counters](../examples/v2-workflow.md#keep-counters-distinct), [preset guidance](../specs/domain/built-in-presets.md), and the same lifecycle/record/preset owners. |

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
remains independently documented. The separately adopted
[CALLS epic](../roadmap/README.md#calls-agent-workflow-guidance) retains its acceptance
record. Its [integrated qualification requirements](#integrated-cli-guidance-qualification)
and [historical feedback](#guidance-source-feedback-and-revisit-conditions)
remain separate from completed structural-program evidence. Batch criterion
assessment (`goal assess-criteria --stdin`) and commit-bound `--with-observation`
mutation responses are owned by `AGENT-001` through `AGENT-006` below; combined
record-and-complete or assess-and-decide single-command verbs remain unadopted.

CALLS observation/recording regression ownership is recorded in
[Testing and Conformance](../specs/quality/testing-and-conformance.md#observation-and-recorded-check-guidance).
The documented nested check-result example is exercised by the registered real
CLI rather than treated as a prose-only schema example. Its focused `--nocapture`
run emits the submitted document, retained exact check-output bytes and original
record response for task-local evidence; normal gates capture passing test output.

Lifecycle/attribution/recovery owners are listed in
[Testing and Conformance](../specs/quality/testing-and-conformance.md#lifecycle-attribution-and-lost-output-guidance).
They reuse the real blocked-stdout/SIGKILL fixture for loss and the existing
single-CLI helpers for retention and citation controls. Keep original requests
and fences in replay checks; a fresh key or changed fence must not stand in for
exact replay.

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
`tools/run_native_qualification.py` owns the retained generation-15 matrix.
It includes Store G1/G2 and Engine G1/G2 at 300 and 1,024 nodes, Engine G3,
the former five-boundary Engine kill case, 200 random kills, G5, G6 and
Store G9. Each ignored test runs once through an immutable copy of its
release-profile Cargo artifact with bounded process-group cleanup. Source is
captured before building and checked before and after every case. A failure
stops the entry point and leaves remaining cases incomplete. The development
gate runs the successor Engine's DE01–DE04 proof separately. ADOPT-005 owns
the integrated successor CLI process matrix.

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

Store's `test_support` module and Engine's successor fixture issuer are
disabled in production builds. The standard Make integration gates select
`podway-engine/test-support`, which also enables Store's `test-support`.
Use `--features test-support` for a standalone Store `int_suite` or an Engine
`int_suite` run that includes the successor transaction fixtures.

## Integration settings regression owners

The [settings contract](../specs/operations/integration-settings.md) separates
configuration support and the explicit Engine seam from the public Git/config-only
query. Config's registered `int_integ002_settings` suite owns strict booleans,
inheritance/value precedence, closed nested objects, YAML hazards and user-input
byte/depth/node limits. `int_phase4_workspace_config` preserves released default
bytes/canonical output and checks v3 legacy-field refusal.

Engine's registered `int_integ002_settings` suite owns Git/config-only resolution,
project-before-HOME failure precedence, invalid-user validation despite project
override, unsafe paths and lossless HOME. The native settings unit regression
replaces roots, ancestors, files, content and missing leaves deterministically.
The integration suite also replaces the project root after Git discovery and
requires refusal before configuration reads; Engine binds the opened project
descriptor to the original native Git discovery evidence.
The maintenance suite's
`integ002_home_root_init_and_removal_preserve_separate_user_settings` uses owned
Git worktrees to verify HOME-root/separate-worktree lookup and init/removal
isolation. No test selects the real user home or changes ambient HOME.

Git's paired `integ003_git_only_discovery_ignores_runtime_but_retains_native_root_witnesses`
regression requires ordinary resolution to reject blocked runtime paths while
Git-only discovery preserves native root evidence and refuses substituted roots.
The CLI unsafe-project regression requires project configuration errors without
legacy reset advice even when `.podway` itself is unsafe.

Use the existing exact registered-test invocation and Make gates. These checks
include the registered CLI `e2e_integ003_settings` suite: both generations,
pre-init and blocked runtime paths, all permission sources, project/user/HOME
failure precedence, rejected fences/names/options, human rendering, help and
completion. Protocol's `int_integ003_settings` validates closed result and envelope
memberships and configuration-error details. Each CLI child selects an owned
HOME; no test changes process-global HOME. An open stdin producer verifies the
query returns without consuming stdin. These checks prove settings/query behavior,
not actual Dolgorae execution or complete integration qualification. The
typed query rejection fixture uses a canonical non-Git root so macOS temporary
directory aliases do not preempt its intended `NOT_A_GIT_WORKTREE` control. The
[manifest qualification](#integration-manifest-qualification) adds the
normal Store read/write/original-replay matrix and authentic previous-writer
refusal alongside that query evidence.


## Repository metadata verifier controls

`make dolgorae-connection-test` runs every deterministic control in
[`verify_dolgorae_connection_tests.py`](../../tools/verify_dolgorae_connection_tests.py).
It is registered in `architecture-static`, hence `make test-prepare` and the
required `make test` gate. Its `python-prepare` prerequisite creates the ignored
`target/dev-python/<version>` virtual environment using `./tools/python` and
the exact version in `.python-version`, and installs
[`tools/requirements-dev.txt`](../../tools/requirements-dev.txt) there. The first
preparation, a Python pin change, or a requirements change needs access to the
configured package index or a populated pip cache. Later runs reuse that environment; no global
Python modification or PATH override is needed. `make python-prepare` can prepare
it separately. Installation failure stops Make before the controls run.
The verifier reports missing validator prerequisites as incomplete; the control
gate fails rather than skipping positive validation.

The fixtures resolve their temporary root before constructing verifier inputs,
including macOS `/var` and `/tmp` aliases. A child control uses an explicitly
symlinked TMPDIR; the verifier still rejects a noncanonical output parent supplied
directly by its caller.

The controls run owned executable fixtures against the original frozen positive
capabilities payload. They cover complete nested schemas and error closure,
embedded IDs and offline-only lookup, exact prefix/suffix/case comparison,
unknown families versus malformed supported instances, validated error mapping,
complete malformed JSON versus absent/truncated observations, process status,
shared deadlines, both byte ceilings, executable replacement, owned group reaping,
unknown cleanup state, exclusive output and privacy. No fixture uses real backend
accounts or credentials. Native Darwin cleanup observes only the created process
group and distinguishes zombie-only groups from unknown state.

A separate explicit observation can run the repository verifier against an
existing selected Dolgorae and the exact candidate Podway. Retain its report and
executable hashes privately; unsupported/incomplete exports are honest limits,
and a conclusive mismatch is a failure. Neither this observation nor the mock
controls qualify actual delegation, results, receipts, disposition or activation.
The [manifest qualification](#integration-manifest-qualification) separately
checks normal Store continuity/refusal and query isolation.


## Integration manifest qualification

The registered `e2e_integ005_continuity` module invokes
[`qualify_integration_settings.py`](../../tools/qualify_integration_settings.py)
through the existing real-binary E2E harness. Its portable current-manifest
scenario is part of `make test`. It creates and accepts a deterministic delegated
fixture, then exercises all four combinations of absent/poisoned HOME and valid
project true/false. Reads and lookup preserve logical state; fresh block/unblock
writes advance sequence once each; original acceptance replay retains its exact
stdin, key and fences after those revisions. Disabling permission still permits
receipt recovery and explicitly fenced completion, followed by historical receipt
inspection and old-request replay. Writer identity and Procedure snapshot bytes
remain unchanged. A PATH sentinel detects accidental backend launch.

Exact selection runs the entire one-case registered module:

```sh
make test-e2e-exact E2E_TEST=podway-cli::e2e_suite::e2e_integ005_continuity::integ005_permission_matrix_preserves_receipts_and_original_replay
```

Authentic previous-manifest qualification is an additional explicit operation,
not an implicit standard-gate prerequisite. Select a verified local previous
binary and its retained private identity report (`sha256` plus
`identity.result`), then use an exclusively new output:

```sh
./tools/python tools/qualify_integration_settings.py --podway /absolute/candidate/podway \
  --previous /absolute/verified-previous/podway --previous-sha256 SELECTED_SHA256 \
  --previous-identity /absolute/private/previous-identity.json \
  --output /private/tmp/owned-evidence/matrix.json
```

The helper rechecks actual binary hashes and embedded identities. It creates the
old Store and accepted receipt through that selected CLI before any candidate
observation, retaining exact request argv/stdin and original success/receipt.
Status, outcome lookup, receipt read, fresh write and original replay must all
refuse the old writer across the four HOME/toggle cases. Before/after native
snapshots preserve original Store/Git/runtime bytes, inode/device identities,
modes and modification/change times, with no new SHM or lease side effects.
Project configuration changes are setup baselines, not candidate side effects.
Both-generation settings queries succeed on that same Store; invalid project,
user and HOME cases preserve normal precedence and redaction. A temporarily
blocked runtime path supplies a separate executable no-admission control.

Read logical state from quiescent byte-identical main plus present WAL captures
in separate owned inspection scratch, opened by SQLite in read-only mode. An
absent WAL stays absent. Check original runtime/Git bytes and native witnesses
before/after each capture; create no original SQLite connection, SHM or checkpoint.
SQLite may create SHM beside the disposable inspection copy, which is removed
immediately after its last read. This proves the observed owned quiescent state,
not atomic capture against concurrent outside writers. Whole-fixture cleanup and
bounded CLI child reaping are required; preserve final selected binaries and
reports while remaining consumers need them.

Compare exact retained canonical `response_json` bytes and lookup response
identity independently from stdout serialization and top-level per-invocation
request/time fields. The original request's argv, stdin, key and fences remain
unchanged; successful exit alone is insufficient. Retain persistence, sequence,
receipt and byte/native witnesses in the private report. After the final required
`make test`, bind this qualification to the exact final CLI hash; renew it if the
artifact changes. The same rule applies to selected-tool metadata observation,
whose unsupported/incomplete outcome remains explicit and never qualifies live
execution, models, accepted external results or activation.

## AGENT integrated qualification

`AGENT-001` registers the reserved `podway.criterion-assessment-many-input/v1`,
`podway.criterion-assessment-many-result/v1`, and
`podway.mutation-observation-result/v1` schemas, staged pre-fence batch input
decoding, opt-in canonical request identity/digest helpers, and non-recursive
`outcome.lookup` headroom checks in `podway-protocol`, verified through
`podway-protocol` unit tests and the existing registered `int_suite` modules
(`int_v2_result_contract`, `int_v2agt004_record_many`,
`int_v2dfc006_direct_envelopes`, `int_v2dfc008_current_contract`,
`int_guide007_contract_generations`, and `int_release_contract_verifier`).
`AGENT-002` unifies stdin-owned fence classification (`StdinFencedRoute`) and
preflight exclusion in `podway-cli`. `AGENT-003` activates
`podway goal assess-criteria --stdin` (`goal.assess_criteria`) with default
`podway.criterion-assessment-many-result/v1`, verified by `podway-core`
`goal_state_v2::tests`, `podway-store` `int_v2_goal_state` and
`int_v2dfc006_execution_boundary`, `podway-engine` `int_v2dfc006_goal_ports`,
and `podway-cli` `e2e_v2dfc007_local_cli`, `e2e_v2dfc007_direct_cli`, and
`e2e_v2dfc007_runtime_cli`. `AGENT-004` extracts the private
`v2_read_service/observation.rs` submodule, unifies Direct observation
post-processing (`finalize_direct_observation_in`), adds the in-transaction
successor observation context (`SuccessorObservationContextV1` over
`DirectTransactionV1::delegation_receipts_for_graph`), and replaces temporary
size-measurement buffers with `serialized_json_len_v1`, verified by
`podway-engine` `v2_read_service::tests::agent004_*`,
`direct_execution::reads004_stage5_tests`,
`int_guide006_generation1_baselines`, `int_guide009_generation2_observation`,
`int_v2rel002_maximum_next`, `int_v2run002_views`, and `int_v2dfc006_errors`,
`podway-store` `int_v2sdg004_receipt` and `int_v2sdg005_semantics`, and
`podway-protocol` `json_len::tests`. `AGENT-005` activates `--with-observation`
(`podway.mutation-observation-result/v1` with embedded `podway.observation-result/v6`)
across plain non-dry-run `session.start`, `session.begin`, `item.record_many`,
`session.complete`, `session.decide`, and `goal.assess_criteria`, verified by
`podway-protocol` `int_v2_result_contract`, `int_v2dfc006_direct_envelopes`, and
`int_v2dfc008_current_contract`, `podway-engine` `direct_mutation::tests::agent005_*`
and `int_guide009_generation2_observation`, and `podway-cli`
`command::tests::agent005_*` and
`e2e_guide009_generation2_lifecycle::agent005_with_observation_lifecycle_replay_lookup_and_conflicts`.
`AGENT-006` caller regressions use the production successor CLI. The
`e2e_guide009_generation2_lifecycle` cases
`agent006_small_change_v3_integrated_caller_workflow_and_call_counts` and
`agent006_scoped_goal_assessment_and_closeout` measure two-call record/complete
segments and the two-call three-criterion assessment/decision segment. They
carry the commit-bound v8 observation into the following call. The
`agent006_integrated_caller_recovery_contention_and_fallback_paths` case
exercises contention, original-key replay and lookup, and a fresh `observe`
after recovery. These controls complement the scoped check-result binding and
native batch rejection tests.

### AGENT source feedback and revisit conditions

AGENT selects DF-014 only. The original V2SDG validation maintainability review
found duplicate identity-flag rejection and different preflight paths for
`record --stdin` and `delegation accept`. Both routes already rejected conflicting
flags and preserved document-owned identity; the deferral addressed future
maintenance risk. Its original revisit condition was to share the CLI
classification and flag-rejection logic before adding an identity flag or another
stdin-fenced mutation, then verify that preflight never replaces document fences
and conflicting flags remain rejected.

AGENT-002 implements that local ownership through `StdinFencedRoute` in
`podway-cli` grammar, direct dispatch and preflight; AGENT-003 adds the accepted
batch route. The registered CLI conflict/preflight regressions above preserve
the original rejection and fence guarantees. Promotion closed the deferral;
implementation and acceptance remain recorded by the roadmap. DF-025, DF-028,
DF-050 and the remaining TYPED candidate are outside this selected work.

Development verification covers repository source and native fixture behavior.
Ignored heavy qualification cases remain unexecuted unless explicitly selected;
the caller fixtures execute bounded owned-file readback and hashing, not an
external login suite. Structural counts apply only to the declared normal
segments. Release, distribution, installation, global skill activation and
external-consumer acceptance require their own scope and evidence.

## Execution group source admission checks

Config's registered `int_execution_groups` suite admits actual internal base/group
documents and checks their canonical output against the local canonical schemas
with an offline retriever. It covers all seven shared item kinds and defaults,
header bounds, interface equality and declaration order, required references
versus optional source items, custom and default-style guards, selected ports,
group-only success exits, session-goal assessment ownership, anchor/return
dominance, ordinary and explicit boundary exits, local rework and conditional
prefix manual targets. The tracked-base audit cases reject a replan route on a
session-goal assessment, prefix-only assessments, and a branch bypassing the
post-return assessment. They admit a supplemental prefix assessment alongside
a qualifying suffix assessment, and an assessment at the return placement
itself. Mixed replan/ordinary cycles remain rejection cases.
The source/format/graph checks preserve array order, normalize authoring maps,
retain supported YAML comments and prove prompt changes still change definition
digests. Explicit replan graph edges never acquire a synthetic local target.

Run the registered namespace through Gaori using
`cargo test --locked -p podway-config --test int_suite int_execution_groups::`.
Then run Config's library and integration targets together to retain the existing
exhaustive shared-graph reference checks. The library
`execution_groups::projection::tests::frame002_projection_limits_preserve_canonical_bytes_and_formatted_scalars`
checks exact/one-over source ceilings, including the distinction between canonical
encoded bytes and formatted Unicode scalars. Node/definition/port/route/manual
collection limits use real admitted documents. The dense composition has 1,024
placements: 1,023 eight-route decisions and one actual terminal, yielding 8,184
ordinary edges. Admission and graph projection assert that attainable maximum.
The independent 8,192/8,193-edge boundary is unreachable under the unchanged
node/route/success-exit caps; no malformed private model is injected to claim it.

Config's `jsonschema` test dependency reuses the repository's existing locked
version with default features disabled; schema retrieval reads local assets only.
The complete development gate remains `make test` through configured Gaori
`full`. These checks prove internal source admission and projection contracts.
Additional registered settlement cases reproduce post-return Action and Decision
escapes and missing/reversed selected input controllers before their fixes.
Controls preserve first prefix arrival, local Advance/Rework and exact ordered
interfaces. Finite-exit traps, malformed route targets, typed closed-shape
rejections and formerly masked reference/dominance cases have direct assertions.
Hand-authored nonlexical base/group YAML distinguishes source-map order from
option-ordered graph edges and preserves an interior comment. Decoder seam
cases cover nulls, unsupported YAML, duplicate mappings, depth and node count.

Public API wiring tests admit an exact-1-MiB padded source and reject one byte
over; a bounded dense long-ID definition fits source/canonical caps but exceeds
graph/pretty-source/format output caps. Compact declarations whose materialized
defaults exceed the canonical ceiling fail validation. These fixtures inspect
actual sizes and typed bounds rather than mutating private limits.

These source and projection cases exercise the current source limits. Protocol
owns successor wire allocation. GROUP-002 and GROUP-003 verify pre-effect budget
admission for their candidate boundary operations. ADOPT-001 selects the public
families.

## Execution-group candidate transaction checks

Select the corresponding module of the registered Engine suite with its
test-support feature:

```sh
gaori --json run --tag rust --tag unit -- cargo test --locked -p podway-engine --features test-support --test int_suite int_group002_transactions::
gaori --json run --tag rust --tag unit -- cargo test --locked -p podway-engine --features test-support --test int_suite int_group003_replan::
gaori --json run --tag rust --tag unit -- cargo test --locked -p podway-engine --features test-support --test int_suite int_group004_fences::
```

The suites drive the typed Engine issuer against owned generation-16 SQLite
fixtures. The activation and return suite checks one success-only commit per operation,
exact replay after the cursor advances, current plan and retained provenance,
and unchanged state, sequence and outcomes on failure. Dense admitted
definitions exercise combined response refusal for both operations and a
lookup-only headroom refusal for activation.
A Store-level control verifies rollback when the shared success validator
rejects the envelope after a state write.

The replan suite exercises a running group's guarded Decision at score 80 and
an admitted base review of a completed group. It checks exact retained guard
bindings, old attempt records, the fresh anchor, original response replay
after reopen and refusal of a key reused with changed feedback. The base-review
case checks cumulative base attempt counts and clears the closed group's
decision-entry ordinal from current counters. Negative cases
exercise an Action, an undeclared option, a forward option, a missing value, an
unscored or lightweight assessment, a success verdict and score 79. Dense
anchor guidance with maximum-length escaped feedback exercises pre-effect
response refusal. A fixture-only SQLite sequence collision rejects outcome
insertion after the state write, and cold reopen checks rollback before a
successful retry.

The fence and recovery suite submits independently prepared writes to separate
items across a Store reconnect, rejects a sequential write with a stale
same-item revision, and checks atomic record-many against both session and item
revisions. Invalid batch cases cover empty, excessive count, missing item,
wrong type, and the direct input byte limit. A registered Engine test calls
Core directly to admit the exact aggregate content limit and reject one extra
scalar. Same-value sets, empty clears and entirely unchanged batches preserve
retained row bytes and the current acceptance, while a mixed batch updates only
differing slots.
A rehashed low session
revision fails cold admission even with valid final item slots. The suite also
exercises current-only evidence and counter reads, original-key recovery and
exact replay after the cursor advances. Same-definition replacement checks
required and stale prior-work dispositions, an old plan, a prior scoped attempt,
and a stale delegation binding against a valid current cursor. The delegated
base anchor also requires renewed acceptance after a plan edit. Tracked cases
exercise the selected activation fence at Base review, rejected crossed prefix
rework, goal revision for running and completed selections, reactivation, and
cancellation after group completion. The suite exercises declared skip and
group-local decide, observation and response bounds, duplicate acceptance,
and current blocker resolution, continued completion and rejection of a late
resolve.

The [process qualification](#execution-group-process-qualification) exercises
contention, response loss and forced termination with this same Engine and
Store.

## Execution-group process qualification

Run the registered successor process suite with its exact child mode and
captured scenario evidence:

```sh
gaori --json run --tag rust --tag unit -- cargo test --locked -p podway-engine --features test-support --test int_suite int_group005_processes:: -- --nocapture
```

The parent starts its current `int_suite` executable with an exact child test
filter and an explicit fixture request. Each child opens its own generation-16
Store connection in an owned, standalone Git fixture. The ordinary child test
returns immediately without that request. The parent reports the executable
digest, database generation, writer identity and canonical DDL digest.

Contender connections report readiness before the first writer acquires its
lock. A contender then reports the SQLite busy callback from the actual
`BEGIN IMMEDIATE` while the first child is parked. The parent releases the
writer after observing that contention. The matrix checks different-key and
same-key activation, activation against a late anchor report, both orderings
of replan against a group report, independent writes to separate items, and a
same-slot stale loser. A bounded watchdog diagnoses missing barriers or child
responses.

The Store test-support observer parks after tentative state, sequence, outcome
and journal writes before COMMIT, and after confirmed COMMIT before the result
returns to Engine. The pre-COMMIT cases exercise SIGKILL and controlled process
exit. A fresh process sees the prior state and no outcome, then the same key
commits once on retry. The post-COMMIT case kills the parked process before its
reply file exists. A fresh process recovers the exact success, compares its
response digest and original response ID, then checks lookup and replay after
later work. The readback compares retained row fields and ledger counts. The
parent reaps each child and verifies removal of its owned fixture.

The suite also verifies replan from a Base review of a completed group,
same- and different-definition replacements, distinct working-copy bases,
first-to-third execution provenance, goal revision and explicit reactivation,
cancellation after group completion, and combined guidance/counter refusal.
The maintained v2 controls are
`int_group001_store::production_v2_store_classifies_candidate_generation_without_opening_it`
and `int_v2dfc006_runtime_ports::v2run004_production_retry_is_clean_durable_replayable_and_re_resolves_evidence`.
Public successor admission, paged history, and released binary qualification
have their own later gates.

## Execution-group writer cutover qualification

ADOPT-004 exercised the Store 15 to 16 boundary on native Apple Silicon macOS
with separate preserved and recovery Git fixtures. The original writer was extracted from the
[published v0.2.17 release](https://github.com/irootkernel/podway/releases/tag/v0.2.17)
after comparing the fixed archive checksum, provenance and packaged binary
digest to the published metadata. The release tag resolves to
`31cbd110573271ea3376cb07eb961f3dc257a05d`. The archive SHA-256 is
`4ca62ec943aa21e59c3173201c1f2e80cf274bb459dbb2ff3af0f01e34926546`.

The development (debug) CLI reports source commit
`3e739950d6a7b6a0d90d00b45a14b72d41032c02`. Read-only inspection of owned
SQLite copies verified the persisted writer rows after native execution:

| Executable | Store | Contract manifest digest |
| --- | --- | --- |
| Published `v0.2.17` arm64 binary | 15 | `sha256:f5e56e344a79022766fb69af7991c60d57e7793495b2599c1c3d3ef296309100` |
| Development successor arm64 binary | 16 | `sha256:a20cbfa86383a523eb6105ee6747a4fa2294e191b5b576ecf7a8e205a2f352ba` |

- The previous writer created the old Stores using the actual packaged
  Procedure v2 source and positively retrieved a recorded outcome key. The
  successor rejected observation, detailed original-key lookup, and ordinary
  completion in both old fixtures with `LEGACY_RUNTIME_STATE_UNSUPPORTED`,
  exit 5. Each request's before and after comparison matched file names, modes,
  identities and bytes, including the existing WAL and SHM. The preserved
  fixture retained its original runtime through the refusal checks.
- The successor refused an unconfirmed reset in the recovery fixture. The
  separately confirmed `reset --all --yes` allocated a fresh workspace UUID
  and wrote Store 16. Exact replay returned the original reset response bytes.
  The old active and archived sessions and recorded key were absent afterward.
  Git metadata, tracked and untracked sentinels, configuration, `.gitignore`
  and the original Procedure v2 bytes were preserved.
- On the admitted successor Store, starting the retained v2 source returned
  `PROCEDURE_SCHEMA_UNSUPPORTED`, exit 1. The source, SQLite main and WAL,
  path names, modes and identities matched, and the rejected key had no
  outcome. In a newly reset fixture, the existing SHM file changed bytes in
  place. An earlier supplementary comparison stopped on an unclassified
  mismatch before retaining its baseline or looking up that request key.
  A later probe after archive used a fresh key and kept the full snapshot
  unchanged. The successor admitted and activated the default
  `small-change-v3` group, returned its declared outputs, completed and
  archived the base, and read the retained archive.
- The authentic previous binary refused observation, known-key lookup and an
  ordinary mutation on the fresh Store 16 while the successor group was active.
  All three returned `WORKSPACE_SCHEMA_UNSUPPORTED`, exit 5, with
  `store_schema_version: 16` and `supported_store_schema_version: 15`. The
  runtime snapshot was unchanged around each request. The successor resumed
  its normal lifecycle from that state.

The refusal envelopes were checked against each executing version's schemas.
All fixture processes returned, the owned Git worktrees were removed, and the
source repository's worktree registry was unchanged. The
[upgrade QA scenarios](release-upgrade-qa.md#direct-to-direct-boundary-for-store-16)
define how to repeat the comparison with an exact candidate. Release
qualification has its own selected candidate and evidence requirements.

## Successor CLI integration qualification

Run the `native_process_` cases in the registered `e2e_adopt001_scoped_cli`
module and the retained-work and bounded-history cases in
`e2e_v2dfc007_native_conformance`. The suite starts the product binary in owned
Git fixtures. Run the complete development gate with the configured Gaori
`full` command, which runs `make test`, on the same source candidate.

| Case | Public result measured |
| --- | --- |
| Activation and scoped write contenders | Two original activation keys compete for one execution. A duplicate request replays its original success. Separate item slots remain writable at the same scoped attempt, and the stale same-slot loser retains no outcome. |
| Guarded replan and atomic report | Both requests use the same exact cursor. The winner determines the current scope, and the rejected request leaves no retained success. |
| Interrupted activation before commit | A separate CLI activation observes the held SQLite write lock while the interrupted CLI is alive. Kill and reap the child before releasing the lock. A fresh process reads unchanged durable state and misses both original keys. The interrupted request can then commit on exact retry. |
| Lost activation output after commit | A saturated stdout pipe blocks delivery. Another CLI process retrieves the committed outcome while the writer is alive. The killed writer delivered no response bytes. Advancing the session preserves the original lookup and exact replay. |
| Replacement provenance | Two goal revisions close the selected groups. The third activation uses a different definition and cites the first group's exact retained item through an empty second group. The fixture file keeps its revised contents, and prior work leaves the current slot unrecorded. |
| Current, archived and purged history | Record 65,536 mixed escaped and Unicode scalars and refuse one more. Reassemble both bounded pages in current and archived history. Reject a current token in archived history. Purging removes the archived value while lookup can still recover the original mutation response. |

Use these checks with the registered [guarded replan and Engine process
qualification](#execution-group-process-qualification), the [writer
cutover qualification](#execution-group-writer-cutover-qualification) and the
[consumer contract](../specs/interfaces/execution-group-consumers.md). The
Engine cases exercise internal transaction boundaries. The CLI cases exercise
public invocation, durable outcomes, response delivery and the selected wire
contracts. Stable release qualification uses its own selected release gate.

## HANDS integrated qualification

The integrated run used the product source at
`20639278a4130a01ef8fe83a8021d9abe4bf019d`. The first `make test` run stopped
at two stale Engine expectations. The activation transaction check now expects
the typed mismatch with the admitted source digest. The history check now expects
the distinct refusal for a retained group digest without an execution. Their
rollback and retention assertions remain in place. Both corrected exact checks
passed, followed by the complete configured Gaori `full` run of `make test` on
the aligned inputs.

The hands-on pass used the same optimized `aarch64-apple-darwin` candidate in
owned Git fixtures. All 317 captured candidate JSON envelopes passed offline
validation against the packaged Draft 2020-12 schemas with format checks.
Removing the request ID from each envelope caused all 317 negative controls to
fail validation. The package verifier checked 143 schemas and 183 manifest assets.
The matrix includes both contract selectors and the default selector.
The checked scopes and regression owners complement the
[CLI specification](../specs/interfaces/cli-specification.md),
[error contract](../specs/interfaces/errors-and-exit-codes.md), and
[JSON contract](../specs/interfaces/json-contract.md).

The Store 15 renderer regression runs in the normal offline `make test` gate.
The authentic E2E case requires `PODWAY_TEST_LEGACY_V017_BINARY` pointing to a
verified `v0.2.17` writer. The [upgrade scenario](release-upgrade-qa.md#direct-to-direct-boundary-for-store-16)
gives the exact opt-in invocation. The H4 row records the separate native run
with the optimized candidate and the verified writer.

| Scope | Native scenario and observed result | Regression owners |
| --- | --- | --- |
| M1 | Domain and activation refusals retain the catalog prefix and typed fields, append the exact cause or admitted digest, and land within the 512-scalar bound. Rejected keys are absent and observation and sequence are unchanged. | Engine `int_v2dfc006_errors`, `int_group002_transactions`; CLI `e2e_hands_usability` |
| H1 | The literal archive and timeout syntax refusals identify trusted route and option labels. A real replacement activation reports the unfilled `prior_work` token and pointer, with fallback and human escaping controls. Blank block, retry and cancel reasons and an unknown scoped criterion give their own diagnostics. | CLI `e2e_v2dfc007_local_cli`, `e2e_hands_usability`; Protocol `int_hands_usability`; Engine `int_v2dfc006_errors` |
| H2 | After returning a completed group, DAG and attempts require its execution ID for its retained digest. The explicit execution works and an ordinary mismatched digest remains retryable. A wrong activation pin names the actual supplied definition digest. | Engine `int_trace002_history`, `int_group002_transactions`; CLI `e2e_hands_usability` |
| H3 | Off-anchor activation with no current activation template, empty prior work, and unknown retained execution each produce the expected cause and leave no outcome. | CLI `e2e_hands_usability`; Engine `int_v2dfc006_errors` |
| H4 | The verified v0.2.17 writer creates Store 15. Human status gives the ordered original-writer and reset guidance, and JSON matches the frozen refusal in both selectors. The closed writer's main and WAL bytes are retained in a separate SHM-free witness, preserved by every read. | CLI `authentic_store_15_preserves_json_and_adds_human_history_guidance` and renderer tests |
| H5 | New init omits the reserved default. An existing v2 preset value remains readable, healthy and byte-identical through init, repair and reset. | Config `int_phase4_workspace_config`; Engine `int_v2dfc006_workspace`; CLI `e2e_hands_usability` |
| H6 | Literal in-worktree absolute preview and start admit the file. Preview emits its relative path and argv. Start retains the relative canonical request identity, confirmed by digest and exact same-key replay. Absolute lexical cleanup works and outside paths, parent traversal, directory roots and symlinks are refused. | CLI `e2e_hands_usability` and `command::procedure_files`; Git `int_phase3_layout`, `int_phase3_resolver`; [filesystem rules](../architecture/git-worktree-and-filesystem.md#path-containment) |
| H7 | Each incomplete JSON and non-interactive human confirmation combination lists the missing requirements in order, exits 2 and retains no success. | CLI `hands001_removal_reports_every_missing_confirmation_in_order` |
| H8 | Prepared, blocked and ready base, running group, and completed views emit the ordered human summary. Tracked native views print the current outcome and revision. Quiet, full status and observe views, history attempts and the next verbose refusal retain their established behavior. | CLI `e2e_hands_usability`, `agent006_scoped_goal_assessment_and_closeout` and renderer controls; GUIDE-004/006 compatibility baselines |
| H9 | The optimized batch help, CLI specification and source [goal reference](../../skills/use-podway/references/goal.md#assess-and-decide) carry the exact citation-responsibility sentence. | CLI `agent003_goal_assess_criteria_help_example_decodes_and_names_status_goal_revision` |
| H10 | The minimal Procedure v3 scaffold has its own identity and digest. Strict check, canonical format, preview and Mermaid graph agree with admission, and every shipped v3 base/group pin differs. | Config `minimal_v3_scaffold_is_canonical_admissible_and_distinct_from_presets`, retained v2 scaffold suite; CLI `scaffold_v3_admits_a_distinct_custom_procedure` |

The integrated pass covers development and isolated native behavior. Stable
distribution and release qualification follow their own [release workflow](release.md).

## Candidate group-lineage qualification

Run the registered history cases against the isolated successor Engine and
Store:

```sh
gaori --json run --tag rust --tag unit -- cargo test --locked -p podway-engine --features test-support --test int_suite int_trace001_history::
```

Append the case name to the module prefix and add `-- --exact` to select
one case:

- `exact_group_history_survives_replacement_cancellation_and_archive` checks
  two activations of one definition, both plan bindings, guarded replan,
  abandoned and pending Decision entries, group-detail continuation and
  selector refusals. It checks logical Store contents around reads and rejected
  writes, terminal-only archival without an outcome, and exact replay after
  archived reopen.
- `unguarded_choice_retains_its_exact_assessment_across_rework_and_archive`
  checks two distinct ordinary-choice assessments through rework and reopen.
  Cold replay refuses the first Decision when its rehashed record instead
  points at the later valid attempt.
- `goal_lineage_keeps_closures_and_boundary_decisions_across_reactivation`
  pages revisions before activation, after replan and after completion.
  It preserves both group closures and reads the group Decision's Plan
  confirmation and the base Decision's execution-result and goal assessment.

The feature-enabled Engine library case
`successor_execution::history::tests::history_page_reserves_response_metadata_before_accepting_rows`
checks the exact encoded response limit and one byte above it with a
continuation. TRACE-004 owns full process and destructive lifecycle checks.

## Candidate retrospective DAG and value checks

Run the registered Engine cases against the isolated candidate Store:

```sh
gaori --json run --tag rust --tag unit -- cargo test --locked -p podway-engine --features test-support --test int_suite int_trace002_history::
```

The DAG case admits a 1,024-placement graph with a projection larger than
the unpaged authoring limit, rejoins every canonical page and checks the
original graph digest and edge count. The rework case compares stale and
fresh attempts, verifies the original item revision and delegation binding,
and reads 65 retained blockers across two pages before and after archive.
The replacement case exercises ordered attempts, complete typed check results,
empty and unset slots, and maximal text and list values that include escaped
and mixed-width scalars. It distinguishes present, missing and unverified
artifact paths, including missing or unsafe parent directories. The cases
check scoped selector and continuation refusals and compare logical SQLite
rows, including the mutation ledger, before and after history reads.

## Candidate prior-work qualification

Run the registered scenario against the isolated candidate Store:

```sh
gaori --json run --tag rust --tag unit -- cargo test --locked -p podway-engine --features test-support --test int_suite int_trace003_history::third_plan_pages_exact_first_group_work_without_inheriting_gates -- --exact
```

The third activation cites a completed first group's output, failed check and
decision after an empty second group. The case compares the predecessor,
exact design and plan, working-copy basis, choices and original rationale
through current and cold archived reads. It pages 128 dispositions, including
maximum-length rationale,
and rejects wrong definitions, foreign sources and continuation mismatches.
Linked history preserves the assessment score, failure evidence, and the
recorded decision and reason. The retained check remains inspectable while the
third group requires its own matching check item to pass its current gate.

## Candidate history retention qualification

Run the registered private Engine child fixtures:

```sh
gaori --json run --tag rust --tag unit -- cargo test --locked -p podway-engine --features test-support --test int_suite int_group005_processes::trace004:: -- --nocapture
```

The nested `int_group005_processes::trace004::` filter selects all four
TRACE-004 cases. The broader `int_group005_processes::` filter includes them
with the GROUP-005 process cases in the same registered `int_suite` target.

The session-reinstallation cases use an explicit terminal current reset and
replacement history at the same session UUID and revision. They cover both
fresh and reused first attempt IDs through the private driver. The prior
continuation returns a stale-token error and the new token reads the next
group. Logical rows and durable SQLite bytes compare unchanged. An archived
continuation remains usable after a success in a separate current session.

The retention case commits real Engine activations and replans through the
64-group cap. Ordinary later success invokes journal pruning, and original
key lookup still returns the initial session outcome. Fresh children page
the retained lineage and compare the original DAG, attempt, item and
prior-work values across replan, pruning, exhaustion and archive. One-over
activation changes no logical rows or durable SQLite bytes. Cancellation and
archival succeed at that cap. The driver identifies its executable, writer
and DDL, fingerprints logical rows, and reaps its children and owned Git
fixtures.

The deletion case verifies exact workspace, session, revision and location
fences. Archive purge removes the selected history, including its goal
revision and unreferenced definition. It preserves original outcomes and the
sibling archive's goal history and referenced definitions. Terminal current
reset also removes its own outcomes. The fixture's whole-worktree reset
replaces logical SQLite state and workspace identity while preserving Git
and work files. Rejected destructive requests preserve the checked snapshot.
The production controls
`workspace_outcomes_age_and_count_prune_without_pruning_session_history`,
`delegation_disposition_archive_purge_and_session_reset_obey_retention` and
`reset_all_replaces_generation_and_replays_before_old_workspace_fences`
cover workspace-outcome retention, production session deletion and physical
workspace runtime replacement, respectively.

Core `activation_count_and_attempt_budgets_refuse_without_eviction` covers
64 activations and 65,536 cumulative attempts, including one-over refusals.
`exact_cumulative_placement_budget_counts_distinct_definitions` covers 16,384
cumulative placements and its one-over refusal. Config's
`frame002_actual_collection_limits_cover_nodes_definitions_ports_routes_and_manual_targets`
checks the per-group caps of 1,024 placements and eight routes per decision,
including one-over refusals.
`frame002_settlement_attainable_edge_composition_is_admitted_and_projected`
admits and projects the attainable 8,184-edge composition below the separate
8,192-edge ceiling.

## Execution-group candidate persistence checks

Run `int_group001_store` through the Engine's registered `int_suite`. It uses
Config-admitted canonical definitions and isolated generation-16 fixture files
adapted to the [candidate Store layout](../specs/storage/sqlite-model.md#execution-group-candidate-store).
The checks compare the complete Core state across cold reopen, proceed from the
restored cursor, and retain completed and abandoned group state through goal
revision and reactivation. Tampering cases cover definition and item digests,
cross-activation identity, immutable row conflicts and stale snapshot writes.
Additional cases preserve blocker creation order and ID ties across reopen,
reject blocker IDs repeated across attempts, and refuse a higher-revision
branch that changes earlier closed work. The suite reopens a guarded replan,
retained scoped delegation and typed item values, and cancellation. It proves
that candidate storage admits a full set of escaped prior-work rationales and
checks writer identity, ordered rows and byte bounds before decoding.
The production generation-15 classifier refuses the candidate fixture before
opening SQLite. The [candidate transaction checks](#execution-group-candidate-transaction-checks)
cover activation, terminal return, guarded replan, scoped cursor and item
writes, delegation, and blocker transitions through the Engine. The later
process-level suite owns contention, response loss and process interruption.

## Execution group integration checks

FRAME-004's registered case
`int_execution_groups::frame004_admitted_definitions_drive_core_group_lifecycle`
passes Config-admitted definitions and their canonical digests into Core. It
checks an isolated digest mismatch without state change, the current plan port
after activation, and a conditional terminal result that blocks return until
recorded. The base review reads that result from the exact completed group.
The declared replan route returns to the anchor and retains the completed
execution outcome.

The case uses the existing Config-to-Core dependency. The required development
gate, `make test` through Gaori `full`, also checks crate direction and public
Procedure v2 behavior. Config's registered
`int_procedure_document::frame001_successor_documents_remain_outside_public_v2_admission`
continues to check public successor-schema refusal. FRAME's integration case
exercises the in-memory model. Transactional persistence, wire admission,
history paging and the public cutover have their own verification owners.

## Execution group lifecycle checks

Run the Core library namespace `execution_groups::lifecycle::tests` through
Gaori for the pure FRAME-003 lifecycle. The cases cover tracked activation
without a goal, individual activation-identity mismatches, optional and
conditional boundary values, declared replan routes and feedback targets,
ordinary local rework and skip, and exact closure feedback and guard evidence.
They exercise the default scores 79 and 80 and an unevaluable optional guard
source. Separate Core-only fixtures probe boundary defenses after introducing
a route, missing return policy, or result declaration mismatch that Config
would reject.

Same-digest activations distinguish scoped receipt bindings, current result
values, retained earlier values, and base-review ordinals. Blocker cases check
refusal of omissions, rewritten rows, reused IDs and newly resolved rows, plus
explicit resolution and retained closure history. Selected refusal cases
compare the whole state before and after rejection. Creation-budget cases
exercise activation, distinct-placement and attempt caps. Core's terminal controls
exercise an artificial typed bypass that Config would reject. They refuse
completion with only a prefix assessment after the first group and after
same-goal replacement. A fresh assessment bound to the selected replacement
permits completion and remains readable after the selection closes. These
checks exercise the in-memory model. Transactional admission and process
recovery belong to the GROUP checks.

## Execution group model value checks

Core's `execution_groups::tests` verifies bounded immutable identities, exact
prompt-only item compatibility across all seven item types, prior-work records,
scoped delegation and decision-entry identity, score/basis bounds and successor
port/topology constructor limits. Header/document construction, selected-group
base entries, delegation with skip refusal, collection boundaries, self-reference
rejection and positive recorded-value revisions have deterministic coverage.
These checks establish value contracts only;
Config graph admission and execution transitions have their own obligations.

Protocol's registered `int_v2_result_contract::frame001_successor_schema_contracts_are_closed_and_registered`
checks closed successor schemas, shared references, activation history bounds and
manifest enrollment. The `frame001_record_fragments_pair_typed_values_with_closed_schema_boundaries`
and `frame001_nonempty_ports_and_shared_bounds_have_closed_shapes` tests exercise
nonempty typed record components, closed fields, port documents and shared limits.
Test-only enum spellings check component agreement; production wire projection
remains a separate owner. `frame001_scoped_guard_schemas_close_local_and_current_port_sources`
checks local/plan/current-result guard references and historical-source refusal;
Core's `scoped_guards_distinguish_local_plan_and_selected_current_result` checks
the matching typed source and unchanged predicate value semantics.
Protocol's `daemon_free_tests::frame001_successor_schemas_are_embedded_without_runtime_admission`
checks embedded registration without adding a public result family. Config's
`int_procedure_document::frame001_successor_documents_remain_outside_public_v2_admission`
keeps public v2 schema dispatch closed. Existing offline-registry and current-v2
conformance checks remain part of `make test`. These results do not prove group
persistence, concurrent activation, process recovery, history paging or product
CLI availability.
