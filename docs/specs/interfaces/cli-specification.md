# CLI Specification

`podway` is the only supported user and automation CLI. It discovers the owning
Git worktree, communicates with `podwayd` for runtime operations, and emits either
human-readable text or one JSON document. It never writes SQLite directly.

## Command groups

- Static and service: `help`, `version`, `completions`, `daemon`, `workspace`, and
  `reset --all`.
- Procedures: `preset list|show|explain` and `procedure
  validate|show|format|vet|graph|preview|lint|check|scaffold`.
- Session reads: `status`, `next`, `observe`, `evidence read`, `archive list|show`, and
  `job list|status|wait|lookup|cancel`.
- Session mutations: `start`, `begin`, `complete`, `skip`, `retry`, `block`,
  `unblock`, `cancel`, `disposition handed-off|not-required`, `reset`, `archive`,
  `archive purge`, `decide`,
  `rework`, `goal define|revise|assess-criterion`,
  `check|uncheck|set|add|remove|attach|clear`, and `record --stdin`.

The executable grammar is owned by the command catalog and clap definitions.
Removed commands are not aliases and must fail argument parsing.

`daemon wait-ready [--timeout <duration>]` is a read-only service
operation. Its default deadline is 120 seconds and its hard maximum is one hour.
It succeeds only after a v2 status response proves matching live identity,
matching contract identity, and `readiness_state: ready`; it never starts,
installs, restarts, repairs, or otherwise mutates the service. `daemon install`
uses the same bounded waiter after idempotent service reconciliation.

## Procedure commands

All procedure commands accept only `podway.procedure/v2`. Validation and authoring
use the same parser, semantic validator, canonicalizer, diagnostics catalog, and
bounded source reader as daemon admission. `format --write` is the only authoring
operation that modifies a file; it validates and renders fully before an atomic
same-directory replacement. Other procedure commands are read-only. Unsupported
schemas report `PROCEDURE_SCHEMA_UNSUPPORTED` or `PROCEDURE_INVALID` as appropriate.

Procedure inspection and authoring output preserves optional `required_when` and
option `guards` structurally. Validation rejects unknown operators, wrong literal
types, conditional chains, invalid controller order, unselected or optional guard
sources, and out-of-bound predicate arrays before start. Lint diagnoses ambiguous
labels and weak criteria rather than warning from option or cycle counts.

Built-in catalog commands expose only `analysis-v2`, `bug-fix-v2`,
`small-change-v2`, and `sw-dev-v2`. `start` accepts one preset or one safe worktree-local Procedure
path, an optional expected Procedure digest for file sources, a nonempty task
title, and the documented existing-session policy and dry-run controls. Start creates a
prepared session and never accepts or creates initial goal state.

`begin` accepts optional initial goal inputs and actor attribution, fences the
prepared session, and atomically creates the first running attempt. Goal input is
allowed only when the admitted Procedure enables goal tracking and retains the
existing goal and criterion bounds.

## Evidence reads

`evidence read --source <graph-node-id> --item <item-id> [--page-token <token>]` returns one bounded page of an item selected by a currently resolved, declared evidence reference of the current consumer attempt. It is a query: it creates no durable job, no session revision, and no state change, and it cannot browse arbitrary attempts or stale history.

A read without a token starts at the beginning of the value. A response carries the complete-value digest, the total logical size, the page, a `truncated` flag, and a nullable continuation token. Text offsets and sizes count Unicode scalar values, list offsets and sizes count entries, and scalar item types return one terminal page. The token is opaque, bounded to 256 base64url characters, and is not an authentication or authorization credential.

## Mutation rules

Automation mutations require explicit identity and revision preconditions and an
idempotency key. Human mode may obtain current fences from the daemon, but the same
closed transition evaluator applies. `--detach` returns a durable admission receipt;
otherwise the CLI waits for and renders the immutable terminal result. Unknown
outcomes are reconciled with `job lookup --idempotency-key` before retry.

`retry` remains on the active action node. `rework --to <node>` uses the Procedure
v2 manual-rework contract. Decisions, goals, and criterion assessments use their
typed commands. Cursor changes occur only through declared graph effects.

