# ADR-0025: Model External Check Results as Structurally Bound Recorded Items

- Status: Accepted
- Date: 2026-08-20
- Extends: [ADR-0007](0007-stage-items-not-evidence-ledger.md)
- Extends: [ADR-0016](0016-recorded-item-workflow-memory.md)

## Context

Podway's existing item types can record an assertion that an external check
passed, but they cannot distinguish that assertion from a result bound to one
declared operation and one caller-supplied input basis. This makes it easy for a
Procedure author or automation client to omit the identities needed to inspect
which check result was recorded.

Podway cannot establish that an external executor ran honestly, that supplied
digests describe the claimed bytes, or that an input basis matches the worktree.
Turning recorded results into attestations, receipts, or executable checks would
contradict the same-user trust model and the product boundary.

## Decision

Procedure v2 gains one additive `check_result` item type. Its declaration binds
an operation ID, a lowercase SHA-256 operation digest, and one to three unique
accepted outcomes from `pass`, `fail`, and `inconclusive`.

The complete recorded value is a closed bounded object containing the declared
operation identity, one caller-supplied input-basis descriptor and digest,
executor name and version, outcome, summary, and output digest. Every field is
required. The complete canonical value is limited to 32 KiB, participates in the
ordinary item-set digest, and is satisfied only when operation identity and
digest match the declaration and the outcome is accepted.

The value remains an attempt-local recorded item. It uses `item.record_many`,
ordinary retry and rework invalidation, existing evidence read-back, and the
existing single-cursor lifecycle. Podway does not execute the declared operation,
fetch input or output bytes, validate an external schema, store logs, introduce
issuer authority, or create a second evidence ledger.

Observation exposes only the fixed, pattern-constrained operation ID, outcome,
operation digest, input-basis digest, and output digest. Free-text descriptor,
executor, and summary fields remain available through one complete bounded
`evidence.read` page. An unsatisfied check result may receive one copyable
`item.record_many` stdin template, but the template is data guidance and never an
execution hook.

SQLite schema v6 extends the closed item discriminator by rebuilding the item
slot table while preserving every existing `value_json` byte-for-byte. Existing
Procedure v2 documents and the six released item types retain their semantics and
canonical bytes. The Procedure schema identifier remains
`podway.procedure/v2`; exact contract-manifest mismatch remains the compatibility
gate.

## Consequences

Positive:

- Procedures can require a structurally complete external check result;
- observers can inspect operation, input, executor, and result identity without
  parsing prose;
- the feature reuses current atomic recording, freshness, paging, and lifecycle
  contracts;
- existing Procedure and preset digests remain stable.

Negative:

- every field remains caller supplied and is not factual verification;
- large nodes may require several frame-bounded `item.record_many` calls that are
  not atomic as a group;
- schema v6 requires a constrained-table rebuild and downgrade protection;
- integrations that pin the exact contract manifest must update before using the
  new item type.
