# Podway Public Contract Consistency

## Status and authority

- Document state: `Candidate`
- Working label: CNTRT
- Owning roadmap epic: none
- Target product release: undecided
- Repository scope: Podway only

This working label groups proposed work; it reserves no roadmap ID. This
candidate adopts no implementation, public interface, release or consumer change.
The [roadmap](../roadmap/README.md) alone owns adopted work and task status.

## 1. Context

Three findings concern the relationship between published contract assets:
source-projection overflow meaning, exhaustive command/result-schema membership,
and one manifest-enrolled writer-mismatch example. Current runtime and schema
checks pass; diagnostic policy and example synchronization have separate owners
and must not be changed by implication.

## 2. Goal

Shape explicit consistency checks and narrowly justified asset corrections
so diagnostic meaning, registered results and examples agree with their named
authorities without accidental compatibility changes.

## 3. Non-goals

No new runtime capability, general schema redesign, in-place widening of
released closed families, treating human message text as a machine contract, or
hand-editing generated manifests and adapter copies.

## 4. Rough Scope

- Decide whether authoring-format overflow needs its own diagnostic or an
  explicit amendment to the existing source-projection diagnostic contract.
- Cross-check command catalog result memberships against registered schemas,
  with a known-answer failure case for drift.
- Align the writer-mismatch known answer with the catalog message and permitted
  writer diagnosis when its enrolled fixture is intentionally regenerated.
- Account for schema, release-adapter and contract-manifest consequences through
  the canonical generation workflow.

### Source feedback

These entries retain the original source, reason for deferral and revisit
condition. Candidate grouping does not resolve, adopt or implement them.
Only the remaining work described by each entry enters this candidate;
already delivered subparts are preserved as context.

| ID | Source | Feedback | Deferral reason | Revisit condition |
|---|---|---|---|---|
| DF-004 | V2GSC documentation review | The catalog defines `SOURCE_PROJECTION_BUDGET_EXCEEDED` for canonical JSON exceeding 1,048,576 characters or 1 MiB encoded, while `procedure format` also reports it when the emitted authoring document exceeds the character limit. | The format-emitter behavior predates the 1,024-node correction. Resolving the difference in which document is measured requires a separate decision across the diagnostic catalog, schema, release adapter, and CLI contract rather than widening this correction. | Before changing source-projection diagnostics or the emitted-document bound, assign the format overflow its own contract or explicitly broaden the existing code and update every published consumer. |
| DF-008 | V2SDG-006 testing review | The command catalog's `result_schemas` entries have no exhaustive automated match against registered result schemas. | Current catalog, registry, schemas, and emitted generations agree under the development gate; an exhaustive cross-check is separate contract-tooling work. | Before adding or renaming a result generation, extend contract verification to compare catalog command memberships with registered schemas and add a known-answer check. |
| DF-046 | GUIDE-012 integration review, recorded from the GUIDE-004 r2 observation | The `error-workspace_contract_mismatch` known answer (`tests/fixtures/v2/protocol/current-contracts.json:2112-2141`) carries the shared template message "The write-lock deadline elapsed without a state effect." and a `diagnose_workspace`/`workspace.doctor` recovery, while the catalog (`assets/specifications/error-codes.json:588-592`) and the runtime emit "The Store writer contract is not supported by this binary." with the `diagnose_writer` `version --identity` recipe. | The example is schema-valid (`workspace-contract-mismatch-details/v1` admits both recoveries), five other error examples share the same template message, no verifier rule requires example text to equal the catalog, and automation never depends on human-readable output; correcting a manifest-enrolled known answer forces the manifest, adapter and manifest regeneration for a non-authoritative example field. | Before the fixture examples are next regenerated for a contract change, align this example's message and recovery with the catalog summary and the `diagnose_writer` recipe, then run the full contract checks. |

## 5. Open Decisions

- Which document and byte/scalar limits does each overflow diagnostic own,
  and which public consumers require coordinated compatibility treatment?
- What exhaustive catalog/registry comparison respects the supported contract
  generations and detects missing as well as extra memberships?
- Should fixture correction share a necessary contract regeneration, and which
  identities and compatibility checks must be refreshed together?

## 6. Roadmap Promotion Conditions

Resolve diagnostic policy and canonical ownership, select a bounded set
of checks and asset changes, and close generation and compatibility verification
before adopting tasks. Fixture-only text changes still need their real manifest
consequences recorded. Separate independent subsets if one acceptance contract
cannot cover them. This candidate establishes no new diagnostic behavior.

## 7. References

- [TODO authority and lifecycle](README.md)
- [JSON contract](../specs/interfaces/json-contract.md)
- [Errors and exit codes](../specs/interfaces/errors-and-exit-codes.md)
- [Canonical schemas](../../assets/schemas/)
- [Executable specifications](../../assets/specifications/)
- [Deferred feedback register](../deferred-feedback/README.md)
