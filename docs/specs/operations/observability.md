# Observability

## Goals

Observability explains one invoking command and its current worktree. Podway uses
validated command envelopes, committed state projections, explicit Doctor checks
and a bounded local operational journal. It creates no per-user log directory,
telemetry backend, analytics store or task-history export service.
[ADR-0038](../../architecture-decision-records/0038-remove-the-daemon.md) retires
the daemon and bootstrap log sinks with their host.

## Workspace status observability

Current status, next and observation results describe one committed snapshot:
workspace sequence, session revision, current attempt, item revisions, blockers,
goals, evidence identities and applicable delegation currency. Collections report
exact totals and truncation where required by their schemas. Compact status omits
large evidence/history projections and remains independently bounded.

There are no queue counts, running job IDs, admitted revisions or pending-mutation
flags. A read can become stale immediately after its snapshot; it does not reserve
the next mutation. Clients use explicit fences and current observation/recovery
recipes rather than interpreting human output as a synchronization mechanism.
The [JSON contract](../interfaces/json-contract.md) owns exact result fields.

## Operational journal

The owning database's `operational_journal` table is the only persistent product
operational journal. A new successful mutation atomically inserts:

| Field | Current value |
|---|---|
| `recorded_at_ms` | the transaction's single UTC millisecond timestamp |
| `level` | `info` |
| `event_name` | `mutation_committed` |
| `workspace_sequence` | the committed success sequence |
| `summary` | the canonical command name |
| `details_json` | absent/null for this event |

The event contains no task title, item value, artifact bytes/location, idempotency
key, environment or raw error chain. Success replay and rejection add no journal
entry. The journal does not record failed requests or every observed read and is
not an audit log or source for reconstructing domain state.

Retention occurs inside a successful mutation transaction: keep the newest 200
entries regardless of age, remove older entries beyond seven days or outside the
newest 10,000. Idle time causes no pruning process. Workspace removal/reset deletes
the replaced runtime journal. There is no public long-term export or journal-tail
command, and Doctor does not expose recent entries.

The Store's tested predecessor framework has a `writer_migrated` event with a
migration identifier and no task payload. The current compiled predecessor set
is empty and Engine has no active migration caller; this event is not current
production migration evidence. See the
[SQLite model](../storage/sqlite-model.md) and
[retention contract](../storage/recovery-retention-and-maintenance.md).

## Diagnostic IDs

`podway.error/v2` uses closed stable codes, retryability, exit status and bounded
structured details. Internal failure details can carry an opaque request-derived
diagnostic ID. There is no associated daemon log lookup or guaranteed retained
failure record. Raw internal source chains and filesystem-rich OS errors are not
stable machine fields.

`WORKSPACE_BUSY` and `WORKSPACE_MAINTENANCE` identify bounded wait/recovery states.
`MUTATION_OUTCOME_UNKNOWN` identifies COMMIT I/O ambiguity; clients reconcile the
caller-owned key using `outcome lookup` and the exact retry recipe. Missing lookup
is not proof of failure outside the documented retention/workspace boundary.

## Doctor output

`podway doctor` emits a validated `podway.output/v4` query envelope for
`workspace.doctor`, with `healthy`, `deep` and structured `checks` in its result.
It has no separately named Doctor result schema family. It takes exclusive native
exclusion and checks actual SQLite integrity, foreign keys, complete current and
archived graph reconstruction, retained outcomes and public status projection.
The successful current check IDs are:

- `binding_identity`;
- `schema_object_identity`;
- `writer_identity`;
- `quick_check`;
- `foreign_key_check`;
- `current_and_archived_graph_reconstruction`;
- `retained_outcome_records`;
- `public_status_projection`;
- `current_required_local_artifacts`.

The final check is `skipped` without `--deep`; deep mode rehashes current required
local artifacts and reports the checked count. A reconstructable binding can still
support truthful unhealthy results using `storage_integrity` or
`public_status_projection` failures. Unresolvable/corrupt infrastructure may instead
produce a public error envelope. Doctor does not repair runtime files, migrate a
writer, inspect a daemon, or perform a Git-index audit. Ordinary reads do not run
its expensive integrity scans.

## Performance diagnostics

The product has no decode/queue/scheduler log stream or background metrics sink.
Explicit qualification records local bounded timing outside task state, using
exact candidate bytes and owned fixture roots. The
[direct CLI performance qualification protocol](../../implementation-tips/direct-cli-performance-qualification.md)
owns warm/cold, grown-history and baseline matching. Historical feasibility
numbers and test registration do not establish a fresh performance pass.

## Daemon status

This anchor retains historical roadmap links only. `daemon status`, daemon
readiness, logging queues/rotation and bootstrap streams are retired. Their prior
accepted decisions remain in the ADR history; invoking the removed surface returns
`COMMAND_RETIRED` before Store access. No current status result describes an
installed or reachable daemon.

## User-facing privacy

All product task state and the operational journal remain under the worktree.
No account registry, global logs, network telemetry or runtime service is created
or read. Removing a worktree removes its state; removing `.podway` preserves Git
and unrelated user files. Old installation files remain the original generation's
responsibility and must not be silently deleted by a direct command.
