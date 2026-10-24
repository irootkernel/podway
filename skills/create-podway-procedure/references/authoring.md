# Procedure v2 Authoring

## Source and features

Follow the worktree's authority for workflow meaning, naming, location and checks.
The default target is `.podway/procedures/<procedure-id>.yaml` under
configured `procedure_paths`. Preserve other canonical Procedures unless their
revision is requested. Missing CLI/configuration belongs to separately authorized
runtime setup, not authoring repair. Do not infer product decisions from a preset.

Use the exact manifest-bound schema with `podway help procedure.<operation>`,
scaffold, sources such as `podway preset show sw-dev-v2`, and the complete
[capability inventory](capabilities.md). Human views alone do not define grammar.

Select evidence that an external actor can produce:

- `check_result` binds an authored abstract operation's ID/digest and accepted
  `pass`, `fail` or `inconclusive` outcomes; it does not prove execution.
- `required_when` expresses same-node typed conditions on earlier unconditionally
  required items, for action or decision definitions. Keep the dependent item
  optional at its base declaration.
- Decision guards express mechanically decidable predicates over required,
  fresh, explicitly selected evidence. Leave semantic choices to the caller.
- Decision-option criteria explain when to choose that route; they are distinct
  from session goal criteria. Distinguish options sharing the same destination.
- Lists need meaningful count/uniqueness/value bounds. Enable multi-page evidence
  only for a real large-value requirement. Artifacts reference local metadata;
  Podway does not store or attest bytes.
- Required evidence and decision-owning work should not be skippable. Use an
  explicit not-applicable choice when an outcome must be accounted for.

## Evidence and corrections

Consumers select only needed source item IDs; decision-record references have no
item selector. Required sources must dominate consumers. Branch-specific evidence
is optional when that branch did not execute.

Guard sources must be required, fresh, selected by the decision and compatible
with the predicate type. Cover every reachable value. Split same-route options
when the predicate language cannot express the needed disjunction safely.

Route correction to its owner. Option `effect: rework` routes and
`manual_rework.allowed_targets` are separate declarations. Keep goal assessment
near terminals with the smallest fresh evidence set substantiating every criterion.

## Bounds and review

Choose text/list/artifact bounds from actual evidence shape. Counters have
nonnegative minima; positive ordinals begin at one. Keep IDs stable/unique and
increase Procedure version when canonical semantics change. The digest binds
admitted bytes; existing sessions retain their source snapshot.

Keep logs, transcripts and complete source payloads outside Podway. Record bounded
conclusions/references. For each item/option, distinguish mechanical conditions
from caller assertions; prompts must not claim Podway executed, reviewed, approved
or established semantic truth.

Review unreachable paths, phase ownership, missing failure routes, unintended
loops and terminal bypasses. Validation proves admissibility, not workflow meaning.

## Findings and handoff

Inspect canonical rendering before `podway procedure format <file> --write`; review the
result and repeat `procedure check --warnings-as-errors` and preview as described
in the [authoring workflow](../SKILL.md#author-and-verify). Fix the owning declaration for
validation/vet/lint/check findings rather than weakening bounds, guards, evidence
or warnings.

On `WORKSPACE_CONFIG_INVALID` or an excluded target, leave the candidate
undistributed and hand off the prerequisite to `$use-podway` under separate intent.
Preview's digest and exact start suggestion are the consumption handoff. Never
reconstruct them manually or execute start as part of authoring. The runtime
caller preserves this payload and adds its applicable options, key and current
start preconditions; authoring does not authorize that execution.
