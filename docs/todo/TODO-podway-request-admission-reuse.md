# Podway Request Admission Reuse

## Status and authority

Status: Adopted. Owning epic: `ADMIT`. Design registered on 2026-10-19. The
[roadmap](../roadmap/README.md#admit-request-admission-reuse) owns order and status.
Accepted STATE completion is required before this epic starts. Current ADRs,
canonical assets and specifications retain authority over implemented behavior.
This design adopts invocation-local reuse. Workspace-internal Rust integration
may change; CLI grammar, JSON, storage and release contracts remain unchanged.

## Verified context and evidence

The source baseline is `98796e6a20467d1d072bd84f377fd9ddccccab93` on
2026-10-19:

- The [workspace resolver](../../crates/podway-engine/src/workspace.rs) performs
  Git/config checks, a binding read, expected-UUID comparison and bound Git
  revalidation. The later [Direct read](../../crates/podway-engine/src/direct_execution.rs)
  opens another Store read, revalidates bound Git and then obtains its clock/view.
- Store's [graph preparation](../../crates/podway-store/src/direct_store/connection.rs)
  independently admits the current graph and obtains generation/sequence facts.
  Mutation paths already separate read preparation, artifact work and the
  authoritative writer transaction.
- READS preflight facts retain the full Standard status projection/schema and
  envelope-equivalent failure checks. Their guards are released before actual
  mutation preparation; there is a real competing-writer interval. CLI
  `Invocation::preflight_facts` and `Invocation::execute` make separate Engine
  requests with distinct request IDs within one CLI invocation.
- Store already owns `DirectPreparedGraphV1` and its writer-side
  `use_prepared_graph` admission. Sequence/session stamps can trigger reload;
  generation, manifest, workspace or root mismatch returns `BindingMismatch`.
- [Single-item attachment](../../crates/podway-engine/src/direct_items.rs) hashes
  the artifact after binding resolution but before graph preparation. Blindly
  fusing these stages would hold a lease over hashing or move error precedence.

Existing [race checks](../../crates/podway-engine/tests/int_v2dfc006_races.rs) and
READS preflight checks are correctness owners. Separate start/reset observations
are intentionally distinct and are not duplication this epic may erase.

## Goals, non-goals, and scope

Reuse eligible binding/first-snapshot admission and an immutable preflight graph
within one CLI invocation while preserving its distinct Engine requests and
every authoritative revalidation boundary. Reduce repeated acquisition and
preparation. External CLI call sequences remain unchanged.

No new CLI flags/commands, JSON schema, output field, persistent/global cache,
Store layout, writer identity, migration, runtime service or consumer change is
adopted. Do not merge separate start/reset observations or hold leases across
mutation artifact hashing or prompts. No elapsed-time tests, latency percentiles,
throughput benchmarks, performance baselines or thresholds are allowed.

## Accepted design decisions and interfaces

### Binding and first snapshot

Compose binding admission and the eligible first operation through an internal
typed operation/callback under the existing Engine/Store owners. Reuse one
qualified read where it preserves the command's resolver wrapping and complete
order: native Git/config discovery, binding checks, expected UUID, bound Git
revalidation and operation-specific clock/view checks.

Do not remove transaction qualification, no-SHM checks, file witnesses or writer
contract classification because the binding was read earlier. Preserve each
command's existing native identity checks at their existing stages; merging those
checks is not an acceptance target. Engine composes this work; CLI must not
access Store internals and Core gains no filesystem or connection responsibility.

Apply composition per eligible path. For attachment paths with hashing between
binding and graph preparation, keep the split and its error order rather than
opening the later snapshot early or retaining a read lease over hashing. A
structural optimization need not eliminate every read from every command.

Mutations still need the admitted workspace UUID to form their canonical request
digest, and binding/graph facts to prepare required artifacts, before entering
the writer transaction. Do not defer those prerequisites into the writer merely
because read-only commands can compose their first read. Preserve existing
input-bound and idempotency-key checks before later preparation. Do not remove
per-artifact native identity checks as an incidental optimization.

### Preflight and mutation preparation

Use one opaque Engine-owned invocation context, owned by the CLI's existing
`Invocation`, to carry an optional prepared graph between the two in-process
Engine calls. The context retains at most one immutable Store-prepared graph,
contains no live read/writer guard, and is discarded after the eligible mutation
or invocation ends. Keep `DirectPreflightFactsV1` and wire envelopes unchanged;
CLI carries the opaque context without inspecting Store types or stamps. Reuse
Store's existing stamp/admission owner rather than adding a parallel token model.
Stateless Engine callers retain their existing path without this optimization.

Preserve the preflight and mutation request IDs, error attribution, caller-key
surface, deadline and fence-autofill order independently. Standard projection,
schema and envelope-equivalent checks still run before returning facts. Build
the retained Store-prepared graph only from values successful preflight admission
already produced: the qualified writer-contract generation, binding, admitted
graph and view's committed sequence, plus Store-owned root and manifest. Retain
these values in Store-owned read-admission metadata rather than querying them
again. Assemble the optional context after facts succeed, without a second graph
load or any additional SQL statement, decode or validation stage. If a stamp is
unavailable, return facts without retained state; optional retention introduces
no new failure before the CLI's existing input/key checks. Facts-only extraction
is not equivalent preflight: all existing checks remain mandatory.

Release all preflight guards before mutation preparation. Consume the retained
graph only at the existing `prepare_graph` stage, under that stage's current
qualified read, lease, binding and native file revalidation. Store compares its
existing stamps: root, contract manifest digest, generation ID, workspace UUID,
committed sequence, session ID/revision, workspace revision, Procedure snapshot
ID and Procedure digest. Reuse the graph body only if every comparison matches;
stamp the result as prepared at this current read stage.

Any optional-hint mismatch, including identity mismatch or a failed comparison
stamp read/decode, discards the hint and uses today's ordinary cold
`prepare_graph` path. Hint comparison itself never reports a public error or
reclassifies the error the existing graph loader would report. The hint alone
cannot introduce `BindingMismatch` earlier than today. This fallback applies
only to optional hint comparison, once; it does not catch, normalize or retry
authoritative read/binding/native refusal or failure from the cold path. Those
failures keep their existing error and precedence. Stale caller fences are never
silently retried.

Artifact preparation uses the graph admitted at this preparation stage, never
an unchecked preflight graph. Only the resulting current Store-prepared value
can enter the writer transaction's existing `use_prepared_graph` mechanism.
There, sequence/session/Procedure-stamp changes retain the existing reload
behavior; generation/manifest/workspace/root mismatch still returns
`BindingMismatch`. Preserve replay before that check. This keeps the writer's
prepared-state identity window at preparation-to-write rather than widening it
to preflight-to-write. No read lease spans artifact preparation or the gap.

Preserve replay lookup and request identity precedence, including replay before
new clock/ID allocation or state changes. The unchanged response is the original
stored response, not a fresh projection. Artifact bytes remain outside writer
transactions. A hint must not extend a read lease across preparation work,
prompts or the real competing-writer interval.

No cross-invocation token registry, hidden retry loop or long-lived cache is needed.
Do not reintroduce BOUND's removed error side channels. Existing facts and graph
preparation owners remain responsible for their independent failure mapping.

## Failure handling and compatibility boundaries

Preserve expected workspace UUID comparison, bound Git/worktree identity,
descriptor/file replacement detection, unsupported legacy/newer/foreign-writer
refusal, hot-WAL and missing-SHM read-only qualification and maintenance deadlines.
Do not infer the active worktree from a project registry or scan other projects.

Keep full eager stored validation, including hidden historical values, and the
archive-list summary-only boundary. Retain exact error codes/details/exit and
retryability, failure precedence, stale-fence behavior, clocks/IDs, success-only
atomic outcomes and byte-identical old-attempt replay. Separate terminal-start
reset dry-run and interactive post-decision observations remain unchanged.

If combining two stages cannot preserve these properties, retain their split.
Changing their meaning requires separate adoption, potentially through the
[CALLS candidate](TODO-podway-cli-interaction-simplification.md), not an ADMIT
shortcut.

## Roadmap ownership, dependencies, and traceability

| Task | Deliverable and focused verification |
|---|---|
| `ADMIT-001` | Map per-command native/binding/graph/preflight/artifact/writer order, distinct Engine requests, identity/digest prerequisites, resolver error wrappers and guard lifetimes; identify eligible and intentionally split paths. |
| `ADMIT-002` | Compose eligible binding/first-operation reads; verify UUID/native identity priority, clock/view failures, no-SHM/hot-WAL behavior and retained attachment hash boundaries. |
| `ADMIT-003` | Carry one Store-prepared graph in an opaque Engine invocation context using already admitted metadata; verify no new preflight failure stage, cold fallback on hint mismatch or comparison failure, and unchanged writer refusal/reload, request IDs and Standard preflight failures. |
| `ADMIT-004` | Integrate real competing-writer, file replacement, unsupported-state refusal, continuity and exact replay evidence, required development gate and independent review; promote documentation and retire program-planning text in both indexes and testing guidance. |

The roadmap alone owns task status. Each task verifies its own success and
failure paths; ADMIT-004 integrates rather than postpones that verification.

## Verification and release acceptance

Use existing native identity, Store admission, race, typed preflight and replay
cases. Verify guard release with deterministic synchronization and an actual
competing writer, without elapsed-time comparisons. Exercise matching reuse,
changed revision/session/snapshot, and replaced Store/file witness as distinct
cases. Require hint discard at preparation or authoritative refusal/reload at its
existing boundary, with no failed outcome or unauthorized state change. Include
replacement between preflight and preparation and between preparation and writer
as separate cases; the optional hint must not widen the latter refusal window.
Verify separate preflight/mutation request IDs, unchanged stdin-owned/no-preflight
paths, and absence of retained context state in later invocations.

Verify that optional retention adds no preflight SQL/decode/validation stage and
does not pre-empt later CLI input/key errors. Pair an optional comparison failure
with a graph-loading fault and require the existing cold-path error and order;
separately prove that mandatory admission errors still propagate without retry.
Use the existing deterministic admission/query fixtures where applicable, without
new timing instrumentation or a general measurement framework.

Check combined-fault precedence as well as success. Preserve old-attempt replay,
replay after continuation, and supported Store identity without reset/migration
using isolated fixtures. No user worktree or installed runtime is a test target.
Future executable changes require focused checks and `make test` before sharing;
historical latency/scale qualification is not inherited as a new gate.

No release or installed-runtime operation is selected. Promote durable boundaries
and verification guidance before [dossier retirement](README.md#lifecycle), and
replace the program's remaining planning guidance with implemented ownership.

## References

- [Shared program boundaries](../roadmap/README.md#shared-preservation-boundaries)
- [System architecture](../architecture/system.md)
- [Transaction contract](../specs/storage/transactions-concurrency-and-idempotency.md)
- [Supported Store continuity](../implementation-tips/testing.md#supported-store-continuity)
- [Structural verification](../implementation-tips/testing.md#structural-simplification-verification)
