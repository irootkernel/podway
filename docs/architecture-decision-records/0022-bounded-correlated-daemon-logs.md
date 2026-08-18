# ADR-0022: Bounded Correlated Daemon Logs

- Status: Accepted
- Date: 2026-08-18
- Extends: [ADR-0012](0012-explicit-daemon-endpoint-and-canonical-per-user-podway-home.md)
- Superseded in part by: [ADR-0023](0023-daemon-owned-bootstrap-log.md)

## Context

The original daemon log recorded only a timestamp, operation, and outcome. A
workspace Store-open failure therefore collapsed to a generic failed integrity
event and could not be correlated with a daemon lifetime, workspace, session,
request, or job. The LaunchAgent also shared that file with the daemon's
rotating sink, so launchd could retain an older descriptor after rotation.

## Decision

The daemon log is JSON Lines with a fixed, versioned key set. Every record
contains a daemon-lifetime identifier and emission sequence. Command,
workspace, session, request, job, stage, typed error, integrity check, reason,
and diagnostic identifiers are always represented and use `null` when they do
not apply. Logs retain closed summaries only and exclude payloads, task content,
paths, idempotency keys, and raw source chains.

`podwayd.log` is bounded to 1 MiB per file and 10 files total, including the
active file. LaunchAgent standard output and standard error use the separate
`podwayd-bootstrap.log`, bounded to 1 MiB per file and 5 files total. Explicit
log purge owns both streams and their numbered rotations. The existing
`podway daemon logs` command continues to select the main structured log.

## Rejected alternatives

- Keeping the three-field text format cannot reconstruct one failing request or
  daemon lifetime.
- Logging raw error chains risks leaking paths and caller-controlled content.
- Sharing the rotating structured log with launchd preserves stale-descriptor
  ambiguity after rename and reopen.
- Unbounded logs trade a local diagnostic gap for uncontrolled per-user state.

## Consequences

- Operators can order records by daemon ID and sequence even when wall-clock
  timestamps collide.
- Dropped records remain visible as sequence gaps while request processing stays
  non-blocking.
- The global layout gains one non-authoritative bootstrap log stream.
- Log consumers must parse JSON Lines rather than the legacy text form.
