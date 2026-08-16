# Podway Reference Procedures and Authoring Guidance

## Status and authority

- Document state: `Adopted`
- Dossier type: adopted design dossier
- Owning roadmap epic: `V2REF`
- Target product release: v0.3.0
- Repository scope: Podway built-ins and documentation
- Adoption baseline: August 17, 2026

This dossier is the decision-complete plan for making Podway's shipped bundle and
examples reliable references for downstream Procedure authors. The active
roadmap owns task order and status. Canonical presets remain under
`assets/presets/`; documentation never becomes a second source copy.

## 1. Verified context and evidence

All three shipped presets are formally valid, but their quality as examples is
uneven. `small-change-v2` is the clearest bounded path. `bug-fix-v2` already
separates recording from evaluation but does not demonstrate typed results or
guards. `sw-dev-v2` starts at implementation, permits weak empty records, and
relies on prose where the adopted scale, result, and guard contracts should be
shown.

Downstream integrations have compensated with larger phase-owner Procedures,
large read-back sets, and prose-only routing conditions. The bundle should teach
bounded summaries, selected evidence, page tokens, check results, typed guards,
and explicit rework without including Git mutation or integrator-specific
concepts.

## 2. Goals, non-goals, and scope

The epic will:

- keep exactly three stable built-in preset identities;
- give each preset a distinct and testable selection boundary;
- make every required narrative text item declare `min_length: 1`;
- document downstream `non_empty` predicates only where whitespace-only content
  is a progression-safety problem and the resulting option set remains total;
- require English narrative goal and evidence recording in agent guidance; and
- provide copyable authoring patterns and failure, rework, paging, and goal
  walkthroughs.

The epic will not add commit, push, release, command execution, provider-specific
review tools, project management, external repository files, or another shipped
preset. No authored preset string may instruct, require, or imply Git mutation.
The runtime remains locale-neutral and does not reject non-English Unicode input.

## 3. Accepted preset designs

### 3.0 Requirement matrix

`Required` means the preset must demonstrate the feature. `Forbidden` means the
feature must not occur anywhere in that preset. A list is single-page when
`max_items * max_item_length <= 43,690` Unicode scalars, derived from the 256 KiB
page-data allocation and the six-byte worst-case JSON escape charge. A larger
selected list is multi-page.

| Feature | `small-change-v2` | `bug-fix-v2` | `sw-dev-v2` |
|---|---|---|---|
| Goal tracking and criterion assessment | Forbidden | Required | Required |
| Assertion-based verification | Required | Forbidden | Forbidden |
| Structurally bound `check_result` verification | Forbidden | Required | Required |
| `required_when` | Forbidden | Forbidden | Required |
| Decision option guards | Forbidden | Required | Required |
| Explicit selected evidence items | Required | Required | Required |
| Multi-page list evidence | Forbidden | Forbidden | Required |
| Optional artifact declaration | Forbidden | Forbidden | Required |
| Decision route with `effect: rework` | Required | Required | Required |
| Multiple phase-owner rework option targets | Forbidden | Forbidden | Required |
| Declared `manual_rework.allowed_targets` | Required | Required | Required |
| Skippable placement | Forbidden | Forbidden | Forbidden |
| English narrative recording guidance | Required | Required | Required |
| Authored Git-mutation implication | Forbidden | Forbidden | Forbidden |

All presets deliberately omit `empty` and `non_empty` option guards. The authoring
examples teach those operators separately because adding them to these graphs
would require complementary routes and obscure the intended progression. An
explicit recorded state such as `documentation-state: not-applicable` is preferred
to skipping an owned phase.

### 3.1 `small-change-v2`

This preset remains the deliberately lightweight baseline. Goal tracking is off,
verification is an attributed assertion, and the graph contains no typed result,
condition, guard, artifact, paging, or phase-owner option set.

| Node | Kind | Items and bounds | Successor or routes |
|---|---|---|---|
| `inspect` | action | `scope-summary`: required text, 1..4,000 | `implement` |
| `implement` | action | `implementation-summary`: required text, 1..4,000 | `verify` |
| `verify` | action | `verification-command`: required text, 1..2,000; `verification-exit-status`: required integer | `review` |
| `review` | decision | no items; required reason | `ready` advances to `closeout`; `changes-requested` reworks to `implement` |
| `closeout` | terminal action | `closeout-note`: required text, 1..2,000 | terminal |

