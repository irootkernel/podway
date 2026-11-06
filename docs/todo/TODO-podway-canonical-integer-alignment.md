# Canonical Integer Contract Alignment

## 1. Status and authority

- Document state: `Adopted`
- Adoption date: 2026-11-06
- Owning roadmap epic: CANON
- Target product release: unassigned
- Repository scope: Podway only
- Source baseline: `98b8de7da9ac086ea0839f5e62a29dd88dc874ae`

The owner approved this planning registration; implementation has not started.
The [roadmap](../roadmap/README.md#canon-canonical-integer-contract-alignment)
alone owns order and status. This dossier adopts the intended correction; it
changes no canonical asset, code or generated identity at registration.

## 2. Verified context and evidence

The canonicalization asset declares `signed-integers-only`; Core canonicalization
and Rust build support accept i64 and u64; Python's manifest helper rejects floats
without bounding integers. [System architecture](../architecture/system.md#document-admission-reuse)
records that conformance discrepancy. These are source-inspection facts, not
newly executed tests. Existing Core canonical tests cover i64/u64 extremes and
floating-point refusal; the contract known-answer fixture owns bytes and digests.

## 3. Goals, non-goals and scope

Align admission and serialization across the canonical primitive and manifest
consumers while preserving accepted-value canonical bytes. Keep one bounded
implementation-and-verification task. AGENT has no technical dependency on this
correction. No general numeric framework, performance measurement, field-range
expansion, migration or predecessor declaration is selected.

## 4. Accepted design decisions and interfaces

The primitive admits integers in the inclusive range `[-2^63, 2^64 - 1]`.
Booleans retain JSON boolean meaning, not Python integer admission. Float/exponent
representations remain rejected; existing noncanonical spellings remain rejected.
Per-field schemas keep their own bounds, including signed-only item values.
Key order, escaping, array order, whitespace and digest algorithms are unchanged.

Governed surfaces are `assets/specifications/canonicalization-v1.json`, Core's
`canonicalize_json_v1` and canonical verification helpers, Protocol's Rust build
support and `tools/contract_manifest.py`. Core callers include canonical
Procedure documents and request/digest preimages. Check those consumers without
relaxing their schema admission or changing their accepted-value bytes. Python
manifest and fixture processing must use the same range. The existing known-answer
fixture and boundary tests are the cross-language oracle; no new framework.
CANON-001 updates the canonical rule and its generated manifest/adapter identities
together through existing generation workflows, never a handwritten derived copy.

## 5. Failure handling and compatibility boundaries

Reject values just below the minimum and above the maximum under the documented
canonicalization error path. Do not allow Python bool handling to reject valid
booleans or serialize them as numbers. Preserve zero, signed/unsigned boundaries,
existing schema restrictions and canonical verification of original bytes.

Changing a canonical asset changes manifest identity even where value bytes do
not change. Current writer compatibility and previous-writer refusal apply;
consumer re-pinning and any reset remain separately authorized. No transparent
migration, predecessor or external consumer change is added. The existing
higher-authority asset continues to describe current behavior until CANON-001
lands the correction; the mismatch is not declared resolved by this dossier.

## 6. Roadmap ownership, dependencies and traceability

### CANON-001: Align Numeric Admission and Canonical Evidence

**Why**

All implementations claiming the same canonicalization contract should agree on which integers are accepted and on the bytes produced for accepted values.

**What and how**

- Apply the numeric domain and governed documents, digest preimages and consumers fixed in Section 4.
- Reconcile `assets/specifications/canonicalization-v1.json`, Core, Rust build support, and the Python manifest helper.
- Reuse existing canonical known-answer fixtures and test owners where practical.
- Check the minimum, maximum, adjacent out-of-range values, signed/unsigned boundary, zero, and rejected floating-point encodings.
- Preserve accepted-value canonical bytes, key ordering, string escaping, and digest behavior outside the selected correction.
- Update affected specification prose, implementation guidance, schemas if necessary, manifest, and adapter contract identities coherently through existing generation workflows.
- Keep unchanged constraints outside this numeric decision intact.

**Acceptance**

- Relevant Rust/Python admission agrees on the approved range.
- Accepted fixtures produce identical intended canonical bytes and digests.
- Out-of-range values fail under the documented contract.
- Existing per-field numeric constraints are preserved.
- Current canonical and manifest checks pass with one coherent set of assets.
- The release/upgrade implications of changing a canonical asset are stated under current policy; no predecessor or migration is added as part of this task.
- No new cross-language verification framework or mandatory performance measurement is introduced.

CANON contains one bounded task, including its direct verification. Keep the numeric rule, implementation, tests, canonical assets, and generated identity changes coherent in that task.

## 7. Verification and release acceptance

Use existing Core/build-support and Python known-answer owners. Verify minimum,
maximum, immediately adjacent out-of-range integers, i64 maximum and its unsigned
successor, zero, booleans, nested values and rejected float/exponent spellings.
Verify identical accepted bytes/digests and preservation of per-field restrictions.
Run focused checks, canonical/manifest checks and the required `make test` gate
for the future implementation candidate, using repository gate routing.
Registration claims no behavioral execution or conformance repair.

Update the canonical rule, system architecture's discrepancy paragraph, applicable
specifications and testing guidance with the implementation. After required
review and separate owner acceptance, promote durable requirements and regression
ownership, replace dossier links and retire it under the [TODO lifecycle](README.md#lifecycle).
Development acceptance is distinct from release, installation and consumer
adoption. No release target or distribution gate is selected here.

## 8. References

- [Canonical rule](../../assets/specifications/canonicalization-v1.json)
- [Core implementation](../../crates/podway-core/src/canonical.rs)
- [Rust build support](../../crates/podway-protocol/build_support.rs)
- [Python manifest helper](../../tools/contract_manifest.py)
- [Known-answer fixture](../../tests/fixtures/contract/canonicalization-v1.json)
- [Transaction contract](../specs/storage/transactions-concurrency-and-idempotency.md)
- [JSON contract](../specs/interfaces/json-contract.md)
