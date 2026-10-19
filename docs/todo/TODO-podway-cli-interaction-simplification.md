# Podway CLI Interaction Simplification

Status: Candidate. Working label: CALLS. No owning roadmap epic or task is
adopted. Source baseline: 2026-10-19 at
`98796e6a20467d1d072bd84f377fd9ddccccab93`.

## Context

Internal request reuse and external CLI interaction are separate design
questions. The adopted
[structural simplification program](../roadmap/README.md#structural-simplification-program)
preserves current CLI behavior; it does not authorize fewer observations,
combined mutation commands or changes to external consumers.

Existing `observe` may satisfy a caller that otherwise obtains status and next
separately, but it is not a substitute when complete Standard status or history
is required. Existing `record --stdin` already accepts 1-128 unique same-attempt
operations within its 1 MiB input bound as one atomic batch. These are current
capabilities to assess before proposing a new interface, not new features.

The actual fence flags include `--if-workspace-uuid`, `--if-session-id`,
`--if-session-revision` and `--if-attempt`; sketches must use the implemented
grammar rather than inventing `--expected-*` flags.

## Goal

Explore whether callers can express the same intended work with fewer redundant
observations or request documents while retaining clear snapshot, fence,
atomicity, failure and replay meaning. Evaluate structural simplification rather
than elapsed time or claimed speedup.

## Non-goals

No implementation order, task status, completion evidence or public contract is
established by this candidate. No consumer changes, new commands, schemas,
cross-project discovery, runtime service or automatic mutation are authorized.
Elapsed-time tests, latency percentiles, throughput benchmarks, performance
baselines and thresholds are excluded from the proposed investigation.

## Rough Scope

Potential directions, all non-contractual:

- Document when existing observation and atomic record interfaces already cover
  a caller's needs, including the information lost by replacing a richer read.
- Examine duplicated caller-side observations separately from internal Standard
  preflight, which still validates projection/schema/envelope failures.
- Explore a combined record-and-complete request only with an explicit decision
  on transaction boundaries, intermediate failure and exact replay identity.
- Explore returning a next observation with a mutation only with an explicit
  decision on the observation's snapshot, bounds and replay meaning.

Separate terminal-start reset dry-run and interactive post-decision observations
remain intentional existing behavior until independently reconsidered. A shorter
call sequence alone does not establish semantic equivalence.

## Open Decisions

- Which concrete caller needs lack an adequate existing interface, and what
  information and failure behavior must be retained?
- Is an observation tied to mutation commit state or a later snapshot? Does
  replay return the original observation or perform a new read?
- Is a combined operation wholly atomic, and which error wins if record and
  completion would both fail? How are caller keys and digests defined?
- Would changed output require new schemas/result generations, compatibility
  policy or an accepted ADR? Which independent consumers would need explicit
  authorization to change?

## Roadmap Promotion Conditions

Promotion requires a concrete use case, resolved snapshot/fence/replay/atomicity
and error-order decisions, bounded request/output shapes, and an explicit owner
decision to adopt public interface work. Use an accepted ADR before any proposal
changes an architectural invariant. Update canonical contracts and compatibility
checks through the adopted work rather than treating this candidate as authority.

Register an epic and tasks only after those decisions are complete. No
dependency or implementation commitment is implied by the working label CALLS.
Qualification must use correctness and structural evidence without timing or
throughput measurements.

## References

- [TODO authority and lifecycle](README.md)
- [JSON contract](../specs/interfaces/json-contract.md)
- [Transaction contract](../specs/storage/transactions-concurrency-and-idempotency.md)
- [CLI source](../../crates/podway-cli/src/command/)
