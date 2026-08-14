# Podway Procedure v2-Only Transition

## Status and authority

- Document state: `Historical`
- Owning roadmap epic: `V2CUT`
- Intended release train: unreleased v0.2.1 work
- Product version in this task: unchanged at `0.2.0`
- Contract target: `podway.procedure/v2` and `podway.output/v3`

## Goal

Remove the linear Procedure v1 product surface completely while preserving the
procedure-independent contracts that Procedure v2 still uses. A completed
revision accepts and executes only Procedure v2, ships only its two existing
presets, stores only v2 session state, and produces only the v3 success
envelope.

## Boundaries

- Preserve `podway.error/v1`, `podway.ipc/v1`, the contract manifest,
  workspace configuration, canonical JSON, and first-version result/details
  families that Procedure v2 consumes.
- Preserve `sw-dev-v2` and `bug-fix-v2` identities and digests.
- Preserve historical release and architecture records.
- Do not change Cargo versions, publish artifacts, install a daemon, commit,
  push, tag, or release.

## Design

1. Supersede the additive v1/v2 decisions through ADR-0019.
2. Remove v1 parsing, conversion, linear domain transitions, presets, runtime
   dispatch, result schemas, fixtures, and current documentation.
3. Introduce `podway.output/v3` and v3 job read families with a closed v2-only
   result map. Schema-v4 migration preserves terminal receipts for v2 jobs; no
   public producer emits an older success envelope.
4. Introduce SQLite schema v4. Reject nonempty v1 domain state before DDL and
   migrate empty predecessors or v2-only schema-v3 state atomically. The final
   schema contains common operational tables and Procedure v2 domain tables.
5. Add `LEGACY_PROCEDURE_STATE_UNSUPPORTED`, exit 5, non-retryable. The error
   tells users to back up state and run confirmed `reset --all`; automatic
   conversion and deletion are forbidden.

## Acceptance

- v1 Procedure input and old preset names fail deterministically.
- v1-only commands are absent from grammar, completions, routes, and schemas.
- no v1 session result family is packaged or manifest-bound.
- new success responses validate only as `podway.output/v3`.
- schema-v4 migration preserves all v2 state and receipts, rejects every
  nonempty or mixed v1 state without mutation, and supports reset-all recovery.
- current specifications, examples, skill guidance, quality assets, and machine
  contracts describe only the v2 product.
- focused regression tests and the complete `make test` development gate pass.

## Roadmap ownership

- `V2CUT-001`: authority and public boundary.
- `V2CUT-002`: model, parser, and preset removal.
- `V2CUT-003`: v3 success contract and v2-only dispatch.
- `V2CUT-004`: schema-v4 migration and legacy-state rejection.
- `V2CUT-005`: machine assets, tests, and current documentation.
- `V2CUT-006`: complete development verification.
