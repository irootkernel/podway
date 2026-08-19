# ADR-0024: Bounded Evidence Scale and Paged Read-back

- Status: Accepted
- Date: 2026-08-19
- Extends: [ADR-0007](0007-stage-items-not-evidence-ledger.md)
- Extends: [ADR-0016](0016-recorded-item-workflow-memory.md)
- Extends: [ADR-0019](0019-procedure-v2-only-product.md)

## Context

Podway declared one item scale envelope and enforced another. The implementation admitted 64 items per node definition, 16,384 Unicode scalars per text value, 200 list entries, and 1,000 scalars per list entry, while the architecture guide described 128 items, an 8 KiB default with a 64 KiB hard maximum stated in bytes, and 1,000 list entries. The list numbers were not merely inconsistent: the guide named an entry count that the implementation applied to entry length.

The read-back contract had a worse failure. Selected evidence was admitted only when a Procedure's conservative worst-case projection stayed under 512 KiB, and that projection was charged once at authoring time. A value that the item schema accepted was therefore storable but unreadable as selected evidence, because the only way to read it was to embed the whole value in a `session.next` response bounded by the 1 MiB IPC frame. Nothing bounded the runtime read-back projection itself, so the static admission proof was the only thing standing between a large recorded value and an oversized response.

Raising the frame limit would trade a contract mismatch for an unbounded response. Removing the static budget would move the failure from authoring time, where it names a Procedure the author can fix, to runtime, where it strands a session mid-attempt.

## Decision

One scale envelope is authoritative for authoring, runtime, persistence, protocol, and documentation. Every length in it counts Unicode scalar values, never bytes.

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

`podway-core` owns these as public constants. Configuration, runtime records, persistence reconstruction, protocol request slices, and budget calculation consume them instead of re-declaring literals. Canonical schemas keep literal bounds because a JSON Schema cannot import a Rust constant, so executable contract tests must prove exact equality between the two.

List items gain `max_total_length`, and a recorded list must satisfy its entry-count, per-entry, uniqueness, and total-content constraints simultaneously. The effective content ceiling is the minimum of `max_items * max_item_length` and `max_total_length`; a loose total ceiling is valid and is the default. The per-attempt aggregate counts only recorded text values and list-entry content, excluding reasons, blockers, goal criteria, and artifact metadata. Authoring diagnostics warn when declared worst cases exceed the aggregate and name the largest contributors without rejecting the Procedure; the item mutation path and every construction of a complete recorded item set enforce it fail-closed with `ATTEMPT_CONTENT_LIMIT_EXCEEDED`.

Declared current evidence becomes readable through a new `evidence.read` query rather than through a larger response. The command reads one item selected by a currently resolved, declared evidence reference of the current consumer attempt, fenced by workspace UUID, session ID, and consumer attempt ID. It cannot browse arbitrary attempts or stale history, it creates no durable job or revision, and it returns a typed page whose encoded data is at most 256 KiB inside a response of at most 320 KiB. Text pages end only at Unicode scalar boundaries, list pages contain whole entries, and scalar item types return one terminal page.

Continuation uses an opaque page token of at most 256 base64url characters whose canonical payload binds a version byte, the session UUID, the consumer attempt UUID, the source attempt UUID, the item ID, the SHA-256 digest of the complete value, and an unsigned 64-bit logical offset. The token is not an authentication or authorization credential; the daemon validates every field against current authoritative state on each read. A token yields the same page while its bound snapshot remains current, and a changed consumer, source value, reference validity, or session identity fails explicitly instead of silently returning a different snapshot.

Observation and progression guidance carry evidence identity and size rather than evidence content. `session.next` and `session.observe` move to `next-result/v3` and `observation-result/v3`. A `session.next` evidence projection carries source and item identity, the complete-value digest, the total logical size, bounded preview state, and the first continuation token when a preview is present. Observation guidance carries the same identity, digest, and size metadata but emits no preview data and no page token, because a caller that wants content pages it explicitly through `evidence.read`.

Each response has a closed byte-budget composition whose components expose exact totals and truncation flags; silent truncation is forbidden anywhere a collection is windowed. `observation-result/v3` also reduces its status member from the standard tier to a value-free compact projection, dropping `purpose`, `item_values`, and the other standard-only fields, because observation is the current-state guidance surface and its own guidance and active-item windows already carry what a mutation needs. A caller that wants the standard or verbose status projection, including trace history, calls `session.status`.

