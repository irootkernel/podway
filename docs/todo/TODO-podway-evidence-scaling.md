# Podway Bounded Evidence Scale and Read-back

## Status and authority

- Document state: `Adopted`
- Dossier type: adopted design dossier
- Owning roadmap epic: `V2SCL`
- Target product release: v0.3.0
- Repository scope: Podway only
- Adoption baseline: August 17, 2026

This dossier is the decision-complete implementation plan for bounded Procedure
v2 evidence scaling. The active roadmap owns task order and status. Accepted
ADRs, canonical machine assets, and current specifications remain authoritative
for implemented behavior until the owning tasks update them.

## 1. Verified context and evidence

The current implementation and architecture guide disagree about the supported
scale envelope. The implementation admits at most 64 items per definition,
16,384 Unicode scalars per text value, 200 list entries, and 1,000 Unicode
scalars per list entry. `docs/architecture/system.md` instead describes 128
items, an 8 KiB default and 64 KiB hard maximum stated in bytes, and 1,000 list
entries.

Selected evidence read-back is statically rejected when its conservative
worst-case projection exceeds 512 KiB. The IPC frame remains bounded at 1 MiB.
Consequently a value that is valid under the current item schema can be
unavailable as selected evidence even though its complete value is stored. The
failure is a contract mismatch, not a reason to remove the frame bound or return
unbounded values from `session.next`.

## 2. Goals, non-goals, and scope

The epic will:

- provide deterministic pageable reads for declared current evidence;
- make every documented item limit match authoring and runtime enforcement;
- bound large lists by entry count, per-entry length, and total content length;
- keep every IPC response below the existing 1 MiB frame limit;
- report exact field paths, actual values, maxima, and units for limit failures;
- diagnose the largest read-back contributors and useful item selectors; and
- preserve one session, one graph cursor, and one active attempt.

The epic will not add streaming IPC, compression, remote evidence queries,
cross-session history, artifact bytes, arbitrary range queries, or an evidence
archive. It will not increase the 1 MiB frame limit.

## 3. Accepted design and public interfaces

### 3.1 Scale envelope

The following hard limits are authoritative for the implementation work:

| Dimension | Hard maximum | Default |
|---|---:|---:|
| Text value and `max_length` | 65,536 Unicode scalars | 4,000 |
| Text `min_length` | 65,536 Unicode scalars | 0 |
| List entries and `max_items` | 1,000 | 50 |
| List `min_items` | 1,000 | 0 |
| List entry and `max_item_length` | 8,192 Unicode scalars | 500 |
| List total content and `max_total_length` | 1,000,000 Unicode scalars | 1,000,000 |
| Recorded text and list content per attempt | 16,777,216 Unicode scalars | n/a |
| Items per node definition and attempt | 128 | n/a |

List items gain `max_total_length`. A recorded list must satisfy its entry-count,
per-entry, uniqueness, and total-content constraints simultaneously. Omission of
`max_total_length` uses the literal default above. The effective content ceiling
is the minimum of `max_items * max_item_length` and `max_total_length`; a loose
total ceiling is valid. The retained `min_items <= max_items` rule and the new
`min_items <= max_total_length` rule keep declarations satisfiable because list
entries are non-empty. Existing authored lists remain valid.

All table lengths count Unicode scalar values, not bytes. This deliberately
supersedes the byte wording in `docs/architecture/system.md` while retaining the
documented numeric scale. The 8,192-scalar entry maximum is also the paging
safety bound: its 49,152-byte conservative JSON charge fits within one page.

The per-attempt aggregate counts only recorded text values and list-entry
content. It excludes reasons, blockers, goal criteria, and artifact metadata.
Authoring diagnostics warn when the sum of declared worst cases exceeds the
aggregate and name the largest contributors, but do not reject a Procedure.
Core transitions enforce the aggregate at runtime, and persisted-state
reconstruction verifies it fail-closed with
`ATTEMPT_CONTENT_LIMIT_EXCEEDED` and the structured details in section 3.5. The
16,777,216-scalar ceiling remains above the old theoretical maximum of
12,800,000 scalars, so it does not invalidate a previously admissible attempt.

