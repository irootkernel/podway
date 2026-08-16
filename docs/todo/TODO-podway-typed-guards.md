# Podway Typed Guards and Authoring Diagnostics

## Status and authority

- Document state: `Adopted`
- Dossier type: adopted design dossier
- Owning roadmap epic: `V2GRD`
- Target product release: v0.3.0
- Repository scope: Podway only
- Adoption baseline: August 17, 2026

This dossier is the decision-complete implementation plan for limited typed
conditions on Procedure v2 items and decision options and for lint behavior that
recognizes explicit phase-owner routing. The active roadmap owns task order and
status. Higher-authority implemented contracts remain authoritative until the
owning tasks update them.

## 1. Verified context and evidence

Procedure v2 currently checks item presence and type, but cannot express that an
item becomes required after a controlling choice or that a decision option is
available only when recorded typed evidence has a particular value. Procedure
authors therefore repeat conditions in prose and depend on callers to honor them.

Current lint heuristics also warn from raw option or strongly connected component
counts. Because official integrations run `procedure check
--warnings-as-errors`, those advisories become practical prohibitions even for
well-labeled finite phase-owner routing.

## 2. Goals, non-goals, and scope

The epic will:

- add one closed typed predicate vocabulary;
- support same-attempt conditional required items;
- support decision-option guards over selected fresh evidence;
- expose unmet conditions as structured guidance and fail closed at mutation;
- preserve deterministic canonicalization and static semantic validation; and
- replace count-based lint warnings with ambiguity diagnostics.

It will not add arbitrary expressions, boolean syntax, scripts, plugins,
commands, environment reads, cross-session references, dynamic routes, automatic
retry, author-configurable lint thresholds, or multiple active cursors.

## 3. Accepted design and public interfaces

### 3.1 Predicate vocabulary

Every predicate is a closed object with exactly one operator. `required_when`
and `guards` are arrays of one to four predicates combined with logical AND. A
decision has at most eight options and therefore at most 32 option-guard
predicates. OR, NOT groups, nesting, and user-defined functions do not exist.

Allowed operators are:

| Operator | Admitted item types |
|---|---|
| `equals`, `not_equals` | confirm, choice, integer, `check_result.outcome` |
| `empty`, `non_empty` | text, list |
| `at_least`, `at_most` | integer |

Literal values must match the referenced item type, or the outcome enum for a
`check_result` predicate. Choice literals reuse the existing 120-scalar value
bound. `at_least` and `at_most` are inclusive. Empty
text is evaluated after trimming Unicode whitespace; an empty list has exactly
zero entries and does not inspect entry content. A `check_result` predicate must
carry `field: outcome`, where the schema field is the closed constant
`outcome`; every other result field is rejected. No predicate performs substring,
regex, locale, floating-point, artifact-content, check-result truth, digest, or
semantic evaluation.

Predicate evaluation has three states: `met`, `unmet`, and `unevaluable`.
Structured status may echo actual boolean, integer, bounded choice, or outcome
values. Text and list predicates never echo content; they report only the
trimmed text scalar count or list entry count. Status objects contain no arbitrary
messages.

### 3.2 Conditional required items

An action or decision item may declare one `required_when` array:

```yaml
- id: commit-sha
  type: text
  prompt: Record the commit SHA.
  required: false
  required_when:
    - item: commit-state
      equals: committed
```

Every controller must be an earlier item in the same definition, be
unconditionally required, and have no `required_when` of its own. Chains,
cycles, self-reference, and references across definitions or attempts are
rejected. The condition-bearing item must declare `required: false`; otherwise
the condition is redundant and authoring fails. A missing controller makes the
condition `unevaluable` and
`required_now` false. That condition never adds a second completion blocker:
the unconditionally required missing controller already prevents completion.

Observation reports both authored `required` and derived `required_now`, plus a
structured condition result of at most four predicate statuses. Completion uses
`required_now` atomically with the same authoritative item snapshot used for
other satisfaction checks. The optional field has no materialized default.

### 3.3 Decision option guards

A decision option may declare guards over items selected by that placement's
`evidence_from` contract:

```yaml
options:
  - id: approved
    label: Review approved
    criteria: No unresolved valid finding remains.
    guards:
      - evidence:
          node: review
          item: unresolved-valid-findings
        equals: 0
```

