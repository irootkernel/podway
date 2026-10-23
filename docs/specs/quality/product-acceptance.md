# Product Acceptance Criteria

The criteria below define the current Procedure v2 product checked by `make test`.
Distribution qualification in `make dist` adds native packaging evidence.

## Procedure and lifecycle

- Only `podway.procedure/v2` input is admitted.
- The shipped presets are exactly `analysis-v2`, `bug-fix-v2`,
  `small-change-v2`, and `sw-dev-v2`, with pinned embedded identities.
- `start` snapshots the Procedure into a cursor-free prepared session; `begin`
  creates the first running attempt at the unique graph entry.
- Action, decision, and terminal nodes enforce their declared transition rules.
- Retry creates a fresh attempt; rework follows only a declared edge and applies
  its declared invalidation policy.
- Goal definition, revision, criterion assessment, and closeout remain fenced and
  durable.

## Runtime and persistence

- Each CLI invocation uses the in-process Direct Engine; compatible writers
  serialize through native maintenance leases and SQLite without a FIFO queue.
- State, sequence, journal and the validated success outcome commit atomically.
  Rejections retain no outcome, and exact retained success replays the original
  complete response before the transition callback or clock runs.
- Direct SQLite generation 15 binds the canonical Git worktree and writer
  manifest identity. It contains no runtime mode or durable job state.
- Generations 1–14 and legacy markers are refused unchanged, with no migration,
  checkpoint or SHM creation. Confirmed reset makes a fresh Store without backup
  or conversion after the separately authorized legacy-writer retirement.
- Newer Stores refuse even reset and removal. Invalid supported current Stores
  may be discarded only through confirmed unfenced recovery; rejected identity
  fences never authorize adoption of the old UUID.
- Crash recovery and original-key lookup/retry preserve exactly one committed
  effect and success outcome, or no effect and no outcome.
- Reads observe one committed snapshot without writing. Artifact hashes are
  prepared outside locks and their file stamps and recorded identities are
  rechecked inside the transaction without reading artifact bytes.
- One invocation deadline bounds waits only, allows an immediate attempt at
  zero remaining budget and never aborts work after the required locks are held.

## Interfaces

- Successful versioned JSON uses `podway.output/v4`; the compact version summary
  retains its documented unversioned shape. JSON-mode failures, including local
  internal and output-budget failures, emit validated `podway.error/v2`. The
  [CLI failure-rendering boundary](../interfaces/cli-specification.md#output-and-exits)
  explicitly permits bounded stderr and exit 6, without partial JSON or abort,
  when a valid timestamp or rebuilt error envelope cannot be supplied. Every
  Procedure observation uses result v5 in the deprecated default generation 1
  and result v6 under `--contract-generation 2`.
- v1-only lifecycle commands, routes, presets, result schemas, and completions are
  absent.
- Machine fields, command bindings, schemas, limits, and error codes match the
  canonical catalogs and manifest.
- Automation uses explicit identity fences and reconciles unknown mutation
  outcomes through read-only `outcome lookup` with the original key and request.
- Retired commands and flags fail with `COMMAND_RETIRED` before workspace or
  stdin access. Exit class 3 and daemon, IPC, socket, job and runtime-mode error
  families are retired; current codes and recovery recipes match their catalogs.

## Safety and operation

- Podway executes no configured commands, mutates no Git state, performs no
  network requests, and stores no artifact bytes.
- Paths, input, output, collections, journal and lock waits are bounded.
- Worktree state and locks remain under `.podway/runtime/`; the current product
  creates no account state, endpoint, socket, registry or service.
- Native Apple Silicon macOS archive qualification and legacy upgrade fixtures
  are verified separately from installation, old-writer retirement and consumer
  adoption. A nonpublishable development bundle is not a stable release gate.

## Final acceptance rule

A failed `make test` means the development revision is not ready. A failed
`make dist` means it is not release-ready. Review, signatures, or generated
evidence never replace the relevant executable gate.