`podway-core` owns reusable public limit constants. Configuration, runtime
records, persistence reconstruction, protocol request slices, and budget
calculation consume those constants. Canonical schemas retain literal bounds and
executable contract tests prove exact equality with the domain constants.

Wire value bounds are never narrower than the corresponding domain hard maximum.
The item text slice remains 65,536 scalars; item add/remove list-entry slices rise
to 8,192; and `item.record_many` rises to 128 operations, 1,000 list entries, and
8,192 scalars per entry. The 1 MiB request frame remains an independent transport
bound. Consequently every valid list is reachable through bounded `item.add`
mutations, but a maximal list cannot be replaced atomically through one
`item.record_many` frame.

### 3.2 Pageable evidence reads

Add the query command `evidence.read` and CLI route:

```text
podway evidence read --source <graph-node-id> --item <item-id> \
  [--page-token <token>]
```

The command may read only an item selected by a currently resolved, declared
evidence reference of the current consumer attempt. It cannot browse arbitrary
attempts or stale history. The request is fenced by workspace UUID, session ID,
and consumer attempt ID.

The public result is `podway.evidence-read-result/v1`. It contains:

- consumer, source node, source attempt, and item identities;
- item type, item revision, complete-value digest, and total logical size;
- a typed page whose encoded JSON data is at most 256 KiB;
- `truncated` and nullable `next_page_token`;
- the page-token version and logical page offset.

Text pages end only at Unicode scalar boundaries. List pages contain complete
entries and never split an entry. Scalar item types return one terminal page.
Text sizes and offsets use Unicode scalars, list sizes and offsets use entries,
and scalar items use the single-page unit. The complete encoded
`evidence.read` response, including one token and its envelope, is at most
320 KiB.

The opaque page token is at most 256 base64url characters. Its canonical payload
is fixed-order binary containing a one-byte version, 16-byte session UUID,
16-byte consumer attempt UUID, 16-byte source attempt UUID, length-prefixed item
ID, 32-byte SHA-256 value digest, and unsigned 64-bit logical offset, followed by
unpadded base64url encoding. It is not an authentication token. The daemon
validates every field against current authoritative state. A token produces the
same page while its snapshot remains current; a changed consumer, source value,
reference validity, or session identity fails with
`EVIDENCE_PAGE_TOKEN_STALE` rather than silently returning another snapshot.

Malformed, oversized, cross-session, and wrong-binding tokens fail as
`REQUEST_INVALID` before state comparison. A no-token read of an unresolved or
skipped reference fails non-retryably with `EVIDENCE_NOT_AVAILABLE`; a genuinely
stale reference retains `EVIDENCE_REFERENCE_STALE`. A previously valid token
whose bound snapshot changed fails retryably with
`EVIDENCE_PAGE_TOKEN_STALE`. An offset at or past the logical end fails
non-retryably with `EVIDENCE_PAGE_TOKEN_EXHAUSTED`; the daemon never issues such
a token.

### 3.3 Observation and budget behavior

`session.next` changes to `podway.next-result/v3`, and `session.observe` changes
to `podway.observation-result/v2`. Their evidence projections carry source and
item identity, full digest, total size, bounded preview state, and the first
continuation page token when a preview is present. An omitted evidence item
selector means metadata-only. Explicit selectors receive previews in declaration
order for at most 32 items; later admitted items remain metadata-only. At most
128 item metadata records may appear across all references in one response.
Vetting rejects a larger declared selection and recommends the smallest useful
selectors. Metadata remains complete for every item in an admitted
`session.next` projection.

The conservative `session.next` response equation is closed against the
1,048,576-byte frame as follows:

| Allocation | Maximum |
|---|---:|
| Procedure and runtime static content | 256 KiB |
| Complete decision records | 272 KiB |
| Reference and item metadata, at most 128 items | 208 KiB |
| Preview data | 176 KiB |
| Up to 32 page tokens | 64 KiB |
| Serialization and envelope safety | 48 KiB |
| **Total** | **1 MiB** |

The decision-record allocation retains complete records and admits one maximal
262,186-byte goal-assessment record while continuing to reject two, matching the
existing boundary. The metadata allocation covers both the `references` and
`readback` surfaces at maximum fan-out. The preview allocation yields at least
938 worst-case Unicode scalars per preview when all 32 token slots are used.