| Consumer | Required source | Selected items |
|---|---|---|
| `implement` | `inspect` | `scope-summary` |
| `verify` | `implement` | `implementation-summary` |
| `review` | `inspect` | `scope-summary` |
| `review` | `implement` | `implementation-summary` |
| `review` | `verify` | `verification-command`, `verification-exit-status` |

`manual_rework.allowed_targets` is exactly `inspect`, `implement`, and `verify`.
Documentation states that the verification values are caller assertions and that
stronger mechanical checks should use `check_result`.

### 3.2 `bug-fix-v2`

This middle preset adds one structurally bound verification result and guarded
decisions, while keeping every list within one read-back page and omitting
conditional requiredness and multi-target phase-owner option routing.

| Node | Kind | Items and bounds | Successor or routes |
|---|---|---|---|
| `reproduce` | action | `reproduction-status`: required choice `reproduced`/`not-reproduced`; `observed-behavior`: required text 1..4,000; `expected-behavior`: required text 1..2,000; `regression-check`: required text 1..2,000 | `diagnose` |
| `diagnose` | action | `cause`: required text 1..4,000; `affected-boundary`: required text 1..1,000 | `implement` |
| `implement` | action | `fix-summary`: required text 1..4,000; `changed-boundaries`: required list, 1..20 entries of at most 500 | `verify` |
| `verify` | action | `verification-result`: required `check_result`, operation ID `bug-fix-verification`, operation digest fixed below, accepted outcomes `pass`, `fail`, and `inconclusive` | `evaluate-verification` |
| `evaluate-verification` | decision | no items; required reason | `passed` guards outcome `equals: pass` and advances to `review`; `retry` guards outcome `not_equals: pass` and reworks to `implement` |
| `review` | action | `review-summary`: required text 1..4,000; `unresolved-valid-findings`: required integer, minimum 0; `review-findings`: optional list, at most 20 entries of at most 500 | `evaluate-review` |
| `evaluate-review` | decision | no items; required reason | `approved` guards count `equals: 0` and advances to `assess-goal`; `changes-requested` guards count `at_least: 1` and reworks to `implement` |
| `assess-goal` | goal-assessment decision | optional `assessment-note`: text, at most 2,000; required reason | `achieved`, `not-achieved`, and `superseded` all advance to `closeout` with their corresponding goal outcome |
| `closeout` | terminal action | `closeout-note`: required text 1..2,000 | terminal |

| Consumer | Required source | Selected items or record |
|---|---|---|
| `diagnose` | `reproduce` | `reproduction-status`, `observed-behavior`, `expected-behavior`, `regression-check` |
| `implement` | `reproduce` | `regression-check` |
| `implement` | `diagnose` | `cause`, `affected-boundary` |
| `verify` | `implement` | `fix-summary`, `changed-boundaries` |
| `evaluate-verification` | `verify` | `verification-result` |
| `review` | `diagnose` | `cause`, `affected-boundary` |
| `review` | `implement` | `fix-summary`, `changed-boundaries` |
| `review` | `verify` | `verification-result` |
| `evaluate-review` | `review` | `review-summary`, `unresolved-valid-findings`, `review-findings` |
| `assess-goal` | `reproduce` | `observed-behavior`, `expected-behavior`, `regression-check` |
| `assess-goal` | `verify` | `verification-result` |
| `assess-goal` | `evaluate-review` | decision record |
| `assess-goal` | `review` | `review-summary`, `unresolved-valid-findings` |
| `closeout` | `assess-goal` | goal-assessment decision record |

Every guard source is dominating, declared `required: true`, and explicitly
selected. The guarded source items are required at their source nodes, so the
cursor cannot reach either evaluating decision with an unevaluable predicate.
The non-negative count domains make both option sets exhaustive and mutually
exclusive.

The `check_result` declaration accepts all three outcomes intentionally. V2AST
considers a result satisfied only when its outcome is accepted; accepting only
`pass` would block the `verify` node and make the guarded `retry` route unreachable.
The decision guard, not item satisfaction, owns pass/fail routing.

