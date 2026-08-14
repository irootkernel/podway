# Procedure and Item Specification

Procedure definitions are YAML or JSON documents conforming to
`podway.procedure/v2`. The normative machine contract is
[`procedure-v2.schema.json`](../../../assets/schemas/procedure-v2.schema.json).

A procedure declares `id`, `version`, `name`, `purpose`, optional description and
goal tracking, reusable `node_definitions`, and a closed `graph`. The graph names
one entry placement and between one and 64 placements. Every reference must resolve;
all reachable execution paths must terminate; advance cycles are rejected; and a
manual rework target must be explicitly declared.

Action definitions may contain `confirm`, `text`, `choice`, `integer`, `list`, and
`artifact` items. Each item has a lowercase kebab-case ID, prompt, required flag,
and type-specific bounds. Artifact values contain metadata and a worktree-relative
path; Podway verifies metadata at the configured boundary but never stores artifact
bytes. Decision definitions declare closed option IDs and reason policy. Goal
assessment definitions declare the supported assessment mode and evidence guidance.

Canonical JSON is derived from the validated semantic model and hashed with
SHA-256. YAML formatting, comments, and equivalent JSON/YAML syntax do not alter
the digest. Starts from a custom file may fence on that digest; preset starts fence
on the embedded shipped digest.

## Snapshot behavior

Each session stores one immutable admitted Procedure v2 snapshot. Later source or
preset changes do not alter an existing session.

`podway procedure validate`, `format`, `vet`, `graph`, `preview`, `lint`, `check`,
and `scaffold` operate only on Procedure v2 documents. An unsupported schema is
rejected and is never converted implicitly or explicitly.
