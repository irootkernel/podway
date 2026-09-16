# Podway Supervised Delegation: Real Executor Integration

## Status and authority

- Document state: `Candidate`
- Dossier type: research and design candidate
- Research baseline: September 16, 2026
- Contract comparison refinement: September 17, 2026
- Owning roadmap epic: none
- Target release: not assigned
- Related local foundation: [ADR-0035](../architecture-decision-records/0035-host-neutral-supervised-delegation.md)

This candidate retains the real-executor work excluded when the former
supervised-delegation candidate was narrowed to the Podway-only foundation.
It does not block that foundation's local/native deterministic acceptance.
It authorizes no Dolgorae code change, live run, deployment, or provider access.

## 1. Context

The owner intends to connect Dolgorae once its relevant execution interfaces are
ready. Two cases must remain distinct: an existing external Primary using the
External Specialist Engagement facade, and a Dolgorae-hosted Primary using its
internal Broker. Neither case is established by a test adapter or by accepting a
caller-supplied receipt in Podway.

The September 16 local inspection found Dolgorae's Role/task separation and
external v3 task contract implemented, while the internal Primary tool still
used a different task request shape and the live Primary bridge was separately
planned. This is dated local context, not an assertion about installed or future
Dolgorae capabilities. Revalidate the actual canonical schemas, roadmap, source,
and running build before planning an adapter.

The September 17 [field and digest review](../implementation-tips/delegation-adapter-contract-review.md)
records the exact inspected source and the difference between an external
Role-reference hash and the Broker's installed-Role digest. It also separates
internal persistence from a permitted adapter getter. These are concrete
integration prerequisites, not authorization to scrape private state, weaken
Podway's required fields, or claim either path live-compatible. The foundation
owns the offline semantic review before schema reservation; this candidate
owns closing the real-path gaps and qualifying the selected build.

The Podway target contract is
[host-neutral supervised delegation](../specs/interfaces/supervised-delegation.md).
It intentionally does not discover providers, own runtime credentials, or
schedule external work.

## 2. Goal

Determine the minimum external adapter and host integration needed to execute a
delegated Procedure honestly through either supported Primary-host case,
without duplicating orchestration authority or disrupting existing Direct work.

## 3. Non-goals

Do not convert this candidate into permission for Podway to invoke models,
launch commands, store transcripts or artifact bytes, authenticate same-user
actors, manage Profiles, duplicate Dolgorae writer ownership, or run two
semantic Primaries for one task.

Do not assume a running external engagement can be transferred into a brokered
hierarchy, that the two task schemas are interchangeable, or that a Primary
process restart is a new Primary-owner handoff. Fan/join, continuation
activation, and a Web console remain separate candidates.

## 4. Rough scope

The external integration must own the logical binding resolution, immutable
Role/configuration preflight, trusted controller carrier, task construction,
full evidence materialization, idempotent external requests, accepted-task
identity, feedback counts, result validation, reuse/release, and recovery.
Carry the reviewed mapping forward for every required execution field, result
digest, round count and disposition. Establish exact canonical objects and
permitted response/read/host-binding paths, including recovery after a lost
acknowledgement. A request prepared locally is not yet an accepted task, and
an allocated worker ID is not proof of successful provisioning.

For a managed Primary, use the authorized run-bound Broker path. Never send
model-selected Controller/source identities or silently fall back to the
external engagement facade. Prove inline context and completion criteria retain
their meaning; embedding them in an opaque objective is not automatically
equivalent to a checked structured task.

The preregistered live comparison retained from the original candidate should
compare prose/ordinary Podway items plus executor-owned durable state against
the explicit declaration/package/receipt path. Record concrete omissions,
recovery invariants, and operator steps without claiming superiority merely
because the native feature exists. A sufficient simpler composition may narrow
or remove optional adapter conveniences; it does not retroactively falsify
local structural conformance already established by the foundation.

