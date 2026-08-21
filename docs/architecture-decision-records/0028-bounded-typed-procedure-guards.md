# ADR-0028: Add Bounded Typed Procedure Guards

- Status: Accepted
- Date: 2026-08-22
- Extends: [ADR-0016](0016-recorded-item-workflow-memory.md)
- Preserves: [ADR-0017](0017-single-cursor-convergence.md)

## Context

Procedure v2 records bounded typed items and can require an item unconditionally,
but it cannot express that an item becomes required after a controlling choice or
that a decision option is available only when selected fresh evidence has a
particular typed value. Authors repeat those conditions in prose, which Podway
cannot enforce. Existing lint rules also warn from raw option and cycle counts,
making finite, well-labeled phase-owner routing difficult to use with
`--warnings-as-errors`.

A general expression language would add parsing, evaluation, nesting, execution,
and compatibility risks that conflict with Podway's declarative single-cursor
scope. Conditions must remain statically bounded, deterministic, inspectable, and
derived only from already recorded Procedure state.

## Decision

Procedure v2 adds one closed typed predicate vocabulary:

- `equals` and `not_equals` apply to confirm, choice, integer, and
  `check_result.outcome` values;
- `empty` and `non_empty` apply to text and list values;
- `at_least` and `at_most` apply to integer values;
- a predicate result is `met`, `unmet`, or `unevaluable`;
- each `required_when` or `guards` array contains one to four predicates combined
  by logical AND, with no OR, NOT group, nesting, expression, or extension hook.

An item may declare `required_when` only when it is otherwise optional. Every
controller is an earlier, unconditionally required item in the same node
definition and cannot itself be conditional. Missing controllers are
`unevaluable`; they do not make the dependent item required because the missing
unconditional controller already blocks progression. Completion derives
`required_now` and item satisfaction atomically from one authoritative active
attempt snapshot.

A decision option may declare guards only over explicitly selected items from a
required `evidence_from` reference. Existing dominance and freshness rules remain
mandatory. Reads report all authored options, one authoritative bounded
`allowed_option_ids` set, and complete bounded guard statuses when any option is
guarded. Unmet or unevaluable guards remove only `session.decide` for that option;
they do not disable unrelated item, blocker, retry, rework, goal, cancellation, or
lifecycle operations.

Decision admission validates identity and revision fences first, evidence
freshness second, and option guards third. A stale source retains
`EVIDENCE_REFERENCE_STALE`; any other unavailable selection returns the new
non-retryable exit-1 `OPTION_GUARD_UNSATISFIED` error with bounded structured
predicate details. Evaluation reads no state outside the current immutable
Procedure snapshot, active attempt, and already resolved selected evidence.

The authoring catalog replaces `LARGE_OPTION_SET` and `LARGE_CYCLE` with
`OPTION_LABELS_NOT_DISTINCT` and `OPTION_CRITERIA_WEAK`. Pairwise label and
criteria fingerprints reuse the existing normalized-text behavior and retain the
eight-finding cap. `REWORK_TOPOLOGY_CONFUSING` applies only to operationally
indistinguishable targets; distinct labels, targets, and concrete criteria are
valid phase-owner routing.

The optional fields have no materialized defaults. Existing Procedure bytes and
preset digests do not rotate. Conditions live in immutable canonical Procedure
snapshots, so no SQLite migration is required. The unreleased v0.3.0 contract adds
the bounded fields in place to the already reserved `next-result/v3` and
`observation-result/v3` families under exact manifest compatibility; it does not
create v4 result families.

## Consequences

Positive:

- authors can express common progression conditions without prose-only policy;
- callers receive deterministic, bounded explanations and authoritative option
  availability;
- stale evidence keeps its existing conflict semantics;
- the single cursor, one active attempt, data-only Procedure, and same-user trust
  boundaries remain unchanged.

Negative:

- authors cannot express disjunction, nested logic, cross-session conditions, or
  predicates over arbitrary text content;
- guarded responses consume fixed portions of the existing response budgets;
- exact manifest compatibility requires coordinated schema, catalog, fixture, and
  peer updates before runtime admission.