`session.observe` has its own closed composition rather than embedding an
unbounded duplicate of `session.next`:

| Observation component | Maximum |
|---|---:|
| Compact status | 128 KiB |
| Bounded `next-result/v3` guidance | 640 KiB |
| Active item window | 128 KiB |
| Mutation template window | 64 KiB |
| Serialization and outer-envelope safety | 64 KiB |
| **Total** | **1 MiB** |

The guidance bound is derived rather than separately vetted: the existing
256 KiB static and 272 KiB complete-decision-record caps total 528 KiB, leaving
at least 112 KiB for truncatable detail arrays in every admitted Procedure.
Observation guidance emits evidence metadata only, with no previews or page
tokens. Missing-item details, suggestions, and evidence item metadata fill the
remaining budget in their existing priority and declaration order and always
carry exact totals and truncation flags. At the simultaneous static and decision
record maxima, approximately 60 evidence item metadata records fit; ordinary
placements with small static content retain substantially more headroom.

Compact-status items, active items, and mutation templates are independent
byte-budgeted declaration-order windows with exact total and truncation fields.
Active-item choice constraints expose at most eight choices plus exact choice
count and truncation, while the complete declaration remains available from the
Procedure inspection routes. A node with 128 maximal items is therefore safe but
is not promised complete active-item detail in one observation. Template argv
elements are limited to 4,096 scalars.

Authoring vetting budgets metadata and one preview page, not the full worst-case
recorded value. Diagnostics identify the largest contributing item paths and
suggest the smallest useful selectors. Paging never weakens dominance,
freshness, or stale evidence rules.

Raising items per node does not make result detail arrays unbounded.
`missing_required_item_count` remains exact while
`missing_required_items` returns at most 64 entries plus a truncation flag.
Suggestions remain capped at 128, expose exact total and truncation fields, and
order currently actionable session-level suggestions before item suggestions in
item declaration order. Required item work remains recoverable from the missing
item projection even when decision options consume the suggestion window.

### 3.4 Compatibility-sensitive contract inventory

The work updates `procedure-v2.schema.json`, `v2-result-components-v1`,
`item-record-many-input-v1`, `next-result-v3`, `observation-result-v2`, and the
closed `output-v3` selection. It also registers `evidence.read` in
`contracts/command-routes.json`, updates the contract manifest digests, and
updates the public error catalog and protocol tables. Existing shared v1 schema
families whose structure is unchanged receive bounded in-place constraint
updates under manifest fail-closed compatibility; materially changed next and
observation payloads receive the versions above.

`observation-result/v2` intentionally changes its `status` member from standard
`status-result/v2` to value-free `compact-status-result/v2`, because observation
is the current-state guidance surface rather than a history surface. History
consumers use `session.status --verbose`. Its `guidance` remains
`next-result/v3`; evidence metadata total and truncation fields are always
present, are invariantly non-truncated for `session.next`, and may be truncated
only in the observation-specific projection. Compact status, active items, and
mutation templates gain the window totals and truncation fields described above.
The automation observation contract and migration guidance record this semantic
reduction explicitly.

All derived 64-item caps are audited, including status item projections,
read-back items, record-many inputs/results, missing-item details, and
observation active items. Detail windows may retain smaller explicit caps only
when they also expose exact totals and truncation. Silent truncation is forbidden.

### 3.5 Error contract

Bound failures use structured details containing `field`, `actual`, `maximum`,
and `unit`. Authoring diagnostics use canonical YAML/JSON field paths. Runtime
item errors use item-relative public paths. Generic messages such as `invalid
constraints` are not sufficient when the exact exceeded limit is known.
Entry count, per-entry length, total list content, and per-attempt aggregate
failures report the constraint actually violated. `REQUEST_TOO_LARGE` is the
intentional exception: frame decoding happens before JSON parsing, so no field
path exists.

