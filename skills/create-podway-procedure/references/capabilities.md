# Procedure v2 Capability Inventory

Use this inventory after resolving the exact Podway binary that will validate the
Procedure. The canonical schema shipped with that binary remains authoritative;
this reference prevents the minimal scaffold and any one preset from being mistaken
for the complete grammar.

```podway-capability-inventory
{"item_types":["artifact","check_result","choice","confirm","integer","list","text"],"predicate_operators":["at_least","at_most","empty","equals","non_empty","not_equals"]}
```

## Choose item types deliberately

| Type | Use it for | Important boundary |
| --- | --- | --- |
| `confirm` | An attributed boolean assertion. | It is not independent verification. |
| `text` | One bounded narrative value. | Keep logs and large documents outside Podway. |
| `choice` | One value from a closed authored set. | Use stable machine values, not prose variants. |
| `integer` | A bounded count, ordinal, or status number. | Declare a meaningful minimum and maximum when the workflow has them. |
| `list` | Repeated bounded values with count, uniqueness, and total-size controls. | Enable multi-page read-back only when the selected evidence can exceed one page. |
| `artifact` | A stable reference with media-type metadata. | Podway stores neither artifact bytes nor an attestation of their content. |
| `check_result` | A result tied to an authored operation ID and digest, with closed `pass`, `fail`, and `inconclusive` outcomes. | The digest binds the abstract operation; it does not prove execution. |

Every item may use `required_when` only while declaring `required: false`. Its one
to four predicates must reference earlier, unconditionally required items in the
same action definition. Use it for structurally decidable conditional evidence,
not semantic policy.

## Use predicates only where their types fit

The six operators are `equals`, `not_equals`, `empty`, `non_empty`, `at_least`, and
`at_most`. `required_when` predicates select a same-action `item`; decision guards
select prior fresh evidence with `evidence.node` and `evidence.item`. For a
`check_result`, add `field: outcome` when comparing its outcome.

- Use `equals` and `not_equals` for boolean, choice, integer, or check-result outcome
  equality supported by the validating schema.
- Use `empty` and `non_empty` for collections whose cardinality is the intended
  mechanical condition.
- Use `at_least` and `at_most` for integer values or collection counts supported by
  the validating schema.
- Make the guarded options exhaustive over every reachable value. Every guard source
  must be required, fresh, explicitly selected by that decision's evidence, and
  mechanically compatible with the operator.

## Select evidence and read-back scope

An evidence reference names an earlier graph node and optionally selects only the
item IDs the consumer needs. An omitted selector publishes no item preview; it does
not grant arbitrary access. Required sources must dominate the consumer. Optional
branch sources remain optional when their branch did not execute.

Selected text and list evidence may require paged read-back. Size the list from the
workflow requirement first. When paging is genuinely needed, qualify the first page,
continuation token, complete-value digest and counts, plus stale-token rejection after
rework. Do not enlarge evidence merely to demonstrate paging.

## Preserve lifecycle semantics

- `skip` is an authored placement policy, not a general bypass. Do not make required
  evidence or a decision-owning phase skippable; prefer an explicit `not-applicable`
  choice when the phase must account for that outcome.
- `goal_tracking: true` opts the Procedure into goal definition and a fresh goal
  assessment near closeout. Map every terminal outcome deliberately and cite only the
  evidence needed to assess the criteria.
- Decision `effect: rework` routes replace the stale suffix from the owning target.
  `manual_rework.allowed_targets` is a separate operator allowlist. Declare only
  phases where an externally requested correction is meaningful.

## Bind validation to exact provenance

Run both `podway version --json` and `podway version --json --identity` when the
binary supports identity output. Record the version, target, build identity, source
commit when present, contract-manifest schema, and contract-manifest digest. Record
the exact canonical Procedure digest produced by preview.

Do not infer feature support from the product version alone. A source checkout,
development build, and published binary can report the same semver while embedding
different manifests. For a local Procedure, report the exact binary identity used.
For a distributed Procedure, separately identify and test the first immutable
published release whose manifest includes every required capability. An unreleased
source revision may prove future compatibility, but it is not released-support
evidence.
