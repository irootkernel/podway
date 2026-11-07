# Podway Agent Workflow Simplification

## Status and authority

- Document state: `Candidate`
- Working label: Future interaction semantics
- Owning roadmap epic: none
- Target product release: undecided
- Repository scope: Podway only
- Original interaction-research baseline: 2026-10-19 at `98796e6a20467d1d072bd84f377fd9ddccccab93`

Existing-interface guidance has [durable behavior](../specs/interfaces/cli-specification.md)
and [historical feedback](../implementation-tips/testing.md#guidance-source-feedback-and-revisit-conditions).
This continuing candidate
owns only the unselected interaction-semantics questions. It reserves no roadmap
ID and establishes no implementation commitment. The original research baseline
is not a claim that later feedback was inspected at that revision.

The owner adopted atomic criterion batches and opt-in commit-bound observations
under [AGENT](../roadmap/README.md#agent-agent-interaction-simplification) on 2026-11-06.
Their implemented interfaces are defined by
[ADR-0043](../architecture-decision-records/0043-atomic-assessment-and-commit-bound-observations.md).
This continuing candidate now covers unselected combined operations only;
it does not own AGENT implementation or acceptance.

## 1. Context

Internal request reuse and external CLI interaction are separate design
questions. The completed
[structural simplification program](../roadmap/README.md#structural-simplification-program)
preserved current CLI behavior; it did not authorize fewer observations,
combined mutation commands or changes to external consumers. CALLS adopted only
existing-interface guidance. AGENT delivered the selected interaction semantics;
the remaining combined-operation directions are unadopted.

Existing `observe` may satisfy a caller that otherwise obtains status and next
separately, but it is not a substitute when complete Standard status or history
is required. Existing `record --stdin` already accepts 1-128 unique same-attempt
operations within its 1 MiB input bound as one atomic batch. Assess those current
capabilities before proposing another interface.

## 2. Goal

Explore whether a concrete caller needs fewer observations or request documents
than existing interfaces permit while retaining clear snapshot, fence,
atomicity, failure and replay meaning. Evaluate structural simplification rather
than elapsed time or claimed speedup.

## 3. Non-goals

This candidate adopts no work and defines no public contract. It authorizes no
consumer changes, new commands, schemas, cross-project discovery, runtime service
or automatic mutation. CALLS's selected help, recording, lifecycle and preset
guidance is outside this candidate; DF-050 remains independently deferred.
Elapsed-time tests, latency percentiles, throughput benchmarks, performance
baselines and thresholds are excluded.

## 4. Rough Scope

Potential directions, all non-contractual and requiring their own use case:

- Explore a combined record-and-complete request only with an explicit decision
  on transaction boundaries, intermediate failure and exact replay identity.

Separate terminal-start reset dry-run and interactive post-decision observations
remain intentional existing behavior until independently reconsidered. A shorter
call sequence alone does not establish semantic equivalence. Evaluate small-change
and bug-fix flows, stale fences, lost mutation output and exact retry against the
existing observation and atomic-record baseline.

### Source feedback

The complete original source, deferral reasons and revisit conditions of DF-044,
DF-047, DF-048, DF-049 and DF-051 remain with their
[durable owner](../implementation-tips/testing.md#guidance-source-feedback-and-revisit-conditions).
Their roadmap promotion closed the deferrals only; the roadmap owns implementation
and acceptance. They are not Open feedback owned by this continuing candidate.

## 5. Open Decisions

- Which concrete caller need cannot be met adequately by existing interfaces,
  and what information and failure behavior must remain available?
- For a combined operation, is it wholly atomic, and which error wins if record
  and completion would both fail? How are caller keys and digests defined?
- Would changed output require new schemas/result generations, compatibility
  policy or an accepted ADR? Which independent consumers need explicit authority
  to change?

## 6. Roadmap Promotion Conditions

Promotion requires a concrete need and owner-selected scope, with closed snapshot,
atomicity, error-precedence, request-identity, replay, bounds and compatibility
decisions. Register a separate epic only after these decisions are complete;
CALLS is not authority for adopting these remaining directions.

Use an accepted ADR before changing an architectural invariant and update the
affected canonical contracts and compatibility checks through the adopted work.
Qualification must use correctness and structural evidence without timing or
throughput measurements. Existing read-only and exact-replay behavior must remain
explicitly assessed, not inferred from a shorter command sequence.

## 7. References

- [TODO authority and lifecycle](README.md)
- [CALLS roadmap](../roadmap/README.md#calls-agent-workflow-guidance)
- [JSON contract](../specs/interfaces/json-contract.md)
- [Transaction contract](../specs/storage/transactions-concurrency-and-idempotency.md)
- [CLI source](../../crates/podway-cli/src/command/)
