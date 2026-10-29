# CALLS Agent Workflow Guidance

## 1. Status and authority

- Document state: `Adopted`
- Owning roadmap epic: [CALLS](../roadmap/README.md#calls-agent-workflow-guidance)
- Target product release: undecided
- Repository scope: Podway only

This is the decision-complete execution dossier for unfinished CALLS work.
The roadmap alone owns Task membership, order and status. This design adoption
does not implement product behavior or authorize implementation, staging,
commit, release, installation or external-consumer modification. Existing ADRs,
assets and specifications remain authoritative until owning Tasks change their
applicable implementation and documentation together.

## 2. Verified context and evidence

GUIDE delivered command-word help, observation templates, record-field guidance
and runtime-skill improvements. The remaining gaps are presentation and guidance,
not evidence that the current lifecycle or machine shapes are invalid.
`observe` already combines compact current state and guidance; it does not return
complete Standard status or history. `record --stdin` already supports 1..128
unique same-attempt operations atomically within a 1 MiB input limit.

CLI help resolution uses the clap tree and existing guide, retired-family and
flag-selected topic definitions in `command/grammar.rs`; completion scripts
currently use `command/completion.rs`. The parse-failure path discards the typed
parser error and emits a generic message. `command/authoring.rs` builds preset
explanation from validated embedded content but omits goal tracking in its human
rendering. The canonical `bug-fix-v2` enables goal tracking and currently declares
version `3`. These are current source facts, not completed CALLS behavior.

### Source feedback

The following original findings, deferral reasons and revisit conditions move
from the continuing candidate to this dossier. Promotion closes only their
deferral; it does not establish implementation or acceptance. Previously
resolved runtime-skill subparts remain preserved context.

| ID | Source | Feedback | Deferral reason | Revisit condition |
|---|---|---|---|---|
| DF-044 | GUIDE design review | `podway help` topics have no shell completion: the help route completes no values, and multi-word command topics would need contextual completion. | GUIDE-003 resolves topics without changing completion route accumulation; completing multi-word positionals is an independent refinement. | Before changing help grammar or completion route accumulation, complete help topics from the same command tree and verify the bash, zsh and fish scripts. |
| DF-047 | GUIDE-012 UTM agent reruns | Per-command help usage lines omit `--idempotency-key` and global options such as `--contract-generation`, so agents learn the key requirement only from a `REQUEST_INVALID`, and "invalid command syntax" does not name a missing required option such as `--task`. | Help keeps its established per-route usage format and the closed JSON errors remain actionable; enriching usage lines and usage failures is a guidance improvement, not a contract defect. | Before changing help usage rendering or usage-failure messages, consider appending the always-available global options to per-command usage lines and naming the first missing required option in the usage failure. |
| DF-048 | GUIDE-012 UTM agent reruns | The nested `check_result` shape (`input_basis` with `descriptor` and `digest`, `executor` with `name` and `version`) appears only in the observation's `stdin_template` and not in the inspected agent guidance (help, the runtime skill and observation prose); `item.set` results call the session revision just `revision` beside item revisions; and nothing guides agents toward reproducible check-output digests. | The shapes are schema-valid and the observation carries the executable input. The runtime skill now names the required nested fields, uses stdin_template when present, distinguishes session/item revisions and recommends hashing retained check output. The remaining CLI-help and observation-prose guidance is independent. | When record help or observation prose is next revised, explain the nested check-result shape, session-revision field name and reproducible output digests; preserve the runtime guidance already added. |
| DF-049 | GUIDE-012 UTM agent reruns | Help and the runtime skill do not say when to choose `disposition handed-off` versus `not-required`, or what `--reference` holds; `allowed_actions` lists `session.start_replace` once while two prepared templates exist; and the templates omit the optional `--actor` and the `goal assess-criterion` `--evidence`/`--item` options. | The template shapes remain valid by design. Runtime lifecycle guidance now explains disposition choice and reference; shared execution rules permit supported optional/repeatable arguments, and goal guidance names citation flags. CLI-help explanation remains independent. | When lifecycle or goal help is next revised, explain disposition choice and reference, the shared start_replace action covering two variants, and supported optional actor/citation arguments. |
| DF-051 | GUIDE-012 UTM agent reruns on the final candidate build | The `begin` and `disposition` help say "Omitted fences are filled from a fresh status preflight" and their usage lines bracket the fences as optional, while the runtime skill says never to omit a required mutation fence (`774954a:skills/use-podway/SKILL.md:75`), so both agents read the two as conflicting; the envelope's `latest_workspace_sequence`, the init result's `revision` and the archive list's `archived_workspace_revision` are not related anywhere in the inspected guidance; `preset explain` (text and JSON) does not show that `bug-fix-v2` tracks a goal (`assets/presets/bug-fix-v2.yaml:7`); and the `bug-fix-v2` `regression-check` prompt (`:40`) can be read as the check to run later or as its result. | The runtime skill now distinguishes CLI preflight auto-fill from automation preserving observed fences. Both agents had preserved all fences, and the counters, preset explain output and prompt remain valid; their presentation improvements remain independent. | When command help or presets are next revised, explain automation fence practice, relate workspace revision and sequence, show goal tracking in human preset explain, and clarify the regression-check prompt. |

## 3. Goals, non-goals, and scope

Enable agents and human operators to use the existing CLI from its help and
observed state: select the appropriate read, record valid inputs, preserve the
intended fences, reconcile uncertain output, and dispose a terminal result
correctly. Adopt the five source findings above as one guidance epic.

Exclude combined record-and-complete, mutation-returned observations, new
commands or result families, changed observation templates or allowed actions,
Store changes or migrations, automatic resets, external consumers, runtime
services, and DF-050's broader preset enrichment. Elapsed-time tests, latency
percentiles, throughput benchmarks and performance baselines or thresholds are
excluded. UTM agent reruns and distribution readiness are not acceptance gates.

## 4. Accepted design decisions and interfaces

### Help completion

CALLS-001 derives help candidates from the same command tree and topic sources
used by help resolution, without a second help command catalog. Complete visible
command words and groups contextually (for example `help goal` then
`assess-criterion`), canonical dotted routes, overview and guide topics, and the
existing flag-selected and retired-family help spellings. Retired help topics
must not become executable command suggestions. Hide internal routes and retain
existing non-help completion behavior. Help completion is local and must not open
a workspace or run a dynamic state query. Ignore option values when collecting
topic words; retain accepted global option placement.

Adapt the existing retirement completion tests to distinguish executable command
contexts from help-topic contexts. Narrow whole-script absence assertions only
where they conflict with supported retired help topics; retain retirement and
hidden-route coverage. The real-shell checks below must prove that retired help
spellings remain unavailable as ordinary executable-command suggestions.

### Usage and syntax

CALLS-002 appends applicable global-option guidance to route usage and explains
where automation requires a caller-owned idempotency key. Local routes must not
advertise workspace-only options. Atomic record and delegation-accept input own
their keys and fences in stdin, not duplicate CLI flags. Read-only lookup requires
its key; local reads and dry-run previews must not be described as key-requiring
mutations. Preserve interactive key generation and current admission rules.

Use typed parser context to name the first missing required option or positional
in parser declaration order, using only trusted grammar labels. Other syntax
errors keep the generic fallback. Preserve route attribution, late `--json`
recognition, value-position and `--` handling, retired-command precedence and
all error code/detail/exit/retryability values. Do not emit raw parser output or
user-controlled argument values in either human or JSON errors.

### Observation and recording

CALLS-003 explains when `observe` replaces separate narrow current-state reads
and when `status` or verbose history is still necessary. Add recording advice to
help and human observation rendering, without adding prose fields to machine
observation. Use an observed `stdin_template` when available and explicitly
explain `input_basis: {descriptor, digest}` and `executor: {name, version}` along
with the existing check-result fields. A copyable example must decode under the
canonical input and be exercised through the real CLI.

Explain that the caller hashes retained exact check-output bytes for a
reproducible `output_digest`; Podway neither executes nor attests that operation.
Distinguish a single-item mutation result's session `revision` from
`active_items[].revision`, which supplies the next item fence after a fresh read.
Explain workspace sequence, init-result revision and archived workspace revision
using their existing producers; they are distinct from session and item fences
and must not be substituted for them or assumed numerically interchangeable.
Do not rename public fields or derive subsequent writes from stale observations.

### Lifecycle and recovery

CALLS-004 explains that CLI preflight can fill omitted fences while automation
should retain the observed template fences to preserve its intended target.
Clarify both `begin` and disposition help without making optional grammar
mandatory. `handed-off` records an actual external result and reference;
`not-required` records why no handoff is owed. A reference identifies the result,
not an instruction for Podway to execute or a claim that Podway verified it.

Explain that preset/file replacement templates are two variants of the same
`session.start_replace` action. Explain supported `--actor` arguments and the
repeatable `goal assess-criterion --evidence` and `--item` citations with their
existing citation eligibility and bounds. Templates themselves remain unchanged.
For lost mutation output, retain the original key and canonical request, inspect
`outcome lookup`, and use exact replay where supported. A miss is bounded by
retention and does not prove that an external effect never happened.

### Preset guidance

CALLS-005 adds `Goal tracking: enabled|disabled` to human `preset explain` for
all four presets, derived from admitted Procedure content. Add no JSON field.
Clarify only the `bug-fix-v2` reproduction `regression-check` prompt to:

> Record the regression commands actually run and their observed results in English, preserving exact commands verbatim. If they could not be run, record why.

Retain the existing item type, bounds, requiredness, graph and verification
contract. This reproduction record does not replace the later structurally
bound verification result. Keep the preset ID and advance its version from `3`
to `4`; synchronize embedded metadata, digest pins, canonicalization fixtures
where affected, and the manifest through documented tooling. Other presets keep
their source bytes and versions. This change does not add goal criteria or
rewrite existing sessions.

## 5. Failure handling and compatibility boundaries

Preserve CLI grammar, public fields, schema IDs, contract-generation selection,
error codes/details/exits/retryability, validation order, input/output bounds,
transaction boundaries, canonical requests, idempotency and exact response replay.
No new ADR or contract generation is selected because no architectural invariant
or released family shape changes. A need to change either returns to design.

The intentional output deltas are help prose (including JSON help text), shell
scripts, missing-required-argument messages, human-only explanatory lines,
`bug-fix-v2` version and authored prompt, and their resulting preset/manifest
identities in existing output fields. Machine templates and `allowed_actions`
remain unchanged. Compare unaffected payloads after normalizing only volatile
identities and explicitly expected manifest identities; never mask other fields
or regenerate baselines to conceal an unexplained difference.

CALLS-005 changes the contract-manifest digest through the canonical preset
update. Under the existing [writer contract](../specs/storage/sqlite-model.md#workspace-and-writer-identity),
with the currently empty predecessor registry and no migration selected by CALLS,
the resulting candidate is expected to reject normal reads and writes against
Stores carrying the previous manifest with `WORKSPACE_CONTRACT_MISMATCH`. This
applies regardless of which preset a session selected. Existing session snapshots
remain unchanged, but transparent continuation with the new binary is not
promised.

Use newly initialized isolated fixtures for candidate workflow verification and
verify unchanged refusal against an incompatible-writer fixture. Do not reset an
existing user Store, loosen writer admission or add predecessor declarations
under CALLS. Preserve fail-closed peer mismatch behavior. Release or rollout
compatibility, external consumer pin updates, installed runtime changes and
publication remain separately authorized decisions. Promote this consequence to
the affected durable documentation and release notes when CALLS-005 is delivered.

## 6. Roadmap ownership, dependencies, and traceability

The [roadmap](../roadmap/README.md#calls-agent-workflow-guidance) owns the six
ordered work units and their dependency on completed GUIDE. CALLS-001 owns
DF-044; CALLS-002 owns DF-047; CALLS-003 owns DF-048 and DF-051's counter guidance;
CALLS-004 owns DF-049 and DF-051's fence guidance; CALLS-005 owns DF-051's preset
presentation; CALLS-006 owns integrated acceptance. This mapping establishes no
parallel execution or competing status table.

CLI owns help, parsing, human rendering and completion; Presets owns embedded
preset metadata and digest pins; canonical preset YAML and manifest tooling
retain their existing owners. Core, Engine and Store transition logic are not
new implementation targets. Use existing test aggregation and fixture helpers.

During implementation, promote lasting requirements to the CLI specification,
built-in preset specification and affected JSON/automation documentation; promote
regression ownership to the testing guide and copyable examples to existing
examples. Preserve the already delivered runtime skill guidance. Update relevant
release notes for delivered behavior without selecting a release. CALLS-006
inventories all dossier references, replaces them with durable outcome links,
and removes this dossier and its adopted index entry only after exact closeout
approval and last-consumer verification.

## 7. Verification and release acceptance

Before changing executable behavior, identify a focused failing or baseline
check for its actual gap. Extend the existing CLI unit tests for grammar,
rendering and completion generation. Register real CLI process tests in the
existing `podway-cli` `e2e_suite` and reuse its fixture helpers. Component
integration tests in other crates remain in their respective `int_suite`
aggregates. Do not create a new test target merely to implement CALLS.

Map required behavior to existing regression owners before adding tests. The
following registered `e2e_suite` cases are relevant existing owners, not evidence
that they cover every new CALLS example or have run on the current candidate:

| Existing owner | Relevant behavior |
|---|---|
| `e2e_v2dfc007_direct_cli::record_stdin_preserves_identity_applies_one_atomic_batch_and_replays_without_preflight` | Atomic record and replay |
| `e2e_v2dfc007_direct_cli::sigkill_after_commit_before_stdout_preserves_the_original_outcome_and_exact_replay` | Recovery after committed output is lost |
| `e2e_v2dfc007_direct_cli::incompatible_writer_identity_refuses_concurrent_reads_mutations_and_old_replays` | Incompatible writer refusal |
| `e2e_guide009_generation2_lifecycle::generation2_lifecycle_conformance` | Generation-2 lifecycle conformance |

Extend these owners where practical and add only uncovered CALLS behavior,
especially actual completion execution, actionable redacted diagnostics, human
guidance and the exact documented input example. Keep this mapping in ordinary
task verification notes; do not introduce a verification framework or mandatory
machine-readable ledger. Existing regression tests do not substitute for running
a newly documented example or the required representative CLI workflows. An
acceptance item does not require duplicating an already adequate regression test.

Required cases are:

- Execute generated bash, zsh and fish completion functions for root, partial,
  multi-word and dotted help topics, flags before/between words, hidden/retired
  boundaries and existing non-help routes. Script string matching alone fails
  acceptance; unavailable shells leave a verification gap.
- Exercise missing `--task`, a missing positional, multiple missing arguments,
  malformed non-missing syntax, late `--json`, `--` and option-value boundaries.
  Assert the diagnostic label, redaction and preserved error classification.
- Decode and submit the documented record example; check nested check-result
  values, stale session/item fences, whole-batch rejection with unchanged state,
  and original success recovery through lookup and exact replay without a second
  effect. Verify distinct counter meanings across init, writes and archive.
- Run `small-change-v2` and `bug-fix-v2` through the actual CLI in owned standalone
  Git fixtures, including recorded reproduction outcome or limitation, later
  check-result input, goal/citation operations where enabled, terminal disposition
  and both replacement-template variants. Preserve fences and generation-1/2
  machine template behavior. These are CLI correctness runs, not agent-usability
  or VM qualification claims.
- Check human goal-tracking text for enabled and disabled presets, no new JSON
  field, changed bug-fix version/digest consistency, unchanged other preset
  sources, and writer mismatch refusal without forced reset.

Run focused checks first, then the required `make test` development gate for
executable or contract changes and `make lint-all` when tests change. Route long
or noisy checks through configured Gaori commands; the complete development gate
uses `full`. For a focused real CLI case, use the documented
`make test-e2e-exact E2E_TEST=podway-cli::e2e_suite::<module>::<test>` entry point
with an actual registered test name. For example, this existing lifecycle case
uses the same binary-building entry point:

```bash
make test-e2e-exact E2E_TEST=podway-cli::e2e_suite::e2e_guide009_generation2_lifecycle::generation2_lifecycle_conformance
```

Exact selection must execute one passing test; selecting zero tests is not
success. The example identifies an existing execution path, not a requirement to
run that case for every CALLS edit. Use the documented `int_suite` invocation only
for a component integration test owned by that suite. Evidence reuse does not
waive required gates or permit claiming a result from a different or invalidated
candidate.

For design-document-only application, inspect links and authority claims and run
`git diff --check`; it is not product verification and needs no `make dist`.

CALLS-006 assesses the integrated candidate and remaining cross-task gaps. A
non-behavioral wording correction alone does not require a fresh full review of
every accepted Task. Preserve applicable review requirements and required checks;
reuse prior evidence only while its target and verification basis remain valid.

CALLS completion requires independent integrated assessment, all required evidence,
lasting documentation, and the owner's acceptance of the exact final candidate.
No release target is chosen. A later release needs its own gate choice, authority
and qualification; this design and development acceptance establish none of them.

## 8. References

- [Documentation authority](../README.md#precedence)
- [TODO lifecycle](README.md#lifecycle)
- [CLI grammar and globals](../specs/interfaces/cli-specification.md#grammar-and-global-options)
- [Mutation rules](../specs/interfaces/cli-specification.md#mutation-rules)
- [JSON contract generations](../specs/interfaces/json-contract.md#contract-generations)
- [Transaction contract](../specs/storage/transactions-concurrency-and-idempotency.md)
- [Built-in Procedures](../specs/domain/built-in-presets.md)
- [Functional regression owners](../implementation-tips/testing.md#functional-regression-owners)
- [Continuing interaction candidate](TODO-podway-agent-workflow-simplification.md)