The exact external operation definition is `Run the declared regression check
and the repository-authoritative surrounding verification for the current bug
fix in the recorded input basis.` Its identity is the SHA-256 digest of that
exact UTF-8 sentence, including the trailing period and with no trailing newline:
`sha256:01122b6057efcfa3f22c453aa48fb647ccba9f7db437dd44473a9f32a854a979`.
The preset owns this abstract operation contract; Podway does not choose or run
the repository-specific command. The digest versions this abstract slot contract
only, and rewording the contract invalidates previously recorded results. It does
not identify, describe, or attest the integrator's actual command, and a matching
digest is not evidence that any particular operation ran. The item prompt and
help carry the same caveat. An integrator with a concrete operation contract
authors a Procedure that declares it instead of reusing this preset constant.

`manual_rework.allowed_targets` is exactly `reproduce`, `diagnose`, `implement`,
`verify`, and `review`. No item or instruction records a commit, source revision,
or clean-worktree claim.

### 3.3 `sw-dev-v2`

This full preset is the reference for bounded planning, typed external results,
conditional required items, guards, one intentionally multi-page evidence item,
artifacts, goals, and explicit phase-owner routing.

| Node | Kind | Items and bounds | Successor or routes |
|---|---|---|---|
| `plan` | action | `scope-summary`: required text 1..4,000; `success-criteria`: required list, 1..50 entries of at most 500; `risk-summary`: required text 1..2,000 | `implement` |
| `implement` | action | `implementation-summary`: required text 1..4,000; `changed-boundaries`: required list, 1..50 entries of at most 500 | `verify` |
| `verify` | action | `verification-result`: required `check_result`, operation ID `software-change-verification`, operation digest fixed below, accepted outcomes `pass`, `fail`, and `inconclusive`; `verification-observations`: optional list, at most 20 entries of at most 500, required when result outcome is not `pass`; `verification-report`: optional artifact allowing `text/plain`, `application/json`, and `application/xml` | `evaluate-verification` |
| `evaluate-verification` | decision | no items; required reason | `acceptable` guards outcome `equals: pass` and advances to `document`; `retry` guards outcome `not_equals: pass` and reworks to `implement` |
| `document` | action | `documentation-state`: required choice `not-applicable`/`updated`; `documentation-summary`: optional text 1..4,000, required when state equals `updated` | `review` |
| `review` | action | `review-summary`: required text 1..4,000; `unresolved-implementation-findings`: required integer, minimum 0; `unresolved-documentation-findings`: required integer, minimum 0; `review-findings`: optional list, at most 100 entries of at most 4,000 | `evaluate-review` |
| `evaluate-review` | decision | no items; required reason | `approved` guards both counts `equals: 0` and advances to `assess-goal`; `implementation-changes` guards implementation count `at_least: 1` and reworks to `implement`; `documentation-changes` guards implementation count `equals: 0` and documentation count `at_least: 1`, then reworks to `document` |
| `assess-goal` | goal-assessment decision | optional `assessment-note`: text, at most 2,000; required reason | `achieved`, `not-achieved`, and `superseded` all advance to `closeout` with their corresponding goal outcome |
| `closeout` | terminal action | `closeout-note`: required text 1..2,000 | terminal |

| Consumer | Required source | Selected items or record |
|---|---|---|
| `implement` | `plan` | `scope-summary`, `success-criteria`, `risk-summary` |
| `verify` | `implement` | `implementation-summary`, `changed-boundaries` |
| `evaluate-verification` | `verify` | `verification-result`, `verification-observations`, `verification-report` |
| `document` | `implement` | `implementation-summary`, `changed-boundaries` |
| `document` | `evaluate-verification` | decision record |
| `review` | `plan` | `scope-summary`, `success-criteria`, `risk-summary` |
| `review` | `implement` | `implementation-summary`, `changed-boundaries` |
| `review` | `verify` | `verification-result`, `verification-observations`, `verification-report` |
| `review` | `document` | `documentation-state`, `documentation-summary` |
| `evaluate-review` | `review` | `review-summary`, both unresolved-finding counts, `review-findings` |
| `assess-goal` | `plan` | `success-criteria` |
| `assess-goal` | `verify` | `verification-result` |
| `assess-goal` | `evaluate-verification` | decision record |
| `assess-goal` | `review` | `review-summary`, both unresolved-finding counts, `review-findings` |
| `assess-goal` | `evaluate-review` | decision record |
| `closeout` | `assess-goal` | goal-assessment decision record |

