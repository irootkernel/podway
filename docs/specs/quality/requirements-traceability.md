# Requirements Traceability

## Purpose

This matrix gives the implementation and QA teams stable requirement identifiers. Tests should include these IDs in names, metadata, or documentation so release coverage is inspectable.

## Product requirements

| ID | Requirement | Design source | Primary conformance |
|---|---|---|---|
| `PRD-001` | One current task session per worktree | Product overview; invariants | E2E start/replace/reset |
| `PRD-002` | One active attempt per running session; a v1 stage attempt or v2 graph-node attempt | Invariants; domain model | Property suite `INV-S02`; v2 graph property suite |
| `PRD-003` | `next` identifies all missing required items | User workflows; CLI | JSON and preset E2E |
| `PRD-004` | Completion is blocked by missing items or blockers | State transitions | Domain and CLI tests |
| `PRD-005` | Procedure v1 retry creates a clean same-stage attempt | Rework | Domain scenario |
| `PRD-006` | Procedure v1 return creates fresh destination and downstream redo | Rework | Model/property scenario |
| `PRD-007` | Product is current-session focused | Goals/non-goals | UX review and command set |
| `PRD-008` | Four built-in presets ship as data | Presets | Schema and E2E validation |

## Architecture requirements

| ID | Requirement | Design source | Primary conformance |
|---|---|---|---|
| `ARC-001` | CLI never writes live state directly | System architecture | Integration file-access test |
| `ARC-002` | Daemon is sole normal writer | System architecture | Process and store test |
| `ARC-003` | One FIFO queue per worktree | Daemon/queue | Queue ordering suite |
| `ARC-004` | Different worktrees may execute concurrently | Daemon/queue | Multi-worktree concurrency test |
| `ARC-005` | State resides in worktree | Git/filesystem | Layout and deletion tests |
| `ARC-006` | No network listener or client | Security | Static and runtime network test |
| `ARC-007` | Pure core has no infrastructure dependencies | Rust architecture | `make test-prepare` dependency graph check |
| `ARC-008` | macOS daemon is a user LaunchAgent | macOS service | Service integration suite |

## Procedure and domain requirements

| ID | Requirement | Design source | Primary conformance |
|---|---|---|---|
| `DOM-001` | Procedure v1 is a linear ordered stage list | Procedure spec | Schema and property tests |
| `DOM-002` | Procedure contains no executable constructs | Procedure spec; security | Malicious fixture suite |
| `DOM-003` | Snapshot is immutable after start | Procedure spec | Source-drift integration test |
| `DOM-004` | Six item types have deterministic constraints | Procedure spec | Per-type unit suite |
| `DOM-005` | Old attempt values do not carry forward | Rework | Retry/return tests |
| `DOM-006` | Skip requires explicit procedure policy | State transitions | Skip matrix |
| `DOM-007` | Local required artifacts are revalidated at completion | State transitions | Artifact-change test |
| `DOM-008` | Reset defines the session-history boundary | Rework | Reset cascade test |

## Queue and storage requirements

| ID | Requirement | Design source | Primary conformance |
|---|---|---|---|
| `STO-001` | Admission is durable before acknowledgement | Transactions | Crash `C01-C03` |
| `STO-002` | Transition and terminal result commit atomically | Transactions | Crash `C07-C10` |
| `STO-003` | Idempotency prevents duplicate logical effects | Transactions | Duplicate-request test |
| `STO-004` | Same-item stale writes fail | Transactions | Item concurrency test |
| `STO-005` | Cursor commands require current revision and attempt | Transactions | Stale cursor tests |
| `STO-006` | Relational SQLite state is authoritative | SQLite model | Store architecture review |
| `STO-007` | Migrations are forward, transactional, checksummed | SQLite model | Migration suite |
| `STO-008` | Corrupt state fails closed | Recovery | Corruption injection |
| `STO-009` | Retention is bounded | Recovery | Pruning tests |
| `STO-010` | Global registry contains no task data | Recovery | Registry schema test |
| `STO-011` | Initial `schema-0`/uninitialized (`uninitialized-database`) state migrates transactionally to `schema-v1` | SQLite model | Initial-migration atomicity test |

## Interface requirements

| ID | Requirement | Design source | Primary conformance |
|---|---|---|---|
| `API-001` | Every public response is versioned JSON | JSON contract | Schema validation |
| `API-002` | Unknown fields are tolerated only in explicitly open response schemas | JSON contract | Compatibility client test |
| `API-003` | Text is not scraped as API | JSON contract | Documentation and SDK tests |
| `API-004` | IPC uses bounded length-prefixed JSON | IPC | Framing/fuzz suite |
| `API-005` | Peer UID is checked where available | IPC/security | macOS socket test |
| `API-006` | Error codes and exit mappings are stable | Errors | Catalog golden tests |
| `API-007` | Non-interactive destructive commands require `--yes` | CLI | Confirmation suite |
| `API-008` | Shell completion ships for zsh, bash, fish | CLI | Completion tests |

