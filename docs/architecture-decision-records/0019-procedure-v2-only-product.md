# ADR-0019: Make Procedure v2 the Only Product Model

- Status: Accepted
- Date: 2026-08-13
- Partially supersedes: [ADR-0017](0017-single-cursor-convergence.md)
- Supersedes: [ADR-0018](0018-v2-success-envelope.md)

## Context

Podway 0.2.0 shipped Procedure v2 additively beside the released linear
Procedure v1 model. Keeping both models now duplicates parsing, domain,
persistence, protocol, runtime, preset, documentation, and conformance paths.
The v1 model cannot represent decisions, evidence read-back, declared graph
rework, or goal assessment, and retaining it no longer serves the product.

Procedure-independent contracts have their own versions. A `/v1` suffix does
not by itself identify Procedure v1: the IPC request, error envelope, contract
manifest, workspace configuration, and several first-version v2 result
components remain current contracts.

## Decision

Procedure v2 is the only supported authoring and runtime model. Podway does not
parse, start, mutate, display, or convert `podway.procedure/v1`; does not ship
linear presets; and does not expose the v1-only `return`, `reopen`, or
`procedure convert` commands.

All newly produced success responses use `podway.output/v3`, whose closed
command-to-result map contains only Procedure v2 and procedure-independent
families. Failures retain `podway.error/v1`. Procedure-independent `/v1`
contracts remain unchanged when Procedure v2 still consumes them.

Existing Procedure v1 state is never converted or discarded automatically.
Opening state containing a v1 session, v1 snapshot, or v1 durable mutation
fails closed before migration with `LEGACY_PROCEDURE_STATE_UNSUPPORTED`.
Recovery requires an explicit, confirmed `reset --all` after any desired
backup. Empty supported predecessor databases and databases containing only
Procedure v2 state migrate transactionally to the v2-only schema.

The shipped Procedure v2 presets retain the identifiers `sw-dev-v2` and
`bug-fix-v2` and their canonical digests. Historical ADRs, release reports,
roadmaps, and evidence-bound dossiers remain accurate records of earlier
releases.

## Consequences

- Runtime and authoring have one procedure model and one success envelope.
- Existing v2 procedure identities remain stable.
- Existing v1 sessions require explicit destructive reset rather than a lossy
  semantic conversion.
- Consumers must adopt the new success envelope before using the v2-only
  development revision.
- The next release is intentionally compatibility-breaking even though several
  procedure-independent v1 identifiers remain current.