`review-findings` has a 400,000-scalar declared maximum and is the bundle's only
multi-page list example. It appears on the clean path and is selected by
`assess-goal`, so `session.next` can expose its exact count, digest, first page,
and `next_page_token`; later pages use `evidence.read --page-token`.
`verification-observations` remains single-page so it teaches `required_when`
without also carrying the paging lesson. Re-recording a passing result may make
the observations no longer required; any already recorded value remains and the
preset does not depend on clearing it.

The exact external operation definition is `Run the repository-authoritative
verification required for the current software change in the recorded input
basis.` Its identity is the SHA-256 digest of that exact UTF-8 sentence,
including the trailing period and with no trailing newline:
`sha256:b904aefd4dbd6b01337645fd34b1424efc9faf3d22c936de666305335076969e`.
The preset owns this abstract operation contract; Podway does not choose or run
the repository-specific command. The digest versions this abstract slot contract
only, and rewording the contract invalidates previously recorded results. It does
not identify, describe, or attest the integrator's actual command, and a matching
digest is not evidence that any particular operation ran. The item prompt and
help carry the same caveat. An integrator with a concrete operation contract
authors a Procedure that declares it instead of reusing this preset constant.

The guarded source items are required at their source node and every guard
reference is required and explicitly selected. Over non-negative counts, the
three review options are exhaustive and mutually exclusive: implementation
findings take precedence; otherwise documentation findings route to `document`;
otherwise approval advances. This is the deliberate phase-owner routing example.

`manual_rework.allowed_targets` is exactly `plan`, `implement`, `verify`,
`document`, and `review`. Manual rework is a bare target allowlist; the guarded
decision options separately carry distinct criteria and `effect: rework` routes
to `implement` or `document`. The lifecycle preserves one active attempt and one
graph cursor. The removed post-verification refinement phase cannot invalidate
fresh verification by silently changing source afterward.

### 3.4 English recording policy

Procedure definitions, prompts, examples, goal statements, goal criteria, goal
revision and assessment reasons, text/list narrative evidence, check-result
descriptors and summaries, and transition reasons are written in English. Exact
commands, paths, hashes, identifiers, enumerated values, product names, and other
opaque source tokens remain verbatim. Non-English logs or source passages are
summarized in English and represented by a digest or stable reference rather than
copied into Podway.

This is an operator and agent authoring contract. The Podway domain and protocol
remain locale-neutral and continue to accept bounded Unicode.

## 4. Authoring examples and diagnostics

Canonical YAML remains only in `assets/presets/`. Documentation links to those
sources and adds focused excerpts for:

- evidence selectors and largest-contributor diagnostics;
- `evidence.read` pagination, page-token replay, and stale page tokens;
- assertion evidence versus structurally bound `check_result` evidence;
- `required_when`, `empty`/`non_empty`, and exhaustive decision guards;
- clean phase-owner routing that passes `--warnings-as-errors`;
- manual rework versus decision `effect: rework` and the resulting stale suffix;
  and
- clean, failure, rework, goal-superseded, and terminal paths.

Examples must state what Podway formally checks and what remains external
judgment. They must not claim execution, semantic truth, commit state, clean Git
state, or publication from a recorded assertion or check result.

Both goal-tracked graphs keep goal clarification in the first three nodes and
goal assessment within four nodes of every terminal, matching
`GOAL_CLARIFICATION_PREFIX_NODES = 3` and
`GOAL_ASSESSMENT_TERMINAL_DISTANCE = 4`.

## 5. Failure handling, budgets, and compatibility

- Preset IDs remain `small-change-v2`, `bug-fix-v2`, and `sw-dev-v2`.
- Preset versions and digests change. Existing sessions retain immutable admitted
  snapshots and are never migrated or reinterpreted.
- The canonical YAML task that changes a preset also re-pins its corresponding
  constant in `crates/podway-presets/src/lib.rs`; fail-closed admission, preset
  selection, and the packaged manifest must bind the same exact embedded bytes.
- The same task promotes each preset-owned external operation definition verbatim,
  its no-trailing-newline digest rule, and its trust caveat into
  `docs/specs/domain/built-in-presets.md`, so every digest preimage remains
  auditable after this dossier is deleted.