The source must be a declared evidence reference, the item must be explicitly
selected, that reference must declare `required: true`, and ordinary dominance
and freshness rules must hold. Guards cannot query unselected values, optional
references, the current decision record, or another session. The optional
`guards` field has no materialized default.

`session.next` keeps the complete authored `options` array, adds
`allowed_option_ids` with at most eight identifiers, and treats that new array as
the authoritative `session.decide` input set. When any option declares guards,
`option_guard_statuses` covers every option and reports its three-valued state
plus up to four bounded predicate statuses; the field is absent when no option
declares guards. An option without guards reports `met` with an empty predicate
array. Each predicate reports source path, operator, expected typed
condition, and only the bounded actual value or derived count allowed by section
3.1.

An unmet or unevaluable predicate makes its option unavailable. `session.next`
remains a total query and reports `unevaluable` rather than raising. Mutation
admission validates identity and revision preconditions first, evidence freshness
second, and guards third. That order preserves `EVIDENCE_REFERENCE_STALE` for a
stale reference; any other unavailable selection fails with
`OPTION_GUARD_UNSATISFIED` and the same structured predicate details. A decision
with no allowed option remains running. Only `session.decide` is removed by the
guard set; item mutations and ordinary block, cancel, reset, retry, goal, and
declared rework actions remain available whenever their existing state rules
permit them.

### 3.4 Lint semantics

- Remove `LARGE_OPTION_SET`, `LARGE_CYCLE`,
  `LARGE_OPTION_SET_MAXIMUM = 5`, and `LARGE_CYCLE_MAXIMUM = 8`. The Procedure
  schema's separate hard maximum of eight options remains unchanged. Advance
  cycles remain invalid through graph vetting; manual rework remains
  caller-driven.
- Register `OPTION_LABELS_NOT_DISTINCT` for pairwise duplicate label
  fingerprints and `OPTION_CRITERIA_WEAK` for missing or duplicate normalized
  criteria and same-effect, same-target routes without distinct criteria.
- Fingerprints reuse the existing `normalize_text` behavior: lowercase,
  whitespace-collapsed, trimmed, with trailing `.`, `!`, and `?` removed.
  Pairwise rules retain `PAIRWISE_RULE_MAX_FINDINGS = 8`.
- Emit `REWORK_TOPOLOGY_CONFUSING` only for operationally indistinguishable
  rework targets. Distinct targets with distinct normalized labels and concrete,
  distinct criteria are valid phase-owner routing.
- Do not add `routing.style`, attempt-limit acknowledgements, or author-controlled
  warning suppression. Podway does not execute routes automatically.

### 3.5 Response budgets

One option predicate status conservatively charges at most 2,832 bytes when it
includes a 120-scalar bounded expected and actual value. Eight options with four
predicates each plus option status fields charge at most 95,888 bytes. The
`option_guard_statuses` sub-maximum is therefore 96 KiB inside V2SCL's existing
256 KiB `session.next` static allocation; guarded Procedures must fit that
allocation, while unguarded decisions omit the field and incur no charge.

A `required_when` predicate status conservatively charges about 2,384 bytes.
The condition detail therefore adds about 2.4 KiB for a one-predicate item or
9.5 KiB for a four-predicate item inside V2SCL's 128 KiB observation active-item
window. Approximately 20 single-predicate or nine four-predicate conditional
items fit before other active-item fields; the existing exact total and
truncation fields make the reduction explicit.

### 3.6 Compatibility-sensitive contract inventory

V2GRD adds its fields in place to the already reserved `next-result/v3` and
`observation-result/v2` families because V2SCL, V2AST, and V2GRD ship together in
v0.3.0. It does not introduce v4 or observation v3. The reservation task updates
`procedure-v2.schema.json`, `procedure-preview-result-v1`, the two result
families, `authoring-diagnostics.json`, `error-codes.json`, the closed
`v2-runtime-error-details-v1` branch, protocol code tables, fixtures, and every
affected contract-manifest digest.

The authoring catalog removes `LARGE_OPTION_SET` and `LARGE_CYCLE`, adds
`OPTION_LABELS_NOT_DISTINCT` and `OPTION_CRITERIA_WEAK`, and updates the matching
core code table and bounded lint tests. Catalog count alone is not acceptable
proof because the two removals and two additions cancel. Preview exposes authored
guards, while canonical document projection includes both optional condition
fields for digest computation.

