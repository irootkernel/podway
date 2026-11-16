# TYPED: Typed Boundary Ownership

## Status and authority

- Document state: `Adopted`
- Owning roadmap Epic and sole consumer: `TYPED`
- Execution workflow: `aquarium:epic-handler`, only when explicitly invoked
- Target product release: unassigned
- Repository scope: Podway only
- Implementation inspection basis: `2616c3ae5719c46d2e28b3e1a643cd0f023507b0`

The [roadmap](../roadmap/README.md#typed-typed-boundary-ownership) alone owns
Task order and lifecycle state. This dossier is the decision-complete plan for
unfinished work, not a second status table or implementation evidence. Its source
basis includes the separately approved documentation-only candidate retirement
in the working tree; it does not claim a clean committed documentation candidate.
Registration starts no execution, session, commit, release or installation.

Master selected all twelve remaining findings for one bounded Epic.
The original source, complete finding, deferral reason and revisit condition
have one permanent detailed owner in the
[TYPED feedback section](../implementation-tips/testing.md#typed-source-feedback-and-revisit-conditions).
Their promotion closes deferral only. DF-014 remains with AGENT-002;
PARSE-004's completed first-parse CLI authoring facts remain outside this work.

## Verified context

Core now owns `ItemTypeV1::wire_name` for Config's delegation package and
Engine's read projections, and the shared receipt and package/receipt result
byte bounds. Package static reserves retain their fixed charges, with a
source-bound emitted-metadata proof and the actual-byte backstop. Permanent
ownership and regression references live in architecture, testing and
requirements traceability.
Store receipt admission repeats Core Role, round and disposition predicates.

Protocol's `DelegationAcceptV1` carries a JSON Value; CLI constructs it and
Engine `direct_delegation` repeatedly extracts string-keyed fields.
Store's receipt snapshot fast path searches canonical placement JSON for a
delegation key. Config authoring admission and Store persisted admission remain
distinct trust boundaries; their validation must not be collapsed.

Engine `workspace` and the token-backed read in `direct_execution` repeat UUID
comparison. `native_execution` combines pure item/revision/attempt selection
with native artifact preparation and stamp validation. Its hash observer covers
preparation streams, not every potential read in stamp verification.
Engine's citation precheck shares Core's numeric bound but repeats its diagnostic.

These are ownership and regression-protection improvements. The observations
do not establish a current public-result defect, unqualified native read path,
performance regression, live adapter or missing production capability.

## Goals, exclusions and compatibility

Give each selected value, predicate, extraction and pure preparation one owner
at its existing layer while retaining independent admission at protocol,
persistence and filesystem boundaries. Verify the actual changed behavior and
preserve applicable existing evidence.

Preserve public command grammar, JSON shapes and contract generations, accepted
numeric/string domains, bounds, canonical bytes, digest inputs, error codes,
details and precedence, atomic rollback, absent rejected outcomes and exact
replay. Preserve SQLite layout, writer/manifest contracts and supported Store
continuity. No schema reservation, generation bump or canonical asset rewrite is
planned merely to move Rust ownership.

Core owns pure domain values, shared domain bounds and validation results.
Config owns parsed-Procedure facts and source admission. Protocol owns typed
public-input extraction. Store owns closed persisted admission, codecs and SQL.
Engine owns pure preparation/composition and native filesystem coordination.
CLI owns grammar, public invocation and presentation.

No reverse dependency, Store-to-Config edge, provider execution, new adapter,
parallel cursor, Graph feature, global registry or installed-runtime change is
included. No latency, throughput, allocation-count or benchmark acceptance gate
is introduced. Unrelated findings remain outside TYPED.

## Task contracts

### TYPED-001

**Consolidate wire names and delegation bounds — DF-002, DF-003, DF-011.**

Give ItemType wire names one Core-owned representation consumed by both
projections. Give genuinely shared receipt and delegation-result bounds a Core
owner; retain layer-specific budgets at their existing boundary. Keep all
numeric limits and seven existing names unchanged.

Derive package reserve charges from existing source bounds where available;
otherwise assert their worst-case relationship against those bounds. Cover
escaped JSON, goal/criterion and evidence/item metadata. Do not widen unrelated
Core APIs solely to expose an implementation detail or replace the actual-byte
backstop.

Acceptance: exhaustive wire-name agreement; worst-case reserve coverage;
admission and actual emission at/beyond bounds; decoder, Store DDL and schema
agreement; unchanged canonical package bytes and digest. Use existing Core,
Config package, Protocol result-contract and Store receipt suites.
Promote ownership to System Architecture and the Rust codebase guide, and record
the actual regression owners in testing guidance and requirements traceability.

### TYPED-002

**Type delegation acceptance and reuse domain predicates —
DF-005, DF-012, DF-013, DF-016, DF-017. Depends on TYPED-001.**

Expose one Config-owned parsed-Procedure delegation predicate for its consumers.
Keep Store detection structural over admitted persisted/runtime facts instead
of substring matching; add no Store-to-Config dependency. Preserve Direct and
delegated result generations, including a Direct decision option named
`delegation`.

Reuse Core Role constructors, named round bounds and after-accept predicates in
receipt validation while preserving Store's own closed-shape, persisted-value
and input-to-receipt checks. Extract a typed acceptance carrier at the existing
Protocol boundary and use it through CLI and Engine instead of repeated field
lookups. Retain the exact accepted input representation needed for request
canonicalization, digest and replay; introduce no independent competing copy.
Keep validation at its current observable stage and preserve malformed-input,
fence, lifecycle and binding-error precedence.

Acceptance: malformed/unknown fields and invalid Role/round/disposition inputs;
closed wire and unchanged request identity; cold persisted corruption refusal;
Direct/delegated package, acceptance and receipt behavior; exact retry/conflict.
Extend Engine `int_v2dfc006_delegation_ports` with decision-current package and
acceptance rejection, exact code/reason and unchanged session/attempt.
Add actual non-JSON CLI package, acceptance and receipt success cases that
assert meaningful results and absence of error output. Reuse generic renderer
coverage rather than inventing a delegation-specific renderer.
Promote the typed/admission boundary and regression owners to architecture,
supervised-delegation/transaction specifications where affected, testing and
requirements traceability.

### TYPED-003

**Unify workspace identity comparisons — DF-022.**

Give the UUID comparison and typed mismatch construction one Engine owner.
Call it from the ordinary resolver and token-backed read at their existing
positions. Do not merge native revalidation, open another Store, move token
validation or change replay/fence priority.

Acceptance: ordinary and token-backed success/mismatch, stale or wrong token,
competing corruption/configuration errors, unchanged public code/details and
existing ordering. Extend the current Engine workspace/read suites.
Promote the comparison owner and intentional call-site ordering to architecture,
affected automation/error specifications, testing and requirements traceability.

### TYPED-004

**Separate artifact preparation and verify writer reads — DF-024, DF-027.**

Give Engine's pure preparation layer one projection of required item, revision,
active attempt and recorded artifact metadata. Native preparation consumes those
facts and owns file access, descriptor witnesses and stamp revalidation.
Preserve typed error mapping, required-set comparisons and replay ordering.
Keep artifact hashing outside the write transaction and current-state/stamp
revalidation inside it; do not relax native filesystem checks.

Instrument the actual file-byte read boundary reachable during native stamp
verification, not just the preparation hasher. Keep instrumentation test-only.
Exercise attach, record, complete and decide: no artifact bytes may be read while
their writer is admitted. Use the existing native/race fixtures to prove that a
separate writer can commit during actual preparation. Preserve deterministic
same-byte rename/replacement rejection and owned child/fixture cleanup.

Acceptance also covers changed required sets, attempts and item revisions,
missing or changed artifacts, typed failures and replay. Reuse Engine
`int_v2dfc006_races` and direct execution tests; register any additions through
the existing aggregate suites. Promote pure/native ownership and actual proof
limits to architecture, transaction/automation guidance, testing and traceability.

### TYPED-005

**Centralize bounded citation diagnostics — DF-025.**

Expose one Core-owned bounded validation result and consume it in Engine's early
citation precheck. Retain rejection before oversized cloning or quadratic
duplicate work. Keep per-field decoding, combined-count, duplicate, status and
live-fence checks at their established stages; preserve single/batch command
diagnostics and error order. Do not move preparation into the writer.

Acceptance: zero, maximum and over-limit combined citations; oversized individual
arrays; duplicate/invalid citations; competing stale fences; unchanged typed
details, rollback and replay. Extend Core goal-record and existing Engine
single/batch goal assessment suites. Promote ownership and intentional early
admission boundaries to architecture, affected contracts, testing and traceability.

### TYPED-006

**Qualify integration and prepare durable closeout. Depends on TYPED-001..005.**

Reconcile each adopted DF with its actual implementation, original revisit
condition, regression owner and durable documentation. Verify mixed boundary
behavior, both supported contract generations, success-only outcomes, exact
replay and supported Store continuation. Preserve independent source, protocol,
persistence and native admission. Run only still-due integrated checks and the
required development gate; reuse applicable exact-candidate evidence.

Finish lasting documentation and prepare the exact dossier-reference/deletion
inventory. This Task delivers qualification and closeout preparation; it does
not declare whole-Epic validation or Master's final acceptance complete.
No substantive Task may defer its own checks or documentation to this Task.

## Verification and failure handling

Each Task establishes its affected baseline and focused success, failure and
compatibility checks before editing. Use repository Make targets, registered
`int_suite` modules and the configured Gaori routes for long/noisy checks.
Executable changes require `make test` before sharing a development revision.
Do not repeat applicable gates without a changed candidate, failure or gap.

Preserve failed evidence and unknown outcomes. A mismatch with an accepted
contract is an in-scope conformance failure, not permission to update expected
bytes or waive admission. Any proposed public/persisted/architectural change
requires a revised approved scope before implementation. Insufficient native
instrumentation leaves DF-027 unverified; passing the preparation observer alone
cannot close it. Generic compilation or a synthetic receipt is not native CLI,
filesystem or Store-continuation proof.

No `make dist`, release QA, live provider, installation or runtime activation
is selected. The future explicitly invoked Epic Handler owns its execution
envelope, sequential Task delivery, independent whole-Epic validation and actual
final owner acceptance under the installed workflow contract.

## Durable documentation and retirement

| Durable information | Permanent owner |
|---|---|
| Original twelve sources, findings, deferral reasons and revisit conditions | Testing guide, TYPED source feedback section; already retained at adoption |
| Actual crate/type/predicate/preparation owners and call-site ordering | System Architecture and Rust codebase guide, updated by each implementation Task |
| Observable admission, error, canonicalization, replay and filesystem obligations | Existing supervised-delegation, JSON, transaction, automation and error specifications where affected |
| Executable regression owners, scenarios, applicability and limits | Testing guide and requirements traceability, updated by each owning Task |
| Task order, lifecycle, acceptance and final delivered summary | Roadmap alone |

Keep this sole-consumer dossier while TYPED is unfinished. At final closeout,
after independent whole-Epic validation and Master's acceptance of the exact
closeout candidate, first ensure every lasting statement above has a permanent
owner. Replace all roadmap and documentation links with the narrowest canonical
outcomes, remove the adopted-dossier index entry, and delete this file.
Do not copy it to an archive or delete runtime recovery evidence. No executable
evidence may depend on this temporary path. If unique durable information or an
unreconciled reference remains, retirement is incomplete.

## References

- [Documentation authorities](../README.md)
- [TODO lifecycle](README.md#lifecycle)
- [TYPED roadmap](../roadmap/README.md#typed-typed-boundary-ownership)
- [Permanent original feedback](../implementation-tips/testing.md#typed-source-feedback-and-revisit-conditions)
- [System Architecture](../architecture/system.md)
- [Rust codebase](../architecture/rust-codebase.md)
- [Repository dependencies](../architecture/repository-structure.md)
- [Supervised delegation](../specs/interfaces/supervised-delegation.md)
- [Transaction contract](../specs/storage/transactions-concurrency-and-idempotency.md)
- [Requirements traceability](../specs/quality/requirements-traceability.md)
