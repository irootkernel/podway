# Procedure and Item Specification

Procedure definitions are YAML or JSON documents conforming to
`podway.procedure/v2`. The normative machine contract is
[`procedure-v2.schema.json`](../../../assets/schemas/procedure-v2.schema.json).

A procedure declares `id`, `version`, `name`, `purpose`, optional description and goal tracking, reusable `node_definitions`, and a closed `graph`. The graph names one entry placement and between one and 64 placements. Every reference must resolve; all reachable execution paths must terminate; advance cycles are rejected; and a manual rework target must be explicitly declared.

Action definitions may contain `confirm`, `text`, `choice`, `integer`, `list`,
`artifact`, and `check_result` items, at most 128 per definition. Each item has a
lowercase kebab-case ID, prompt, required flag, and type-specific bounds. Artifact
values contain metadata and a worktree-relative path; Podway verifies metadata at
the configured boundary but never stores artifact bytes. A check-result
declaration additionally requires an operation ID, operation digest, and accepted
outcomes. Decision definitions declare closed option IDs and reason policy. Goal
assessment definitions declare the supported assessment mode and evidence
guidance.

## External check results

[ADR-0025](../../architecture-decision-records/0025-structurally-bound-external-check-results.md)
owns the `check_result` assurance boundary. Its declaration is closed and
requires `operation_id`, `operation_digest`, and `accepted_outcomes`. Operation
IDs reuse the 64-character identifier component. Every digest uses
`sha256:<64-lowercase-hex>`. Accepted outcomes contain one to three unique values
from `pass`, `fail`, and `inconclusive`, with no default.

The complete recorded value is a closed object with required `operation_id`,
`operation_digest`, `input_basis`, `executor`, `outcome`, `summary`, and
`output_digest` fields. `input_basis` contains a non-empty descriptor of at most
512 Unicode scalars and a digest. `executor` contains a non-empty name of at most
128 scalars and a non-empty version of at most 64 scalars. `summary` is non-empty
and at most 2,000 scalars. The complete canonical encoded JSON value is at most
32 KiB. An executor without a native version records an honest stable placeholder
such as `unknown`; fields are never omitted and no arbitrary metadata map or
external timestamps are admitted.

A recorded check result satisfies its item only when its operation ID and digest
equal the declaration and its outcome is accepted. A structurally valid mismatch
may be recorded but remains unsatisfied. The complete canonical value contributes
to the existing immutable value and item-set digests. At most 128 values times
32 KiB yields a derived 4 MiB per-attempt check-result product; no second aggregate
invariant or error code is introduced.

Check results use ordinary retry, rework, goal-revision, and evidence-reference
freshness. A retry creates an empty slot, while suffix invalidation may leave an
older value inspectable through history without allowing it to satisfy current
progression. Podway records these caller-supplied fields but does not execute or
verify their external meaning.

## Item bounds

[ADR-0024](../../architecture-decision-records/0024-bounded-evidence-scale-and-paged-read-back.md) owns the scale envelope. Every length counts Unicode scalar values, not bytes. `podway-core` holds these numbers as public constants and configuration, runtime records, persistence reconstruction, protocol request slices, and budget calculation consume them, so one envelope binds every layer.

| Dimension | Hard maximum | Default |
|---|---:|---:|
| Text value and `max_length` | 65,536 | 4,000 |
| Text `min_length` | 65,536 | 0 |
| List entries and `max_items` | 1,000 | 50 |
| List `min_items` | 1,000 | 0 |
| List entry and `max_item_length` | 8,192 | 500 |
| List total content and `max_total_length` | 1,000,000 | 1,000,000 |
| Recorded text and list content per attempt | 16,777,216 | n/a |
| Items per node definition and attempt | 128 | n/a |

A recorded list satisfies its entry-count, per-entry-length, uniqueness, and total-content constraints simultaneously. The effective content ceiling is the minimum of `max_items * max_item_length` and `max_total_length`, so a loose total ceiling is valid. `min_items <= max_items` and `min_items <= max_total_length` keep a declaration satisfiable because list entries are non-empty. Omitting `max_total_length` materializes the literal default, which participates in canonicalization and therefore in the digest.

The per-attempt aggregate counts only recorded text values and list-entry content.
It excludes reasons, blockers, goal criteria, choice values, integers, artifact
metadata, and check-result values. Check results have their independent 32 KiB
per-value bound and derived 4 MiB product. Lint emits the advisory
`ATTEMPT_CONTENT_BUDGET_EXCEEDED` when an action definition's declared text and
list worst case exceeds the aggregate, naming the largest contributing items; it
is a warning rather than a vet finding precisely because a vet finding would make
`session.start` reject the Procedure. The item mutation path enforces the
aggregate fail-closed with `ATTEMPT_CONTENT_LIMIT_EXCEEDED` on the mutation that
would cross it, and every construction of a complete recorded item set rejects an
over-aggregate attempt. Reconstructing raw persisted slots does not itself
re-check the aggregate, because a stored attempt was already admitted under it.

A bound failure names the constraint actually violated and reports `field`, `actual`, `maximum`, and `unit`. Authoring diagnostics report the exact field, maximum, and observed value; runtime failures carry all four in closed error details. The scale-envelope constraints are distinguished from one another, so an over-long entry, an over-full list, an over-large total, and an over-full attempt never collapse into one message.

Canonical JSON is derived from the validated semantic model and hashed with
SHA-256. YAML formatting, comments, and equivalent JSON/YAML syntax do not alter
the digest. Starts from a custom file may fence on that digest; preset starts fence
on the embedded shipped digest.

## Snapshot behavior

Each session stores one immutable admitted Procedure v2 snapshot. Later source or
preset changes do not alter an existing session.

Adding the literal `max_total_length` default in `V2SCL-003` rotates the canonical digest of every Procedure that declares a list item, exactly once; a Procedure with no list item keeps its digest. The Procedure schema identifier remains `podway.procedure/v2`, and stored sessions continue to use their stored canonical snapshots and digests. Automation that pins a digest receives `DIGEST_CONFIRMATION_REQUIRED` until it re-pins.

`podway procedure validate`, `format`, `vet`, `graph`, `preview`, `lint`, `check`,
and `scaffold` operate only on Procedure v2 documents. An unsupported schema is
rejected and is never converted implicitly or explicitly.
