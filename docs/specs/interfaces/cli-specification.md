# CLI Specification

`podway` is the only supported user and automation CLI. It discovers the owning
Git worktree, communicates with `podwayd` for runtime operations, and emits either
human-readable text or one JSON document. It never writes SQLite directly.

## Command groups

- Static and service: `help`, `version`, `completions`, `daemon`, `workspace`, and
  `reset --all`.
- Procedures: `preset list|show|explain` and `procedure
  validate|show|format|vet|graph|preview|lint|check|scaffold`.
- Session reads: `status`, `next`, `observe`, and
  `job list|status|wait|lookup|cancel`.
- Session mutations: `start`, `begin`, `complete`, `skip`, `retry`, `block`,
  `unblock`, `cancel`, `disposition handed-off|not-required`, `reset`, `decide`,
  `rework`, `goal define|revise|assess-criterion`,
  `check|uncheck|set|add|remove|attach|clear`, and `record --stdin`.

The executable grammar is owned by the command catalog and clap definitions.
Removed commands are not aliases and must fail argument parsing.

## Procedure commands

All procedure commands accept only `podway.procedure/v2`. Validation and authoring
use the same parser, semantic validator, canonicalizer, diagnostics catalog, and
bounded source reader as daemon admission. `format --write` is the only authoring
operation that modifies a file; it validates and renders fully before an atomic
same-directory replacement. Other procedure commands are read-only. Unsupported
schemas report `PROCEDURE_SCHEMA_UNSUPPORTED` or `PROCEDURE_INVALID` as appropriate.

Built-in catalog commands expose only `bug-fix-v2`, `small-change-v2`, and
`sw-dev-v2`. `start` accepts one preset or one safe worktree-local Procedure
path, an optional expected Procedure digest for file sources, a nonempty task
title, and the documented replacement and dry-run controls. Start creates a
prepared session and never accepts or creates initial goal state.

`begin` accepts optional initial goal inputs and actor attribution, fences the
prepared session, and atomically creates the first running attempt. Goal input is
allowed only when the admitted Procedure enables goal tracking and retains the
existing goal and criterion bounds.

## Mutation rules

Automation mutations require explicit identity and revision preconditions and an
idempotency key. Human mode may obtain current fences from the daemon, but the same
closed transition evaluator applies. `--detach` returns a durable admission receipt;
otherwise the CLI waits for and renders the immutable terminal result. Unknown
outcomes are reconciled with `job lookup --idempotency-key` before retry.

`retry` remains on the active action node. `rework --to <node>` uses the Procedure
v2 manual-rework contract. Decisions, goals, and criterion assessments use their
typed commands. Cursor changes occur only through declared graph effects.

Prepared sessions expose no cursor-bearing mutations. `disposition handed-off`
requires summary and reference, while `disposition not-required` requires a
reason. Each form accepts optional actor attribution and exact terminal-session
fences. These values are caller assertions; the CLI does not inspect Git or
validate external handoff semantics.

`reset` without `--yes` deletes only a prepared session or a completed or
cancelled session with a disposition for its current revision. `reset --dry-run`
reports the same current eligibility without mutation. Force reset uses `--yes`
and requires `--progress-summary` for a running or undisposed terminal session.
`start --replace-eligible` applies the default eligibility predicate atomically;
`start --replace --yes` is the force replacement form and applies the same
progress-summary rule. Every deletion or replacement remains fully fenced.

`record --stdin` is the only multi-item mutation grammar. It reads at most 1 MiB
of closed `podway.item-record-many-input/v1` JSON. The document supplies the
workspace, session revision, active attempt, idempotency key, and 1..64 unique
item-local revision fences. The daemon canonicalizes operations by item ID and
records or clears the complete set atomically without advancing the cursor.
Identity, revision, and idempotency flags must not duplicate the stdin fields.

## Output and exits

Successful JSON output uses `podway.output/v3`; failures use `podway.error/v1`.
Machine clients consume schema IDs, command names, stable fields, and error codes,
never text. Text mode is advisory rendering of the same result. Exit 0 is success;
1 is a valid negative authoring/check result; 2 is usage; 3 is configuration; 4 is
daemon communication; 5 is state/precondition rejection; and 6 is internal failure.

Every JSON invocation emits exactly one bounded newline-terminated object on stdout.
Diagnostics go to the structured envelope in JSON mode and stderr only where the
command contract explicitly has no JSON response.
