# Procedure v2 Capability Inventory

Read before choosing features. The exact validating binary's manifest-bound
schema remains authoritative; scaffold and any single preset are incomplete views.

```podway-capability-inventory
{"item_types":["artifact","check_result","choice","confirm","integer","list","text"],"predicate_operators":["at_least","at_most","empty","equals","non_empty","not_equals"]}
```

## Items

| Type | Meaning and boundary |
| --- | --- |
| `confirm` | Attributed boolean assertion, not independent verification |
| `text` | Bounded narrative; logs and large documents stay outside Podway |
| `choice` | One value from a closed authored set |
| `integer` | Bounded count, ordinal or status number |
| `list` | Repeated values with count, uniqueness and total-size bounds |
| `artifact` | Stable media-typed reference; no stored bytes or content attestation |
| `check_result` | Authored operation ID/digest and closed pass/fail/inconclusive result; no proof of execution |

`required_when` is permitted only with `required: false`; its 1..4 predicates
reference earlier unconditionally required items in the same node definition
(action or decision), and controllers cannot themselves be conditional.
It selects conditional evidence mechanically, not semantic policy.

## Predicates and evidence

The six operators are `equals`, `not_equals`, `empty`, `non_empty`, `at_least` and
`at_most`. `required_when` selects same-node `item`; decision guards select
`evidence.node` and `evidence.item`. For check-result outcome comparison, use
`field: outcome`.

| Operators | Admitted item types |
| --- | --- |
| `equals`, `not_equals` | `confirm`, `choice`, `integer`, `check_result` with `field: outcome` |
| `empty`, `non_empty` | `text`, `list` |
| `at_least`, `at_most` | `integer` only |

Predicate literals must match the admitted item type. Use an integer count item
for collection thresholds. Guard evidence must be
required, fresh and explicitly selected by the decision. Make options exhaustive
over reachable values.

An omitted evidence item selector publishes no item previews. Required sources
dominate consumers; absent optional branch evidence does not block them.
Selected text/list values may require paged read-back. Qualify complete digest/counts,
continuation and stale-token rejection only when selected evidence needs multiple
pages. Do not inflate evidence merely to demonstrate paging.

## Lifecycle features

- `skip` is placement policy, not a general bypass. Do not skip a required evidence
  source or decision-owning phase; use an accounted not-applicable choice instead.
- `goal_tracking: true` requires goal definition and fresh assessment near closeout.
  Map every terminal outcome and its supporting evidence.
- Decision `effect: rework` replaces the stale suffix from an owned target;
  `manual_rework.allowed_targets` independently limits operator rework.
- A schema-admitted closed `delegation` declaration belongs to an action placement,
  never a decision or skip-enabled action. It binds the logical integration,
  immutable Role, round bound and worker disposition. It is not an execution hook.
  Omission remains Direct. A package/receipt adds a structural completion condition;
  committed item-state changes after acceptance require a fresh attempt. See
  [delegation authoring](delegation.md) for fields and correction design.

## Exact provenance

Use `podway version --json` and `podway version --json --identity`; the identity
call must report Output v4, version-result v2 and contract-manifest v2. Record version, target,
build identity, available source commit and manifest digest, plus preview's
canonical Procedure digest.

Matching semver is insufficient: different source/builds may embed different
manifests. Local work reports its validation binary; distributed work separately
identifies the first immutable published release supporting all required features
and the same Output v4, version-result v2 and contract-manifest v2 schema families.
Validate with that exact released binary. Obtaining it is a prerequisite, not
authority to download or install it.
Unreleased source may establish future compatibility, not released support.
