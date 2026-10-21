# Podway Request Admission Reuse

## Status and authority

Status: Adopted. Owning epic: `ADMIT`. Design registered on 2026-10-19. The
[roadmap](../roadmap/README.md#admit-request-admission-reuse) owns order and status.
Accepted STATE completion is required before this epic starts. Current ADRs,
canonical assets and specifications retain authority over implemented behavior.
This design adopts invocation-local reuse. Workspace-internal Rust integration
may change; CLI grammar, JSON, storage and release contracts remain unchanged.

## Verified context and evidence

The ADMIT-001 characterization baseline is
`a5e77bb14d1958b71518b43fd118f97cf108e5a8` on 2026-10-21, after accepted
VIEWS, PARSE and STATE completion. This replaces the design-registration
baseline `98796e6a20467d1d072bd84f377fd9ddccccab93` (2026-10-19).
Function names are the primary anchors; source line references below describe
the characterization baseline, not a claim that planned reuse is implemented.

- The [workspace resolver](../../crates/podway-engine/src/workspace.rs) performs
  Git/config checks, a binding read, expected-UUID comparison and bound Git
  revalidation. The later [Direct read](../../crates/podway-engine/src/direct_execution.rs)
  opens another Store read, revalidates bound Git and then obtains its clock/view.
- Store's [graph preparation](../../crates/podway-store/src/direct_store/connection.rs)
  independently admits the current graph and obtains generation/sequence facts.
  Mutation paths already separate read preparation, artifact work and the
  authoritative writer transaction.
- READS preflight facts retain the full Standard status projection/schema and
  envelope-equivalent failure checks. Their guards are released before actual
  mutation preparation; there is a real competing-writer interval. CLI
  `Invocation::preflight_facts` and `Invocation::execute` make separate Engine
  requests with distinct request IDs within one CLI invocation.
- Store already owns `DirectPreparedGraphV1` and its writer-side
  `use_prepared_graph` admission. Sequence/session stamps can trigger reload;
  generation, manifest, workspace or root mismatch returns `BindingMismatch`.
- [Single-item attachment](../../crates/podway-engine/src/direct_items.rs) hashes
  the artifact after binding resolution but before graph preparation. Blindly
  fusing these stages would hold a lease over hashing or move error precedence.
- STATE's [read snapshot](../../crates/podway-store/src/direct_store/connection.rs)
  now stages graph/disposition admission for same-snapshot view reuse; it does
  not carry state between Engine requests. The current `prepare_graph` still
  admits the graph cold, then reads generation and sequence (`connection.rs:949`).
- There are eight preparation call sites: cursor and begin
  (`direct_execution.rs:121,312`), procedure (`direct_procedure.rs:125`), single
  item (`direct_items.rs:366`), reset/archive (`direct_sessions.rs:151`), start
  (`direct_start.rs:137`), record-many (`direct_items.rs:194`) and delegation
  acceptance (`direct_delegation.rs:71`). The first six are preflight-reachable;
  record-many and acceptance never preflight. The start site also prepares for
  plain non-dry-run start, which never preflights.
- Evidence with a page token deliberately defers the supplied workspace UUID:
  the resolver receives none; view and token-binding checks precede the
  in-snapshot UUID comparison (`direct_execution.rs:475-534`). Durable-selector
  identity admission remains in the resolver.

Existing [race checks](../../crates/podway-engine/tests/int_v2dfc006_races.rs) and
READS preflight checks are correctness owners. Separate start/reset observations
are intentionally distinct and are not duplication this epic may erase.

### ADMIT-001 responsibility and removal ledger

Stage names in this ledger describe the characterization baseline ownership.
The ADMIT-002 implementation record below describes the composed read paths:

- **B (binding):** native preliminary discovery, candidate/layout/config checks,
  qualified binding read including both file-witness checks, supplied and
  durable-selector UUID checks, then bound Git/path revalidation. Its read
  transaction, connection gate and native lease end before UUID/Git checks.
- **R (operation read):** a new qualified read, operation-stage native
  revalidation, command-specific clock/view/projection work, then its final
  witness check. Typed operation errors are carried inside `Ok(...)` until that
  check completes. All guards end when the read returns.
- **G (graph preparation):** its own qualified read, cold current-graph admission,
  generation and sequence acquisition, immutable prepared value. No guard escapes.
- **W (writer):** fresh lease/gate/read-only qualification, writable connection,
  `BEGIN IMMEDIATE`, qualification/file checks, outcome lookup, identity callback,
  key/digest checks, then exact replay or execution callback. Replay bypasses the
  callback, prepared-state selection, clock and IDs. The execution callback keeps
  its command-specific checks before effects and atomic success persistence.
- **F (fence preflight):** optional separate Engine request before the later CLI
  command/key checks. Full Standard status-equivalent admission precedes facts.
  Its guards are released before any following request or artifact preparation.

Each Engine request retains its own request ID. Within one CLI `Invocation`,
F, terminal-start reset dry-run, mutation and interactive reads share the original
deadline and observation but are distinct requests. CLI's concrete order is
stdin decoding/application -> flag/shape validation -> target/confirmation and
fence parsing -> `Invocation::new` -> F -> optional terminal-start reset preview
and policy resolution -> typed-command construction -> key validation ->
execution -> conditional interactive fallback (`command/direct.rs:193-407`).
Offline fresh-start preview returns before `Invocation::new`. Prepared-state
preflight rejection checks caller key requirements and any explicit session
revision before returning `SESSION_NOT_RUNNING` (`command/preflight.rs:265-305`);
it does not manufacture an attempt or goal fence. Its existing CLI owner is
`prepared_cursor_and_item_mutations_report_session_not_running_without_effects`.

| Owner and path | Eligible duplicate work | Retained responsibility |
|---|---|---|
| Read family `direct_execution.rs::execute_read_v1` (`:470`) | ADMIT-002 candidate: B's binding acquisition and R's first snapshot. | B -> R native check -> clock -> eager current view -> selected archive/receipt view -> command fences/projection/schema/envelope. Archive-list keeps summary-only archived rows; current-session admission remains eager. Preserve both native revalidations and both logical witness checkpoints. |
| Page-token evidence, same owner (`:475-534`) | Conditional ADMIT-002 read composition. | B omits supplied UUID; R native check -> clock/view -> token binding -> supplied UUID -> session/consumer fences and projection. Do not apply ordinary early-UUID ordering to this path. |
| Preflight `direct_execution.rs::preflight_facts_v1` (`:835`) | ADMIT-002 acquisition candidate and ADMIT-003 potential producer. | B -> R native check -> clock/view -> session fence -> full Standard projection/schema -> envelope-equivalent checks -> typed facts. Optional retention follows successful facts and adds no SQL/decode/validation stage. |
| Lookup `direct_execution.rs::lookup_outcome_v1` (`:971`) | ADMIT-002 read composition. | B -> R native check -> retained outcome decode/admission -> result -> clock/workspace sequence/envelope. No graph load or mutation; original response is retained. |
| Show `direct_workspace.rs::show_workspace_v1` (`:86`) | ADMIT-002 read composition. | B -> R native check -> binding-based result, clock/sequence/envelope. Do not add graph admission. |
| Doctor `direct_workspace.rs::doctor_workspace_v1` (`:120`) | Deliberately split. | B -> separate exclusive lease -> complete integrity verifier -> callback native check -> optional deep artifact rehash -> clock/view/projection. Verifier errors alone select the fallback read under that same exclusive lease; callback errors retain their own transport. Deep doctor retains its existing exclusion scope. |
| Reset dry-run `direct_sessions.rs::execute_session_v1` (`:66`) | Conditional ADMIT-002 read composition. | Optional F -> B -> R native check -> graph admission -> first session fences -> clock -> view -> fence recheck -> disposition/eligibility/result. Keep STATE's staged graph admission. |
| Cursor `direct_execution.rs::execute_cursor_v1` (`:92`) | Keep B/G split; ADMIT-003 consumer at G. | Optional F -> B -> canonical identity/digest/request -> G -> required-artifact preparation for complete -> W. Prepared selection, session/lifecycle/attempt fences, native revalidation, receipt readiness, artifact verification and clock retain their existing order. |
| Begin `direct_execution.rs::begin_session_v1` (`:269`) | Keep B/G split; ADMIT-003 consumer. | Optional F -> B -> canonical identity/digest/request -> initial-goal construction -> G -> W. Goal statement/definition/actor failures precede G; prepared lifecycle/goal-tracking checks precede IDs/clock inside W. |
| Procedure `direct_procedure.rs::execute_procedure_v1` (`:43`) | Keep B/G split; ADMIT-003 consumer. | Begin dispatch stays separate. Other commands: optional F -> B -> canonical identity/digest/request -> G -> decide artifacts and retained assessment construction -> W. Do not eagerly surface retained assessment/artifact failures ahead of their writer fences. |
| Ordinary single item `direct_items.rs::execute_item_v1` (`:300`) | Keep B/G split; ADMIT-003 consumer. | Optional F -> B -> canonical identity/digest/request -> retained pure item preparation -> G -> W. Session/lifecycle/attempt/item fences and native revalidation precede consumption of retained preparation failures. |
| Attach, same owner (`:331-366`) | Deliberately split across hashing; ADMIT-003 consumption stays at G. | Optional F -> B -> canonical identity/request -> native path hashing or external-reference construction -> retained item preparation -> G -> W. Never open G early or hold a read lease across hashing. Metadata verification stays after writer fences/native revalidation. |
| Record-many without paths `direct_items.rs::record_many_v1` (`:97`) | Keep split; no F or preflight hint producer. | B -> aggregate encoded-input bound -> count/duplicate checks -> item sorting -> canonical identity/digest/request -> retained attachment/pure preparation -> G -> W. Keep sorted item fences and deferred-error priority. |
| Record-many with paths, same owner (`:147-194`) | Deliberately split across artifact preparation. | Same sequence with native hashing before G, captured failures consumed after writer fences, metadata verification before clock. No preflight is introduced. |
| Delegation `direct_delegation.rs::accept_delegation_v1` (`:42`) | Keep split; G exists but there is no F. | B -> canonical identity/digest/request -> G -> W. Package/document/digest/action checks, receipt IDs and clock retain their existing stages. No receipt-readiness reuse follows from a graph hint. |
| Reset/archive mutations `direct_sessions.rs:122-153` | Keep B/G split; one ADMIT-003 consumer site. | Optional F -> B -> required key -> canonical identity/digest/scope/request -> G -> W. These callbacks revalidate native identity before prepared selection; retain this command-specific order. |
| Purge, same owner (`:148,156`) | Deliberately split; neither F nor G. | B -> key/canonical identity/request -> W -> confirmation/archive identity/revision -> clock/delete. Do not add current-graph preparation. |
| Plain start `direct_start.rs::start_session_v1` (`:65`) | Keep source/G split; G runs but F never does. | B -> retained source preparation -> required key -> G -> W. Replay identity recovers the original source digest. Fresh source failure is consumed only at its existing identity stage. |
| Replacement start, same owner | ADMIT-003 consumer only when F occurred; keep source/G split. | Optional F -> B -> retained source preparation -> key -> G -> W. Preserve replacement fences, current policy/dispositions, source confirmation/revalidation, title, IDs and clock. |
| Start previews and interactive fallback `command/direct.rs:257,325` | Deliberately distinct observations. | Terminal F may lead to a separate reset dry-run request before start. Stateful start dry-run prepares/confirms source and title before its later read. Interactive status follows only a plain-start decision-required response; continuation/replacement is another request. Never merge these observations or retain guards across prompts. |
| Repair/existing init `direct_workspace.rs::existing_mutation` (`:218`) | Keep split. | B -> canonical identity/request -> plain-init preliminary outcome observation where applicable -> W. Retained exact init can replay; new plain init refuses, including when retention prunes the preliminary outcome before authoritative lookup. |
| Fresh/recovering init `direct_workspace.rs::initialize_workspace_inner_v1` (`:303`) | Outside ordinary composition. | Candidate/config/classification -> exclusive intent/publication protocol -> native revalidation -> exact marker/recovery identity or fresh identity -> private Store publication. Preserve bootstrap ownership and refusal order. |
| Reset-all/remove `direct_maintenance.rs::reset_inner` / `remove_inner` (`:171,:514`) | Keep exclusive maintenance protocols separate. | Confirmation -> candidate/classification/destructive-generation policy -> exclusive intent/lease -> candidate revalidation -> exact marker/key/digest/UUID checks -> guarded reads/publication/removal phases. Preserve early no-runtime/residual branches; no ordinary G or F is added. |

The actual CLI-to-Engine call sequence remains unchanged. ADMIT-002 eligibility
does not authorize removing either native observation, independent writer
qualification, file witnesses or the attachment/source boundary.

### ADMIT-001 failure order and typed transport

Resolver Store failures are `DirectWorkspaceResolutionErrorV1::Store`; resolver
UUID/native failures use its `Resolution` arm (`workspace.rs:1089-1122`).
Execution callers retain either inside `DirectExecutionErrorV1::Workspace`.
Later read qualification/witness failures are instead `DirectExecutionErrorV1::Store`.
Workspace commands retain `DirectWorkspaceErrorV1::Resolution(...)` versus
`::Store(...)` (`direct_workspace.rs:33-70`). Writer callback failures stay distinct
from Store failures through `DirectMutationFailureV1`; no error side channel is
introduced.

Binding-read completion, including the final witness, precedes expected UUID and
bound Git (`workspace.rs:1374-1390`). Later reads carry their typed operation result
inside `Ok(...)`, so the final witness can beat a native/view failure
(`direct_execution.rs:488-659`; `connection.rs:160-181`). Ordinary `read` skips
that final check for an outer callback error; this differs from the exclusive
maintenance read's unconditional final witness. A single undifferentiated
composition callback would change precedence or wrappers.

ADMIT-002 must preserve a Store-owned binding witness checkpoint before UUID/Git,
resolver-stage early return without a new overriding operation witness, and the
later operation's final witness priority. If a path cannot preserve all three,
retain its split; changing their ranking requires Master's explicit decision.
The current observer API has no post-binding-read hook. Characterization therefore
pairs the ordinary Store callback/witness test with actual Engine resolver/operation
tests and source evidence for the intervening ordering. This is not a claim of
end-to-end injection at the resolver's post-binding witness.

Non-hashing does not imply mutation eligibility. Required key checks occur at
the common dispatch or command-specific boundary (`direct_api.rs:459`, session
and start handlers). Canonical identity construction can reject unsupported
numbers in caller JSON, with each handler's existing `InvalidInput` mapping.
Begin constructs goal values before G (`direct_execution.rs:296-311`);
record-many enforces encoded size, count and uniqueness before identity and
preparation (`direct_items.rs:35-194`). Start source/dry-run checks and captured
artifact/reference/pure-preparation failures keep their immediate or deferred
timing. The selected implementation boundary keeps all mutations' B/G split.
ADMIT-003 consumes a hint only inside each eligible mutation's existing G
qualified read, without a nested read, after every preceding check has run.

Recorded invariants, not fabricated fixtures: SHA-256 formatting produces a valid
digest; typed request IDs and fixed command names satisfy the corresponding
`DirectMutationRequestV1::new` guards; reset's later missing-graph guard follows
successful same-snapshot graph admission; rebuilding a public Git value loses
native transient evidence and cannot be a successful native control. STATE's
receipt-window and memory invariants remain recorded in the
[staged-state architecture](../architecture/system.md#staged-snapshot-state-reuse).

### ADMIT-001 eager-validation disposition

The [shared preservation boundaries](../roadmap/README.md#shared-preservation-boundaries)
explicitly permit an identity-checked invocation context. The Epic evaluation
consultation accepted the bounded interpretation within that adopted authority:
one fully admitted immutable graph body may be reused across the two requests
only with full preflight admission, mandatory preparation qualification,
independent native/file/binding checks, stateless cold preparation, integrity
verification, one cold fallback for optional comparison failure, and unchanged
writer replay/refusal/reload. ADMIT-003 must narrowly update
[Request-owned prepared value reuse](../specs/storage/transactions-concurrency-and-idempotency.md#request-owned-prepared-value-reuse)
to state the invocation-local exception and preparation-stage restamping.

Compatible writers advance the sequence/revisions and preserve immutable
metadata as described in that contract. Matching stamps do not establish
unchanged row bytes after out-of-protocol content modification. Today's cold G
detects such corruption even when every stamp remains unchanged; matching-hint
reuse may defer its detection to later cold reads or integrity checks. Direct
database modification is an [explicit limitation](../specs/operations/security-and-trust.md#explicit-limitations),
and need not be malicious. File witnesses establish object identity, not row
content. Existing writer-side Arc reuse (`transaction.rs::use_prepared_graph`)
is relevant precedent, not proof of arbitrary unstamped-corruption detection.

The new cold-preparation characterization pins only the stateless cold path;
it does not require matching-hint reuse to detect arbitrary unchanged-stamp
changes. Any extension beyond the bounded conditions above, or relaxation of
preserved validation or refusal behavior, requires Master's explicit decision
before ADMIT-003 implementation. Receipt readiness and attempt/receipt identity
checks are outside the graph stamps and keep independent acquisition, including
the prepared-Arc write path's point query (`system.md:478`).

### ADMIT-001 characterization and regression owners

New cases in existing registered suites:

- Store [direct suite](../../crates/podway-store/tests/int_v2dfc005_direct_store.rs):
  `admit001_ordinary_read_witness_precedes_carried_failure` pins binding-success,
  carried-inner-failure and outer-error transport with database/lock replacement
  and unreplaced controls. It checks one callback and unchanged post-injection
  files; no timing or new observer is used.
- Engine [error suite](../../crates/podway-engine/tests/int_v2dfc006_errors.rs):
  `admit001_resolver_and_operation_failures_keep_order_and_wrappers` pins native,
  qualification, UUID, bound revalidation and operation/view ordering using
  native-forwarding resolver faults, exact typed errors and a rejecting clock.
  It covers status, preflight, lookup and show's distinct wrapper family; show
  also proves that corrupt graph bytes do not add graph admission.
- The same Engine suite's
  `admit001_operation_witness_precedes_carried_native_and_view_errors` replaces
  a database or lock witness at the existing operation-stage native callback.
  It pins final Store maintenance refusal over carried native/view errors, with
  unreplaced controls and unchanged bytes, without adding a production hook.
- Store direct suite's
  `admit001_prepared_graph_identity_components_refuse_independently` isolates
  generation, manifest and UUID. Its Store uses the current manifest and its
  callback-entry assertion proves qualification passed and the prepared
  comparison ran. Refusal precedes clock/effects and leaves binding, sequence
  and the rejected outcome unchanged. Existing root coverage remains separate.
- Engine error suite's
  `admit001_cold_preparation_rejects_unstamped_post_preflight_corruption` admits
  real preflight facts, changes a hidden historical value without changing any
  comparison stamp, and checks stateless cold refusal before writer admission
  or clock, with unchanged files/stamps and no rejected outcome.

These source and registered tests map the later tasks' correctness owners:

- **ADMIT-002:** the new typed-order/witness cases; Engine
  `reads004_equivalence` in the error suite (success/envelope bytes, corrupt
  admission/timestamps, wrong session, out-of-range clock and native-evidence
  refusal); `direct_resolution_rejects_a_replaced_workspace_uuid_before_reading_the_clock`
  in the [execution suite](../../crates/podway-engine/tests/int_v2dfc006_direct_execution.rs);
  `v2scl004_production_dispatch_serves_and_fences_evidence_read` in the
  [readback suite](../../crates/podway-engine/tests/int_v2dfc006_readback_ports.rs)
  for token/UUID priority; Store `cold_read_without_shm_uses_a_write_free_heap_snapshot`,
  `hot_wal_read_snapshot_is_write_free_with_an_open_writer_process` and
  `foreign_writer_git_and_schema_fail_before_readwrite_open`; STATE's existing
  reset/doctor order and reuse owners in the
  [functional mapping](../implementation-tips/testing.md#functional-regression-owners).
- **ADMIT-003:** [race suite](../../crates/podway-engine/tests/int_v2dfc006_races.rs)
  `reads004_cold_facts_release_all_guards_before_a_competing_commit` and
  `reads004_shared_facts_release_all_guards_with_warm_sidecars` must also run
  through the actual context-bearing Engine path. Verify matching reuse,
  missing metadata, single consumption, changed stamps, comparison-read/decode
  failure and cold-loader error order. Distinguish hint discard, mandatory
  qualification refusal, writer reload and writer refusal. Store owners are
  `prepared_graph_refreshes_before_fences_and_replay_bypasses_preparation`,
  `prepared_graph_refreshes_after_sequence_only_success_and_replays_exactly`,
  `prepared_graph_cannot_cross_store_root_or_reuse_after_clock` and the new
  component test. The [query inventory](../../crates/podway-store/tests/int_reads001_query_inventory.rs)
  `prepared_reload_counts_unchanged_sparse_and_dense_selective_misses` owns
  reload accounting; source inspection must also establish no added preflight
  SQL/decode/validation because accounting excludes some qualification SQL.
  [Receipt owners](../../crates/podway-store/tests/int_v2sdg004_receipt.rs)
  `state004_readiness_acquisition_pins_window_derivation_absence_fallback_and_write_path`
  and `state001_prepared_arc_write_path_receipt_snapshot_refuses_unjoinable_active_receipt`
  retain independent readiness admission.
- **CLI wiring for ADMIT-003:** separate evidence must exercise actual automatic
  preflight/context wiring, distinct request IDs and attribution, unchanged
  deadline/autofill/key order, and no retained state in a later invocation.
  Current separate UUID allocations are source evidence (`command/direct.rs:63,106`),
  not a behavioral context test. The [CLI direct suite](../../crates/podway-cli/tests/e2e_v2dfc007_direct_cli.rs)
  owns `record_stdin_preserves_identity_applies_one_atomic_batch_and_replays_without_preflight`,
  `delegation_stdin_acceptance_receipt_and_completion_preserve_the_real_attempt`
  and the prepared-state rejection above. Do not introduce F on these stdin-owned paths.
- **ADMIT-004:** integrate both preflight-to-preparation and preparation-to-writer
  windows. [Identity ports](../../crates/podway-engine/tests/int_v2dfc006_identity_ports.rs)
  `runtime_database_replacement_before_begin_rejects_the_pinned_generation`
  owns writer file replacement. Existing race-suite
  `another_process_commits_during_actual_hashing_before_writer_admission`,
  artifact-stamp/fence cases and typed revalidation cases already own hashing
  guard release and preparation/writer races; no duplicate ADMIT-001 test is
  added. Execution/CLI replay, file-loss and lookup owners retain original
  responses and old-attempt fences. Execute complete
  [supported Store continuity](../implementation-tips/testing.md#supported-store-continuity),
  integrate unsupported-state/read-only refusal, required development gate and
  independent review, then promote durable guidance at closeout. Retained
  baseline artifacts alone are not continuity success.

Run new cases exactly through each crate's `int_suite` with
`--features podway-store/test-support`; CLI cases use `e2e_suite`. New test code
requires `make lint-all`; the Coordinator runs `make test` through Gaori before
acceptance. Read back this ledger, verify references and run `git diff --check`.
No release, installation or runtime operation is selected by characterization.

### ADMIT-002 implementation record

The implementation composes binding admission and the first operation for
`execute_read_v1` (including page-token evidence), `preflight_facts_v1`,
`lookup_outcome_v1`, `show_workspace_v1` and reset dry-run. Existing public Engine
signatures and CLI request sequencing are unchanged. All mutations retain their
binding/preparation split, as do doctor, bootstrap, maintenance and independent
start/interactive observations. Reset dispatch selects only its dry-run branch
before resolution. Mutation command derivation stays after split resolution and
before key/canonical preparation.

Store's `read_admitting_typed` retains its lease, gate and qualified transaction
for both stages. `DirectReadFailureV1` carries `Admission`, `Resolver` and
`Operation` failures explicitly. Engine's internal `resolve_existing_and_read`
uses the same preliminary/config and bound-binding helpers as stateless split
resolution. Admission failures become the existing workspace-resolution Store
wrapper; resolver callbacks retain their typed resolution wrapper; operation
witness failures retain the operation Store wrapper. Operation errors remain
carried values until the final witness succeeds. Neither Engine callback enters
Store error normalization or a read-only-recovery retry branch.

Store checks initial file identity, then the binding-completion witness before
UUID/bound Git resolution. Resolver failure exits without an overriding operation
witness. Resolver success requires an operation-entry witness before the existing
in-read native revalidation and command body, followed by the operation-final
witness even on a carried operation error. The narrow
`BeforeBindingReadCompletion` observer event occurs only immediately before the
binding-completion checkpoint, for deterministic test coordination.

The snapshot now starts during binding admission. Resolver-stage native
revalidation runs under the transaction/gate/read lease, including exclusive Cold
exclusion when required sidecars are missing. A compatible warm writer may commit
while the reader retains its earlier coherent snapshot. The removed internal
acquisition gap is not a promised scheduling opportunity. The original deadline,
nonblocking acquisition attempt, file/writer classification and guard release
before the Engine call returns remain required. There is no retained graph or
cross-request context in ADMIT-002.

New registered correctness owners, alongside unchanged ADMIT-001/READS/STATE
owners:

- Engine error suite's `admit002` module checks controlled successful admissions'
  single accounted binding query and three native observations; binding witness
  priority over UUID; resolver early return versus operation-entry witness;
  lookup/reset operation-final witness priority; cold write-free files/inodes
  with no SHM or no sidecars; exhausted deadlines and guard release after resolver
  rejection. Its matrix covers the selected entry points and ordinary read variants.
- Store direct suite's
  `admit002_typed_read_callbacks_are_once_only_and_not_store_normalized` checks
  exact callback counts and preservation of caller `Busy` errors in both stages.
- Engine race suite's
  `admit002_bound_revalidation_holds_guards_and_one_snapshot_with_a_real_writer`
  uses a bounded owned writer child: warm reads retain one coherent snapshot
  across a competing commit without writing files; Cold reads exclude the writer,
  which succeeds after return. It also checks process-gate/native exclusion at
  bound revalidation and a subsequent read's new sequence/revision.

The [read architecture](../architecture/system.md#read-path) records only this
delivered composition. ADMIT-003 still owns invocation-local prepared-graph reuse
and its specification amendment. ADMIT-004 owns integrated continuity/replay
qualification and program-documentation promotion. The roadmap remains the
acceptance/status authority; this record does not claim those later tasks passed.

### ADMIT-003 implementation record

`DirectInvocationContextV1` is an opaque, non-cloneable Engine-owned optional
`DirectPreparedGraphV1`, owned by CLI `Invocation` and passed through mutable
context-aware execution/preflight facades. Existing stateless entry points keep
their cold path. Facts and wire envelopes are unchanged. Every preflight clears
old state; only successful facts plus the owned view returned through the composed
read can populate the context after its final witness. Every eligible mutation
clears an unconsumed hint on return, including pre-preparation failures. Dry-run
and read-only requests leave it untouched; invocation drop ends its lifetime.

Store's existing qualification query now retains its validated generation in
private read-admission metadata. The current view carries it with the admitted
graph, binding and existing sequence. Its consuming conversion performs no SQL,
decode or validation and returns no hint if metadata is absent. Exclusive and
archived views carry no retention metadata. Source review complements existing
query/decode accounting because qualification SQL is not fully accounted.

Cursor, begin, Procedure, single-item, reset/archive and non-dry-run start consume
only inside their existing preparation read. Record-many and delegation acceptance
remain cold, and purge gains no graph preparation. Attachment hashing and start
source preparation retain their original position before this read. Full
preparation qualification and file witnesses remain mandatory. Store compares
root, manifest, generation, UUID, sequence, session/revisions and Procedure
snapshot/digest; a match reuses the graph body with preparation-stage stamps.
Optional comparison failure discards the hint and runs the unchanged cold
loader -> generation -> sequence body once in that same read. Writer selection,
replay, receipt readiness and integrity verification are unchanged.

The transaction specification now records this bounded exception and the accepted
unstamped-corruption limitation. The CLI/read/staged-state architecture records
actual ownership, publication and guard boundaries. No roadmap status or later
ADMIT-004 integration acceptance is established by this implementation record.

Focused correctness owners:

- Store `direct_store::connection::admit003_tests` checks matching body identity,
  single consumption, missing metadata, every identity/stamp mismatch, mandatory
  admission priority and cold fallback. Its comparison-result injection is
  `cfg(test)` only, unavailable through public APIs or `test-support` builds.
  Real SQLite query/conversion errors are injected at that private result boundary
  with a real corrupt cold loader; a separate non-STRICT SQLite fixture exercises
  the actual comparison decoder. STRICT production tables reject that malformed
  cell before it can be persisted through the supported contract.
- Engine race-suite `admit003_*` cases check equal preflight query/decode work,
  matching reuse, single consumption, seeded-context cleanup, both cold/warm
  READS-004 guard-release rendezvous paths, actual hashing with a competing writer,
  and generation/file replacement before preparation versus identity refusal
  before the writer. Existing stateless READS owners remain registered.
- Engine race-suite `admit003_reset_dry_run_and_read_preserve_pending_hint_until_mutation` pins preservation across reset dry-run and read calls, followed by single consumption.
- Engine error-suite coverage extends `reads004_equivalence` through the context
  facade and checks that a failing final preflight witness cannot publish or
  preserve a hint. The identity suite runs its database-replacement writer refusal
  through both stateless and context-bearing paths.
- CLI `command::direct::admit003_tests` checks distinct constructed request IDs,
  success-envelope identity and the unchanged invocation deadline/observation.
  The real-binary `admit003_*` case checks automatic preflight/context wiring,
  replacement/terminal-preview behavior, key/policy rejection, stdin refusal and
  later invocations. Only the existing token-gated debug READS mechanism exposes
  retention/consumption/reuse event counts; release and ordinary output gain none.

ADMIT-004 still owns integrated supported Store continuity, complete refusal/replay
qualification and program-documentation retirement. Task-local tests do not defer
ADMIT-003 correctness to that integration task.

## Goals, non-goals, and scope

Reuse eligible binding/first-snapshot admission and an immutable preflight graph
within one CLI invocation while preserving its distinct Engine requests and
every authoritative revalidation boundary. Reduce repeated acquisition and
preparation. External CLI call sequences remain unchanged.

No new CLI flags/commands, JSON schema, output field, persistent/global cache,
Store layout, writer identity, migration, runtime service or consumer change is
adopted. Do not merge separate start/reset observations or hold leases across
mutation artifact hashing or prompts. No elapsed-time tests, latency percentiles,
throughput benchmarks, performance baselines or thresholds are allowed.

## Accepted design decisions and interfaces

### Binding and first snapshot

Compose binding admission and the eligible first operation through an internal
typed operation/callback under the existing Engine/Store owners. Reuse one
qualified read where it preserves the command's resolver wrapping and complete
order: native Git/config discovery, binding checks, expected UUID, bound Git
revalidation and operation-specific clock/view checks.

Do not remove transaction qualification, no-SHM checks, file witnesses or writer
contract classification because the binding was read earlier. Preserve each
command's existing native identity checks at their existing stages; merging those
checks is not an acceptance target. Engine composes this work; CLI must not
access Store internals and Core gains no filesystem or connection responsibility.

Apply composition per eligible path. For attachment paths with hashing between
binding and graph preparation, keep the split and its error order rather than
opening the later snapshot early or retaining a read lease over hashing. A
structural optimization need not eliminate every read from every command.

Mutations still need the admitted workspace UUID to form their canonical request
digest, and binding/graph facts to prepare required artifacts, before entering
the writer transaction. Do not defer those prerequisites into the writer merely
because read-only commands can compose their first read. Preserve existing
input-bound and idempotency-key checks before later preparation. Do not remove
per-artifact native identity checks as an incidental optimization.

### Preflight and mutation preparation

Use one opaque Engine-owned invocation context, owned by the CLI's existing
`Invocation`, to carry an optional prepared graph between the two in-process
Engine calls. The context retains at most one immutable Store-prepared graph,
contains no live read/writer guard, and is discarded after the eligible mutation
or invocation ends. Keep `DirectPreflightFactsV1` and wire envelopes unchanged;
CLI carries the opaque context without inspecting Store types or stamps. Reuse
Store's existing stamp/admission owner rather than adding a parallel token model.
Stateless Engine callers retain their existing path without this optimization.

Preserve the preflight and mutation request IDs, error attribution, caller-key
surface, deadline and fence-autofill order independently. Standard projection,
schema and envelope-equivalent checks still run before returning facts. Build
the retained Store-prepared graph only from values successful preflight admission
already produced: the qualified writer-contract generation, binding, admitted
graph and view's committed sequence, plus Store-owned root and manifest. Retain
these values in Store-owned read-admission metadata rather than querying them
again. Assemble the optional context after facts succeed, without a second graph
load or any additional SQL statement, decode or validation stage. If a stamp is
unavailable, return facts without retained state; optional retention introduces
no new failure before the CLI's existing input/key checks. Facts-only extraction
is not equivalent preflight: all existing checks remain mandatory.

Release all preflight guards before mutation preparation. Consume the retained
graph only at the existing `prepare_graph` stage, under that stage's current
qualified read, lease, binding and native file revalidation. Store compares its
existing stamps: root, contract manifest digest, generation ID, workspace UUID,
committed sequence, session ID/revision, workspace revision, Procedure snapshot
ID and Procedure digest. Reuse the graph body only if every comparison matches;
stamp the result as prepared at this current read stage.

Any optional-hint mismatch, including identity mismatch or a failed comparison
stamp read/decode, discards the hint and uses today's ordinary cold
`prepare_graph` path. Hint comparison itself never reports a public error or
reclassifies the error the existing graph loader would report. The hint alone
cannot introduce `BindingMismatch` earlier than today. This fallback applies
only to optional hint comparison, once; it does not catch, normalize or retry
authoritative read/binding/native refusal or failure from the cold path. Those
failures keep their existing error and precedence. Stale caller fences are never
silently retried.

Artifact preparation uses the graph admitted at this preparation stage, never
an unchecked preflight graph. Only the resulting current Store-prepared value
can enter the writer transaction's existing `use_prepared_graph` mechanism.
There, sequence/session/Procedure-stamp changes retain the existing reload
behavior; generation/manifest/workspace/root mismatch still returns
`BindingMismatch`. Preserve replay before that check. This keeps the writer's
prepared-state identity window at preparation-to-write rather than widening it
to preflight-to-write. No read lease spans artifact preparation or the gap.

Preserve replay lookup and request identity precedence, including replay before
new clock/ID allocation or state changes. The unchanged response is the original
stored response, not a fresh projection. Artifact bytes remain outside writer
transactions. A hint must not extend a read lease across preparation work,
prompts or the real competing-writer interval.

No cross-invocation token registry, hidden retry loop or long-lived cache is needed.
Do not reintroduce BOUND's removed error side channels. Existing facts and graph
preparation owners remain responsible for their independent failure mapping.

## Failure handling and compatibility boundaries

Preserve expected workspace UUID comparison, bound Git/worktree identity,
descriptor/file replacement detection, unsupported legacy/newer/foreign-writer
refusal, hot-WAL and missing-SHM read-only qualification and maintenance deadlines.
Do not infer the active worktree from a project registry or scan other projects.

Keep full eager stored validation, including hidden historical values, and the
archive-list summary-only boundary. Retain exact error codes/details/exit and
retryability, failure precedence, stale-fence behavior, clocks/IDs, success-only
atomic outcomes and byte-identical old-attempt replay. Separate terminal-start
reset dry-run and interactive post-decision observations remain unchanged.

If combining two stages cannot preserve these properties, retain their split.
Changing their meaning requires separate adoption, potentially through the
[CALLS candidate](TODO-podway-cli-interaction-simplification.md), not an ADMIT
shortcut.

## Roadmap ownership, dependencies, and traceability

| Task | Deliverable and focused verification |
|---|---|
| `ADMIT-001` | Map per-command native/binding/graph/preflight/artifact/writer order, distinct Engine requests, identity/digest prerequisites, resolver error wrappers and guard lifetimes; identify eligible and intentionally split paths. |
| `ADMIT-002` | Compose eligible binding/first-operation reads; verify UUID/native identity priority, clock/view failures, no-SHM/hot-WAL behavior and retained attachment hash boundaries. |
| `ADMIT-003` | Carry one Store-prepared graph in an opaque Engine invocation context using already admitted metadata; verify no new preflight failure stage, cold fallback on hint mismatch or comparison failure, and unchanged writer refusal/reload, request IDs and Standard preflight failures. |
| `ADMIT-004` | Integrate real competing-writer, file replacement, unsupported-state refusal, continuity and exact replay evidence, required development gate and independent review; promote documentation and retire program-planning text in both indexes and testing guidance. |

The roadmap alone owns task status. Each task verifies its own success and
failure paths; ADMIT-004 integrates rather than postpones that verification.

## Verification and release acceptance

Use existing native identity, Store admission, race, typed preflight and replay
cases. Verify guard release with deterministic synchronization and an actual
competing writer, without elapsed-time comparisons. Exercise matching reuse,
changed revision/session/snapshot, and replaced Store/file witness as distinct
cases. Require hint discard at preparation or authoritative refusal/reload at its
existing boundary, with no failed outcome or unauthorized state change. Include
replacement between preflight and preparation and between preparation and writer
as separate cases; the optional hint must not widen the latter refusal window.
Verify separate preflight/mutation request IDs, unchanged stdin-owned/no-preflight
paths, and absence of retained context state in later invocations.

Verify that optional retention adds no preflight SQL/decode/validation stage and
does not pre-empt later CLI input/key errors. Pair an optional comparison failure
with a graph-loading fault and require the existing cold-path error and order;
separately prove that mandatory admission errors still propagate without retry.
Use the existing deterministic admission/query fixtures where applicable, without
new timing instrumentation or a general measurement framework.

Check combined-fault precedence as well as success. Preserve old-attempt replay,
replay after continuation, and supported Store identity without reset/migration
using isolated fixtures. No user worktree or installed runtime is a test target.
Future executable changes require focused checks and `make test` before sharing;
historical latency/scale qualification is not inherited as a new gate.

No release or installed-runtime operation is selected. Promote durable boundaries
and verification guidance before [dossier retirement](README.md#lifecycle), and
replace the program's remaining planning guidance with implemented ownership.

## References

- [Shared program boundaries](../roadmap/README.md#shared-preservation-boundaries)
- [System architecture](../architecture/system.md)
- [Transaction contract](../specs/storage/transactions-concurrency-and-idempotency.md)
- [Supported Store continuity](../implementation-tips/testing.md#supported-store-continuity)
- [Structural verification](../implementation-tips/testing.md#structural-simplification-verification)
