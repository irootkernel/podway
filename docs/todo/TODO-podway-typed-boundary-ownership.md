# Podway Typed Boundary Ownership

## Status and authority

- Document state: `Candidate`
- Working label: TYPED
- Owning roadmap epic: none
- Target product release: undecided
- Repository scope: Podway only

This working label groups proposed work; it reserves no roadmap ID. This
candidate adopts no implementation, public interface, release or consumer change.
The [roadmap](../roadmap/README.md) alone owns adopted work and task status.

## 1. Context

The recorded findings identify repeated extraction, wire-name mapping,
delegation detection, bounds and predicates across existing crate boundaries.
Current checks agree; these are independent maintainability and regression
protection opportunities, not evidence that current results are incorrect.
PARSE-004 already moved CLI authoring detection into Config-owned first-parse
facts. That delivered part is outside the remaining scope.

## 2. Goal

Define a bounded ownership cleanup that reuses domain values and admitted
facts while preserving independent validation at persisted-state, protocol and
filesystem trust boundaries.

## 3. Non-goals

No new item type, delegation mode, live adapter, runtime, Store-to-Config
dependency, public wire shape or relaxed persisted-value admission. No timing
targets, general code cleanup or automatic acceptance of every listed refactor.
The separate [INTEG candidate](TODO-podway-executor-integration.md) owns real
executor compatibility and qualification.

## 4. Rough Scope

- Give item wire names, delegation bounds and reusable domain predicates
  explicit owners consistent with the existing dependency direction.
- Explore typed acceptance extraction at the protocol boundary, preserving
  separate Store validation, malformed-input refusal and exact replay identity.
- Keep Config parsing and Store structural detection apart; stdin-fence
  classification is separately adopted under AGENT-002.
- Review workspace identity comparison, pure artifact/item preparation and bounded
  citation diagnostics while retaining native descriptor checks and error order.
- Carry decision-current delegation, human success rendering and actual in-writer
  artifact-read coverage into the relevant adopted changes.

