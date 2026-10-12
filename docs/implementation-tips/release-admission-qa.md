# Direct Mutation Outcome QA

Use this independent scenario when a release delta changes mutation commit,
response validation, idempotency, or reconciliation. Exercise the actual typed
Direct Engine, native Git resolution, and real Git-backed Store. A call to an
error normalizer, a fabricated outcome, or an in-memory projection does not
establish the behavior.

## Execution boundary

Use the assigned fixture beneath the release-QA pass's physical evidence root.
Build the exact clean candidate's production libraries offline with the pinned
Rust toolchain, `--locked`, and release profile, with build output and caches
inside that root. Author a bounded non-test executable against those libraries;
do not run or copy regression tests as independent release-QA evidence.

Invoke `podway_engine::direct_api::execute_direct_v1` with
`DirectWorkspaceResolverV1<NativeGitResolverV1>`, `NativeDirectWorkspaceV1` as the
Procedure provider and artifact verifier, and the current direct Store. Use
standalone Git repositories beneath the assigned root. Requests enter
through the public typed Direct API with actual command payloads, explicit
workspace/session/attempt/item fences where required, stable idempotency keys,
and pinned canonical preset digests. No daemon, registry, account namespace,
renderer sealer, or transport adapter participates.

Do not add a shipped failpoint or edit a database, outcome, marker, or runtime
link. Bound each child and record its controlled environment. All observed
responses must pass the current full `output/v4` or `error/v2` validation.

## Independent process cases

1. Initialize a fresh worktree and start the canonical `small-change-v2` preset.
   Validate the complete returned success envelope. Through a new Engine/read
   transaction, look up its idempotency key and require the original committed
   success, including command, request identity, timestamps, result, and warnings.
   Compare the real prepared session and workspace revision with that success.
2. Begin the session, then submit a genuinely stale fence and a domain-invalid
   mutation with distinct keys. Require their documented typed errors, unchanged
   graph/item revisions and evidence, and not-found for both outcome lookups.
   Neither rejection records an outcome or committed sequence. A stale fence
   rejects before reading the mutation clock; domain evaluation may read it once
   after valid fences and prepared-input checks, then roll back without an effect.
   If clock evidence is required, use the existing injected clock dependency
   rather than a product flag. Submit the corrected operation with current fences
   and a new key; require exactly one state effect and one validated success outcome.
3. Perform a normal mutation with another stable key and deliberately discard its
   returned success at the caller boundary. Close that process before the caller
   records an acknowledgement. In a new process, use read-only `outcome.lookup`
   and then replay the identical original payload and preconditions with a new
   invocation request ID. Require the entire original success envelope, including
   its original request ID, and no added effect, outcome, revision, or clock read.
   This proves caller response loss and reconciliation, not physical COMMIT I/O
   failure or `MUTATION_OUTCOME_UNKNOWN`.
4. As a maintenance control, execute explicitly confirmed reset-all with a stable
   key. Require its new workspace UUID and empty current/archive state, while
   configuration, Procedure files, Git metadata and unrelated files survive.
   Replay the exact reset with its original fences and key; require the original
   response even though the old UUID is no longer current. This normal control
   does not claim a maintenance crash or COMMIT-failure trigger.

Drop every Engine and Store handle between the original operation and cold
reconciliation. A fresh process must recover the same durable success through
production reads, not a retained in-memory object. Session-scoped retention ends
on session/worktree reset; perform session outcome comparisons before that reset.
For workspace outcomes, respect the documented 100-entry/30-day boundary.

`MUTATION_OUTCOME_UNKNOWN` is reserved for an actual I/O failure during COMMIT.
If a frozen release matrix requires that case, use an independently established
safe production trigger or report `INCOMPLETE`. Neither discarding a response nor
constructing that error yourself is evidence for this ambiguity.

## Required evidence and cleanup

Record exact candidate/build/manifest identities, canonical preset identity,
commands and typed inputs, full validated envelopes, exit statuses, fresh-read
state and outcome comparisons, clock observations when claimed, and source
status before/after. Prove source HEAD/status and its Git worktree registry are
unchanged. Reap only owned children and remove only the exact fixture worktrees
and executables; retain bounded text/JSON evidence in the assigned root.

The scenario passes only when validated successes are atomically durable,
rejections leave no outcome or state change, reconciliation and full-envelope
replay survive a cold process, and cleanup passes. An execution gap is
`INCOMPLETE`; report reproducible mismatches as findings. Automated Direct Engine
and real CLI tests remain development evidence, separate from this independent
QA. Public error behavior is owned by
[errors and exit codes](../specs/interfaces/errors-and-exit-codes.md).
