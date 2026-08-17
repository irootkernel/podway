# Podway External Check Result Typing

## Status and authority

- Document state: `Adopted`
- Dossier type: adopted design dossier
- Owning roadmap epic: `V2AST`
- Target product release: v0.3.0
- Repository scope: Podway only
- Adoption baseline: August 17, 2026

This dossier is the decision-complete implementation plan for distinguishing an
ordinary recorded assertion from a structurally bound external check result.
The active roadmap owns task order and status. Accepted ADRs, canonical machine
assets, and current specifications remain authoritative for implemented behavior
until the owning tasks update them.

## 1. Verified context and evidence

ADR-0007 and ADR-0016 deliberately model workflow memory as typed attempt-local
items rather than a general evidence ledger. The shipped item types can record
that a check passed, but none can distinguish that assertion from a result bound
to a declared operation identity and digest that also carries an exact
input-basis descriptor and digest.

This is a demonstrated omission-prevention gap: a Procedure cannot require a
structurally complete check result, and an observer cannot distinguish one from
free-form text. Podway still cannot prove that an external executor ran honestly
or that the supplied digest describes the claimed bytes. The new type therefore
provides structural binding and attempt-scoped freshness, not factual
verification.

## 2. Goals, non-goals, and scope

The epic will add one `check_result` item type that:

- binds a recorded result to a Procedure-declared operation identity and digest;
- records one exact external input-basis descriptor and digest;
- records bounded executor attribution, outcome, summary, and output digest;
- participates in ordinary item satisfaction, attempt snapshots, read-back,
  retry, rework, and invalidation; and
- remains additive for existing Procedure v2 files and stored sessions.

The epic will not execute commands, call models, fetch input or output bytes,
validate external schemas, add signatures or attestation, create an issuer
authority, store logs, add human approval as an assurance class, or create a
separate evidence ledger. Human approval remains an attributed decision record.

## 3. Accepted design and public interfaces

### 3.1 Assurance classes

Existing `confirm`, `text`, `choice`, `integer`, `list`, and `artifact` values are
ordinary actor-recorded assertions. No stored representation or completion
behavior changes for them.

`check_result` is a new typed item. It is still caller-supplied data under the
same-user trust model, but its closed fields make operation, input, executor, and
result identity mechanically inspectable.

The term is deliberately distinct from Podway's durable admission, terminal,
and idempotency receipts. Product contracts call this value a structurally bound
external check result and never shorten it to a receipt.

### 3.2 Procedure declaration

A check-result item uses this closed shape:

```yaml
- id: verification
  type: check_result
  prompt: Record the current external verification result.
  required: true
  operation_id: make-test
  operation_digest: sha256:<64-lowercase-hex>
  accepted_outcomes:
    - pass
```

`operation_id` reuses the normal 64-character identifier component, and all
digest fields reuse the lowercase SHA-256 digest component. `operation_digest`
is the digest of the external operation definition as
understood by its owner; Podway neither stores nor executes that definition.
`accepted_outcomes` is required with no default and is a unique array of one to
three values from `pass`, `fail`, and `inconclusive`. A result is satisfied only
when its operation identity and digest match the declaration and its outcome is
accepted.

### 3.3 Recorded value

`item.record_many` accepts this additional record variant:

```json
{
  "type": "check_result",
  "operation_id": "make-test",
  "operation_digest": "sha256:<64-lowercase-hex>",
  "input_basis": {
    "descriptor": "HEAD and dirty-tree snapshot",
    "digest": "sha256:<64-lowercase-hex>"
  },
  "executor": {
    "name": "gaori",
    "version": "1.0.0"
  },
  "outcome": "pass",
  "summary": "The complete development gate passed.",
  "output_digest": "sha256:<64-lowercase-hex>"
}
```

Bounds are:

- input descriptor: 512 Unicode scalars;
- executor name: 128 Unicode scalars;
- executor version: 64 Unicode scalars;
- summary: 2,000 Unicode scalars; and
- complete canonical result value: 32 KiB encoded JSON.

