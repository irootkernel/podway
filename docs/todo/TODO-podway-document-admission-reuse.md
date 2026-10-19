# Podway Document Admission Reuse

## Status and authority

Status: Adopted. Owning epic: `PARSE`. Design registered on 2026-10-19. The
[roadmap](../roadmap/README.md#parse-document-admission-reuse) owns task order and
status. Accepted VIEWS completion is the prerequisite; accept PARSE before STATE
starts. This dossier defines intended work and does not change existing public
or persistence contracts. No release is selected.

## Verified context and evidence

The source baseline is `98796e6a20467d1d072bd84f377fd9ddccccab93` on
2026-10-19. The current paths contain distinct checks as well as duplicate work:

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
