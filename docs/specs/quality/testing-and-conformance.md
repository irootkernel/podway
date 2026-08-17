# Testing and Conformance

Podway tests are organized around Procedure v2 semantic drift, durable mutation
ordering, crash ambiguity, filesystem boundaries, and public contract stability.

## Required layers

1. Pure graph, goal, item, decision, retry, and rework domain tests.
2. Procedure v2 parsing, validation, canonicalization, and preset identity tests.
3. SQLite schema-v5 initialization, migration, rollback, and legacy-state rejection.
4. Daemon queue, idempotency, concurrency, restart, and crash-injection tests.
5. IPC framing, bounded decoding, schema validation, and fuzzing.
6. Git/worktree, service, CLI, and real-binary end-to-end tests.

Every valid graph fixture must prove a unique entry, reachable nodes, an acyclic
advance subgraph, and a finite terminal route when declared rework edges are
included. Both shipped presets must validate and retain their pinned canonical
digests.

## Persistence and compatibility

The store suite creates canonical schema-v5 from an empty database, migrates each
supported empty or v2-only predecessor transactionally, preserves v2 domain and
receipt state, rejects newer schemas, and rejects every nonempty or mixed
Procedure v1 predecessor without mutation. Reset-all recovery is tested against
isolated fixtures rather than user state.

The protocol suite binds every command to its result schema and stable error
details, validates known answers through production decoders, checks the exact
262,144-byte compact-envelope boundary, and fails closed on missing, unknown,
mistyped, mismatched, or cross-command fields. Successful responses use
`podway.output/v3` only.

## Crash and concurrency coverage

The canonical [crash registry](../../../quality/crash-boundaries-v1.json) binds
each current boundary to an executable test and implementation locator. Restart
must converge to no admission, one queued retry, one terminal result, or an exact
idempotent replay; no domain effect may occur twice.

Concurrency tests cover FIFO worktree ordering, parallel independent worktrees,
bounded queues, stale identity fences, same-key replay and conflict, item races,
and deletion of a worktree with pending jobs.

## Development and distribution gates

`make test` is the required development gate. It runs preparation and static
contracts, Rust tests, the feature-gated contract verifier, serial real-binary
E2E tests, preset tooling checks, and the isolated contributor runtime self-test.
The E2E inventory covers normal public Procedure v2 admission, restart,
goal-closeout, decision rework, idempotency reconciliation, service operation,
and both shipped preset identities.

`make dist` is the release gate. It reruns `make test`, bounded fuzzing,
release-profile builds, native qualification, packaging, Dolgorae handoff, and
final bundle verification. Optional diagnostics and advisory evidence never
replace either gate.
