# TODO and Adopted Design Dossiers

This directory owns planning documents for work that is not complete. Every
`TODO-*.md` file declares either `Candidate` or `Adopted`. Completed work does not
remain in this directory.

## Candidate documents

A candidate has not entered the active roadmap. It contains:

1. Context
2. Goal
3. Non-goals
4. Rough Scope
5. Open Decisions
6. Roadmap Promotion Conditions
7. References

A candidate does not contain task status, implementation order, completion
evidence, or normative product behavior. Promotion requires resolving its
material decisions and registering the adopted work in the active roadmap.

### Research candidates

A candidate whose purpose is to preserve investigation results may additionally
contain a research baseline date, an external landscape survey, a ledger of
preferred and rejected directions, illustrative non-contractual sketches, risks,
and experiment questions. It declares `Dossier type: research and design
candidate` in its status block.

The additional material is subject to the same authority limit as the rest of the
document. A preferred direction is a record of what an investigation favored, not
a decision the product has taken, and an illustrative sketch must be marked
non-contractual where it appears. External comparisons are snapshots and must
carry the date on which they were reviewed.

A research candidate must still satisfy the seven-part structure above, and its
promotion conditions must state which of its recorded preferences require an
accepted ADR before they bind anything.

## Adopted design dossiers

An adopted dossier is the authoritative, decision-complete implementation plan
for one unfinished roadmap epic or for one release program composed of a closed
set of related epics. It contains:

1. Status and authority
2. Verified context and evidence
3. Goals, non-goals, and scope
4. Accepted design decisions and interfaces
5. Failure handling and compatibility boundaries
6. Roadmap ownership, dependencies, and traceability
7. Verification and release acceptance
8. References

The active roadmap remains the sole owner of task order and status. A dossier may
map requirements to roadmap task IDs, but it must not maintain a competing status
table. Specifications, accepted ADRs, and canonical machine assets remain the
sources of truth for implemented behavior; a dossier records intended changes
until the owning task updates those sources.

A release-program dossier must name its release target, enumerate every owning
epic, define their dependency graph, and map each epic's tasks independently. It
must not use the release-program identifier as a synthetic epic or task prefix.

## Lifecycle

- Keep an adopted dossier here while any task in its owning epic is incomplete.
- Update the dossier before implementation when a material design decision
  changes. During implementation, promote every lasting decision and behavior to
  its proper durable authority: accepted ADRs, canonical machine assets,
  specifications, architecture, implementation tips, examples, and roadmap
  evidence as applicable.
- After the owner accepts the exact closeout candidate and before marking the
  epic complete, replace every roadmap and documentation link
  to the dossier with the appropriate durable authority, remove the dossier from
  this index, and delete its `TODO-*.md` file. No completed behavior, rationale,
  operational tip, compatibility boundary, or verification claim may exist only
  in the deleted dossier.
- Do not move a completed TODO dossier into `docs/roadmap/archive/`. That directory
  is reserved for compacting historical roadmap records when the active roadmap
  becomes too large; it is not a TODO completion destination.
- Do not bind executable evidence to a temporary TODO path. Do not leave generated
  reports, logs, mutable qualification receipts, or temporary plans here.

## Current adopted dossiers

The remaining dossiers follow the design-only sequence adopted on 2026-10-19.
The [roadmap](../roadmap/README.md#structural-simplification-program) alone owns
task order and current status.
Accept each epic before starting the next, without cross-epic implementation
interleaving:

1. [STATE: Snapshot state reuse](TODO-podway-snapshot-state-reuse.md)
   shares admitted graph/disposition, attempt lookup and receipt facts within a
   coherent snapshot.
2. [ADMIT: Request admission reuse](TODO-podway-request-admission-reuse.md)
   composes eligible admission stages and retains at most one Store-prepared
   graph in an opaque Engine context for one CLI invocation, without retaining
   leases across mutation artifact hashing, prompts or the competing-writer gap.

VIEWS and PARSE are accepted and their dossiers are retired. Completed behavior
and verification guidance live in
[projection composition](../architecture/system.md#request-scoped-read-composition),
[document admission reuse](../architecture/system.md#document-admission-reuse) and
[functional regression owners](../implementation-tips/testing.md#functional-regression-owners).

No CLI/JSON interface, storage identity, release or external-consumer change is selected.
Elapsed-time tests, latency percentiles, throughput benchmarks, performance
baselines and thresholds are excluded. Use
[structural correctness verification](../implementation-tips/testing.md#structural-simplification-verification).
CALLS remains a candidate, without roadmap tasks.

### Completed preceding sequence

The owner adopted three sequential internal-improvement epics on 2026-10-14
with one shared goal and a fixed order, not parallel work. OPTIM and BOUND were
accepted on 2026-10-17 and their dossiers are removed. The
[roadmap](../roadmap/README.md#current-internal-improvements) alone owns order and
status. READS was accepted on 2026-10-19 and its dossier is also retired; no
adopted dossier remains for this sequence. These epics are not a release program
and do not select a release version. Existing external/storage contracts
and cold-read validation remain unchanged; archive-list retains its summary-only
boundary. The owner withdrew OPTIM performance measurement on 2026-10-16
and reaffirmed the exclusion of all READS performance measurement on 2026-10-19.
Roadmap status is authoritative; a missing OPTIM baseline is not implementation
evidence.

READS behavior and correctness boundaries now live in the
[system architecture](../architecture/system.md#request-scoped-read-composition)
and [functional closeout guidance](../implementation-tips/direct-cli-performance-qualification.md#reads-005-functional-closeout).

Each dossier is retired independently after its owning epic is accepted and its
lasting information is promoted. References to a completed prerequisite use the
durable roadmap and architecture rather than a removed dossier.

## Related candidates

- [CLI interaction simplification (CALLS)](TODO-podway-cli-interaction-simplification.md): possible caller and public interface changes; no epic, tasks or implementation commitment is adopted.
- [Real executor integration](TODO-podway-supervised-delegation-integration.md): live external/managed Primary adapters and qualification excluded from V2SDG.
- [Graph engineering](TODO-podway-graph-engineering-evolution.md): separately motivated work-unit and parallel graph research. Rebased on the current Direct Engine; successor work units/leases/fan groups remain unadopted.
- [Continuation generations](TODO-podway-versioned-continuation-generations.md): separately motivated topology replacement and plan lineage.
- [Web operations console](TODO-podway-web-operations-console.md): human operations after its graph/runtime prerequisites are settled. Foreground-only exploration over the same Direct Engine and explicit worktree inventory; no resident/login/global control plane is adopted.