## Security and operations requirements

| ID | Requirement | Design source | Primary conformance |
|---|---|---|---|
| `SEC-001` | Same-user trust is explicit; no access key | Security; ADR-0006 | Documentation review |
| `SEC-002` | Worktree and runtime paths cannot escape | Git/filesystem | Symlink and traversal tests |
| `SEC-003` | Artifact bytes are never stored | Security; ADR-0009 | DB inspection test |
| `SEC-004` | Logs redact task and item content | Observability | Log capture tests |
| `SEC-005` | No telemetry or remote loading | Security | Static/runtime checks |
| `OPS-001` | LaunchAgent install is idempotent | macOS service | Service integration |
| `OPS-002` | Daemon health is queryable | Observability | Status tests |
| `OPS-003` | Doctor is read-only | Recovery/observability | Filesystem mutation audit |
| `OPS-004` | Reset-all recovers interrupted recreation | Recovery | Crash `C14-C16` |

## Release requirements

| ID | Requirement | Design source | Primary conformance |
|---|---|---|---|
| `REL-001` | Sole release tuple is `{triple: aarch64-apple-darwin, arch: arm64, host_arch: arm64, mach_o_arch: arm64}` | Release | `make dist`; native distribution qualification |
| `REL-002` | Archive contains `podway` and `podwayd`, completions, schemas, presets, license | Release | `make dist`; archive inspection |
| `REL-003` | Published archives include checksums and source/toolchain provenance | Release | `make dist`; distribution metadata inspection |
| `REL-004` | Upgrade migration is tested | Release/storage | `make test-int`; `make test-e2e` |
| `REL-005` | All product acceptance criteria pass | Product acceptance | `make test`, `make dist`; product-acceptance matrix validation |
| `REL-006` | `v0.1.0` is the first public release, while public and storage contracts remain v1 | Release packaging; release readiness | `make test-prepare`; package inspection |
| `REL-008` | Release readiness is decided solely by the repository-local `make dist` gate | ADR-0011 | `make dist` |

The omitted seventh release requirement is retired by [ADR-0011](../../architecture-decision-records/0011-local-make-test-release-gate.md), which explains the intentional ID gap.

## Podway v2 traceability contracts

The v2 acceptance baseline is organized by five manifest-bound index assets:

- [`v2-acceptance-matrix-v1.json`](../../../quality/v2-acceptance-matrix-v1.json)
  binds every mandatory §17 statement to its source line, contract paths, planned
  test class, and owning tasks;
- [`v2-compatibility-matrix-v1.json`](../../../quality/v2-compatibility-matrix-v1.json)
  closes the retained v1 families and the additive v2 compatibility boundaries;
- [`v2-payload-matrix-v1.json`](../../../quality/v2-payload-matrix-v1.json)
  assigns the complete resolved `next` shape and status projections to bounded
  wire components;
- [`v2-release-gate-matrix-v1.json`](../../../release/v2-release-gate-matrix-v1.json)
  partitions the acceptance statements into the nine release categories and
  preserves the admission and final-gate conditions;
- [`v2-fixture-catalog-v1.json`](../../../tests/fixtures/contract/v2-fixture-catalog-v1.json)
  inventories every raw v2 fixture by path and digest, binds the parser, graph,
  compatibility, and payload recipes to their §17 rows and specialized matrices,
  and keeps their future runtime ownership explicit.

Every raw file under `tests/fixtures/v2/` is also directly manifest-bound; the
catalog's one-to-one inventory and digest check is an additional closed mapping,
not an indirect substitute for manifest coverage.

The separate manifest-bound
[`dolgorae-v2-adapter-contract-v1.json`](../../../release/dolgorae-v2-adapter-contract-v1.json)
projects those repository authorities into the exact downstream route, result,
schema-pin, error, diagnostic, migration, reactivation, and acceptance boundary.
Its `prepared-not-released` status records an integration obligation only; it is
not Dolgorae-side evidence and does not alter the historical v1 DOLGI acceptance
matrix.

Rows and fixture recipes marked `planned` are traceability obligations only. The
YAML/JSON pair is a current schema known answer, but the graph, compatibility,
payload, and admission recipes are not evidence that their future production
paths, the complete §17 acceptance gate, public v2 admission, or GA release have
passed.

## Change-control rule

Automation requirements use `AUT-*` identifiers in the
[automation client contract](../interfaces/automation-client-contract.md).
Implemented requirements and their remaining integration evidence stay linked
through that contract's acceptance matrix and roadmap mappings. The
machine-validated product acceptance matrix is reserved for `PAC-*` criteria; a
dedicated machine-readable automation evidence registry requires an explicit
schema and verifier rather than additional rows in the product matrix.

A code change that affects a requirement must update:

- the relevant design document;
- affected schema or machine-readable spec;
- at least one conformance test;
- this traceability matrix when the requirement or test mapping changes.