`OPTION_GUARD_UNSATISFIED` is a non-retryable exit-1 error using a new closed
code-bound branch of `podway.v2-runtime-error-details/v1`. It carries the option
ID and bounded predicate statuses. Retrying without different authoritative
evidence cannot succeed.

## 4. Failure handling and compatibility

- Procedure schema remains `podway.procedure/v2`; omitted condition fields retain
  exact current behavior.
- Old stored Procedure snapshots remain valid and reconstruct without migration
  of runtime rows because conditions reside in immutable canonical snapshots.
- Exact manifest compatibility makes older peers reject the new authoring shape.
- `required_when` and `guards` are optional with no materialized defaults, so
  existing canonical Procedure bytes and the V2SCL-re-pinned preset digests do
  not rotate in V2GRD.
- Unknown operators, wrong literal types, ambiguous sources, cycles, and
  unsupported references fail authoring validation before session start.
- Guard evaluation never reads outside the already resolved current evidence set
  and never changes state.
- Item and option mutation admission re-evaluates conditions inside the same
  authoritative transaction and fails on stale preconditions normally.
- Removing `LARGE_OPTION_SET` and `LARGE_CYCLE` and adding the two replacement
  diagnostic codes is observable to integrations that pin warning codes or run
  `procedure check --warnings-as-errors`; compatibility fixtures and migration
  guidance name the replacement mapping.

## 5. Roadmap ownership and dependencies

`V2GRD` depends on completed `V2AST` and precedes `V2REF`.

- `V2GRD-001` adopts the typed-predicate ADR and normative authoring, runtime, and
  diagnostic specifications.
- `V2GRD-002` reserves condition schemas, result fields, frozen-catalog changes,
  the closed runtime error branch, protocol tables, budgets, and compatibility
  fixtures without runtime admission.
- `V2GRD-003` adds predicate domain values, `required_when`, parser,
  canonical document projection, preview projection, validation, derived
  satisfaction, and observation fields.
- `V2GRD-004` adds option guards, required selected-evidence validation, complete
  option projection, allowed-option derivation, and bounded status projection.
- `V2GRD-005` adds transactional enforcement in the specified fence order,
  registered errors, CLI rendering, and mutation guidance.
- `V2GRD-006` replaces the count-based lint codes and constants, updates focused
  phase-owner fixtures, promotes durable documentation, removes the completed
  dossier, and closes failure, compatibility, and complete development-gate
  evidence.

Before `V2GRD` is marked complete, its tasks must promote every lasting predicate,
runtime-gate, diagnostic, lint, compatibility, and authoring decision into the
affected ADRs, machine contracts, specifications, architecture, implementation
tips, and examples. The final task then replaces roadmap references to this
dossier with those durable authorities, removes its TODO index entry, and deletes
this file. This dossier is not moved to `docs/roadmap/archive/`.

## 6. Verification and acceptance

Acceptance covers every operator and item-type pairing, invalid pairings,
Unicode-empty text, boundary integers, controller ordering, redundant
`required: true` conditions, cycles, optional and
stale evidence, selected-item enforcement, all-guards-met and mixed failures,
stale mutation races, JSON/YAML equivalence, canonical digest stability, bounded
diagnostics, warnings-as-errors phase-owner examples, invalid advance cycles, and
the complete `make test` development gate. Contract tests prove the 96 KiB guard
status charge and active-item window behavior, complete options versus allowed
IDs, `check_result.outcome` guards, unevaluable and stale-source precedence,
exit-class and details-schema registration, exact catalog replacement rather than
count-only equality, and no Procedure or preset digest rotation.

## 7. References

- [TODO and Adopted Design Dossiers](README.md)
- [ADR-0016: Recorded Item Workflow Memory](../architecture-decision-records/0016-recorded-item-workflow-memory.md)
- [ADR-0017: Single-Cursor Convergence](../architecture-decision-records/0017-single-cursor-convergence.md)
- [Procedure and Item Specification](../specs/domain/procedure-and-item-specification.md)
- [State Transitions](../specs/domain/state-transitions.md)
- [Authoring Diagnostics](../specs/interfaces/errors-and-exit-codes.md)
- [`procedure-v2.schema.json`](../../assets/schemas/procedure-v2.schema.json)
