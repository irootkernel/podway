# Team Work Breakdown

## Suggested streams

The work can be parallelized across six streams. One engineer may own multiple streams on a small team, but ownership boundaries should remain explicit.

## Stream A: Domain and specifications

Owns:

- `podway-core`;
- procedure and item semantics;
- canonicalization contract with Stream D;
- status and next planning;
- state-transition and property tests;
- ADR and requirement consistency.

Primary deliverables:

- pure transition API;
- invariant checker;
- reference model for property tests;
- command/result domain types.

Dependencies: none after contract lock.  
Blocks: store, daemon, CLI integration.

## Stream B: Store and queue

Owns:

- `podway-store`;
- SQLite DDL and migrations;
- durable admission and claiming;
- idempotency and receipts;
- transaction orchestration;
- pruning, integrity, and reset-all;
- crash injection infrastructure.

Primary interfaces:

```text
admit(request) -> existing or new job
claim_next(workspace) -> job
execute(job, prepared_inputs, transition_fn) -> terminal response
read_workspace_view() -> coherent state
```

Dependencies: core command/state types.  
Blocks: daemon end-to-end reliability.

## Stream C: Daemon and IPC

Owns:

- `podway-daemon`;
- `podway-protocol` framing and server side;
- socket and singleton lifecycle;
- peer-user checks;
- per-worktree scheduler;
- registry recovery;
- read queries, waits, and job cancellation;
- structured daemon logging.

Dependencies: protocol schemas, store interfaces, Git resolver.  
Blocks: CLI integration and service operation.

## Stream D: CLI, config, and presets

Owns:

- `podway-cli`;
- `podway-config`;
- `podway-presets`;
- command grammar and help;
- static procedure validation;
- automatic precondition reads;
- text and JSON renderers;
- shell completion;
- all four preset source files.

Dependencies: core types and protocol client.  
Can begin: command parsing, schemas, presets, help, and static validation immediately.

## Stream E: Git, macOS service, and distribution

Owns:

- `podway-git`;
- `podway-service`;
- worktree identity and containment;
- initialization layout;
- LaunchAgent install/lifecycle;
- socket/runtime/log paths;
- packaging, optional signing, checksums, and distribution tooling.

Dependencies: limited; can build fixtures and service abstraction early.  
Blocks: complete macOS release.

## Stream F: Quality and integration

Owns:

- conformance harness;
- property/reference-model review;
- IPC fuzzing;
- crash process harness;
- Git fixture repository generation;
- macOS service integration environments;
- CLI/JSON golden tests;
- product-acceptance and crash-boundary mappings in the local gate.

This stream should not be postponed until feature completion. It defines test interfaces with all other streams from the first milestone.

## Integration contracts to freeze first

Within the first engineering milestone, freeze:

1. core command and state transition interface;
2. procedure/config schemas and canonicalization;
3. public error-code enum;
4. IPC request envelope and frame codec;
5. success/error JSON envelopes;
6. store admission and terminal-result interface;
7. Git resolver result shape;
8. service manager trait.

## Suggested parallel schedule

### Integration milestone 1

- A: domain types, invariants, start/complete skeleton;
- B: DDL, connection setup, migration harness;
- C: frame codec and daemon skeleton;
- D: CLI grammar, schemas, presets;
- E: Git fixtures and LaunchAgent template;
- F: Makefile gate, requirement IDs, test harness skeleton.

### Integration milestone 2

- A: all transitions and property tests;
- B: durable queue, idempotency, item transactions;
- C: scheduler and read service;
- D: config canonicalization and static CLI;
- E: worktree discovery and service install;
- F: concurrency and protocol suites.

### Integration milestone 3

- full CLI to daemon to store vertical path;
- retry and return end-to-end;
- restart recovery;
- one preset complete scenario;
- LaunchAgent-managed daemon.

### Integration milestone 4

- all commands and presets;
- crash matrix;
- artifact handling;
- completion and help;
- distribution packaging.

## Handoff artifacts between streams

| Producer | Consumer | Required artifact |
|---|---|---|
| A | B/C/D/F | Domain command/result types and transition fixtures |
| B | C/F | Store interface, migration fixtures, fail points |
| C | D/F | Protocol client contract and daemon test harness |
| D | A/F | Parsed procedure fixtures, preset expected views |
| E | C/D/F | Worktree resolver and service-manager test doubles |
| F | All | Failing conformance cases with requirement IDs |

## Code review requirements

Changes to core transitions, storage transactions, IPC framing, path containment, or service install require review by at least one owner outside the authoring stream.

Cross-stream review is mandatory for:

- public schema changes;
- database migrations;
- idempotency behavior;
- unsafe Rust;
- macOS service commands;
- changes to reset or worktree deletion semantics.

## Definition of done for a feature

A feature is done only when:

- domain behavior is specified;
- public CLI and JSON are documented;
- errors and exit codes are cataloged;
- storage and concurrency behavior is implemented;
- help and completion are updated;
- unit, integration, and conformance tests pass;
- traceability mapping is updated;
- no other document or schema contradicts it.
