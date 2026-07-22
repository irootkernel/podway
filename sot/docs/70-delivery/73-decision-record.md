# Final Decision Record

## Locked product decisions

1. Podway manages one current task session in one Git worktree.
2. The purpose is procedure adherence and omission prevention, not evidence review or post-mortem.
3. A session has exactly one active stage attempt.
4. A worktree has at most one session.
5. Workspaces require Git and fail closed outside a valid non-bare worktree.
6. Task state lives inside `.podway/runtime/state.sqlite3`.
7. Deleting the worktree deletes task state.
8. `podwayd` is the sole normal writer.
9. CLI mutations are durable jobs in a per-worktree FIFO queue.
10. Different worktrees may process mutations concurrently.
11. The implementation language is Rust.
12. The first complete platform is macOS.
13. The daemon is a user LaunchAgent and starts at login.
14. The public interfaces are CLI, versioned JSON, and shell completion.
15. Procedures are data-only ordered stages.
16. Stage requirements use six typed item types, not a generic evidence ledger.
17. Retry and return create fresh attempts with empty items.
18. Return conservatively marks reached downstream stages `redo`.
19. SQLite relational state is authoritative; full event sourcing is not used.
20. Operational history is bounded and reset defines the task-history boundary.
21. Artifact bytes are not stored; only path/reference, SHA-256, size, and media type are stored.
22. Local required artifacts are revalidated at completion.
23. Same-user local trust is accepted; no worktree access key is used.
24. No network listener, telemetry, arbitrary command execution, or Git mutation is permitted.
25. The built-in presets are `sw-dev`, `bug-fix`, `docs-only`, and `analysis`.
26. There is no Dolgorae-specific package and no Orca adapter.
27. External tools integrate through generic CLI and JSON.
28. The public product name is Podway.
29. The repository license is MIT.
30. The release is one complete product, not a reduced MVP.
31. The complete first public release is `v0.1.0`.
32. The design package is `1.0.1-design`; public and storage contracts remain v1.
33. Initial `schema-0`/uninitialized (`uninitialized-database`) state migrates transactionally to `schema-v1`.
34. ADR-0011 retires the former `REL-007` detached-approval and quorum requirement. The identifier is not reused, and the recorded S0 digest is historical design provenance rather than a release gate.
35. Gate S S1 contains only derivative SOT edits that implement the recorded S0 baseline.
36. Gate S S2 validates before emitting checksums as its final action, and Gate S S3 completes read-only acceptance before any product workspace is created.
37. Gate S covers Phases 0 through 8; Phase 8 closes through the repository-local release gate.
38. The repository-root `make test` command is the sole required release-readiness gate and runs preparation, unit, integration, bounded protocol fuzzing, and actual-binary end-to-end targets sequentially.

## Intentionally deferred capabilities

The following are outside the first release and require a new decision before implementation:

- Linux service packaging;
- Windows support;
- remote synchronization or multi-user server;
- multiple active sessions per worktree;
- parallel stage groups, joins, or arbitrary graphs;
- long-term archive, export, import, or post-mortem features;
- artifact content storage;
- cryptographic identity or signatures;
- automatic shell, test, build, or Git command capture;
- product-specific integration packages;
- GUI, TUI, or web UI;
- remote procedure/schema includes;
- plugins or executable procedure expressions.

## Implementation choices left to the team

The following may be selected by the development team as long as public contracts and constraints remain satisfied:

- specific Rust libraries for async runtime, SQLite, Git reading, YAML, CLI parsing, and logging;
- exact internal thread-pool size and scheduler implementation;
- exact Git fingerprint representation;
- internal table access patterns beyond the normative DDL contract;
- text-output styling;
- package manager integration;
- distribution automation used after the local release gate;
- internal error types and module names.

## Decision-change process

A locked decision changes only through:

1. a proposed ADR stating context, alternatives, consequences, and migration;
2. synchronized changes to affected docs, schemas, specs, tests, and traceability;
3. review by architecture, implementation, and QA owners;
4. explicit acceptance before code relying on the change merges.

Implementation convenience alone is not sufficient to weaken a core invariant.