DF-014 was promoted to AGENT-002 on 2026-11-06. Its full original details now
belong to the [AGENT dossier](TODO-podway-agent-interaction-simplification.md#selected-feedback-df-014).
Promotion closes the deferral only, not implementation or acceptance.

### Source feedback

These entries retain the original source, reason for deferral and revisit
condition. Candidate grouping does not resolve, adopt or implement them.
Only the remaining work described by each entry enters this candidate;
already delivered subparts are preserved as context.

| ID | Source | Feedback | Deferral reason | Revisit condition |
|---|---|---|---|---|
| DF-002 | V2SDG-003 maintainability review | The `ItemTypeV1` wire-name mapping is repeated in the delegation package projector and Engine read projection. | Exhaustive Rust matches and current schema checks keep the seven names aligned; consolidating both consumers crosses this task's reviewed scope. | Before adding or renaming an item type, give its wire name one owner and verify both projections and schemas. |
| DF-003 | V2SDG-003 maintainability review | Delegation package static-budget reserves depend on bounded goal and evidence values owned elsewhere. | Current limits fit the reserves and the projection has an actual-byte backstop; exporting all bounds solely for this task would widen the public core API. | Before changing goal, selector, identifier, or item bounds, derive or assert the worst-case reserve against those limits and verify admission-time rejection. |
| DF-005 | V2SDG-006 and V2SDG validation maintainability reviews | Delegated-placement detection is repeated across Config, Engine, and Store; the Store receipt fast path searches canonical placement JSON. PARSE-004 already relocated CLI authoring detection to Config-owned first-parse facts. | Current typed predicates and fail-closed Store checks preserve emitted results; a Direct decision option named `delegation` only causes redundant receipt queries. Consolidation is independent drift prevention. | Before changing placement variants or delegation detection, give parsed-Procedure detection one config-owned API and keep Store detection structural behind its persisted-state boundary, without adding a Store-to-config dependency; verify Direct and delegated result generations together. |
| DF-011 | V2SDG final epic maintainability review | Receipt byte limits are repeated in Store and protocol validation, and delegation result limits are repeated in protocol and Engine projection. | Current constants, schemas, Store DDL, and emitted bounds agree; consolidating ownership is future drift prevention rather than a current correctness fix. | Before changing receipt or delegation result byte limits, give shared bounds a core owner and verify decoder, Store DDL, schema, and emission agreement. |
| DF-012 | V2SDG final epic maintainability review | Store receipt validation repeats core RoleRef, round-count, and after-acceptance disposition predicates. | Current core, Store, and protocol validators agree; sharing their implementation is independent maintainability work. | Before changing role references, round bounds, or after-acceptance dispositions, reuse core constructors or named bounds while preserving closed JSON shapes and verify authoring, receipts, protocol, and schemas together. |
| DF-013 | V2SDG validation maintainability review | Delegation acceptance passes validated JSON between protocol, CLI, Engine, and Store through repeated string-keyed field access. | Closed protocol validation and transaction-time input-to-receipt checks currently reject malformed input; a shared typed carrier would prevent future field drift rather than repair current behavior. | Before changing acceptance-input fields, consolidate their typed extraction at the existing protocol boundary while retaining independent Store validation and the closed wire shape; verify malformed inputs and replay identity. |
| DF-016 | V2SDG validation testing review | Native delegation route tests do not separately assert package and acceptance rejection while a decision placement is current. | Both routes use the same explicit non-action rejection in the package projector; Direct rejection and valid decision-source evidence are covered, and no current result defect is known. This is additional branch coverage. | Before changing active-placement classification or delegation error mapping, add a decision-current route case asserting the error code and reason for package and acceptance, plus unchanged session and attempt state. |
| DF-017 | V2SDG validation testing review | Delegation command success paths lack dedicated human-output smoke cases. | All three use the existing generic v2 success renderer, whose complete result rendering is already tested; there is no missing delegation-specific rendering branch. Dedicated cases are future coverage. | Before changing delegation success rendering or adding schema-specific output, assert successful non-JSON package, acceptance, and receipt output through the CLI, including meaningful result content and absence of error output. |
| DF-022 | V2DFC-013 maintainability review | The expected workspace UUID comparison is duplicated in the workspace resolver and the token-backed read path. | Both paths currently preserve the same typed error and details, and public ordering tests cover their intentional placement. Consolidating them is independent maintenance work with no current behavior defect. | Before changing workspace identity fence rules or their public error context, consolidate the comparison under one owner and verify both token-backed and ordinary reads retain their ordering and typed details. |
| DF-024 | V2DFC-014 maintainability review | Required-artifact preparation in the native Engine adapter maps orchestration errors and traverses workflow-memory slots. | Current required-set, active-attempt and item-revision checks agree with pure preparation and fail closed; relocating their ownership is independent maintenance work. | Before changing required-artifact slot or attempt structure, give the pure item/revision projection one Engine owner and keep native stamp checks at the filesystem boundary; preserve typed errors and replay ordering. |
| DF-025 | V2DFC-014 maintainability review | The Engine's bounded citation precheck repeats the domain's diagnostic text. | The shared numeric bound agrees today; early rejection prevents quadratic duplicate checks and cloning oversized input, so removing it would weaken resource bounds. Diagnostic ownership is independent future work. | Before changing citation bounds or their diagnostics, expose one domain-owned bounded validation result and verify Engine and domain errors together without moving preparation into the writer. |
| DF-027 | V2DFC-014 testing review | The hash observer sees preparation streams but does not instrument every potential future read inside stamp verification. | The current descriptor-stamp resolver reads metadata only, and same-byte rename tests reject a digest-only regression. Broader instrumentation is independent future regression protection, not an unverified current byte-reading path. | Before changing stamp verification or introducing another artifact read there, instrument the actual in-writer file-read boundary and prove all four routes read no artifact bytes while a separate writer can progress during preparation. |

## 5. Open Decisions

- Which existing domain types and constructors can serve each consumer
  without widening public APIs or reversing crate dependencies?
- Which duplicate checks are intentional independent admission rather than
  reusable computation, and which pure facts can safely be shared?
- What bounded subset has one coherent acceptance contract? Split independently
  motivated subsets before adoption rather than making all 12 remaining findings mandatory
  members of one implementation epic.
- What error, ordering, schema, canonical-byte, projection and replay fixtures
  demonstrate preserved behavior for each selected seam?

## 6. Roadmap Promotion Conditions

Close the ownership map and selected subset against current source and
canonical contracts. Preserve the original revisit conditions in the source table,
choose focused success/failure/compatibility checks and the required development
gate, and register a decision-complete dossier or roadmap contract. Any change
to an architectural invariant needs accepted ADR authority first. Candidate
grouping grants no implementation or review authorization.

## 7. References

- [TODO authority and lifecycle](README.md)
- [Repository structure](../architecture/repository-structure.md)
- [System Architecture](../architecture/system.md)
- [Supervised delegation](../specs/interfaces/supervised-delegation.md)
- [Transaction contract](../specs/storage/transactions-concurrency-and-idempotency.md)
- [Deferred feedback register](../deferred-feedback/README.md)