Every shown field is required, every nested object is closed, and additional
fields are forbidden. The descriptor, executor name, executor version, and
summary are non-empty. An executor without a native version records an honest
stable placeholder such as `unknown`; it does not omit the field. External start
and finish timestamps are not stored because Podway cannot trust them and they
do not determine freshness. There is no arbitrary metadata map.

The immutable value digest is calculated from the complete canonical result.
The terminal attempt's existing complete item-set digest continues to bind
all item values. No second result table or lifecycle is introduced.

At most 128 values times 32 KiB yields a derived per-attempt maximum of 4 MiB of
canonical check-result JSON. The existing per-value and item-count bounds enforce
that product; no second aggregate invariant or error code is added. This does not
change V2SCL's separate text-and-list aggregate.

### 3.4 Recording and observation

The existing `item.record_many` command, exposed as `podway record --stdin`, is
the only mutation route required. One request is atomic and frame-bounded. A
node with many maximal results may require multiple calls; the set of calls is
not atomic, and no single-item duplicate route is added.

For an unsatisfied required `check_result`, `session.next` includes
`item.record_many` in `allowed_actions` and emits the suggestion command
`item.record_many` with argv `podway record --stdin` and the target item ID.
`session.observe` owns the copyable body guidance. Its mutation-template contract
conditionally permits one `stdin_template`, at most 16 KiB, only on the
`item.record_many` template. The template targets the first unsatisfied check
result in declaration order, prioritizing required items. It mirrors the exact
closed stdin shape with real workspace, session, attempt, session-revision,
item-revision, item, operation ID, and operation digest values, and placeholders
for caller-supplied result fields and the idempotency key. Accepted outcomes
remain in the active-item declaration constraints rather than becoming an
invalid stdin field. The template is absent when no check-result item needs a
value and is forbidden on every other command. This extends the V2SCL 64 KiB
mutation-template window without adding a command-execution surface.

Observation and `session.next` previews never character-truncate the structured
object. They expose the fixed, escape-free subset `operation_id`, `outcome`,
`operation_digest`, `input_basis.digest`, and `output_digest`, omitting the free
text descriptor, executor, and summary. The public contracts use separate
definitions for this projection and the complete value. Actual encoded projection
size remains below the observation value limit because every projected field is
ASCII pattern-constrained.

Check-result values participate in the pageable evidence contract owned by
`V2SCL`. `evidence.read` returns the complete value in one terminal page because
the encoded-byte paging unit is capped at 32 KiB, below the 256 KiB page-data
limit. A stale source attempt may remain visible through history surfaces but
cannot be read as current evidence or satisfy current progression.

### 3.5 Compatibility-sensitive contract inventory

The reservation task updates `procedure-v2.schema.json` with the declaration;
`v2-result-components-v1` with disjoint complete-value, preview, read-back, and
compact item shapes; `observation-result-v3` with the type, declaration
constraints, five-field projection, and conditional `stdin_template`; and
`item-record-many-input-v1` and `item-record-many-result-v1` with the closed
record variant. It also reserves the canonical SQLite v6 DDL and named migration
step and updates every affected contract-manifest digest.

The complete check-result object and `artifactValue` remain disjoint `oneOf`
branches: both are closed and require non-overlapping field sets. Exact-one
validation is an executable contract. The shared `v2Command` enum already admits
`item.record_many`, so no command route or new error code is required. Existing
errors cover malformed and oversized requests, item constraints and type
mismatches, migration failure, and unsupported downgrade state.

## 4. Failure handling and compatibility

- Procedure schema remains `podway.procedure/v2`; older peers reject the changed
  exact contract manifest rather than ignoring the new item type.
- SQLite schema v6 adds `check_result` to the closed item discriminator. Because
  the discriminator is a `CHECK` constraint, migration rebuilds
  `v2_item_slots` and preserves every existing `value_json` byte-for-byte; no
  stored value is reinterpreted. The migration ships canonical v6 DDL, advances
  the current-version constant, registers a named v5-to-v6 step, and retains
  downgrade protection.
- Existing sessions, procedures, result records, and six item types retain their
  behavior and are classified as assertions only in documentation.
- All declaration and value fields are required and have no materialized
  defaults. Existing Procedure canonical bytes and the V2SCL-re-pinned preset
  digests therefore remain stable throughout V2AST; this epic performs no
  Procedure digest rotation.
