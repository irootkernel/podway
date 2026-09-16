# ADR-0034: Remove Fail-Closed Inactive-Session Retention

- Status: Accepted
- Date: 2026-09-16
- Supersedes in part: [ADR-0026](0026-retain-inactive-terminal-sessions.md)
- Supersedes in part: [ADR-0027](0027-state-aware-session-start.md)
- Supersedes in part: [ADR-0030](0030-retire-workspaces-and-prune-stale-registry-entries.md)

## Context

ADR-0026 retains disposed terminal sessions as immutable inactive state so the
next task can start without erasing the previous terminal record. It also capped
that history at 32 sessions per worktree and failed archival, including
start-time archival, when the cap was full.

That fail-closed cap blocks ordinary succession in a long-lived worktree. The
product already has explicit purge, reset, and a skill advisory at count 10.
A second hard stop at 32 does not prevent accidental loss; it only stops new
work until the user deletes history they may still want.

Podway is still not an evidence archive. Inactive sessions remain immutable,
non-restorable, and purge-only. List and IPC responses stay bounded.

## Decision

A worktree still has at most one current session. It does not fail archival,
plain start of a disposed terminal session, or terminal start-resolution
archival because of inactive count. Automatic eviction remains forbidden.
`archive purge`, session reset, `reset --all`, and workspace removal remain the
only deletion paths.

SQLite schema v11 drops the `archive_slot BETWEEN 1 AND 32` check. Inactive
rows keep a unique positive `archive_slot`; the next slot is the first unused
positive integer so purge holes are reused.

`session.archive_list` emits `podway.session-archive-list-result/v2`. `count` is
the exact total. `sessions` is newest-first and windowed to 256 summaries with
`truncated`. `oldest` is the oldest summary, or null when the archive is empty,
so callers can name a purge candidate without paging. Show and purge keep their
v1 envelopes; `archive_slot` is an ordinary positive integer.

`SESSION_ARCHIVE_LIMIT_REACHED` remains in the error catalog for historical
documents. Current builds do not emit it.

## Rejected alternatives

- Raising 32 to another fail-closed number repeats the same surprise stop.
- Automatic oldest-session eviction still hides irreversible data loss.
- An unbounded complete list would exceed the 1 MiB frame and bounded-collection
  rules.
- Dropping the skill's count-greater-than-10 advisory removes the only
  non-blocking cleanup reminder.

## Consequences

- Task succession no longer depends on deleting retained history.
- Disk use can grow with complete retained session graphs; cleanup stays
  explicit.
- Archive list is a windowed collection with an exact total, not a retention
  cap.
- Schema v11, the list v2 family, specifications, and tests change together.
