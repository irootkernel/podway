# Product Acceptance Criteria

The criteria below define the current Procedure v2 product checked by `make test`.
Distribution qualification in `make dist` adds native packaging evidence.

## Procedure and lifecycle

- Only `podway.procedure/v2` input is admitted.
- The shipped presets are exactly `bug-fix-v2`, `small-change-v2`, and
  `sw-dev-v2`, with pinned embedded identities.
- A session snapshots its Procedure and starts at the unique graph entry.
- Action, decision, and terminal nodes enforce their declared transition rules.
- Retry creates a fresh attempt; rework follows only a declared edge and applies
  its declared invalidation policy.
- Goal definition, revision, criterion assessment, and closeout remain fenced and
  durable.

## Runtime and persistence

- `podwayd` is the sole normal writer and executes one mutation per worktree.
- Admission, idempotency, ordering, terminal receipts, and domain state are atomic.
- SQLite schema-v4 contains only Procedure v2 domain state.
- Empty predecessors and v2-only schema-v3 stores migrate transactionally.
- Any nonempty Procedure v1 predecessor fails without mutation and requires user
  backup followed by confirmed `reset --all`.
- Crash recovery never duplicates an admitted mutation or terminal result.

## Interfaces

- Every successful public command emits `podway.output/v3`; failures emit the
  procedure-independent `podway.error/v1` envelope.
- v1-only lifecycle commands, routes, presets, result schemas, and completions are
  absent.
- Machine fields, command bindings, schemas, limits, and error codes match the
  canonical catalogs and manifest.
- Automation uses explicit identity fences and reconciles unknown mutation
  outcomes through idempotency lookup.

## Safety and operation

- Podway executes no configured commands, mutates no Git state, performs no
  network requests, and stores no artifact bytes.
- Paths, frames, queues, collections, logs, and timeouts are bounded.
- Worktree state remains under `.podway/runtime/`; global state is limited to the
  documented endpoint, registry, socket, and bounded logs.
- Native Apple Silicon macOS service installation and lifecycle behavior are
  verified separately from product semantics.

## Final acceptance rule

A failed `make test` means the development revision is not ready. A failed
`make dist` means it is not release-ready. Review, signatures, or generated
evidence never replace the relevant executable gate.