Decision guidance renders the complete authored options and separately marks the
authoritative allowed option IDs and bounded guard statuses. Human output explains
unmet and unevaluable predicates without printing text or list content. A guarded
selection rejected after fresh evidence validation returns
`OPTION_GUARD_UNSATISFIED`; automation branches on its structured details rather
than the message.

Prepared sessions expose no cursor-bearing mutations. `disposition handed-off`
requires summary and reference, while `disposition not-required` requires a
reason. Each form accepts optional actor attribution and exact terminal-session
fences. These values are caller assertions; the CLI does not inspect Git or
validate external handoff semantics.

`reset` without `--yes` deletes only a prepared session or a completed or
cancelled session with a disposition for its current revision. `reset --dry-run`
reports the same current eligibility without mutation. Force reset uses `--yes`
and requires `--progress-summary` for a running or undisposed terminal session.
`start` first observes the current lifecycle. Human TTY mode offers continue or
delete for prepared state; continue, preserve as superseded, or delete for
running state; and continue, handed-off, or not-required for an undisposed
terminal state. Enter or EOF continues without creating a session. JSON, quiet,
detached, non-TTY, and policy invocations never prompt. Automation uses
`--on-existing preserve|delete`; preserve requires `--supersede-reason` and may
include `--actor`, while deleting a running session requires
`--progress-summary` and `--yes`. Missing policy returns
`SESSION_START_DECISION_REQUIRED` with no mutation.
An interactive continue result performs a fresh idle-barrier read and renders
current guidance after stating that no session was created. Prepared and running
sessions use `session.next`; completed and cancelled sessions use the terminal-safe
`session.observe` route.

Plain `start` archives a completed or cancelled current session whose disposition
is current, then creates the new prepared session. That automatic path rejects
`--on-existing`, supersede, actor, and progress-summary fields instead of silently
discarding them. Preserving a running session
atomically cancels it, records `superseded` with the new session ID, archives it,
and creates the prepared successor. Recording a disposition during terminal
start resolution, archiving, and successor creation are also atomic. `archive` explicitly
retains an eligible terminal session as inactive. `archive list` returns at most
32 summaries; `archive show --session-id <uuid>` returns one immutable status
projection plus its terminal disposition and accepts the normal verbose history controls. `archive purge`
requires the inactive session ID, `--if-session-revision`, and `--yes` and deletes
only that retained session. Purge is a fenced, serialized control-plane deletion,
not a durable job. If its response is lost, re-read `archive list`; absence proves
the requested deletion outcome but does not identify which caller completed it.
No archive command restores or mutates inactive state.

Plain `start --dry-run` without an existing-session policy remains a local
Procedure preview. A policy dry run consults the daemon and returns the observed
session identity, revision, lifecycle, and proposed action without mutation.

`record --stdin` is the only multi-item mutation grammar. It reads at most 1 MiB
of closed `podway.item-record-many-input/v1` JSON. The document supplies the
workspace, session revision, active attempt, idempotency key, and 1..128 unique
item-local revision fences. The daemon canonicalizes operations by item ID and
records or clears the complete set atomically without advancing the cursor.
Identity, revision, and idempotency flags must not duplicate the stdin fields.

A `check_result` record uses this same stdin grammar and has no single-item CLI
alias. Observation may provide one copyable closed stdin template for the first
unsatisfied check-result item, but the caller must replace its result and
idempotency placeholders after performing the external check. Podway does not
invoke the operation named by the declaration. Human output calls the value a
recorded or structurally bound external check result and never describes it as
verified, trusted, attested, or secure evidence.

## Output and exits

Successful JSON output uses `podway.output/v3`; failures use `podway.error/v1`.
Machine clients consume schema IDs, command names, stable fields, and error codes,
never text. Text mode is advisory rendering of the same result. Exit 0 is success;
1 is a valid negative authoring/check result; 2 is usage; 3 is configuration; 4 is
daemon communication; 5 is state/precondition rejection; and 6 is internal failure.

Every JSON invocation emits exactly one bounded newline-terminated object on stdout.
Diagnostics go to the structured envelope in JSON mode and stderr only where the
command contract explicitly has no JSON response.
