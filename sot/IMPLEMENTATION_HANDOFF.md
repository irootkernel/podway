# Implementation Handoff

## Immediate kickoff

1. Read the root `README.md` in its recommended order.
2. Import the files in `schemas/`, `presets/`, and `spec/` into the initial repository unchanged.
3. Assign the six work streams in `docs/70-delivery/71-team-work-breakdown.md`.
4. Create requirement-tagged test skeletons from `docs/60-quality/62-requirements-traceability.md`.
5. Freeze public type names, schema IDs, error codes, and transition command names before parallel implementation.

## First vertical slice

The first integrated path should be:

```text
Launch podwayd manually
  -> initialize a temporary Git worktree
  -> start the sw-dev preset
  -> status and next over IPC
  -> set a required item through a durable job
  -> complete the first stage
  -> restart daemon
  -> verify state and idempotent result
```

This slice must use the real frame codec, real SQLite schema, real worktree resolver, and pure domain transition. Do not build a throwaway alternate path.

## First correctness milestone

Before broad CLI work, demonstrate:

- one active attempt invariant;
- retry with empty items;
- return with downstream redo;
- same-item revision conflict;
- two-worktree concurrency;
- lost-response idempotency;
- daemon restart recovery.

## Prohibited shortcuts

Do not:

- let the CLI write SQLite directly;
- use a global task database;
- execute procedure commands or shell expressions;
- infer stage completion from worker or process exit;
- copy old item values into new attempts;
- omit idempotency because the daemon is local;
- defer crash tests until the end;
- introduce product-specific adapter concepts into core crates;
- replace the current-session model with an audit/event model.

## Handoff completion

The project is complete only when `docs/60-quality/61-product-acceptance.md` passes and the native Apple Silicon macOS release artifacts in `docs/50-operations/52-release-and-packaging.md` are produced.
