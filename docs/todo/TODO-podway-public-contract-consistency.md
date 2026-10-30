# Podway Public Contract Consistency

## Status and authority

- Document state: `Adopted`
- Owning roadmap epic: [CNTRT](../roadmap/README.md#cntrt-public-contract-consistency)
- Target product release: undecided
- Repository scope: Podway only

This dossier is the temporary execution SOT for CNTRT's three member tasks.
The [roadmap](../roadmap/README.md#cntrt-public-contract-consistency) alone owns
membership, ordering, dependencies and status. Adoption closes the selected
feedback deferrals, not implementation or acceptance. Specifications, accepted
ADRs and canonical machine assets keep describing current behavior until their
owning implementation task updates them. This design authorizes no product
implementation, release, installation or external-consumer change by itself.

## 1. Verified context and evidence

Three findings concern diagnostic meaning, exhaustive command/result-schema
membership and one manifest-enrolled writer-mismatch example. Existing checks
cover diagnostic code/severity agreement, authoring route membership, registered
result round trips and contract-generation selection, but do not exhaustively
compare the command catalog with every registered closed result membership.

The Config emitter in `procedure_v2_format.rs` already measures emitted authoring
text in Unicode scalar values and reports `SOURCE_PROJECTION_BUDGET_EXCEEDED`.
Canonicalization separately bounds canonical JSON characters and encoded bytes.
The catalog summary describes only the latter document. The writer-mismatch
known answer is schema-valid but carries another error's message and a workspace
recovery instead of the runtime's writer diagnosis.

These are source and test observations, not a fresh development-gate claim.
The selected design preserves runtime behavior and existing contract families.

### Source feedback

The following entries preserve their original source, deferral rationale and
revisit condition. All three are selected; their implementation owners are in
section 6. Keep this detail until closeout promotes it to durable owners.

| ID | Source | Feedback | Deferral reason | Revisit condition |
|---|---|---|---|---|
| DF-004 | V2GSC documentation review | The catalog defines `SOURCE_PROJECTION_BUDGET_EXCEEDED` for canonical JSON exceeding 1,048,576 characters or 1 MiB encoded, while `procedure format` also reports it when the emitted authoring document exceeds the character limit. | The format-emitter behavior predates the 1,024-node correction. Resolving the difference in which document is measured requires a separate decision across the diagnostic catalog, schema, release adapter, and CLI contract rather than widening this correction. | Before changing source-projection diagnostics or the emitted-document bound, assign the format overflow its own contract or explicitly broaden the existing code and update every published consumer. |
| DF-008 | V2SDG-006 testing review | The command catalog's `result_schemas` entries have no exhaustive automated match against registered result schemas. | Current catalog, registry, schemas, and emitted generations agree under the development gate; an exhaustive cross-check is separate contract-tooling work. | Before adding or renaming a result generation, extend contract verification to compare catalog command memberships with registered schemas and add a known-answer check. |
| DF-046 | GUIDE-012 integration review, recorded from the GUIDE-004 r2 observation | The `error-workspace_contract_mismatch` known answer (`tests/fixtures/v2/protocol/current-contracts.json:2112-2141`) carries the shared template message "The write-lock deadline elapsed without a state effect." and a `diagnose_workspace`/`workspace.doctor` recovery, while the catalog (`assets/specifications/error-codes.json:588-592`) and the runtime emit "The Store writer contract is not supported by this binary." with the `diagnose_writer` `version --identity` recipe. | The example is schema-valid (`workspace-contract-mismatch-details/v1` admits both recoveries), five other error examples share the same template message, no verifier rule requires example text to equal the catalog, and automation never depends on human-readable output; correcting a manifest-enrolled known answer forces the manifest, adapter and manifest regeneration for a non-authoritative example field. | Before the fixture examples are next regenerated for a contract change, align this example's message and recovery with the catalog summary and the `diagnose_writer` recipe, then run the full contract checks. |

## 2. Goals, non-goals and scope

Make published diagnostic descriptions, command/result memberships and the
selected known answer agree with their actual owners. Detect future membership
drift automatically under the normal development gate.

This epic introduces no new runtime capability, diagnostic code, schema version,
contract generation, numeric bound, command or Store layout. It does not redesign
schemas, widen released closed families, make human messages a stable API,
hand-edit derived assets, change external consumers, migrate existing Stores,
measure performance or select a product release.

## 3. Accepted design decisions and interfaces

### Existing overflow diagnostic

Retain `SOURCE_PROJECTION_BUDGET_EXCEEDED` and its existing severity, carriers and
failure behavior. Amend its catalog summary and the owning CLI/error
specifications to name both measurements explicitly:

- Complete canonical JSON: at most 1,048,576 Unicode scalar values and at most
  1,048,576 UTF-8 encoded bytes; either existing excess produces this code.
- Emitted canonical authoring text: at most 1,048,576 Unicode scalar values;
  existing emission overflow produces the same code.

These are separate measured documents. Do not apply the canonical JSON byte
limit to emitted authoring text, relax either check or bypass the independent
input/output envelope budgets. Exactly-at-limit remains admissible to this
particular check; other existing admission and envelope rules still apply.
Existing diagnostics identify the failed representation and measurement through
their current message/field values; their wire shape and runtime text stay intact.
No new ADR or diagnostic schema generation is needed for this clarification.

### Exhaustive result membership verification

Own the exhaustive check in the existing Protocol `int_suite` result-contract
suite. Reuse `serde_norway` and the public registration tables already available
to that suite; do not expose a new production API or parse Rust source text.

Compare the complete set of `(command, schema discriminator)` pairs from:

1. Every command-catalog `result_schemas` entry.
2. `EXISTING_ROUTE_RESULT_SCHEMAS_V2`, `NEW_ROUTE_RESULT_SCHEMAS_V1` and the
   command-bearing entries of `DAEMON_FREE_DOCUMENT_SCHEMAS_V1`.
3. The current Output v4 command-bound closed result selections, resolving local
   branches and registered schema references from canonical schema assets.

Compare membership, not ordering. Reject duplicate command declarations, duplicate
memberships within each source, malformed or unresolved family references,
missing/extra pairs and association with the wrong command before set conversion
can hide defects. Multiple results for one command and one family shared by
several commands are valid. Preserve the union across both supported contract
selections, including deprecated generation 1 and opt-in generation 2; retain
the existing generation-declaration checks rather than replacing them.

Generic-object result branches have no closed family membership. Commands such
as help and preset inspection therefore need no invented `result_schemas` entry.
Schema documents with no command membership, including input and detail families,
are outside the pair set. Preserve the current internal `__complete` route's
classification without promoting it into the public command catalog. Enumerate
actual Output v4 result alternatives, including command-selected status tiers,
prepared/running guidance and legacy decodable graph/preview variants; do not
mistake a nested component schema for a top-level result.

Negative controls exercise the same comparison helper with in-memory mutated
inputs. A catalog/registry disagreement fails verification; it must never trigger
an automatic asset correction or redefine which runtime routes are supported.

### Writer-mismatch known answer and generation

Change only `error-workspace_contract_mismatch` in the enrolled current-contracts
fixture: use the `WORKSPACE_CONTRACT_MISMATCH` catalog summary and the existing
`diagnose_writer` / `version --identity` recovery recipe with a truthful writer
inspection reason and `requires_explicit_authorization: false`. Retain its
`podway.error/v2` and `workspace-contract-mismatch-details/v1` families, digests,
correlation data, exit class and retryability. Generation 2 keeps its distinct
existing detail family. Do not repair the other unrelated template-message
examples under this scope.

Combine this correction with the diagnostic-catalog clarification in CNTRT-002.
Regenerate through the [release workflow](../implementation-tips/release.md):

```bash
python3 tools/contract_manifest.py --write
python3 tools/create_dolgorae_handoff.py prepare-adapter
python3 tools/contract_manifest.py --write
```

Review the complete derived diff. Refresh only consequences of the approved
canonical asset changes; never copy a new digest into a frozen compatibility
baseline to make a test pass. This prepares Podway-owned contract artifacts and
does not modify or qualify Dolgorae or Aquarium.

## 4. Failure handling and compatibility boundaries

Public routes, code/severity inventories, result/detail schemas, contract
selection and bounds remain unchanged. Human-message alignment in one example
does not establish a global exact-message comparison contract.

Catalog and enrolled-fixture byte changes rotate the contract manifest and its
prepared adapter identity. Existing exact-manifest writer admission continues
to refuse incompatible Stores. Declare no new predecessor, conversion or
transparent migration. Exercise failures only in owned fixtures; do not reset a
user workspace or operate installed services to demonstrate compatibility.

If verification reveals an actual missing or extra runtime family, stop and
adjudicate it against the named authorities. This epic's checks do not authorize
a schema or runtime expansion. Any material change to this contract requires a
revised dossier and explicit scope decision before implementation continues.

## 5. Canonical ownership and required artifacts

- Protocol owns membership verification and its negative controls. The command
  catalog owns declared memberships; Output v4 and Protocol's registration tables
  retain their present production roles.
- Config owns canonicalization and emitted-text budget behavior. The diagnostic
  catalog and the CLI/errors specifications own the clarified description.
- The current-contracts fixture owns the corrected example. Manifest and prepared
  adapter files remain derived through the canonical workflow.
- The testing guide owns durable regression ownership and preserved source
  feedback; the JSON and CLI/error specifications own lasting contract guidance.
  Promote these during implementation, before the final candidate review.

## 6. Roadmap ownership, dependencies and traceability

| Requirement | Implementation owner | Required outcome |
|---|---|---|
| DF-008 | CNTRT-001 | Exhaustive three-way membership comparison with positive and known-failure coverage in the normal test gate. |
| DF-004 and DF-046 | CNTRT-002 | Clarified measured-document contract, unchanged behavior, corrected writer example and coherent regenerated identities. |
| Integrated acceptance and durable owners | CNTRT-003 | Complete focused and development verification, promoted documentation and an exact closeout candidate for independent whole-epic validation and owner acceptance. |

The roadmap fixes the execution sequence `CNTRT-001 -> CNTRT-002 -> CNTRT-003`.
Each member must leave its direct checks and applicable development gate passing;
CNTRT-002 does not postpone required generation work until integration. No other
candidate program is a prerequisite. Accepted GUIDE and CALLS behavior is the
compatibility baseline.

## 7. Verification and release acceptance

### CNTRT-001

- Validate the real catalog, registration tables and Output v4 selections together.
- Require failures for missing and extra memberships in either direction, swapped
  command associations, duplicates and an unknown/unresolved schema.
- Prove legitimate multi-result commands, shared families, both observation
  generations and generic/non-command documents are handled correctly.
- Retain existing registered-result serialization/decoding and generation tests.
  Register new integration checks through `int_suite`, not a standalone target.

### CNTRT-002

- Retain canonical JSON character and encoded-byte overflow checks. Include
  multibyte input to demonstrate that encoded bytes and scalar counts differ.
- Exercise emitted-text overflow after canonical admission, exact boundary checks
  at the owning measurement layer, and the unchanged diagnostic code/severity.
  Use a valid fixture or the existing emitter's local test boundary; do not widen
  a public limit merely to reach the check.
- Through the actual CLI in an owned fixture, show that failed `format --write`
  preserves the original file and produces no partial successful source result.
- Validate the corrected known answer with the current closed schemas and assert
  its selected catalog message plus the writer recovery action, command, argv and
  non-destructive authorization flag. Keep this assertion scoped to that example.
- Verify the regenerated manifest and prepared adapter, schema closure and both
  contract generations. Frozen GUIDE compatibility baselines stay unchanged.

### CNTRT-003 and closeout

Run focused checks first, then the required `make test` development gate on the
final candidate through the configured Gaori commands. Use repository Make
targets for architecture, contract-manifest and integration verification; direct
contract sentinels and adapter self-tests supplement rather than replace that
gate. Inspect repository status and the complete diff after generation and checks.
Required coverage must execute; compilation or a schema-valid mock is insufficient.
No `make dist`, packaged qualification or release-readiness claim is selected.

Before whole-epic acceptance, publish lasting descriptions and regression owners
to their canonical documents and inventory every dossier reference. CNTRT-003's
normal completion commit and internal whole-epic validation precede the owner's
explicit acceptance of the exact closeout. Only then replace the roadmap's
Detailed SOT with durable outcomes, remove the adopted index entry and delete this
dossier under the approved last-consumer deletion boundary. Preserve all three
feedback entries' source, rationale and revisit conditions in durable guidance
before retirement. Completing member tasks alone does not complete CNTRT.

## 8. References

- [Documentation authority](../README.md#precedence)
- [TODO authority and lifecycle](README.md)
- [JSON contract](../specs/interfaces/json-contract.md)
- [Errors and exit codes](../specs/interfaces/errors-and-exit-codes.md)
- [CLI specification](../specs/interfaces/cli-specification.md#procedure-commands)
- [ADR-0039](../architecture-decision-records/0039-opt-in-contract-generations.md)
- [Testing](../implementation-tips/testing.md)
- [Release generation workflow](../implementation-tips/release.md)
- [Canonical schemas](../../assets/schemas/)
- [Executable specifications](../../assets/specifications/)
- [Deferred feedback register](../deferred-feedback/README.md)
