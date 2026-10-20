# Contributor Documentation

This directory is the canonical home for the human-readable knowledge needed to
understand, change, test and release the single `podway` CLI and its in-process
Direct Engine. Worktree-local authority, fenced success-only transactions and
write-free snapshot reads are the current product boundaries.

## Start here

1. [Architecture](architecture/) explains system boundaries, components, data
   flow, repository layout, and risks.
2. [Architecture Decision Records](architecture-decision-records/) preserve the
   reasons behind accepted and superseded decisions.
3. [Specifications](specs/) define required product, domain, interface, storage,
   operational, and quality behavior.
4. [Implementation Tips](implementation-tips/) explain how to work safely in the
   repository and run verification.
5. [TODO](todo/) holds candidate work and the authoritative design dossiers for
   adopted work that is not yet complete.
6. [Deferred Feedback](deferred-feedback/) records small review findings that are
   intentionally postponed.
7. [Roadmap](roadmap/) owns adopted work, ordering, and status.

[Examples](examples/) provide walkthroughs and versioned known-answer payloads.

Current adopted work is tracked by the [active roadmap](roadmap/).
The [structural simplification program](roadmap/README.md#structural-simplification-program)
adopts `VIEWS -> PARSE -> STATE -> ADMIT`: projection construction, document
admission, same-snapshot state and request admission reuse. The design was
registered on 2026-10-19; the roadmap owns current execution status.
The [two remaining adopted dossiers](todo/README.md#current-adopted-dossiers)
define unfinished scope, failure boundaries and task-local correctness checks.
Completed VIEWS and PARSE behavior lives in
[projection composition](architecture/system.md#request-scoped-read-composition) and
[document admission reuse](architecture/system.md#document-admission-reuse), with
[functional regression owners](implementation-tips/testing.md#functional-regression-owners).
Elapsed-time tests,
latency percentiles, throughput benchmarks and performance baselines/thresholds
are excluded. Existing CLI/JSON interfaces and persisted validation remain preserved;
CLI interaction changes remain an [unadopted CALLS candidate](todo/TODO-podway-cli-interaction-simplification.md).
This program selects no release or external-consumer change.

The preceding [internal-improvement sequence](roadmap/README.md#current-internal-improvements)
is OPTIM, then BOUND, then READS. OPTIM and BOUND are completed and owner-accepted
on 2026-10-17. READS is completed and owner-accepted on 2026-10-19; its
[delivered behavior and correctness scope](implementation-tips/direct-cli-performance-qualification.md#reads-005-functional-closeout)
replace the retired design dossier.
On 2026-10-16 the owner withdrew OPTIM latency, count, and memory measurement,
including any baseline that would have blocked implementation. Completed OPTIM
behavior is one validated request-scoped read projection and one unpublished
batch draft. BOUND separates pure domain rules, persisted admission, typed
mutation failures and CLI responsibilities. READS delivers session-scoped
family acquisition, bounded changed-slot reloads, owned read projections and
typed fence-preflight facts while preserving eager validation. The owner
excluded all READS performance measurement from acceptance.
External/storage contracts and existing command-specific validation stay
preserved, including summary-only archive-list. These improvements are not a
selected release program.

The implemented direct model is specified by [ADR-0038](architecture-decision-records/0038-remove-the-daemon.md)
and the [direct execution requirements](specs/quality/requirements-traceability.md#direct-execution-requirements).
Historical feasibility baselines and withdrawn comparison registrations remain in the
[performance qualification guide](implementation-tips/direct-cli-performance-qualification.md).

## Canonical assets

Files consumed directly by builds and verification live outside this directory:

- [`assets/presets/`](../assets/presets/) contains built-in Procedure YAML;
- [`assets/schemas/`](../assets/schemas/) contains public JSON Schemas;
- [`assets/specifications/`](../assets/specifications/) contains executable
  catalogs, current Store DDL, transition data, and canonicalization rules;
- [`contracts/`](../contracts/) contains repository and interface contracts.

There are no generated documentation mirrors. Edit each canonical asset in place
and run the relevant contract checks.

## Precedence

When sources disagree, resolve them in this order:

1. accepted [ADRs](architecture-decision-records/);
2. canonical machine assets and executable contracts;
3. behavioral documents under [specs](specs/);
4. [architecture](architecture/) and [implementation guidance](implementation-tips/);
5. the active roadmap and adopted TODO design dossiers for unfinished work;
6. examples, TODO candidates, deferred feedback, and archived roadmap history.

The active roadmap owns status and ordering, and an adopted TODO dossier owns the
decision-complete implementation plan for its unfinished work. Neither overrides
implemented behavior in an ADR, machine asset, or specification. Update every
affected source when resolving a contradiction.

## Maintenance rules

- Write the root README, changelog, and all Markdown under `docs/` in English.
- Link to the narrowest stable document and heading that supports a claim.
- Update specifications, machine assets, ADRs, tests, and roadmap state together
  when a change crosses those boundaries.
- Keep temporary plans, generated reports, logs, and local evidence out of this
  canonical documentation tree.
- Run the documentation and contract verifiers before the complete `make test`
  gate.