`EVIDENCE_NOT_AVAILABLE` and `ATTEMPT_CONTENT_LIMIT_EXCEEDED` are non-retryable
exit-1 constraint failures. `EVIDENCE_PAGE_TOKEN_EXHAUSTED` is a non-retryable
exit-2 usage failure. `EVIDENCE_PAGE_TOKEN_STALE` is a retryable exit-4 conflict
and uses `podway.recoverable-v2-runtime-error-details/v1` with the state-refresh
recovery recipe, matching `EVIDENCE_REFERENCE_STALE`. The remaining new codes
use `podway.v2-runtime-error-details/v1`.

## 4. Failure handling and compatibility

- Existing Procedure v2 files and stored sessions remain readable.
- The Procedure schema identifier remains `podway.procedure/v2`; exact manifest
  compatibility continues to make older peers fail closed on the new contract.
- Adding the literal `max_total_length` default intentionally rotates every
  newly canonicalized Procedure v2 digest once. The three preset digests are
  re-pinned in the same task and remain stable for the rest of the epic. Stored
  sessions continue to use their stored canonical snapshots and digests.
  Digest-pinned automation receives `DIGEST_CONFIRMATION_REQUIRED`; migration
  and release guidance make the operator-visible rotation explicit.
- Malformed, oversized, cross-session, wrong-item, exhausted, and stale page
  tokens fail without changing state.
- Page reads are queries and never create durable jobs or revisions.
- Storage reads may decode a bounded complete value internally, but no public
  response may exceed the page or frame budget.
- Observation v2 callers that previously consumed standard status fields move
  history reads to `session.status --verbose`; current progression guidance and
  active-item mutation information remain self-contained in observation.
- Large lists are constructed non-atomically through `item.add`. Each mutation
  remains atomic, but a crash between additions can leave a valid partial list;
  `item.record_many` remains atomic only for values that fit one request frame.

## 5. Roadmap ownership and dependencies

`V2SCL` depends on completed `V2REC` and precedes `V2AST`.

- `V2SCL-001` adopts the ADR and normative scale/read-back specifications.
- `V2SCL-002` registers the query route, versioned results, schemas, errors, and
  compatibility fixtures as reserved contracts.
- `V2SCL-003` aligns domain, configuration, schema, protocol slices,
  persistence, preset identity, and diagnostic limits.
- `V2SCL-004` implements snapshot-bound paging through protocol, store, daemon,
  CLI, and observation composition.
- `V2SCL-005` closes maximum-size, stale-token, restart, projection, and
  observation-composition conformance, proves the five observation component
  windows and compact-status semantic migration, promotes durable documentation,
  and closes the complete development gate.

Before `V2SCL` is marked complete, its tasks must promote every lasting scale,
paging, diagnostic, compatibility, and operational decision into the affected
ADRs, machine contracts, specifications, architecture, implementation tips, and
examples. The final task then replaces roadmap references to this dossier with
those durable authorities, removes its TODO index entry, and deletes this file.
This dossier is not moved to `docs/roadmap/archive/`.

## 6. Verification and acceptance

Acceptance requires focused domain, parser, protocol, store, daemon, and CLI
tests plus the complete `make test` development gate. Coverage includes every
limit at and one above the boundary, worst-case Unicode escaping, a 1,000-entry
list built incrementally, a maximum-length add/remove entry, framed
`item.record_many` acceptance and rejection, total-list and attempt-aggregate
overflow, reconstruction, 128 item definitions, multi-page text and list reads,
stable page-token replay, malformed and maximum-size token boundaries, stale
invalidation, restart, selector diagnostics, and proof of every response
allocation above. Contract checks cover the closed `output-v3` selection,
command routes, manifest digests, exact public constants, preset digest re-pins,
derived detail caps and truncation flags, and the complete 1 MiB IPC bound.
Observation checks prove all five component windows and their composed frame,
the compact-status semantic migration, deterministic detail truncation, and the
invariant that `session.next` never marks its admitted evidence metadata window
as truncated.

## 7. References

- [TODO and Adopted Design Dossiers](README.md)
- [System Architecture](../architecture/system.md)
- [Procedure and Item Specification](../specs/domain/procedure-and-item-specification.md)
- [IPC Protocol](../specs/interfaces/ipc-protocol.md)
- [SQLite Model](../specs/storage/sqlite-model.md)
- [`procedure-v2.schema.json`](../../assets/schemas/procedure-v2.schema.json)