- A mismatched operation or unaccepted outcome may be recorded but does not
  satisfy the item.
- Malformed digests, unknown fields, oversized values, and unsupported outcomes
  fail before mutation admission when possible and never partially update a slot.
- Retry creates an empty check-result slot on the new attempt. Rework and goal
  revision use existing suffix invalidation without a separate revocation model.
- Each `item.record_many` call is atomic, but a frame-sized sequence of calls is
  not atomic as a group and may leave structurally valid partial progress after a
  client interruption.

The product and documentation must say `external check result`, `recorded`, or
`structurally bound`. They must not say `Podway verified`, `trusted`, `attested`,
or `secure evidence` for this value. `input_basis` is caller-supplied inspectable
data. Podway never compares it with worktree state or external bytes; ordinary
attempt-suffix invalidation supplies attempt-scoped freshness and nothing more.

## 5. Roadmap ownership and dependencies

`V2AST` depends on completed `V2SCL` and precedes `V2GRD`.

- `V2AST-001` adopts an ADR that extends ADR-0007 and ADR-0016 and updates their
  extension metadata, plus the normative domain, interface, storage, and
  trust-boundary specifications.
- `V2AST-002` reserves the public schema shapes, observation windows, exact
  manifest changes, canonical SQLite v6 DDL, named migration, and compatibility
  fixtures without runtime admission.
- `V2AST-003` adds the pure domain values, declaration, satisfaction,
  canonicalization, parser, and authoring diagnostics.
- `V2AST-004` rebuilds the constrained table, adds codec and protocol decoding,
  records atomic frame-sized batches, and proves replay, restart, idempotency,
  and downgrade protection.
- `V2AST-005` exposes allowed actions, suggestions, the bounded observation
  projection and stdin template, pageable read-back, CLI JSON guidance, and
  honest human rendering.
- `V2AST-006` closes public-contract, trust-language, migration, recovery,
  maximum-size, documentation-promotion, and complete development-gate evidence.

Before `V2AST` is marked complete, its tasks must promote every lasting assurance,
trust-boundary, storage, compatibility, and operator-guidance decision into the
affected ADRs, machine contracts, specifications, architecture, implementation
tips, and examples. The final task then replaces roadmap references to this
dossier with those durable authorities, removes its TODO index entry, and deletes
this file. This dossier is not moved to `docs/roadmap/archive/`.

## 6. Verification and acceptance

Acceptance requires domain boundary tests, YAML/JSON equivalence, canonical
digest stability with no V2AST preset rotation, schema-v5 table-rebuild migration
and downgrade protection, atomic record and replay tests, restart reconstruction,
satisfaction for all three outcomes, operation mismatch, malformed and oversized
payload failures, retry/rework stale behavior, and the complete `make test`
development gate. Contract coverage proves artifact/check-result `oneOf`
disjointness, `allowed_actions` and suggestion exposure, one-operation
`stdin_template` bounds, maximal single-batch frame rejection and successful
split batches, the 4 MiB derived attempt product, the escape-free five-field
observation/preview projection, a complete single-page `evidence.read`, and exact
public schema and manifest validation.

## 7. References

- [TODO and Adopted Design Dossiers](README.md)
- [ADR-0006: Same-User Local Trust](../architecture-decision-records/0006-same-user-local-trust.md)
- [ADR-0007: Typed Stage Items](../architecture-decision-records/0007-stage-items-not-evidence-ledger.md)
- [ADR-0009: Artifact Metadata Only](../architecture-decision-records/0009-artifact-metadata-only.md)
- [ADR-0010: Generic CLI and JSON Integration](../architecture-decision-records/0010-generic-cli-json-integration.md)
- [ADR-0016: Recorded Item Workflow Memory](../architecture-decision-records/0016-recorded-item-workflow-memory.md)
- [Goals and Non-Goals](../specs/product/goals-and-non-goals.md)
- [Security and Trust](../specs/operations/security-and-trust.md)
- [Procedure and Item Specification](../specs/domain/procedure-and-item-specification.md)
- [SQLite Model](../specs/storage/sqlite-model.md)
