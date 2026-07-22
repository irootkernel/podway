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

Each record is one structured line with exactly three fields:

```text
ts=<seconds> operation=<name> outcome=<name>
```

Operation and outcome are closed internal categories. The record cannot carry task titles, item values, artifact locations, procedure content, or full requests.

## Queueing and configuration

The sink uses bounded primary and priority queues. Saturation does not block request processing; dropped records and saturation episodes are counted and included in the frozen shutdown report.

Podway v0.1.0 has no log levels, filtering, or runtime log configuration.

## Rotation

Default rotation:

- 10 MiB maximum active file;
- 5 rotated files;
- atomic rename and reopen;
- no compression requirement;
- oldest file removed first.

A logging failure must not corrupt task state. Sink failures and dropped records are accounted internally; daemon status has no logging-warning field.

## Daemon status

`podway daemon status --json` reports:

```text
status
installed
loaded
reachable
daemon_version
protocol_versions
pid
process_id
started_at
uptime_ms
socket_path
registered_worktree_count
active_scheduler_count
queued_job_count
running_job_count
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

Entries contain summaries and IDs, not item values. The journal has no public long-term export command, and doctor does not expose its recent entries.

## Diagnostic IDs

Unexpected internal errors generate an opaque diagnostic UUID in the public error details. The three-field event log does not carry diagnostic IDs or internal source-chain text in v0.1.0.

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
