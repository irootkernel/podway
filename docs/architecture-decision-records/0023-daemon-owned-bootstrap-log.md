# ADR-0023: Daemon-Owned Bootstrap Log

- Status: Accepted
- Date: 2026-08-19
- Supersedes in part: [ADR-0022](0022-bounded-correlated-daemon-logs.md)

## Context

ADR-0022 separated launchd standard output and standard error from the main
structured daemon log, but launchd still opened the bootstrap log itself. An
automatically restarted process could therefore append through a descriptor
that Podway did not own, outside the daemon's rotation policy. Raw startup
errors written to standard error could also include caller-controlled paths.

## Decision

The LaunchAgent directs standard output and standard error to `/dev/null`.
`podwayd` is the only writer to `podwayd-bootstrap.log` and writes bootstrap
records synchronously through its bounded rotating file sink. The stream keeps
5 files total at 1 MiB each, including the active file.

Bootstrap records retain the `podway.daemon-bootstrap-log/v1` fixed key set.
The `message` field is always `null`; startup outcomes use only closed `stage`
and `error_kind` values. A direct foreground invocation may emit the same closed
failure record to standard error when no trusted service path is available.

Bootstrap logging remains non-authoritative. Failure to open or write the log
does not change task state or make an otherwise healthy daemon unavailable.

## Rejected alternatives

- Rotating only during explicit service commands does not cover launchd crash
  restarts and cannot control an already-open descriptor.
- Retaining raw error strings preserves diagnostics by allowing local paths and
  caller-controlled content into a persistent file.
- Removing bootstrap diagnostics entirely loses the only closed startup signal
  available before the main structured sink is active.

## Consequences

- Launchd cannot bypass Podway's bootstrap retention policy or retain a stale
  descriptor across rotation.
- Bootstrap diagnostics remain bounded and path-private across repeated startup
  failures.
- Operators continue to use `podway daemon logs` for the main structured log;
  the bootstrap stream remains a separate non-authoritative diagnostic file.