Cover direct execution, mandatory delegation, feedback, retained reuse,
required release, external/managed Primary restart, executor restart, lost
responses, exhaustion, retry, rework, goal revision, cancellation, and late
results. Also cover a cosmetic or optional-item correction after acceptance,
old acceptance replay while current state is stale, and explicit fresh-attempt
recovery without automatic redispatch. Before reusing an artifact, prove its
current input/evidence applicability and define the actual accepted-task
provenance for revalidation; do not relabel an old receipt or invent a new task
identity. Primary choice, external quiescence and required worker settlement
remain separate from Podway's local retry. Only run live models and
provider-sensitive tests with explicit owner authorization in disposable,
bounded, non-production fixtures.

## 5. Open decisions

- Which repository owns each adapter and the trusted host bridge; no executable
  provider adapter is owned by the Podway foundation by default.
- Exact compatible Dolgorae capability/schema/build baseline for each host case,
  and how internal task semantics reach parity where required.
- The permitted immutable Role/configuration getter or fully specified input
  derivation for each path. Do not map the inspected Role-reference hash to a
  Role-content digest; do not assume the Broker's complete Role object and a
  Role-source document have the same digest domain.
- Exact accepted-task and complete-result byte domains, normalization, permitted
  retrieval, acknowledgement linkage and durable adapter recovery. Preserve
  all required result components, including an isolated patch when applicable.
- Binding configuration ownership, Profile and access-policy selection, exact
  Role resolution, and credential delivery outside model-controlled arguments.
- How the adapter recovers a stopped Primary's durable external task identity
  without duplicating work or adopting unverified results.
- The explicit revalidation path after a post-acceptance correction and fresh
  Podway attempt: which artifacts can be reused, what current evidence must be
  checked, what task records the executor actually accepts, and how unnecessary
  repeated implementation is avoided without copying historical acceptance.
- What approval policy applies to new Specialist instances, including a denied
  or pending approval that must not be mistaken for a worker failure.
- Canonical-workspace writer quiescence, isolated-change handling, cancellation,
  and cleanup debt without making Podway maintenance wait on a remote service.
- Whether live Primary replacement is needed at all. If adopted, define owner
  generation fencing and an explicit handoff, preserving the difference between
  stale Primary authority and still-valid worker results.
- What non-secret external correlation appears in operator views and which
  content remains private to the executor.
- Which live comparison findings justify convenience features beyond the
  foundation and what release claims the evidence actually supports.

## 6. Roadmap promotion conditions

Promotion requires a compatible implemented executor contract for each claimed
host case, a chosen adapter owner, closed configuration/authority/recovery
boundaries, an explicit live-test authorization, and an adopted integration
acceptance matrix. A planned transport or private draft is not dependency proof.
Revalidate the field/digest worksheet against the selected real build. Every
required row needs an allowed source or a proven deterministic derivation with
acceptance and recovery linkage; unresolved rows leave that adapter unavailable.
Source comparison or synthetic receipt acceptance alone cannot satisfy this
condition, and no core schema is silently relaxed to hide a missing getter.

Real qualification must preserve Podway's Direct continuity matrix and exact
manifest/mode checks. Test metadata assembly against the full-content contract,
accepted task and result identity, release-before-accept, accept-before-complete,
response loss, and outcome-unknown handling. Do not equate successful result
collection with Primary acceptance or Procedure completion.

Document the live comparison's preregistered criteria and observed results.
Keep unsupported host cases unavailable rather than emulating them through a
different authority boundary. Any new Primary handoff or parallel model needs
its own explicit architecture decision. Register implementation work only once
these decisions and dependencies are closed.

## 7. References

- [ADR-0035](../architecture-decision-records/0035-host-neutral-supervised-delegation.md)
- [Podway target contract](../specs/interfaces/supervised-delegation.md)
- [Non-disruptive implementation guidance](../implementation-tips/supervised-delegation.md)
- [Adapter field and digest review](../implementation-tips/delegation-adapter-contract-review.md)
- [Graph engineering candidate](TODO-podway-graph-engineering-evolution.md)
- [Same-user trust](../architecture-decision-records/0006-same-user-local-trust.md)
- [Generic CLI/JSON integration](../architecture-decision-records/0010-generic-cli-json-integration.md)