- `podway init` continues to default to `sw-dev-v2`. Its new graph therefore
  changes the first-run experience only for newly initialized workspaces and new
  sessions; admitted sessions retain their snapshots.
- `crates/podway-config/src/procedure_v2_budget.rs` known-answer values are updated
  per preset in the task that changes that preset.
- Every preset must fit the V2SCL `session.next` static allocation of 256 KiB,
  complete-decision-record allocation of 272 KiB, evidence metadata allocation
  of 208 KiB, observation active-item allocation of 128 KiB, and the V2GRD
  guard-status sub-maximum of 95,888 bytes.
- Every preset must pass validate, vet, lint, check with warnings as errors,
  projection equivalence, embedding, packaging, and isolated runtime path tests.

## 6. Roadmap ownership and dependencies

`V2REF` depends on completed `V2GRD`.

- `V2REF-001` promotes the adopted preset quality contract, exact graph tables,
  requirement matrix, and executable path fixtures into durable authorities
  before canonical YAML changes.
- `V2REF-002` rebuilds `sw-dev-v2`, re-pins its embedded digest, and updates its
  exact-source identity, manifest, projections, budget known answers, tests, and
  product specification, including verbatim promotion of its external operation
  definition and digest rule.
- `V2REF-003` rebuilds `bug-fix-v2` and `small-change-v2`, re-pins both embedded
  digests, and updates their identity, manifest, projections, budget known answers,
  tests, and selection boundaries, including verbatim promotion of the bug-fix
  external operation definition and digest rule. The shared budget module is
  intentionally touched by both preset tasks; each owns only the answers for its
  presets.
- `V2REF-004` updates English recording guidance, authoring examples, CLI
  walkthroughs, `podway init` migration guidance, and the built-in preset
  specification without copying canonical preset files.
- `V2REF-005` dogfoods every clean, failure, guarded, conditional, paging, and
  rework path in isolated workspaces; proves budget fit, guard dominance,
  satisfiability, and option totality; and passes the complete `make test`
  development gate.

Before `V2REF` is marked complete, its tasks must promote every lasting preset,
selection, authoring, English-recording, compatibility, and operating decision
into the affected specifications, architecture, implementation tips, canonical
assets, examples, and roadmap evidence. The final task replaces roadmap
references to this dossier with those durable authorities, removes its TODO index
entry, and deletes this file. This dossier is not moved to
`docs/roadmap/archive/`.

## 7. Verification and acceptance

Acceptance requires:

- warnings-as-errors authoring checks with no `OPTION_LABELS_NOT_DISTINCT`,
  `OPTION_CRITERIA_WEAK`, or narrowed `REWORK_TOPOLOGY_CONFUSING` finding;
- canonical YAML, embedded digest, selector, contract manifest, and packaged
  bundle identity;
- schema, projection, and preset budget known-answer validation;
- dominance and required-selected-source validation for every guard;
- a legal evidence state for every option, exhaustive guarded option sets over
  their declared domains, and no permanently unavailable option;
- isolated runtime coverage for every route and manual rework target, including
  check-result failure, conditional requiredness, page-token continuation and
  staleness, goal outcomes, stale evidence, restart, and snapshot retention;
- the V2SCL and V2GRD response-budget proofs named in section 5, including
  headroom for the complete goal-assessment decision record selected by each
  `closeout` node;
- durable `built-in-presets.md`, Procedure, IPC, error, security/trust, workflow,
  example, and migration guidance with valid links; and
- the complete development gate.

No commit, push, distribution, installation, or live activation is implied.

## 8. References and durable promotion targets

- [TODO and Adopted Design Dossiers](README.md)
- [Built-in Presets](../specs/domain/built-in-presets.md)
- [Procedure and Item Specification](../specs/domain/procedure-and-item-specification.md)
- [IPC Protocol](../specs/interfaces/ipc-protocol.md)
- [Errors and Exit Codes](../specs/interfaces/errors-and-exit-codes.md)
- [Security and Trust](../specs/operations/security-and-trust.md)
- [User Workflows](../specs/product/user-workflows.md)
- [Procedure v2 Workflow Example](../examples/v2-workflow.md)
- [`procedure-v2.schema.json`](../../assets/schemas/procedure-v2.schema.json)
- [`assets/presets/`](../../assets/presets/)
- [Contract Manifest](../../contracts/contract-manifest-v1.json)
