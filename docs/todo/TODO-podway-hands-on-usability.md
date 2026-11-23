# Podway Hands-on Usability Corrections

## 1. Status and authority

- Document state: `Adopted`
- Dossier type: epic execution design
- Owning epic: [HANDS](../roadmap/README.md#hands-hands-on-usability-corrections)
- Planning release target: none assigned
- Repository scope: Podway only
- Delivery boundary: adopted design; product execution requires its separately approved workflow
- Architectural prerequisite: none. HANDS stays inside [ADR-0039](../architecture-decision-records/0039-opt-in-contract-generations.md) and changes no Procedure, Store layout, runtime model or contract manifest.

The [roadmap](../roadmap/README.md) alone owns task identity, ordering,
dependencies and status. This dossier owns the decision-complete plan for the
selected corrections. It grants no implementation, test run, commit, release,
installation, runtime activation or external-repository change. Accepted ADRs,
canonical assets and current specifications remain authoritative until the
named tasks change them.

### How to use this dossier

Execute one task at a time, in roadmap order. For every task:

1. Read the task's cards in section 4. Read only their listed files before editing.
2. Reproduce each finding with the card's **Reproduce** commands, using a
   freshly built debug CLI (`cargo build --locked -p podway-cli`) in a scratch
   Git repository outside this checkout.
3. Write the card's regression test first. Run it and confirm it fails for the
   stated reason.
4. Implement exactly the card's **Change**. Use the exact message strings.
5. Run the card's focused tests, then the task gate in section 7.
6. Update every file in the card's **Also update** list in the same task.
7. Stop and report instead of improvising when any **Stop if** condition holds.

Global invariants for every task; a violation means stop and report:

- **No manifest change.** `make contract-manifest` must leave
  `contracts/contract-manifest-v2.json` byte-identical. Do not edit `assets/`,
  `contracts/`, `tests/fixtures/` or anything else the manifest enrolls.
  HANDS therefore needs no writer predecessor, and `v0.2.18` Stores stay usable.
- **No machine-contract change** beyond the single H2 classification change.
  Keep error codes, exit codes, retryability, details objects, result schemas and
  JSON output byte-identical.
- **No user-value echo** in new messages, except the template-placeholder token
  defined in H1-b (bounded to 64 characters).
- **No new dependency.** Use the existing crates only.

## 2. Verified context and evidence

An owner-requested hands-on evaluation on 2026-11-23 exercised the published
`v0.2.18` Apple Silicon archive in isolated scratch Git fixtures. The archive's
source commit is `a180561c`, and its checksum and provenance were verified. The
evaluation covered start, begin, recording, group activation, rework, guarded
replan with prior work, goal assessment, custom Procedure authoring, archive,
legacy-Store refusal, reset, workspace removal and concurrent writers. All 223
captured envelopes validated against the packaged schemas. No correctness
defect was found. A comparison with the installed `v0.2.17` showed seven
findings were inherited and three are new with the Execution Group surface.

| ID | Finding | Origin |
|---|---|---|
| H1 | Syntax, stdin-schema, blank-reason and domain-state refusals are generic. | Inherited; stdin placeholder case new |
| H2 | A history query with a valid group digest but no `--group-execution` returns retryable `GROUP_DEFINITION_MISMATCH`. | New |
| H3 | `GROUP_ACTIVATION_NOT_ALLOWED` shows one message for every cause. | New |
| H4 | Legacy-Store refusal names only the destructive reset, not the original-writer path. | Inherited |
| H5 | `init` writes an inert `default_preset`. | Inherited |
| H6 | `procedure preview` and `start --procedure` reject absolute paths that `check/vet/lint/show` accept. | Inherited |
| H7 | `workspace remove --json` reveals its requirements one refusal at a time. | Inherited |
| H8 | Human `status` and `next` dump the whole result as JSON. | Inherited |
| H9 | Citation-free `satisfied` assessments are admitted. This is specified behavior, and its consequence is undocumented. | Inherited (specified) |
| H10 | The minimal scaffold is byte-identical to built-in `small-change-v3`. | New |

Facts verified in source on 2026-11-23 that constrain the design:

- Engine-mapped errors always use the catalog `summary` as `message`
  (`crates/podway-engine/src/direct_error.rs`, `Presentation` and the envelope
  builder near the `CATALOG` lookup). CLI-local failures
  (`LocalFailure::request_invalid` in `crates/podway-cli/src/command.rs`) carry
  free text. The [JSON contract](../specs/interfaces/json-contract.md) states
  that human messages are not stable API.
- ADR-0039 decision 1: all generations share codes, exits, retryability and
  messages. Changing generation-2 selections has required an ADR (ADR-0044).
  H4 therefore uses human hints, not a new details family.
- Legacy classification reads only file headers
  (`crates/podway-store/src/direct_store/classification.rs`). It cannot read the
  stored writer manifest. Store generation 15 was written by `v0.2.13` through
  `v0.2.17`. Generations 1–13 were written by `v0.2.12` or earlier daemon-era
  releases. No release shipped generation 14.
- `v0.2.15`–`v0.2.17` support `--contract-generation 2`. Their
  `WORKSPACE_CONTRACT_MISMATCH` details name the stored writer release
  (`actual_product_version`).

## 3. Goals, non-goals, and scope

Goals:

- Make each refusal identify its correctable cause.
- Show the non-destructive legacy recovery path in human output.
- Remove inert or surprising authoring and configuration behavior.
- Make human status output readable, and document citation responsibility.

Non-goals:

- No Procedure, Store, runtime-model, manifest or details-schema change.
- No structured legacy alternatives; [DF-055](../deferred-feedback/README.md) retains them.
- No citation enforcement. H9 is documentation only.
- No unification of destructive confirmation grammar.
- No release, installation, migration or consumer-repository change.

## 4. Accepted design decisions and implementation cards

### Shared mechanism M1: Engine message suffix (used by H1-c, H2-b, H3)

- **Change:** add `message_suffix: Option<String>` to `Presentation` in
  `crates/podway-engine/src/direct_error.rs`. `simple()` sets `None`. When it is
  `Some(s)`, the envelope `message` becomes `"{summary} {s}"`, with exactly one
  space. The summary prefix stays intact, so existing prefix checks still hold.
  Cap the final message at 512 Unicode scalars by truncating the suffix.
- **Stop if:** any envelope validator or test requires `message == summary` for
  runtime output. Only the example-fixture check in
  `crates/podway-protocol/tests/int_v2_result_contract.rs` compares a fixture
  example; that is not runtime output.

### H1: Name correctable causes (HANDS-001)

**H1-a Unknown argument and invalid value**

- Reproduce: `podway --json archive show --session x` gives
  `REQUEST_INVALID` with `invalid command syntax`.
- Change: in `syntax_failure_message` (`crates/podway-cli/src/command/grammar.rs`)
  handle `ErrorKind::UnknownArgument`, `ErrorKind::InvalidValue` and
  `ErrorKind::ValueValidation` after the existing `MissingRequiredArgument`
  branch. Resolve the selected subcommand as the existing loop does. Never echo
  the unknown token or the rejected value.
  - Unknown argument:
    `unknown argument for <route words>; run podway help <route words>`,
    for example `unknown argument for archive show; run podway help archive show`.
  - Invalid value (either kind): `invalid value for <label>`. `<label>` is
    regenerated from the grammar exactly like the missing-argument label, for
    example `invalid value for --timeout <DURATION>`.
- Test: extend `crates/podway-cli/tests/e2e_v2dfc007_local_cli.rs` next to the
  existing `missing required argument` assertions. Assert the exact message for
  `archive show --session x` and for `status --timeout banana`; the latter
  currently returns `invalid command syntax`.

**H1-b Successor stdin document**

- Reproduce: in a `small-change-v3` replacement activation, submit the
  `group.activate` `stdin_template` with `prior_work` left as
  `"<prior-work-dispositions-array>"`. Result: `invalid successor input document`.
- Change: in `decode` (`crates/podway-protocol/src/successor_input.rs`), when
  `validate_daemon_free_document_v1` returns `false`, walk the parsed value
  depth-first in key order. If a string equals the regex `^<[^<>]{1,62}>$`,
  return that string and its RFC 6901 pointer in the error message:
  `invalid successor input document <schema>: replace template placeholder "<token>" at <pointer>`.
  Otherwise return `invalid successor input document <schema>: the document does not match its schema`.
  Search only after validation failed, so valid input never changes behavior.
  Keep the existing `SliceErrorV1` variant shape. If the variant's field is a
  `&'static str`, add a new variant carrying an owned message, rather than
  leaking strings.
- Test: an `int_suite` case in `crates/podway-protocol/tests` for both
  messages, and one e2e case in a new `crates/podway-cli/tests/e2e_hands_usability.rs`
  for the activation placeholder.

**H1-c Blank reasons and domain-state refusals**

- Reproduce: after `begin`, run `podway --json block --reason "   "` (also
  `retry` and `cancel`). Result: `invalid domain state: record reason`.
- Change:
  - Before every CLI `ReasonV2::new`, call a new
    `validate_reason_cli(value, "--reason")`. The call sites are
    `crates/podway-cli/src/command.rs` (decide and rework) and the `reason()`
    helper in `crates/podway-cli/src/command/successor_cursor.rs`. Message:
    `--reason must contain a non-whitespace character`. Leave length limits to
    the existing domain checks.
  - Engine path: every mapping of `DomainError::InvalidState { reason }` to
    `REQUEST_INVALID` in `direct_error.rs` sets M1 suffix `({reason}).`.
    `reason` is a `&'static str`. This covers settled or unknown criteria,
    for example the core text
    `criterion assessment repeats or names an unknown criterion`.
- Test: e2e for block, retry and cancel with a blank reason. An `int_suite`
  Engine case for assess-criteria naming an unknown criterion asserts the
  suffix.

### H2: History definition selection (HANDS-001)

**H2-a Group digest without execution**

- Reproduce: complete one group in `small-change-v3`, then run
  `podway --json history dag --session <S> --definition-digest <group digest>`.
  Result: `GROUP_DEFINITION_MISMATCH`, retryable.
- Change: in `SuccessorHistoryDefinition::resolve`
  (`crates/podway-engine/src/successor_execution/history/detail.rs`, the
  `Self::Base(_)` arm), check whether the digest equals the definition digest of
  any group retained by the state. If so, return a new
  `SuccessorExecutionError::HistoryGroupExecutionRequired`. Map it in
  `direct_error.rs` to `REQUEST_INVALID` with M1 suffix
  `--definition-digest names a retained group definition; add --group-execution <uuid> from group list.`
  Every other mismatch keeps `GROUP_DEFINITION_MISMATCH`. The check runs before
  paging and reads no extra state.
- Also update: the history route rows in
  [CLI specification](../specs/interfaces/cli-specification.md) and the error
  precedence text in [errors and exit codes](../specs/interfaces/errors-and-exit-codes.md).
  Do not edit `assets/specifications/error-codes.json`; `REQUEST_INVALID`
  already exists.
- Test: e2e for `history dag` and `history attempts` in
  `e2e_hands_usability.rs`, asserting code, exit 2, `retryable: false` and the
  message. A base-digest mismatch must still be `GROUP_DEFINITION_MISMATCH`.

**H2-b Activation-time mismatch digest**

- Change: at the activation comparison in
  `crates/podway-engine/src/successor_execution.rs` (`if prepared.digest() !=
  &request.expected_definition` before the `GroupDefinitionMismatch` return),
  carry `prepared.digest()` in the variant. Set M1 suffix
  `Supplied source digest: <sha256:…>.`
- Test: an activation with a wrong `expected_definition` asserts the suffix.

### H3: Activation causes (HANDS-001)

- Reproduce: (a) activate when the template is absent; (b) replacement
  activation with `prior_work: []`; (c) a disposition citing an unknown
  execution. All three give the same message.
- Change in `direct_error.rs`:
  - `GroupActivationNotAllowed`: suffix
    `The current attempt is not the unselected base anchor; observe and use the current group.activate template.`
  - `GroupActivationPriorWorkInvalid(DomainError::InvalidState { reason })`:
    suffix `Prior work is invalid ({reason}).`
  - Other `DomainError` inner values: suffix `Prior work is invalid.`
- Test: e2e for (b) and (c), and an `int_suite` case for (a).

### H7: Removal confirmation (HANDS-001)

- Change: in `crates/podway-cli/src/command.rs`, replace the two sequential
  `workspace remove` checks with one check that collects every missing
  requirement in this fixed order: `--force` (JSON only), `--yes`,
  `--if-workspace-uuid`. Message:
  `JSON workspace removal requires --force, --yes and --if-workspace-uuid; missing: <comma-separated list>`.
  For non-JSON, non-interactive use the prefix is
  `non-interactive workspace removal requires --yes and --if-workspace-uuid; missing: …`.
- Test: e2e for each missing combination; exit 2 and `REQUEST_INVALID` unchanged.

### H4: Non-destructive legacy guidance (HANDS-002)

- Reproduce: create a Store with the verified `v0.2.17` CLI in a scratch
  repository (`init`, `start --preset small-change-v2`), then run the candidate
  `podway status` (human) and `--json status`.
- Change: in `crates/podway-cli/src/command/render.rs`, add
  `legacy_runtime_state_hint_lines(details)` next to
  `workspace_contract_mismatch_hint_lines`, and call it for
  `LEGACY_RUNTIME_STATE_UNSUPPORTED` in `render_human_error_direct`. It prints
  these lines, in this order, after the existing lines:
  1. `stored Store generation: <n>` (omit when `store_schema_version` is null).
  2. Writer line from a CLI-local constant table:
     - generation 15:
       `to keep history, finish or abandon the session with its original writer: Podway v0.2.13 through v0.2.17`
     - generations 1–13:
       `to keep history, use the Podway v0.2.12-or-earlier release that created this Store`
     - otherwise:
       `to keep history, use the Podway release that created this Store`
  3. Generation 15 only:
     `with v0.2.15 or later, --contract-generation 2 --json observe names the exact writer release when it is not the writer`
  4. Always:
     `reset --all destroys current and retained history and requires explicit authorization`
- JSON output, details, code, message and `recovery` stay byte-identical in
  both generations.
- Also update:
  - [errors and exit codes](../specs/interfaces/errors-and-exit-codes.md): the
    legacy paragraph names the human hint lines.
  - [CLI specification](../specs/interfaces/cli-specification.md): the
    human-hint section.
  - [release upgrade QA](../implementation-tips/release-upgrade-qa.md): the
    Store 16 refusal row expects these human lines.
  - The runtime skill [recovery reference](../../skills/use-podway/references/recovery.md#older-writers):
    add the generation-to-writer table.
- Test: e2e using an authentic frozen generation-15 fixture or a generated
  `v0.2.17` Store, following the existing legacy-refusal e2e pattern. Assert
  the exact stderr lines, byte-identical JSON in both generations, and
  unchanged Store bytes with no SHM.
- Stop if: no authentic generation-15 fixture is available offline. Report
  `INCOMPLETE`; do not fabricate one.

### H5: Inert default preset (HANDS-003)

- Change: remove the `default_preset: sw-dev-v3\n` line from
  `DEFAULT_WORKSPACE_CONFIG_YAML_V1` in `crates/podway-config/src/lib.rs`. Keep
  the struct field, the serde default and the schema property, so existing files
  still parse. Do not rewrite existing configuration on reset, removal or init.
- Also update: `crates/podway-config/tests/int_phase4_workspace_config.rs`,
  `crates/podway-engine/tests/int_v2dfc006_workspace.rs` (the
  `default_preset: sw-dev-v3` line assertion),
  [git-worktree-and-filesystem](../architecture/git-worktree-and-filesystem.md),
  `docs/examples/.podway/config.yaml` and the configuration text in the
  [CLI specification](../specs/interfaces/cli-specification.md). State that the
  field is accepted, ignored and reserved.
- Leave `V15_FIXTURE_WORKSPACE_CONFIG_YAML_V1` untouched.
- Test: the existing init tests assert the new bytes. A config containing
  `default_preset: sw-dev-v2` still initializes and `doctor` stays healthy.

### H6: Absolute in-worktree paths (HANDS-003)

- Reproduce: `podway --json procedure preview "$PWD/.podway/procedures/x.yaml"`
  and `podway --json start --procedure "$PWD/.podway/procedures/x.yaml" …` are
  rejected.
- Change: add `relativize_procedure_path(path, roots) -> Option<PathBuf>` to
  `crates/podway-cli/src/command/procedure_files.rs`. For an absolute path,
  strip the first matching root **lexically**, never canonicalizing the input
  path, and return the remainder. Candidate roots:
  - `preview`: the process working directory as returned by
    `std::env::current_dir()`, and its canonical form.
  - `start --procedure`: the worktree spelling supplied with `--worktree`, or
    the current directory, plus the canonical worktree root the request resolves.
  Apply it before the existing `is_worktree_relative_procedure_path` checks in
  `crates/podway-cli/src/command.rs` and `successor_authoring.rs`. Then run the
  unchanged relative validation and `O_NOFOLLOW` open, so symlink, `..` and
  outside-root refusals stay as they are. A relative path is unchanged. The
  normalized relative path is what enters request identity and output.
- Test: e2e for an accepted absolute preview path, an accepted absolute start
  path (session `procedure` reference equals the relative spelling), an
  absolute path outside the worktree (still refused), and a symlinked component
  (still refused). Existing path-safety tests must pass unchanged.

### H10: Distinct scaffold identity (HANDS-003)

- Change: `scaffold` in `crates/podway-cli/src/command/successor_authoring.rs`
  currently returns the `small-change-v3` preset bytes. Add a constant
  `SCAFFOLD_TEMPLATE_MINIMAL_V3` beside the existing v2 scaffold in
  `crates/podway-config/`. It equals `assets/presets/small-change-v3.yaml` with
  only these lines changed:
  - `id: custom-procedure-v3`
  - `name: Custom Procedure`
  - `purpose: Replace this purpose with the outcome your Procedure delivers.`
  - `description: A minimal Procedure v3 starting point derived from small-change-v3.`
  Compute the result `digest` by validating that constant through the same
  admission `procedure check` uses. Do not reuse the preset digest.
- Also update: [procedure and item specification](../specs/domain/procedure-and-item-specification.md)
  (`scaffold` sentence) and the CLI specification scaffold row.
- Test: a podway-config unit test proves the constant is canonical (format is
  a no-op), passes every authoring stage, and has a digest that differs from
  every embedded preset digest. Update the existing scaffold e2e cases in
  `e2e_v2dfc007_local_cli.rs` only where they assert the identity.

### H8: Human status summary (HANDS-004)

- Change: in `render_human_output_direct`
  (`crates/podway-cli/src/command/render.rs`), when `command` is
  `session.status` or `session.next`, the result schema is an observation
  result, and the invocation is not `--verbose`, replace the final
  `result: <pretty JSON>` line with these lines. Omit a line whose source is
  null or empty.
  ```
  lifecycle: <lifecycle>
  attempt: <scope kind> <node>[ group <group_execution_id>]
  ready: <true|false>
  missing items: <comma-separated item ids>
  open blockers: <count>
  operations: <comma-separated guidance.available_operations>
  goal: <outcome or "undetermined"> (revision <n>)
  hint: use --json for the complete result, or --verbose for the full human dump
  ```
  `--verbose` and every other command keep the current dump. JSON is unchanged.
- Test: e2e asserting the exact human lines for a prepared, a running and a
  completed session. The test that parses `result: ` in
  `e2e_v2dfc007_runtime_cli.rs` uses `history attempts`, so it is unaffected.
  GUIDE-006 baselines are JSON and stay unchanged.

### H9: Citation responsibility (HANDS-004)

- Change (documentation only): add this sentence to the
  `goal assess-criterion` paragraph of the
  [CLI specification](../specs/interfaces/cli-specification.md), the
  `goal assess-criteria` help text in `grammar.rs`, and the "Assess and decide"
  section of the runtime skill
  [goal reference](../../skills/use-podway/references/goal.md):
  `Citations are optional for satisfied and unsatisfied results; a citation-free result is a caller assertion that Podway records but does not substantiate, so cite resolved evidence for every non-retirement status.`
- Test: the existing help-text tests are updated for the new sentence; no
  behavior test.

## 5. Failure handling and compatibility boundaries

- Every refusal keeps fail-closed atomicity. A rejected mutation retains no
  outcome and no state change.
- The only classification change is H2-a, from retryable
  `GROUP_DEFINITION_MISMATCH` to non-retryable `REQUEST_INVALID`, and only for a
  digest that names a retained group. It applies to both generations, because
  ADR-0039 shares codes across generations. Record it in the specifications and
  in the release notes of the release that ships it.
- Message, human-hint and help-text deltas are permitted by the JSON contract
  (human messages are not stable API). JSON fields other than `message` stay
  byte-identical. The GUIDE-004 and GUIDE-006 frozen baselines must pass
  unchanged. If one fails, that is a stop condition, not a baseline update.
- The manifest stays unchanged (section 1), so no writer predecessor is needed
  and `v0.2.18` Stores remain readable by the candidate.

## 6. Roadmap ownership, dependencies, and traceability

| Requirement | Task |
|---|---|
| M1, H1, H2, H3, H7 | `HANDS-001` |
| H4 | `HANDS-002` |
| H5, H6, H10 | `HANDS-003` |
| H8, H9 | `HANDS-004` |
| Integrated qualification and closeout | `HANDS-005` |

HANDS depends on the completed ADOPT cutover. H4 is ordered second so that its
guidance exists before the separately planned Aquarium upgrade evaluation.
Structured, machine-readable legacy alternatives stay deferred as DF-055.

## 7. Verification and release acceptance

Focused commands, as documented in
[testing](../implementation-tips/testing.md):

- One integration case:
  `cargo test --locked -p <package> --test int_suite <module>::<function> -- --exact`.
  Engine and Store cases that need successor fixtures add `--features test-support`.
- One e2e case:
  `make test-e2e-exact E2E_TEST=podway-cli::e2e_suite::e2e_hands_usability::<function>`.
- Register the new `e2e_hands_usability.rs` module in
  `crates/podway-cli/tests/e2e_suite.rs` with a `#[path]` attribute like its
  neighbors. `tools/verify_test_layout.py --check` must pass.

Task gate, required before marking any HANDS task In Review:

1. Every card test for the task fails before the change and passes after it.
2. `make format-check`, `make lint`, and `./tools/python tools/verify_docs.py` pass.
3. `make contract-manifest` leaves `contracts/contract-manifest-v2.json`
   unchanged (`git diff --exit-code contracts/`).
4. The GUIDE-004 and GUIDE-006 baseline suites pass unchanged.
5. Every **Also update** file for the task is changed in the same task.

`HANDS-005` gate:

- `make test` passes on the integrated candidate.
- An independent whole-Epic review covers M1 and H1–H10.
- A repeated hands-on pass runs every **Reproduce** step against a release
  build and records the new behavior. JSON envelopes still validate against the
  packaged schemas.
- Owner acceptance precedes dossier retirement under the
  [TODO lifecycle](README.md#lifecycle).

## 8. References

- [Errors and exit codes](../specs/interfaces/errors-and-exit-codes.md)
- [CLI specification](../specs/interfaces/cli-specification.md)
- [JSON contract](../specs/interfaces/json-contract.md)
- [SQLite model and writer contract](../specs/storage/sqlite-model.md)
- [Release upgrade QA](../implementation-tips/release-upgrade-qa.md)
- [Testing guidance](../implementation-tips/testing.md)
- [Runtime skill recovery reference](../../skills/use-podway/references/recovery.md)
- [Runtime skill goal reference](../../skills/use-podway/references/goal.md)
- [ADR-0039](../architecture-decision-records/0039-opt-in-contract-generations.md), the opt-in contract-generation rules