Bound failures report the constraint actually violated with `field`, `actual`, `maximum`, and `unit`. `REQUEST_TOO_LARGE` remains the single exception, because frame decoding precedes JSON parsing and no field path exists at that point.

The contract adds four public error codes:

| Code | Exit | Retryable | Details schema |
|---|---:|---|---|
| `EVIDENCE_NOT_AVAILABLE` | 1 | no | `podway.v2-runtime-error-details/v1` |
| `ATTEMPT_CONTENT_LIMIT_EXCEEDED` | 1 | no | `podway.v2-runtime-error-details/v1` |
| `EVIDENCE_PAGE_TOKEN_EXHAUSTED` | 2 | no | `podway.v2-runtime-error-details/v1` |
| `EVIDENCE_PAGE_TOKEN_STALE` | 4 | yes | `podway.recoverable-v2-runtime-error-details/v1` |

Malformed, oversized, cross-session, and wrong-binding tokens are decoding failures and return `REQUEST_INVALID` before any state comparison. A genuinely stale evidence reference keeps `EVIDENCE_REFERENCE_STALE`.

The 1 MiB IPC frame limit does not change. Streaming IPC, compression, remote evidence queries, cross-session history, artifact bytes, arbitrary range queries, and an evidence archive remain out of scope.

## Adoption sequence

This ADR is adopted authority, not a description of shipped behavior. At acceptance the canonical schemas, machine error catalog, command catalog, command routes, and contract manifest still carry the superseded envelope and do not contain `evidence.read`, `next-result/v3`, `observation-result/v3`, or the four error codes above. The `V2SCL` roadmap epic makes each layer conform: `V2SCL-002` reserves the schemas, routes, error codes, and manifest digests without runtime admission; `V2SCL-003` aligns the domain, configuration, schema, protocol, persistence, and preset-identity limits; `V2SCL-004` implements paging through protocol, store, daemon, and CLI and makes the route executable; and `V2SCL-005` integrates the observation budgets, closes conformance, and synchronizes the consumer-facing surfaces.

## Rejected alternatives

- Raising the IPC frame above 1 MiB removes the bound that makes response size provable and pushes unbounded allocation into every peer, without making any value reachable that paging cannot already reach.
- Returning complete evidence values from `session.next` keeps one round trip but makes the guidance surface grow with recorded content, so a single large item can make progression guidance itself undeliverable.
- Lowering the item schema bounds to match the 512 KiB projection preserves the existing code with no new surface, but deletes admissible recorded state and still leaves the documented envelope wrong.
- Charging the full worst-case recorded value in authoring vetting keeps one budget model, but rejects Procedures whose evidence is perfectly readable through paging.
- Stating lengths in bytes matches the superseded architecture wording, but makes every bound depend on encoding and makes a bound both untestable at the schema layer and surprising for non-ASCII recording.
- Signing or encrypting the page token implies an authorization property Podway does not have under same-user local trust and would invite callers to treat it as a capability.
- Keeping the full status projection inside observation preserves one existing consumer shape, but nests two independently budgeted maximal surfaces in one frame with no joint proof.

## Consequences

- Every value the item schema admits is reachable as selected evidence, and reachability no longer depends on how conservative a static projection happened to be.
- Adding the literal `max_total_length` default rotates the canonical digest of every Procedure that declares a list item, exactly once. A Procedure with no list item keeps its digest, so `small-change-v2` is unaffected while `bug-fix-v2` and `sw-dev-v2` are re-pinned in the owning task. Stored sessions keep their stored canonical snapshots, and digest-pinned automation receives `DIGEST_CONFIRMATION_REQUIRED` until it re-pins.
- A maximal list is reachable through bounded `item.add` mutations but cannot be replaced atomically in one `item.record_many` frame, so a crash between additions can leave a valid partial list.
- Observation consumers that read history from the observation status member move to `session.status --verbose`; current progression guidance and active-item mutation information remain self-contained in observation.
- Response budgets become a composition of named allocations rather than two numbers, so every future surface added to `next` or `observe` must claim its allocation explicitly instead of consuming slack.
