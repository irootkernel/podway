# ADR-0033: Name Unsatisfied Check Result Reasons

- Status: Accepted
- Date: 2026-09-11
- Extends: [ADR-0025](0025-structurally-bound-external-check-results.md)

## Context

ADR-0025 satisfies a `check_result` item only when the recorded operation ID and digest equal the declaration and the outcome is accepted, and it deliberately stores a structurally valid mismatch as an unsatisfied value. Observation reported that state as a bare `satisfied: false` beside the declared constraints and the recorded projection, so a caller had to compare two 64-character digests by eye to learn why progression stayed blocked.

A 2026-09-11 incident showed the cost. An automation client recorded an operation digest that differed from the declaration by one hexadecimal character, read the silent `satisfied: false` as a daemon defect, and reset its workspace before the mismatch was found in the stored value. The same review found that `item-record-many-result/v1` still published a disjoint check-result entry carrying `type` and `value_digest` that V2AST-002 reserved and no producer ever emitted.

## Decision

Observation names the first failed declaration comparison of a stored, unsatisfied check result as one closed `unsatisfied_reason`: `operation_id_mismatch`, `operation_digest_mismatch`, or `outcome_not_accepted`, evaluated in that fixed order. The reason is derived only from the immutable declaration and the caller-supplied value. It is absent for empty slots, satisfied results, and every other item type, and it never appears in compact status projections. The domain predicate that decides satisfaction and the reason share one implementation so they cannot diverge. The CLI prints the reason together with the declared and recorded identity, while the machine field remains the only automation contract.

Recording is unchanged: a valid mismatch is still admitted and stored, no warning is emitted on the mutation response, and no doctor check evaluates items. The `item-record-many-result/v1` family stays record-kind agnostic; the reserved disjoint check-result entry is withdrawn so the published schema matches the daemon.

## Consequences

Positive:

- a caller can distinguish a wrong identity, a wrong digest, and a rejected outcome from one observation without diffing digests;
- the reason is reproducible from disk, survives restarts, and adds no persistence or codec change;
- the atomic recording result contract no longer advertises a shape that cannot occur.

Negative:

- the reason still says nothing about whether the external operation ran or which side is right;
- clients that pin the exact contract manifest must update before reading the new field;
- a mismatch is still discovered only after the record commits, so a client must observe before it advances.
