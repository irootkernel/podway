# ADR-0037: Bound Procedure Graphs at 1,024 Nodes

- Status: Accepted
- Date: 2026-09-22
- Preserves: [ADR-0017](0017-single-cursor-convergence.md), [ADR-0024](0024-bounded-evidence-scale-and-paged-read-back.md)
- Delivery owner: [V2GSC](../roadmap/README.md#v2gsc-procedure-graph-scale-correction)

## Context

Procedure v2 admitted at most 64 node definitions and 64 graph placements. That
limit was an implementation-era constant rather than a product requirement, and
it prevented a valid Aquarium workflow with 65 placements from reaching graph
validation.

Removing every ceiling is incompatible with Podway's fail-closed resource model.
Procedure source, canonical projections, IPC frames, SQLite state, and graph
analysis all require deterministic bounds. The product therefore needs a much
larger practical limit whose memory, response, and algorithmic costs remain
provable.

## Decision

Procedure v2 admits from 1 through 1,024 node definitions and from 1 through
1,024 graph placements. The implied directed-edge bound is 8,192 because one
decision placement may declare at most eight routes. Independent collection
limits remain unchanged: manual-rework targets remain capped at 64, evidence
references at eight per placement, routes at eight per decision, and items at
128 per definition.

Graph membership uses a fixed 1,024-bit representation. Strongly connected
components and traversal order use iterative graph walks; dominators use
iterative fixed-point passes over reverse postorder, intersecting fixed
sixteen-word bitsets; and terminal assessment analysis uses a reverse traversal.
These choices keep call-stack usage independent of author-controlled graph depth.
The traversal analyses are linear in the admitted graph, while dominator
convergence is bounded by the finite 1,024-node bitset lattice rather than being
claimed as a single linear pass.

The complete graph counter array remains present in status, next, compact-status,
and observation projections. Public current and reserved result schemas admit
1,024 counters. Authoring admission charges the exact compact-JSON worst case for
the complete counter array against the existing 1 MiB response frame. Observation
charges that array twice, because complete compact status and complete guidance
each carry it, together with the placement's retained guidance charges and a
32 KiB non-counter status reserve. A Procedure that cannot fit those complete
members is rejected before session start. State
transitions persist only counter rows whose values changed; the SQLite schema and
on-disk model do not change.

Standalone compact status retains its separate 256 KiB protocol envelope. Its maximum-size
production proof combines the widest admitted 1,024-counter, 128-item projection with the bounded
maximum outer envelope. That fixed domain proof is independent of per-placement observation
admission; changing either graph or compact-output bounds must update the proof.

Canonical and generated text projections admit at most 1,048,576 characters.
Canonical JSON must also fit the existing 1 MiB encoded-document bound. The
source parser's 1 MiB byte limit, depth limit, parsed-value limit, IPC frame size,
and every unrelated collection bound remain unchanged. In particular, one
authoring diagnostic still carries at most 64 related graph-node identifiers;
that is a diagnostic payload bound, not a Procedure graph ceiling.

## Rejected alternatives

- Leave the limit at 64: known valid repository workflows already exceed it.
- Remove the limit: this would make parser, memory, response, persistence, and
  graph-analysis costs author-controlled without a closed resource proof.
- Make the ceiling user-configurable: two daemons could then disagree about
  Procedure validity, and public result contracts would no longer have one
  portable maximum.
- Raise every occurrence of 64: many 64-entry bounds protect unrelated blocker,
  media-type, command-argument, item-window, or manual-rework collections.

## Consequences

Long single-cursor workflows up to 1,024 placements are supported without a
storage migration or runtime-mode change. Existing Procedure digests do not
change because the canonical model is unchanged for existing source.

The limit remains deliberately finite. A future increase must re-derive graph
storage, response admission, projection bounds, algorithm behavior, and public
schema ceilings together rather than changing one parser constant.
