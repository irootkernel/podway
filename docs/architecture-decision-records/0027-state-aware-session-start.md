# ADR-0027: State-Aware Session Start

- Status: Accepted
- Date: 2026-08-21
- Supersedes in part: [ADR-0026](0026-retain-inactive-terminal-sessions.md)
- Extends: [ADR-0021](0021-separate-session-preparation-from-execution.md)

## Context

The former `--replace-eligible` and `--replace` flags exposed storage-oriented
replacement modes before the user knew which current lifecycle state Podway had
observed. That made the common `start` path harder to understand and encouraged
destructive handling of work whose useful history could instead remain inactive.

Starting a new task still needs an automation-safe, race-free policy. A human
interview cannot be the wire contract, and observing then issuing several
independent mutations would expose partial state if a process stopped midway.

## Decision

`podway start` owns state-aware conflict resolution. The CLI removes
`--replace-eligible` and `--replace`.

In an interactive terminal, an existing prepared session offers continue or
permanent deletion. A running session offers continue, cancel and retain as
`superseded`, or permanent deletion. An undisposed completed or cancelled
session offers continue, record `handed_off`, or record `not_required`; either
recording choice archives the terminal session and starts the successor.
Pressing Enter or reaching end-of-input chooses continue and creates no session.
A disposed terminal session is archived automatically before the successor is
created.

Noninteractive callers use `--on-existing preserve|delete`. `preserve` applies
only to running state and requires `--supersede-reason`; it may include
`--actor`. `delete` applies to prepared or running state. Deleting running work
requires `--progress-summary` and explicit `--yes`; deleting prepared state does
not. Continuing an existing task remains interactive-only. JSON, quiet,
detached, and non-TTY execution never prompts and returns the closed
`SESSION_START_DECISION_REQUIRED` conflict when no sufficient policy exists.

Every replace branch is one sole-writer transaction. Running preservation
cancels the exact current revision, records a `superseded` disposition that
names the successor session, archives the cancelled predecessor, and creates the
prepared successor atomically. Terminal disposition plus archive plus successor
creation is likewise atomic. The 32-session inactive limit fails the whole
transaction without eviction or partial mutation. Permanent deletion remains
explicit and retains the existing fenced deletion semantics.

The public `session.start_replace` route remains decodable for compatible
automation, including its earlier payload shapes. New callers send a typed
resolution through that route. Plain local `start --dry-run` remains a
Procedure-only preview; a state policy dry-run consults the daemon and reports
the observed lifecycle and proposed action without mutation.

## Rejected alternatives

- Automatically marking every running session inactive would preserve an
  ambiguous live state and bypass a caller-owned cancellation decision.
- Always deleting prepared or running state makes irreversible loss the default.
- A sequence of public cancel, disposition, archive, and start calls could expose
  partial state after interruption and race between independently observed
  revisions.
- Keeping replacement flags as the primary interface leaves lifecycle discovery
  and policy selection in the wrong order.

## Consequences

- Human users choose from lifecycle-specific outcomes after Podway observes the
  current session.
- Automation is deterministic, prompt-free, fenced, and idempotent.
- Superseded running work remains immutable and queryable with its reason, actor,
  and successor identity.
- Reset remains an independently requested destructive operation and never
  archives implicitly.
