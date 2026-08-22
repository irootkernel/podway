# Built-in Procedures

Podway ships exactly three built-in Procedure v2 presets. Their YAML files under
[`assets/presets/`](../../../assets/presets/) are canonical and embedded into the
binary with pinned digests.

| ID | Selection boundary |
| --- | --- |
| `small-change-v2` | A bounded change that needs assertion-based verification and one review rework route, but no goal tracking or typed external result. |
| `bug-fix-v2` | A defect workflow that needs reproduction, diagnosis, a structurally bound verification result, guarded retry and review decisions, and goal assessment. |
| `sw-dev-v2` | The full software-development reference with bounded planning, conditional evidence, paging, an optional artifact, guarded phase-owner rework, and goal assessment. |

`podway preset list`, `show`, and `explain` expose only this catalog. A workspace
created by `podway init` defaults to `sw-dev-v2`. A preset start admits the exact
embedded source and fails closed if its shipped digest does not match.

Custom Procedure v2 files remain supported through the configured safe relative
procedure paths. Built-in and custom procedures pass through the same v2 parser,
validator, canonicalizer, and runtime model.

## Requirement matrix

`Required` means that the preset demonstrates the feature. `Forbidden` means that
the feature does not occur anywhere in that preset. A list is single-page when
`max_items * max_item_length <= 43,690` Unicode scalars, derived from the 256 KiB
page-data allocation and the six-byte worst-case JSON escape charge. A larger
selected list is multi-page.

| Feature | `small-change-v2` | `bug-fix-v2` | `sw-dev-v2` |
| --- | --- | --- | --- |
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

All presets deliberately omit `empty` and `non_empty` option guards. Authoring
examples teach those operators separately because adding them to these graphs
would require complementary routes and obscure their intended progression. An
explicit state such as `documentation-state: not-applicable` is preferred to
skipping an owned phase.

## `small-change-v2`

This preset is the deliberately lightweight baseline. Goal tracking is off,
verification is an attributed assertion, and the graph contains no typed result,
condition, guard, artifact, paging, or phase-owner option set.

| Node | Kind | Items and bounds | Successor or routes |
| --- | --- | --- | --- |
| `inspect` | action | `scope-summary`: required text, 1..4,000 | `implement` |
| `implement` | action | `implementation-summary`: required text, 1..4,000 | `verify` |
| `verify` | action | `verification-command`: required text, 1..2,000; `verification-exit-status`: required integer | `review` |
| `review` | decision | no items; required reason | `ready` advances to `closeout`; `changes-requested` reworks to `implement` |
| `closeout` | terminal action | `closeout-note`: required text 1..2,000 | terminal |

| Consumer | Required source | Selected items |
| --- | --- | --- |
| `implement` | `inspect` | `scope-summary` |
| `verify` | `implement` | `implementation-summary` |
| `review` | `inspect` | `scope-summary` |
| `review` | `implement` | `implementation-summary` |
| `review` | `verify` | `verification-command`, `verification-exit-status` |

`manual_rework.allowed_targets` is exactly `inspect`, `implement`, and `verify`.
The verification values are caller assertions. A workflow that needs stronger
mechanical binding uses `check_result` instead.

## `bug-fix-v2`

This middle preset adds one structurally bound verification result and guarded
decisions. Every list fits in one read-back page. It omits conditional requiredness
and multi-target phase-owner option routing.

| Node | Kind | Items and bounds | Successor or routes |
| --- | --- | --- | --- |
| `reproduce` | action | `reproduction-status`: required choice `reproduced`/`not-reproduced`; `observed-behavior`: required text 1..4,000; `expected-behavior`: required text 1..2,000; `regression-check`: required text 1..2,000 | `diagnose` |
| `diagnose` | action | `cause`: required text 1..4,000; `affected-boundary`: required text 1..1,000 | `implement` |
| `implement` | action | `fix-summary`: required text 1..4,000; `changed-boundaries`: required list, 1..20 entries of at most 500 | `verify` |
| `verify` | action | `verification-result`: required `check_result`, operation ID `bug-fix-verification`, operation digest fixed below, accepted outcomes `pass`, `fail`, and `inconclusive` | `evaluate-verification` |
| `evaluate-verification` | decision | no items; required reason | `passed` guards outcome `equals: pass` and advances to `review`; `retry` guards outcome `not_equals: pass` and reworks to `implement` |
| `review` | action | `review-summary`: required text 1..4,000; `unresolved-valid-findings`: required integer, minimum 0; `review-findings`: optional list, at most 20 entries of at most 500 | `evaluate-review` |
| `evaluate-review` | decision | no items; required reason | `approved` guards count `equals: 0` and advances to `assess-goal`; `changes-requested` guards count `at_least: 1` and reworks to `implement` |
| `assess-goal` | goal-assessment decision | optional `assessment-note`: text, at most 2,000; required reason | `achieved`, `not-achieved`, and `superseded` advance to `closeout` with the corresponding goal outcome |
| `closeout` | terminal action | `closeout-note`: required text 1..2,000 | terminal |

| Consumer | Required source | Selected items or record |
| --- | --- | --- |
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

The `check_result` declaration accepts all three outcomes intentionally. Item
satisfaction owns whether the node may advance, while the decision guard owns
pass/fail routing; accepting only `pass` would make the guarded retry route
unreachable.

