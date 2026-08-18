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
<effective-user-home>/.podway/logs/
```

The directory is derived from the effective OS account and does not use ambient
home-directory environment variables.

Each record is one JSON object followed by a newline. Every record has the same
keys:

```json
{"schema":"podway.daemon-log/v1","ts":1700000000123,"daemon_id":"...","seq":42,"operation":"integrity_check","outcome":"failed","command":null,"workspace_uuid":"...","session_id":null,"request_id":null,"job_id":null,"stage":"store_open","error_kind":"storage_integrity","integrity_check":"internal_codec","reason":"integrity_validation_failed","diagnostic_id":null}
```

`ts` is UTC Unix milliseconds. `daemon_id` identifies one daemon lifetime and
`seq` is assigned when an event is emitted. Sequence gaps reveal dropped events;
records from one lifetime are ordered by `seq`. Correlation keys are always
present and use `null` when the operation has no corresponding identity.
Request correlation is attached after typed envelope decoding. Workspace,
session, and job correlation uses the durable identity available at the owning
runtime boundary or the validated response projection; events emitted before
that boundary retain `null` rather than copying workspace paths or arbitrary
payload fields.

Operation, outcome, stage, error kind, integrity check, and reason are closed
internal categories. Records cannot carry task titles, item values, artifact
locations, procedure content, full requests, idempotency keys, filesystem paths,
or raw error source chains.

## Queueing and configuration

The sink uses bounded primary and priority queues. Saturation does not block request processing; dropped records and saturation episodes are counted and included in the frozen shutdown report.

Podway v0.1.0 has no log levels, filtering, or runtime log configuration.

## Rotation

Default structured-log rotation:

- 1 MiB maximum per file;
- 10 files total (`podwayd.log` plus `.1` through `.9`);
- atomic rename and reopen;
- no compression requirement;
- oldest file removed first.

A logging failure must not corrupt task state. Sink failures and dropped records are accounted internally; daemon status has no logging-warning field.

The LaunchAgent directs standard output and standard error to the separate
`podwayd-bootstrap.log` stream. Bootstrap records are JSON Lines and use a 1 MiB
per-file limit with 5 files total (`podwayd-bootstrap.log` plus `.1` through
`.4`). The daemon's rotating structured sink is the only writer to `podwayd.log`,
so launchd never retains a descriptor to a rotated structured-log file.

## Daemon status

`podway daemon status --json` reports:

```text
status
installed
loaded
reachable
daemon_version
protocol_versions
contract_manifest_digest
pid
process_id
executable_path
started_at
uptime_ms
configured_socket_path
effective_socket_path
registered_worktree_count
active_scheduler_count
queued_job_count
running_job_count
```

It contains aggregate counts only, not task titles or item data.
The process UUID and start time remain stable for one daemon lifetime, while uptime is
monotonic. A stopped or unreachable installation retains static build identity,
executable, and configured socket fields but reports every live process field as `null`.
Contract-handshake failures are returned as `DAEMON_CONTRACT_MISMATCH`, never rewritten
as an unreachable status result.

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

The fixed log schema includes `diagnostic_id`; an originating boundary that has
an opaque diagnostic UUID copies it into the corresponding record. Logs retain
typed failure categories, not raw internal source-chain text.

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
