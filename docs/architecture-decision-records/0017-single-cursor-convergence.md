# ADR-0017: Permit Single-Cursor Convergence

- Status: Accepted
- Date: 2026-08-04
- Supersedes: [ADR-0015](0015-constrained-single-cursor-graph.md)
- Procedure v1 preservation: Superseded by [ADR-0019](0019-procedure-v2-only-product.md)

## Context

ADR-0015 introduced a constrained graph for Podway v2 but rejected every
join. That wording also rejects a useful topology in which several alternative
routes enter one later placement even though only one route and one cursor are
active. Such convergence does not require parallel execution or synchronization.

Keeping all convergence outside the product would force authors to duplicate
the same downstream action or decision for every branch. Allowing a general
join, however, would introduce multiple active tokens, branch completion state,
and synchronization semantics that conflict with Podway's single-cursor model.

## Decision

Podway v1 remains a linearly ordered stage contract with unchanged semantics.

Podway v2 retains ADR-0015's finite, declarative, single-cursor graph and may
add a convergence node under these constraints:

- multiple declared incoming routes may target the same graph placement;
- exactly one incoming route is traversed when the placement becomes active;
- a running session still has one authoritative cursor and exactly one active
  node attempt;
- convergence never waits for, combines, or synchronizes multiple branches;
- there are no forks that execute simultaneously, multiple active tokens,
  parallel node attempts, or background graph executions;
- required recorded-item references downstream of convergence may name only
  placements that dominate the consumer; branch-specific references must be
  optional.

All other ADR-0015 constraints remain in force: routes are declared data,
decision options select declared routes, rework creates fresh attempts and
invalidates the affected trace suffix, graph vetting fails closed, and
procedures contain no expressions, plugins, hooks, or commands.

## Consequences

Positive:

- alternative routes can share a common review or closeout placement;
- duplicated downstream definitions and placements are unnecessary;
- status, next, persistence, and mutation ordering retain one-cursor semantics.

Negative:

- graph vetting must distinguish convergence from a synchronizing join;
- dominance rules restrict required read-back after convergence;
- workflows that require parallel work or synchronization remain outside Podway.