The exact external operation definition is `Run the declared regression check
and the repository-authoritative surrounding verification for the current bug
fix in the recorded input basis.` Its identity is the SHA-256 digest of that
exact UTF-8 sentence, including the trailing period and with no trailing newline:
`sha256:01122b6057efcfa3f22c453aa48fb647ccba9f7db437dd44473a9f32a854a979`.
The preset owns this abstract operation contract; Podway does not choose or run
the repository-specific command. The digest versions this abstract slot contract
only, and rewording it invalidates previously recorded results. It does not
identify, describe, or attest the integrator's actual command, and a matching
digest is not evidence that any particular operation ran. An integrator with a
concrete operation contract authors a Procedure that declares it instead of
reusing this preset constant.

`manual_rework.allowed_targets` is exactly `reproduce`, `diagnose`, `implement`,
`verify`, and `review`. No item or instruction records a commit, source revision,
or clean-worktree claim.

## `sw-dev-v2`

This full preset is the reference for bounded planning, typed external results,
conditional required items, guards, one intentionally multi-page evidence item,
artifacts, goals, and explicit phase-owner routing.

| Node | Kind | Items and bounds | Successor or routes |
| --- | --- | --- | --- |
| `plan` | action | `scope-summary`: required text 1..4,000; `success-criteria`: required list, 1..50 entries of at most 500; `risk-summary`: required text 1..2,000 | `implement` |
| `implement` | action | `implementation-summary`: required text 1..4,000; `changed-boundaries`: required list, 1..50 entries of at most 500 | `verify` |
| `verify` | action | `verification-result`: required `check_result`, operation ID `software-change-verification`, operation digest fixed below, accepted outcomes `pass`, `fail`, and `inconclusive`; `verification-observations`: optional list, at most 20 entries of at most 500, required when result outcome is not `pass`; `verification-report`: optional artifact allowing `text/plain`, `application/json`, and `application/xml` | `evaluate-verification` |
| `evaluate-verification` | decision | no items; required reason | `acceptable` guards outcome `equals: pass` and advances to `document`; `retry` guards outcome `not_equals: pass` and reworks to `implement` |
| `document` | action | `documentation-state`: required choice `not-applicable`/`updated`; `documentation-summary`: optional text 1..4,000, required when state equals `updated` | `review` |
| `review` | action | `review-summary`: required text 1..4,000; `unresolved-implementation-findings`: required integer, minimum 0; `unresolved-documentation-findings`: required integer, minimum 0; `review-findings`: optional list, at most 100 entries of at most 4,000 | `evaluate-review` |
| `evaluate-review` | decision | no items; required reason | `approved` guards both counts `equals: 0` and advances to `assess-goal`; `implementation-changes` guards implementation count `at_least: 1` and reworks to `implement`; `documentation-changes` guards implementation count `equals: 0` and documentation count `at_least: 1`, then reworks to `document` |
| `assess-goal` | goal-assessment decision | optional `assessment-note`: text, at most 2,000; required reason | `achieved`, `not-achieved`, and `superseded` advance to `closeout` with the corresponding goal outcome |
| `closeout` | terminal action | `closeout-note`: required text 1..2,000 | terminal |

| Consumer | Required source | Selected items or record |
| --- | --- | --- |
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
the observations no longer required; an already recorded value remains and the
preset does not depend on clearing it.

The exact external operation definition is `Run the repository-authoritative
verification required for the current software change in the recorded input
basis.` Its identity is the SHA-256 digest of that exact UTF-8 sentence, including
the trailing period and with no trailing newline:
`sha256:b904aefd4dbd6b01337645fd34b1424efc9faf3d22c936de666305335076969e`.
The preset owns this abstract operation contract; Podway does not choose or run
the repository-specific command. The digest versions this abstract slot contract
only, and rewording it invalidates previously recorded results. It does not
identify, describe, or attest the integrator's actual command, and a matching
digest is not evidence that any particular operation ran. An integrator with a
concrete operation contract authors a Procedure that declares it instead of
reusing this preset constant.

Every guarded source item is required at its source node, and every guard reference
is required and explicitly selected. Over non-negative counts, the three review
options are exhaustive and mutually exclusive: implementation findings take
precedence; otherwise documentation findings route to `document`; otherwise
approval advances. This is the phase-owner routing example.

`manual_rework.allowed_targets` is exactly `plan`, `implement`, `verify`,
`document`, and `review`. Manual rework is a bare target allowlist. Guarded
decision options separately carry criteria and `effect: rework` routes to
`implement` or `document`. The lifecycle retains one active attempt and one graph
cursor.

## Recording, compatibility, and trust boundaries

Procedure definitions, prompts, examples, goal statements, goal criteria, goal
revision and assessment reasons, text/list narrative evidence, check-result
descriptors and summaries, and transition reasons are written in English. Exact
commands, paths, hashes, identifiers, enumerated values, product names, and other
opaque source tokens remain verbatim. Non-English logs or source passages are
summarized in English and represented by a digest or stable reference instead of
being copied into Podway. This is an operator and agent authoring contract; the
domain and protocol remain locale-neutral and continue to accept bounded Unicode.

Preset IDs remain stable, while a changed canonical preset receives a new version
and embedded digest. Existing sessions retain immutable admitted snapshots and
are never migrated or reinterpreted. The canonical YAML, pinned embedded digest,
preset selection, and packaged manifest must bind the same exact bytes.

`podway init` continues to select `sw-dev-v2`. Changes to that preset affect only
newly initialized workspaces and new sessions. They do not change an already
admitted snapshot.

Podway enforces the declared structure, freshness, conditions, guards, and
progression rules. It does not execute an external operation, judge semantic
truth, attest a Git state, or prove publication. No preset instructs or implies a
commit, push, release, or other Git mutation.
