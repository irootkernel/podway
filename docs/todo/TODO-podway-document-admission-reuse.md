# Podway Document Admission Reuse

## Status and authority

Status: Adopted. Owning epic: `PARSE`. Design registered on 2026-10-19. The
[roadmap](../roadmap/README.md#parse-document-admission-reuse) owns task order and
status. Accepted VIEWS completion is the prerequisite; accept PARSE before STATE
starts. This dossier defines intended work and does not change existing public
or persistence contracts. No release is selected.

## Verified context and evidence

The PARSE-001 source baseline is `ad735f4a17259131b21abb939508ebcaf81f0014` on
2026-10-20, after accepted VIEWS completion. At that baseline, the paths contain
distinct checks as well as duplicate work:

- [`CanonicalExecutionJsonV1`](../../crates/podway-store/src/lib.rs) parses a
  string for bounded admission; Core's
  [canonical verifier](../../crates/podway-core/src/canonical.rs) parses again.
  Store's [outcome reader](../../crates/podway-store/src/direct_store/transaction.rs)
  then parses for result handling and success validation: four parses per outcome
  read in total. These checks have different responsibilities even where they
  consume the same document.
- Protocol's [direct envelope](../../crates/podway-protocol/src/direct_envelope.rs)
  and [result contract](../../crates/podway-protocol/src/result_contract.rs)
  perform ownership conversions, cloning and serialization for validation.
  Engine's direct read service already moves its result object; that eliminated
  clone is not new work.
- CLI's [authoring owner](../../crates/podway-cli/src/command/authoring.rs) runs preview
  then reparses the Procedure to select delegated result generation. Config's
  [preview report](../../crates/podway-config/src/procedure_v2_preview.rs) exposes
  details only after later validation succeeds. This is deferred finding DF-006.

Do not conflate parse reuse with removing canonical, schema, depth, size or
relational validation. Each admitted representation must still satisfy its own
trust boundary.

### PARSE-001 responsibility and removal ledger

This ledger records the prechange behavior for the later implementation tasks;
it does not claim that reuse is implemented.

| Owner and path | Eligible duplicate work | Retained responsibility |
|---|---|---|
| Core `canonical.rs::verify_canonical_json_v1` | Reparse when Store already has the operation's generic JSON value. | Pure canonical encoding, integer admission, UTF-8 key-byte ordering and exact comparison against the original bytes. A normalized value alone does not prove canonical input. |
| Store `lib.rs::CanonicalExecutionJsonV1::new` | Its Core verifier's second parse; expose the admitted value only through a private operation-local helper. | Empty/byte/syntax/depth/canonical admission and original `String` ownership; public fresh-write API stays validated. |
| Store `direct_store/transaction.rs::read_outcome` and `validate_success` | Third parse for response UUID extraction and fourth parse for Store success checks. | SQL/row/sequence/time/scope admission, request identity and independent Store envelope correlations; no Protocol type or dependency. |
| Protocol `direct_envelope.rs::OutputEnvelopeV4::new` | Rebuilding already-owned result and warning trees through `to_value(input)`. | Complete envelope admission through `from_value`, including command-bound schema and bounds. |
| Protocol `result_contract.rs` | Whole-result/version clones to create temporary `Value` objects, nested package clone for inspection, and serialized buffers used only for their length. | Existing result, delegation, depth, schema, size and relational checks, in their existing order; reuse `serialized_json_len_v1`. |
| Config preview and CLI `command/authoring.rs` | CLI's second source parse and local delegated-placement selector. | Config-owned ordered facts from the first parse, including semantic/vet failures; malformed source retains the plain preview fallback. |

`direct_read_service.rs::validate_projection` already moves its owned object into
and out of `Value`; this is not new PARSE work. Adapt immediate callers as needed
to borrow or move result values. Do not expand that adaptation into broad Engine
or CLI renderer cleanup. Postcommit Engine response decoding and complete
Protocol output admission are independent checks and remain.

### Failure-stage baseline

- Canonical wrapper admission is empty input, byte length, JSON syntax, Store
  depth, then canonical-byte equality. The Store depth starts at zero and allows
  64. Parseable depth 65 plus noncanonical whitespace returns
  `CanonicalExecutionJsonDepthExceeded` before canonical refusal. Parser recursion
  refusal remains `InvalidCanonicalExecutionJson`; oversized malformed input
  remains `ValueTooLong`. Exact re-encoding rejects duplicate keys, alternate
  escapes, whitespace, key-order changes and unsupported numeric forms.
- Outcome reads first decode the bounded SQL row, then admit sequence against the
  committed sequence, nonnegative timestamp and scope/session pairing. Wrapper
  admission follows; then response UUID extraction, digest/request admission and
  Store success validation. A SQL response over 1 MiB becomes NULL and fails its
  `String` row decode as `DirectStoreErrorV1::Sqlite`; this precedes wrapper
  admission and differs from later `Unreadable` mappings. The later sequence-zero
  guard is already unreachable after sequence admission and is outside this task's
  removal target.
- Store success validation permits only the existing success members and checks
  output schema, original request UUID, row command, generated-at string, result
  object and warnings array. This is deliberately less than full Protocol output
  admission. Mutation replay still follows Store/file/writer identity admission
  and outcome validation, before execution callback or clock; it returns the
  retained original canonical string.
- Protocol generic document admission checks discriminator, depth, byte size,
  selected schema and recovery correlation in that order. Result-map admission
  starts depth at one, then checks selected schema and result correlations.
  Delegation document admission checks selected schema before size, then its
  document-specific correlations. These separate orders survive ownership changes.
- Config preview parses, validates, vets, lints and projects. Parse or semantic
  failure stops before vet/lint; vet failure still retains lint diagnostics and
  withholds success details. CLI currently selects delegated preview generation
  from its separate successful parse even after semantic failure. First-parse
  facts must therefore live outside success-only details and preserve authored
  placement order.

The canonicalization machine asset currently says `signed-integers-only`, while
Core and its integer-edge tests admit signed and unsigned integers. This existing
conformance discrepancy is not resolved by PARSE. The owner approved preserving
current bytes and behavior here and reconciling the discrepancy separately; do
not change canonical assets, writer identity or numeric acceptance in this epic.

### Baseline regression owners

- Core canonical unit tests own encoding, ordering, escaping and integer edges.
  Store `int_v2dfc008_persistence` owns wrapper byte/depth bounds and PARSE-001's
  exact failure classifications, combined faults and retained input bytes.
- Store `int_v2dfc005_direct_store` owns persisted outcome corruption, success
  validation/rollback and replay before callback/clock/new UUID. Its existing
  outcome equality includes the original canonical string; no additional replay
  copy/count framework is needed.
- Protocol `int_v2dfc006_direct_envelopes` owns complete constructor/decoder
  admission. `int_v2_result_contract` owns registered route families, deep/closed
  failures, version identity, delegation size and identity correlations.
- Config `int_v2_procedure_preview` owns parse/semantic stopping, vet/lint ordering,
  projection overflow and YAML/JSON equivalence. CLI
  `e2e_v2dfc007_local_cli::v2sdg006_delegated_preview_and_graph_select_v2_without_changing_direct_results`
  owns delegated success/vet failure plus PARSE-001's semantic-failure, malformed
  fallback and authored-order characterizations.
- Engine `int_v2dfc006_direct_execution` owns original-response lookup without
  writes, old-attempt replay, clock/ID refusal and file-loss replay. Real CLI
  retained-command and postcommit SIGKILL cases remain the integration owners.

## Goals, non-goals, and scope

Reuse one parsed representation within one admission operation, remove avoidable
response copies and carry first-parse preview facts to their existing consumer.
Preserve exact canonical/stored bytes and the original error order.

No public envelope, schema, error, command, Store layout, writer identity or
compatibility change is adopted. No global cache, unchecked constructor or
cross-boundary validation bypass is included. No elapsed-time tests, latency
percentiles, throughput benchmarks, performance baselines or thresholds are
allowed in this work.

Only DF-006 is promoted here. Delegation predicate consolidation (DF-005),
workspace UUID comparison cleanup (DF-022), artifact responsibility (DF-024) and
artifact replay preparation (DF-026) are not silently absorbed.

## Accepted design decisions and interfaces

### Store outcome admission

Parse one stored outcome document into a private operation-local representation
and let the existing checks inspect that value. Retain the original canonical
string as the authoritative replay bytes rather than serializing the parsed
value to recreate a response. Within canonical-wrapper admission, keep
empty-input refusal, byte-length refusal, syntax parsing, Store depth validation
and canonical-byte comparison in that order. Keep the surrounding outcome-row,
scope, request and relational checks at their original stages, including those
that precede wrapper admission.
A parseable noncanonical document deeper than the Store limit must still return
the depth error before the canonical error; the parser's own recursion refusal
keeps its existing classification.

If Core needs a value-based canonical verification helper, it remains pure and
verifies the same byte representation against the admitted value. It must not
claim that a normalized JSON value proves the original input was canonical.
Reject duplicate keys, whitespace or ordering deviations, disallowed numeric
forms and malformed nesting just as before. Preserve integer-only number rules
and UTF-8 byte ordering of object keys. Store owns persistence admission; no
infrastructure moves into Core and no public unchecked admission route is added.
The acceptance target is stored-outcome read admission. Fresh writes retain
their current admission path; they may use the same pure lower-level helper
without a separate mutation-result redesign or retained AST on every wrapper.

### Response ownership

Pass borrowed values or move owned values through workspace-internal envelope/result
construction where their lifetime permits it. Remove unnecessary whole-result
clones and conversions used only to inspect an already available object. Count
serialized size through VIEWS' Protocol-owned exact-length helper rather than
introducing another counting writer. Workspace-internal Rust signatures may
change to accept borrowed objects or owned values; prefer adapting existing
functions over adding parallel validation variants. CLI/JSON/error contracts
remain unchanged.

Keep each independent schema, compatibility, bound and output-decoding check.
Postcommit Engine validation remains required; removing reparsing after commit
is not an acceptance target of this epic. Store success validation is not full
Protocol output admission. Store may share its parsed generic JSON internally,
but must not retain a Protocol envelope or gain a Store-to-Protocol dependency.
Preserve BOUND's typed failure transport and avoid paired error side channels.

### Preview facts

Carry delegation-generation facts from the first successful Procedure parse in
the internal preview report. Keep them available even if semantic validation or
vetting subsequently fails; placing them only inside success-only `details`
would change parsed-but-invalid delegated preview behavior.

Config owns ordered delegated-placement IDs derived from the parsed Procedure.
Move the existing CLI authoring predicate to that owner and reuse it for graph
and preview authoring rather than copying it into the report. Retain first-parse
facts through Config's failed-admission path as well as its successful preview
path; no raw AST must be retained solely for a later CLI reparse. This bounded
relocation is part of PARSE-004; cross-layer Engine/Store consolidation remains
separate DF-005 work.

The CLI selects the same result generation from these facts without parsing
again. A malformed source retains the existing fallback. Plain, delegated,
semantically invalid and vet-invalid inputs retain their existing result fields,
diagnostics and ordering. The report may change internally; CLI grammar and
public JSON do not. DF-006's deferral is closed by promotion under the register's
lifecycle rule; PARSE-004 alone tracks implementation acceptance. Promotion is
not evidence that the duplicate parse is fixed.

## Failure handling and compatibility boundaries

Preserve source versus persisted admission responsibilities. Stored Procedure
reuse must still verify canonical bytes and the stored digest before using its
runtime declarations; memory declarations alone do not establish stored identity.
This epic does not adopt a separate execution-parser ownership redesign.

No rejection records an outcome. Preserve atomic success outcomes, original
request identity, byte-identical old-attempt replay, error codes/details/exit and
retryability, clock/ID order and unsupported writer/Store refusal. Do not replace
eager stored admission with output-window validation. Retain bounded input,
depth, output and collections without changing packaged manifest inputs.

## Roadmap ownership, dependencies, and traceability

| Task | Deliverable and focused verification |
|---|---|
| `PARSE-001` | Inventory parses, ownership transfers and independent validators; pin failure precedence for malformed, noncanonical, oversized and relationally invalid documents. |
| `PARSE-002` | Share the Store outcome parse; verify malformed/deep/noncanonical values, combined depth/canonical faults, duplicate keys and numeric forms, schema/result mismatches, exact stored bytes and lookup/replay identity. |
| `PARSE-003` | Remove response copies at identified owners; verify complete envelopes, schema/bound refusals and postcommit output admission without weakening independent checks. |
| `PARSE-004` | Give authoring delegation facts one Config owner and carry them through preview success/failure; verify malformed fallback plus plain/delegated semantic and vet failures, then record actual implementation acceptance separately from DF-006 promotion. |
| `PARSE-005` | Integrate canonical-byte/error/replay evidence, required development gate and independent review; promote lasting documentation and retire the dossier after acceptance. |

The roadmap alone records status. Each task includes its focused verification;
PARSE-005 does not defer testing of the earlier tasks.

## Verification and release acceptance

Use existing canonicalization, Store outcome/replay, Protocol schema/size and CLI
preview suites. Add a regression only for a material gap created by changed
ownership or reuse. Verify the same original bytes rather than equivalent
reserialization. Where useful, a narrow deterministic parse-reuse assertion may
support the source-level proof; no broad counter framework is required.

Future executable changes require focused checks and `make test` before sharing.
Acceptance requires all independent checks and their failure order to survive,
the targeted duplicate parses/copies to be removed and preview fallback to remain
equivalent. Performance measurements and historical benchmark baselines are not
acceptance requirements. No release or installed-runtime qualification is selected.

At epic closeout promote durable ownership and verification guidance under the
[TODO lifecycle](README.md#lifecycle).

## References

- [Shared program boundaries](../roadmap/README.md#shared-preservation-boundaries)
- [Deferred feedback](../deferred-feedback/README.md)
- [Transaction and replay contract](../specs/storage/transactions-concurrency-and-idempotency.md)
- [JSON contract](../specs/interfaces/json-contract.md)
- [Structural verification](../implementation-tips/testing.md#structural-simplification-verification)
