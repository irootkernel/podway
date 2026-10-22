# Podway Agent Guidance and Contract Generation 2

## Status and authority

Status: Adopted. Owning epic: `GUIDE`. Design registered on 2026-10-22. The
[roadmap](../roadmap/README.md#guide-agent-guidance-and-contract-generation-2)
alone owns task order and status.
[ADR-0039](../architecture-decision-records/0039-opt-in-contract-generations.md)
owns the contract-generation and fenced plain start decisions.

This dossier defines unfinished work. Existing ADRs, canonical assets and
specifications keep authority over implemented behavior until the owning task
updates them. Registration authorizes no release, push, installation or change to
an external consumer. The release target `v0.2.15` is conditional on GUIDE completion
and a separately authorized release.

## Verified context and evidence

The source baseline is `c26f65f44219284638d09cf022b0292d77bc5339` on 2026-10-22.
Its product source is identical to the verified `v0.2.15` candidate `eb042f5`.
Pre-release verification on that candidate passed `make test` and
`make release-prepare` and found no behavior regression against published `v0.2.14`.
A 592-invocation CLI transcript was identical after volatile normalization, and
`v0.2.14`-written Stores continued under the candidate. Two Claude Code agents
completed `small-change-v2` and `bug-fix-v2` tasks on a native macOS VM. That
evidence is host-local and not tracked. Independent Claude Opus and Gemini Argon
reviews of this design shaped the decisions below.

The agents still hit these gaps. Every one of them is already present in `v0.2.14`:

1. **Local commands reject workspace options inconsistently.** `execute_local` in
   [`command.rs`](../../crates/podway-cli/src/command.rs) rejects `--worktree`,
   `--timeout`, fences, `--idempotency-key` and `--yes` for `help`, `version`,
   `completions`, `preset` and `procedure`, but silently accepts
   `--if-goal-revision`. The [runtime skill](../../skills/use-podway/SKILL.md) says
   to pass `--worktree` on every command, so both agents received
   `REQUEST_INVALID` from local commands.
2. **Help topics resolve only exact identifiers.** `help_text` in
   [`grammar.rs`](../../crates/podway-cli/src/command/grammar.rs) matches exact
   topics, so `podway help init`, `help start` and `help record` fail with "unknown
   help topic". Multi-word topics fail as invalid syntax, and the root help lists
   command words that are not valid topics.
3. **Batch record operation fields are undocumented.** `podway help item.record_many`
   names the document fields but not the per-operation fields, and both agents
   guessed `item_revision`. One value has three names: `active_items[].revision`,
   `preconditions.item_revision` and stdin `expected_item_revision`.
4. **Lifecycle templates are not executable as given.**
   - `prepared_mutation_templates` and `terminal_mutation_templates` in
     [`v2_read_service.rs`](../../crates/podway-engine/src/v2_read_service.rs) emit
     bare argv (`podway begin`, `podway disposition`, `podway reset`,
     `podway start`). Fences appear only in `preconditions`, and there is no
     idempotency-key placeholder.
   - The disposition template lacks its required subcommand, and no archive
     template follows a disposition.
   - [`observation-result-v5`](../../assets/schemas/observation-result-v5.schema.json)
     pins these lifecycle counts at three for a prepared session and one or two
     for a terminal one, and its lifecycle command set excludes `session.archive`.
   - Running templates keep their own established shapes: fenced argv built with
     `append_flag`, the `item.record_many` key inside `stdin_template`, and
     placeholders such as `<text>` and `<graph-node-id>`.
5. **Citation errors give no hint.** `CRITERION_CITATION_INVALID` details carry only
   the criterion and the rejected citation. `assess_goal_criterion_v2` in
   [`graph_v2.rs`](../../crates/podway-core/src/graph_v2.rs) admits only resolved
   evidence sources with a valid source attempt, plus active-attempt items with
   values, with at most four citations.
6. **Writer mismatches show digests, not releases.** `WORKSPACE_CONTRACT_MISMATCH`
   details in [`direct_error.rs`](../../crates/podway-engine/src/direct_error.rs)
   carry the expected and actual manifest digests. The Store records no product
   version. Every version bump changes the digest because the manifest includes the
   product version. The released digests are:
   - `v0.2.13`: `sha256:dc47bcc5449f7c2e60c920c5b820eb793405b5f2d2f573346ee326f90264d2aa`
   - `v0.2.14`: `sha256:c699977dd232891041f4063d66295b94004702875855baace876f8a7e0c6391d`
7. **Socket tests break under a long TMPDIR.**
   `artifacts_reject_final_and_intermediate_symlinks_and_preserve_non_utf8_bytes` in
   [`int_phase3_resolver.rs`](../../crates/podway-git/tests/int_phase3_resolver.rs)
   and `config_conflicting_node_kinds_fail_without_layout_mutation` in
   [`int_phase4_init_layout.rs`](../../crates/podway-git/tests/int_phase4_init_layout.rs)
   bind Unix sockets under TMPDIR. They fail with "path must be shorter than
   SUN_LEN" once the socket path reaches the 104-byte macOS address capacity. The
   requirements traceability names both test modules.
8. **Plain `start` cannot be fenced.**
   - Plain start (`SessionStartV1`) carries no session preconditions, and the CLI
     rejects `--if-session-id` and `--if-session-revision` on it.
   - When both fences are present, `needs_facts` skips the facts preflight.
   - Only the replacement request (`SessionStartReplaceV1`) carries identity
     preconditions, and its `Eligible` mode deletes a prepared session.
   - Forcing the CLI facts preflight would reject a valid exact replay once the
     original session has been archived, because the preflight runs before the
     idempotency lookup.

   As a result, automation cannot fence the plain start that archives a disposed
   terminal session.

## Goals, non-goals, and scope

**Goals.**

- Agents can operate Podway from help, the runtime skill and observation output
  without guessing:
  - help topics resolve;
  - local-command errors are actionable;
  - record fields are documented;
  - lifecycle templates are executable;
  - citation and writer diagnostics are useful.
- Existing consumers keep working unchanged on the default generation while an
  opt-in generation 2 delivers the contract-level fixes.
- Automation can fence plain start without losing exact replay.
- Tests pass regardless of TMPDIR length.

**Non-goals.**

- Store layout, writer identity or migration changes.
- A writer-predecessor declaration.
- CALLS interaction changes, such as combined or fewer calls.
- Shell completion of help topics (DF-044).
- Runtime deprecation warnings.
- Executable `session.next` or prepared-guidance suggestions. Those suggestions stay
  bare in both generations, because templates are the executable surface.
- Removal of generation 1 and its value-aware retirement. Both belong to the removal
  release.
- Changes to Aquarium or Dolgorae.
- The `v0.2.15` release itself: upgrade notes, version pins, gates and publication.

**Scope.**

- GUIDE-001 through GUIDE-005 change no JSON contract.
- GUIDE-006 through GUIDE-011 implement ADR-0039.
- GUIDE-012 qualifies and closes the epic.

## Accepted design decisions and interfaces

### GUIDE-001 Socket fixture roots

Add a local helper to each of the two socket test files, following the existing
helper duplication in each file. The helper creates the fixture root under the
inherited temporary directory when the bound socket path still fits below the
104-byte macOS `sun_path` capacity. That path is the canonical root plus the test's
fixed suffix. Otherwise the helper uses a short directory from
`tempfile::tempdir_in("/tmp")` and asserts that the fallback fits.

- Use the helper only in the two socket tests.
- Sockets must stay inside the worktree fixture, because the resolver validates
  containment.
- Never change the process working directory, because tests run concurrently.
- Add a regression that re-executes both exact tests in a bounded child process
  with a TMPDIR of about 120 bytes and requires success. It kills the child on
  timeout, as the existing race tests do.
- Keep the test modules and test names.
- Document the fallback in the [testing guidance](../implementation-tips/testing.md).

### GUIDE-002 Local command options

In `execute_local`, derive the supplied rejected options in a fixed order:

1. `--worktree`
2. `--timeout`
3. `--idempotency-key`
4. `--if-workspace-uuid`
5. `--if-session-id`
6. `--if-session-revision`
7. `--if-attempt`
8. `--if-item-revision`
9. `--if-goal-revision`
10. `--yes`

This adds `--if-goal-revision`, which is ignored today. The code, exit 2 and command
attribution stay `REQUEST_INVALID`. The message states that the command runs
locally, opens no workspace, and names each supplied option to omit. GUIDE-009 adds
`--contract-generation` to the accepted options.

Update these:
- the root help "Direct execution" text;
- [CLI specification](../specs/interfaces/cli-specification.md) option
  applicability: worktree options apply to workspace routes only;
- the [recipe note](../specs/interfaces/errors-and-exit-codes.md): the
  `version --identity` recipe takes no worktree;
- the runtime-skill text that says "every command".

### GUIDE-003 Help topic resolution

Help accepts one or more topic words. The JSON result keeps `{topic, text}`, where
`topic` is the words joined by single spaces, or null. Resolution order:

1. No topic, or `overview`, returns the overview.
2. A retired family or retired command words return the existing retirement guidance.
3. A guide (`workflow`, `automation`, `procedures`, `artifacts`) or a dotted route
   identifier returns the existing route text.
4. Executable command words are resolved by walking the clap command tree through
   non-hidden subcommands and mapping the path with `parse_failure_command_context`.
   For example:
   - `init` → `workspace.init`
   - `record` → `item.record_many`
   - `goal define` → `goal.define`
   - `disposition not-required` → `session.terminal_disposition`
5. A group word returns a generated listing of its commands with their route
   identifiers.
6. Anything else returns `REQUEST_INVALID` with a message pointing to `podway help`.

`help rework` resolves to `session.rework`. Remove the example-only "Rework:" topic,
because the route text covers its content.

The overview replaces its partial route list with a topic index generated from the
clap tree. The index also lists:
- the flag-selected routes: `reset --all` for `workspace.reset_all` and
  `start --on-existing` for `session.start_replace`;
- the guides;
- the retired families.

The overview keeps the phrases that existing tests assert.

The retirement classifier must not advance past a positional that accepts multiple
values. Retired flags after a multi-word topic therefore stay `COMMAND_RETIRED`
attributed to `help`. Keep the `podway help procedure.<operation>` wording that
`tools/verify_docs.py` requires. Update the CLI specification help row, the README
help sentence and the runtime-skill route wording.

### GUIDE-004 Record, observation and recovery guidance

**`item.record_many` help.** It states atomic semantics and lists each field with
its observation source:

| Field | Observation source |
|---|---|
| `schema` | the constant `podway.item-record-many-input/v1` |
| `workspace_uuid` | `workspace.uuid` |
| `session_id` and `session_revision` | `result.status.session` |
| `attempt_id` | `result.status.current.attempt.attempt_id` |
| `idempotency_key` | a new caller-owned key |

Each operation has:
- `item_id`;
- `expected_item_revision`: that item's `active_items[].revision`, also shown as
  `preconditions.item_revision`;
- exactly one of `record` (a type and its value fields) or `clear: true`.

Value fields follow the
[record input schema](../../assets/schemas/item-record-many-input-v1.schema.json).
The help includes a one-line example, and a test decodes it.

**`session.observe` help.** It covers:
- how observation fields map to fences;
- that guidance is null once the session is terminal;
- how to use `mutation_templates`;
- what `SESSION_NOT_FOUND` and `WORKSPACE_NOT_INITIALIZED` mean, and that
  initialization needs explicit intent.

**`workspace.init` help.** It names the tracked files and separates them from the
ignored runtime state:
- `.podway/config.yaml`;
- `.podway/procedures/`;
- `.podway/.gitignore`, which ignores `runtime/`.

**Human-mode hints** in the [renderer](../../crates/podway-cli/src/command/render.rs).
JSON output is unchanged.
- `WORKSPACE_CONTRACT_MISMATCH`: name the stored and current digests, then direct
  the user to either:
  - continue with the writer whose `version --identity` reports the stored digest;
  - or confirm an unfenced `reset --all`.
- `CRITERION_CITATION_INVALID`: cite only resolved `guidance.references[]` sources
  and recorded items of the decision attempt, each once, at most four.

### GUIDE-005 Runtime skill guidance

Update [`use-podway`](../../skills/use-podway/SKILL.md) with:

- tracked `.podway` files versus ignored runtime state;
- `WORKSPACE_NOT_INITIALIZED`: the worktree has no workspace, so report it and
  initialize only with explicit intent;
- record operation fields and their observation sources;
- in the [goal guidance](../../skills/use-podway/references/goal.md): the exact
  citation rule, and re-observing after `CRITERION_CITATION_INVALID`;
- in the [recovery guidance](../../skills/use-podway/references/recovery.md): the
  released-manifest digest table above. Any other digest is identified with that
  binary's `version --json --identity`;
- in the [lifecycle guidance](../../skills/use-podway/references/lifecycle.md): init
  authority and tracked files.

This task is independent of the contract generation; GUIDE-011 adds the
generation-2 guidance. The closed skill file set, required markers and English-only
rule enforced by `tools/verify_docs.py` still apply.

### GUIDE-006 Generation-1 baselines

Before any generation-2 change, add Engine and CLI tests that pin exact
generation-1 outputs:

- `podway.observation-result/v5` in these states:
  - prepared;
  - running, including the dense template window;
  - completed without a disposition;
  - cancelled with a disposition;
  - delegated prepared.
- The default `observe`, `status` and `next` CLI outputs.
- The generation-1 details of `CRITERION_CITATION_INVALID` and
  `WORKSPACE_CONTRACT_MISMATCH`.

**Normalization.** Replace generated workspace, session, attempt and request
identifiers, timestamps and fixture roots with first-appearance placeholders.
Alternatively, use the deterministic identifier and clock sources that Engine tests
already inject.

**"Unmodified".** The pinned expected values stay unchanged through GUIDE-011.
Mechanical test-code updates are allowed, such as new fields on request-struct
literals.

### GUIDE-007 Generation-2 contract reservation

**Canonical schemas.** Add three schemas:

- `observation-result-v6`. It is self-contained and has a local template definition.
  That definition:
  - admits `session.start` and `session.archive` among the lifecycle commands;
  - pins the lifecycle counts and heads;
  - applies the fence and key-token rules only to lifecycle templates.

  Running templates keep the v5 shapes.
- `criterion-citation-invalid-details-v1`.
- `workspace-contract-mismatch-details-v2`.

**Selections.**

- Output v4 `session.observe` uses `oneOf` [v5, v6].
- The Error v2 rules for the two codes use details `oneOf` [generation-1 family,
  successor].
- In the command catalog, `session.observe` lists both observation families under
  `result_schemas`.
- The error catalog keeps `details_schema` on the generation-1 family, so its bytes
  stay unchanged.

**Generation declaration.** Add `assets/specifications/contract-generations.json`
with schema `podway.contract-generations/v1`:
- a `default` member set to `1`;
- generation `1` with status `supported`;
- generation `2` with status `reserved`.

The closed status set is `reserved`, `supported`, `deprecated` and `retired`. The
per-generation status is the only retirement record; the declaration has no
separate retired list.
GUIDE-009 moves generation 2 to `supported`, and GUIDE-011 moves generation 1 to
`deprecated`.

**Tools.**

- Enroll the declaration in the manifest through `tools/contract_manifest.py`.
- `validate_catalog_inventory` in `tools/verify_contracts.py` accepts either a single
  `$ref` or a `oneOf` of exactly the catalog `details_schema` plus a successor
  declared in `contract-generations.json`.
- Add a consistency check between the declaration and the Output v4 and Error v2
  selections, with drift cases in the `--sentinels` run.
- `tools/create_dolgorae_handoff.py` keeps copying catalog entries unchanged. The
  regenerated adapter adds only schema pins and counts; its runtime errors stay
  unchanged.
- Regenerate in this order, as in the [release guide](../implementation-tips/release.md):
  the manifest, then the adapter catalog, then the manifest again.

**Protocol crate.** In `podway-protocol`:

- Add a contract-generation type.
- Add `RELEASED_CONTRACT_MANIFESTS_V1` (`v0.2.13`, `v0.2.14`) with a lookup, following
  the `DECLARED_PREDECESSORS` precedent.
- Register the new families in `DAEMON_FREE_DOCUMENT_SCHEMAS_V1`,
  `result_schema_top_level_fields_v2` and `validate_result_correlations_v2`.
- Add template-correlation validation:
  - lifecycle argv fences equal `preconditions`;
  - exactly one key placeholder, counted across argv and `stdin_template`;
  - stdin-template identities equal `preconditions`;
  - lifecycle preconditions equal the status session.

**Known answers and inventory.** Add new known-answer entries only; existing entries
stay byte-identical. Update the registry inventory tests.

**Specifications.**

- A "Contract generations" section in the
  [JSON contract](../specs/interfaces/json-contract.md).
- A generation column for detail families in the error specification.

Nothing is emitted yet.

### GUIDE-008 Fenced plain start

Plain `start` accepts `--if-session-id` and `--if-session-revision` together in both
generations.

**Request shape.**

- Supplying only one of the two is `REQUEST_INVALID`.
- `--if-attempt` stays rejected.
- The optional session identity preconditions belong to the plain-start command
  variant. They are not added to `SessionStartV1`, because the replacement request
  `SessionStartReplaceV1` embeds that type and already carries its own
  preconditions.
- When the fences are absent, the canonical request bytes and request digest stay
  exactly as before, so existing keys and stored outcomes keep replaying. When
  present, the fences are part of the canonical request.

**Engine verification.** The Engine verifies the fences inside the start writer
transaction, after the idempotency lookup. An exact replay therefore returns the
original success even after its session was archived or replaced. A fresh mismatch
returns the identity or revision error that the fenced replacement path returns for
the same state (`SESSION_ID_MISMATCH` or `SESSION_REVISION_CONFLICT`), with no effect
and no outcome. The CLI facts preflight is not the enforcement point.

**Lifecycle semantics stay those of plain start.**

- A matching disposed terminal session is archived and the new session starts.
- A matching prepared, running or undisposed terminal session returns
  `SESSION_START_DECISION_REQUIRED` and is never deleted.
- A fenced plain start never enters the human interactive start interview, so a
  terminal user cannot turn it into a deletion.

**Dry run.** A fenced `--dry-run` never takes the offline shortcut. It verifies the
fences and the lifecycle read-only and reports the same refusals without effect.

**Canonical catalog.** In `assets/specifications/command-catalog.yaml`, the
`session.start` preconditions gain a paired optional session-fence precondition.
Regenerate the manifest, then the adapter catalog, then the manifest again, and run
the GUIDE-007 contract checks.

**Specifications.** The [automation contract](../specs/interfaces/automation-client-contract.md)
adds `AUT-LIF-016`. The CLI specification documents the fences.

**Tests** cover:
- every lifecycle state and the no-session case;
- a fenced start from a terminal that receives `SESSION_START_DECISION_REQUIRED`
  without an interview;
- partial, stale identity and stale revision fences;
- exact replay and outcome lookup after the original session was archived;
- the fenced dry run;
- byte-identical canonical requests and replay for unfenced starts.

### GUIDE-009 Generation selection and observation v6

**CLI.**

- Add the global `--contract-generation` option exactly as ADR-0039 decision 2
  defines it, including validation before local execution or stdin.
- Local commands accept it; update the GUIDE-002 message and docs.
- Top-level help alias normalization accepts it before `-h` or `--help`.
- Add completion entries, a help overview paragraph and a `contract-generations`
  topic.
- Set generation 2 to `supported` in the declaration.

**Engine.**

- `DirectRequestV1`, `DirectPreflightFactsRequestV1`, `DirectReadRequestV1` and
  `DirectErrorContextV1` carry the generation next to their existing timeout or
  deadline values.
- `project_direct_observation_v1` projects v5 or v6 and validates against the
  selected schema.
- New generation-2 lifecycle template builders; the generation-1 builders stay
  unchanged.

**v6 lifecycle templates.** Every template ends with `--if-workspace-uuid`,
`--if-session-id` and `--if-session-revision` set to current values, then
`--idempotency-key <idempotency-key>`.

| State | Templates, in order | Explicit authorization |
|---|---|---|
| prepared (revision 0) | `begin`; `reset`; `start --on-existing delete --preset <preset> --task <task>`; the same with `--procedure <procedure-file> --expect-procedure-digest <procedure-digest>` | begin no; others yes |
| completed or cancelled without a current disposition | `disposition handed-off --summary <summary> --reference <reference>`; `disposition not-required --reason <reason>` | no |
| disposed terminal | `archive`; `reset`; `start --preset <preset> --task <task>`; the same with `--procedure <procedure-file> --expect-procedure-digest <procedure-digest>` | yes |
| running | identical to v5 | unchanged |

The disposed-terminal start templates are fenced plain starts from GUIDE-008. They
carry the command `session.start`, which is what their execution, replay and
outcome lookup record. The prepared-state replacement templates keep
`session.start_replace`. The closed placeholder
vocabulary of lifecycle templates is `<preset>`, `<procedure-file>`,
`<procedure-digest>`, `<task>`, `<summary>`, `<reference>`, `<reason>` and
`<idempotency-key>`.

**Automation contract.** The automation contract adds:
- `AUT-GEN-001` onward: selection, presentation-only use and the default;
- `AUT-OBS-013` to `AUT-OBS-017`: v6 lifecycle templates.

It scopes `AUT-OBS-006` to `AUT-OBS-008` and `AUT-LIF-009` to generation 1.

**Tests.**

- **Engine:** v6 templates for each lifecycle and the dense window.
- **CLI end-to-end:** create the generation-2 lifecycle end-to-end test that GUIDE-011
  registers as a packaged scenario. It covers:
  - flag validation: invalid values exit 2 with no state change, and local commands
    accept the flag;
  - an explicit generation 1 equal to the default;
  - each v6 lifecycle template executing after only its placeholders are replaced,
    with exact replay and outcome lookup;
  - each template's `command` equal to the command that its replay and outcome
    lookup report;
  - replay identical across generations;
  - the exact schema family asserted for each generation, because envelope
    validation alone admits either family.

### GUIDE-010 Generation-2 error details

**Core.** The citation error carries bounded citable sources on both failure paths,
computed with the admission rule:
- evidence source nodes: at most eight, in declaration order;
- item identifiers: at most 128.

**Engine.**

- Error rendering selects generation-1 or generation-2 details from the
  invocation's generation.
- A writer mismatch maps the stored digest through the released table, or to null
  when unknown.
- The recovery reason, at most 256 characters, names the original writer and the
  confirmed unfenced reset.
- The human hints use the enriched fields when present.

**Specifications.** The automation contract adds `AUT-ERR-006` (citation details) and
`AUT-ERR-007` (writer release).

**Tests.**

- **Engine:** the citable lists for each failure path.
- **CLI end-to-end:** a stored writer digest equal to `v0.2.14`'s reports `"v0.2.14"`,
  and an unknown digest reports null. Use the existing Store writer-row technique of
  the direct CLI suite.

### GUIDE-011 Declaration and guidance alignment

- Add a CHANGELOG `Deprecated` entry for generation 1, and set generation 1 to
  `deprecated` in the declaration.
- [Release and packaging](../specs/operations/release-and-packaging.md): generation
  policy, consumer adoption (re-pin, then adopt generation 2) and the removal
  criteria.
- [Release guide](../implementation-tips/release.md): in the open-development commit
  after each release, append that release's manifest digest to both
  `RELEASED_CONTRACT_MANIFESTS_V1` and the runtime skill's digest table.
- Upgrade QA, with owner approval recorded on 2026-10-22. The verified
  previous-release CLI may run against owned fixtures only, for two purposes: to
  create authentic previous Stores, and to confirm that it refuses the new Store.
  It runs without daemons, installation or account state. Record this exception in
  two places:
  - the "Podway Release Upgrade Hands-on QA" rule in [AGENTS.md](../../AGENTS.md),
    which today says to run only the exact candidate CLI;
  - [upgrade QA](../implementation-tips/release-upgrade-qa.md), in a direct-to-direct
    upgrade section covering:
    - refusal leaves the Store unchanged and creates no new files;
    - confirmed unfenced reset, with exact replay;
    - a fresh lifecycle;
    - the previous CLI refuses the new Store.
- The README (including its agent-guidance template) and the runtime skill tell
  agents to pass `--contract-generation 2` on every command and describe the v6
  templates and enriched details. Generation-1 notes stay for deprecated consumers.
- Update the workflow example, the supervised-delegation table, the system
  architecture read path, requirements traceability and the testing specification.
- Register GUIDE-009's generation-2 lifecycle end-to-end test in the packaged
  conformance scenarios of `tools/release_evidence.py`.
- Verification:
  - the contract checks of GUIDE-007, because the declaration is manifest-enrolled;
  - `tools/release_evidence.py self-test`;
  - the documentation checks;
  - `make test`.

### GUIDE-012 Qualification and closeout

- **Integration review.** Independent read-only reviewers cover three areas:
  correctness and regression; contract and compatibility; and CLI, UX and
  documentation. Fix in-scope findings, and record independent ones in deferred
  feedback.
- **Hands-on re-verification** on a native macOS VM, using a release build of the
  final candidate:
  - a default-generation differential transcript against `v0.2.14` that shows only
    intended deltas;
  - a generation-2 lifecycle walk;
  - the two agent tasks rerun with the updated skill. They pass when the original
    failure points do not recur.
- **Closeout.**
  - Run `make test`.
  - Promote the regression owners to the testing guidance.
  - Retire this dossier.
  - Record the owner's epic acceptance in the roadmap.

## Failure handling and compatibility boundaries

- **Default output.** Generation-1 default JSON keeps its fields, error codes, exits
  and retryability. The default output stays byte-identical except for these intended
  changes:
  - help and completion text, including the `contract-generations` topic;
  - local-command error messages;
  - rejection of `--if-goal-revision` on local commands;
  - acceptance of session fences on plain `start`;
  - human-mode hints;
  - manifest digests reported by `version --identity`, workspace views and the
    expected digest of mismatch details, because the manifest itself changes.
- **Generation selection** never changes request identity, idempotency, stored
  outcomes, replay, outcome lookup, Store layout or writer identity.
- **Unfenced plain start** keeps its canonical request bytes.
- **Schemas.** New schemas are additions. Released families and the error catalog
  stay unchanged.
- **Consumer re-pin.** The manifest, the Output v4 and Error v2 schema bytes and the
  Dolgorae adapter schema pins change, so Dolgorae must re-pin. The adapter's
  runtime errors stay unchanged.
- **Fail closed.** An invalid generation value fails before workspace access. A
  template-to-precondition mismatch is a projection failure, so inconsistent templates
  are never emitted.
- **Upgrades.** No writer predecessor is declared. The release that ships GUIDE
  refuses previous-release Stores, and the release documentation owns that upgrade
  guidance.

## Roadmap ownership, dependencies, and traceability

The roadmap is the only status authority. Tasks run in numeric order with one active
task.

| Task | Deliverable and focused verification |
|---|---|
| `GUIDE-001` | Short socket roots. The regression fails first under a long TMPDIR, then passes. |
| `GUIDE-002` | Complete local-option rejection and messages. Each option and each local family is checked, and no `.podway` is created. |
| `GUIDE-003` | Help topic resolution. A word-to-route table is compared with the clap tree, and every route identifier resolves. |
| `GUIDE-004` | Help text and human hints. The example decodes, JSON is byte-identical, and human stderr is exact. |
| `GUIDE-005` | Runtime skill guidance. Documentation checks. |
| `GUIDE-006` | Normalized generation-1 baselines whose expected values stay unchanged through GUIDE-011. |
| `GUIDE-007` | Reserved generation-2 contracts. Protocol contract tests, `verify_contracts.py --check` and `--sentinels`, handoff self-test, manifest check. |
| `GUIDE-008` | Fenced plain start. Lifecycle, fence, replay, dry-run and terminal cases; unfenced bytes unchanged; catalog contract checks. Owns `AUT-LIF-016`. |
| `GUIDE-009` | Selection and observation v6. Every v6 lifecycle template executes with exact replay; exact family per generation. Owns `AUT-GEN-*` and `AUT-OBS-013` to `AUT-OBS-017`. |
| `GUIDE-010` | Generation-2 details. Citable lists per failure path; known and unknown writers. Owns `AUT-ERR-006` and `AUT-ERR-007`. |
| `GUIDE-011` | Declarations, AGENTS.md exception and guidance. Contract checks, release-evidence self-test, documentation checks, `make test`. |
| `GUIDE-012` | Integration review, VM re-verification, `make test`, promotion and retirement. |

GUIDE-011 records the verification owners of the new requirements in the
requirements traceability.

## Verification and release acceptance

- **Executable tasks.** Run focused checks, `make lint-all` when tests change, and
  `make test`.
- **Documentation-only tasks.** Run `tools/verify_docs.py`, its self-test and
  `git diff --check`.
- **Contract tasks.** Also run `tools/verify_contracts.py --check` and `--sentinels`,
  `tools/create_dolgorae_handoff.py self-test` and `tools/contract_manifest.py --check`.
- **Backward compatibility.**
  - The GUIDE-006 expected values stay unchanged.
  - Existing known answers stay unchanged.
  - The default output equals an explicit generation 1.
  - Replay is identical across generations.
  - Unfenced plain start keeps its canonical bytes.
- **Usability.** The GUIDE-012 agent reruns no longer hit the original failure points.
- **Release.** GUIDE completion makes the work eligible for `v0.2.15`. The release
  requires separate authorization and its own gate.

## References

- [AGENTS.md](../../AGENTS.md)
- Architecture decisions:
  [ADR-0010](../architecture-decision-records/0010-generic-cli-json-integration.md),
  [ADR-0019](../architecture-decision-records/0019-procedure-v2-only-product.md),
  [ADR-0027](../architecture-decision-records/0027-state-aware-session-start.md),
  [ADR-0038](../architecture-decision-records/0038-remove-the-daemon.md),
  [ADR-0039](../architecture-decision-records/0039-opt-in-contract-generations.md)
- Specifications:
  [JSON contract](../specs/interfaces/json-contract.md),
  [automation contract](../specs/interfaces/automation-client-contract.md),
  [CLI specification](../specs/interfaces/cli-specification.md),
  [errors and exit codes](../specs/interfaces/errors-and-exit-codes.md),
  [release and packaging](../specs/operations/release-and-packaging.md)
- Implementation guidance:
  [testing](../implementation-tips/testing.md),
  [release](../implementation-tips/release.md),
  [upgrade QA](../implementation-tips/release-upgrade-qa.md)
- [CALLS candidate](TODO-podway-cli-interaction-simplification.md)
- Prior generation reservations: `82c348f` (V2SDG-002) and `ab53f90` (V2DFC-003)
