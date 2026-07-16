# Observability

## Goals

Observability exists to operate the daemon and diagnose the current task workflow. It must not become a global task-history or analytics system.

Podway provides:

- structured local daemon logs;
- `podway daemon status`;
- `podway doctor`;
- queue and revision indicators in `status`;
- diagnostic IDs for internal failures;
- bounded worktree operational journal.

Podway provides no telemetry backend or network export.

## Daemon logs

Default macOS directory:

```text
~/Library/Logs/Podway/
```

Log records are structured and include:

```text
timestamp
level
component
event
workspace_uuid when applicable
job_id and sequence when applicable
command name
error code
duration_ms
diagnostic_id when applicable
```

They exclude task titles, item values, artifact locations, procedure content, and full requests by default.

## Log levels

- `error`: operation cannot continue or invariant failed;
- `warn`: recoverable anomaly, invalid client, stale registry, or pruning issue;
- `info`: daemon lifecycle, workspace scheduler lifecycle, job terminal state, migration;
- `debug`: bounded protocol and transaction diagnostics without payload values;
- `trace`: development-only, disabled in public builds unless explicitly enabled.

Runtime log level is configured through daemon installation metadata or a documented environment variable set in the LaunchAgent, not worktree config.

## Rotation

Default rotation:

- 10 MiB maximum active file;
- 5 rotated files;
- atomic rename and reopen;
- no compression requirement;
- oldest file removed first.

A logging failure must not corrupt task state. Persistent inability to write logs is reported by daemon status as a warning.

## Daemon status

`podway daemon status --json` reports:

```text
installed
loaded
reachable
pid
version
supported_protocols
started_at
uptime_ms
socket_path
log_path
registered_worktrees
active_schedulers
queued_jobs
running_jobs
last_fatal_diagnostic_id
```

It contains aggregate counts only, not task titles or item data.

## Workspace status observability

`podway status` includes:

- latest committed workspace sequence;
- session revision;
- queued count;
- running job ID;
- pending mutation flag;
- current attempt ID and number;
- current item revisions;
- open blocker IDs.

These fields let clients reason about staleness without reading logs.

## Operational journal

The worktree database keeps a bounded journal for events such as:

```text
workspace.initialized
workspace.migrated
daemon.recovered-jobs
job.admitted
job.claimed
job.succeeded
job.failed
job.cancelled
session.started
session.transitioned
session.reset
retention.pruned
integrity.failed
```

Entries contain summaries and IDs, not item values. The journal has no public long-term export command. `doctor --deep` may inspect recent entries for diagnosis.

## Diagnostic IDs

Unexpected internal errors generate an opaque diagnostic UUID. The public error includes it, and the daemon log includes the same value with internal source-chain details.

Example:

```text
INTERNAL_ERROR diagnostic_id=0e8a...
```

This lets developers correlate a report without making internal messages part of the public API.

## Doctor output

Doctor emits a structured check list:

```json
{
  "checks": [
    {
      "id": "database.integrity",
      "status": "pass",
      "summary": "SQLite quick check passed.",
      "remediation": null
    }
  ],
  "overall": "pass"
}
```

Statuses are `pass`, `warning`, `fail`, or `skipped`.

## Performance diagnostics

Debug logs and status MAY report:

- IPC decode duration;
- job queue wait duration;
- state transaction duration;
- artifact hash duration and bytes;
- database open and migration duration;
- scheduler count and backlog.

No global analytics store is created. Measurements are local and bounded.

## User-facing privacy

The product documentation must state:

- all task state remains in the worktree;
- a minimal root registry and redacted logs exist outside it;
- no network telemetry is sent;
- deleting a worktree deletes task state but not daemon logs that may contain identifiers and error summaries;
- `podway daemon uninstall --purge-logs --yes` removes logs explicitly.
