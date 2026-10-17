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
