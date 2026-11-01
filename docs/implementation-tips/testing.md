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

Through Gaori with the pinned repository toolchain, run `make architecture`,
`make contract-manifest` and `make test-int`. Architecture's static prerequisites
include documentation/link checks, contract/schema integrity and the prepared
adapter self-test; `python3 tools/verify_contracts.py --sentinels` supplements
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
- The GUIDE-006 baselines are the byte-level regression owners of the default
  generation: the Engine [baseline suite](../../crates/podway-engine/tests/int_guide006_generation1_baselines.rs)
  `int_guide006_generation1_baselines` and the CLI [baseline suite](../../crates/podway-cli/tests/e2e_guide006_generation1_baselines.rs)
  `e2e_guide006_generation1_baselines`, with the exact expected bytes under
  `tests/guide006_baselines/`. Their expected values change only through an
  owner decision such as generation-1 removal. The two suites normalize
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
regression owners, with the continuing interaction candidate retaining its
unselected questions and DF-050 retaining its independent owner.

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
remain separate from completed structural-program evidence. Combined operations
and returned observations remain unadopted.

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
not actual Dolgorae execution or complete integration qualification. The complete
manifest-specific normal Store read/write/replay matrix remains INTEG-005 work;
INTEG-003 separately qualifies its query against an authentic previous-manifest Store.


## Repository metadata verifier controls

`make dolgorae-connection-test` runs every deterministic control in
[`verify_dolgorae_connection_tests.py`](../../tools/verify_dolgorae_connection_tests.py).
It is registered in `architecture-static`, hence `make test-prepare` and the
required `make test` gate. Select a Python environment that already provides
`jsonschema` and `referencing` through
[`tools/requirements-dev.txt`](../../tools/requirements-dev.txt). For a new local
development environment, install that file into an isolated virtual environment
and put its Python on PATH before Make; no global Python modification is needed.
The verifier reports missing validator prerequisites as incomplete; the control
gate fails rather than skipping positive validation.

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
The normal Store read/write/replay matrix remains INTEG-005 work.
