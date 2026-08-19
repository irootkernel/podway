# Procedure and Item Specification

Procedure definitions are YAML or JSON documents conforming to
`podway.procedure/v2`. The normative machine contract is
[`procedure-v2.schema.json`](../../../assets/schemas/procedure-v2.schema.json).

A procedure declares `id`, `version`, `name`, `purpose`, optional description and goal tracking, reusable `node_definitions`, and a closed `graph`. The graph names one entry placement and between one and 64 placements. Every reference must resolve; all reachable execution paths must terminate; advance cycles are rejected; and a manual rework target must be explicitly declared.

Action definitions may contain `confirm`, `text`, `choice`, `integer`, `list`, and `artifact` items, at most 128 per definition once `V2SCL-003` raises the shipped cap of 64. Each item has a lowercase kebab-case ID, prompt, required flag, and type-specific bounds. Artifact values contain metadata and a worktree-relative path; Podway verifies metadata at the configured boundary but never stores artifact bytes. Decision definitions declare closed option IDs and reason policy. Goal assessment definitions declare the supported assessment mode and evidence guidance.

## Item bounds

[ADR-0024](../../architecture-decision-records/0024-bounded-evidence-scale-and-paged-read-back.md) owns the scale envelope. Every length counts Unicode scalar values, not bytes, and one set of numbers binds authoring validation, runtime recording, persistence reconstruction, and protocol request slices. `V2SCL-003` raises every layer to the values below and adds `max_total_length`; until it lands, `procedure-v2.schema.json` and the domain constants still enforce the superseded envelope of 64 items, 16,384 text scalars, 200 list entries, and 1,000 scalars per entry.

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

The per-attempt aggregate counts only recorded text values and list-entry content. It excludes reasons, blockers, goal criteria, and artifact metadata. Authoring diagnostics warn when the sum of declared worst cases exceeds the aggregate and name the largest contributing item paths without rejecting the Procedure. Runtime transitions and persisted-state reconstruction enforce it fail-closed with `ATTEMPT_CONTENT_LIMIT_EXCEEDED`.

A bound failure names the constraint actually violated and reports `field`, `actual`, `maximum`, and `unit`. Authoring diagnostics use canonical YAML or JSON field paths; runtime item errors use item-relative public paths.

Canonical JSON is derived from the validated semantic model and hashed with
SHA-256. YAML formatting, comments, and equivalent JSON/YAML syntax do not alter
the digest. Starts from a custom file may fence on that digest; preset starts fence
on the embedded shipped digest.

## Snapshot behavior

Each session stores one immutable admitted Procedure v2 snapshot. Later source or
preset changes do not alter an existing session.

Adding the literal `max_total_length` default in `V2SCL-003` rotates every newly canonicalized Procedure v2 digest exactly once. The Procedure schema identifier remains `podway.procedure/v2`, shipped preset digests are re-pinned in the owning task, and stored sessions continue to use their stored canonical snapshots and digests. Automation that pins a digest receives `DIGEST_CONFIRMATION_REQUIRED` until it re-pins.

`podway procedure validate`, `format`, `vet`, `graph`, `preview`, `lint`, `check`,
and `scaffold` operate only on Procedure v2 documents. An unsupported schema is
rejected and is never converted implicitly or explicitly.
