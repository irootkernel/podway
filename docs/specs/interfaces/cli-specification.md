# CLI Specification

`podway` is the only supported user and automation CLI. It discovers the owning
Git worktree, communicates with `podwayd` for runtime operations, and emits either
human-readable text or one JSON document. It never writes SQLite directly.

## Command groups

- Static and service: `help`, `version`, `completions`, `daemon`, `workspace`, and
  `reset --all`.
- Procedures: `preset list|show|explain` and `procedure
  validate|show|format|vet|graph|preview|lint|check|scaffold`.
- Session reads: `status`, `next`, and `job list|status|wait|lookup|cancel`.
- Session mutations: `start`, `complete`, `skip`, `retry`, `block`, `unblock`,
  `cancel`, `reset`, `decide`, `rework`, `goal define|revise|assess-criterion`,
  and `check|uncheck|set|add|remove|attach|clear`.

The executable grammar is owned by the command catalog and clap definitions.
Removed commands are not aliases and must fail argument parsing.

## Procedure commands

All procedure commands accept only `podway.procedure/v2`. Validation and authoring
use the same parser, semantic validator, canonicalizer, diagnostics catalog, and
bounded source reader as daemon admission. `format --write` is the only authoring
operation that modifies a file; it validates and renders fully before an atomic
same-directory replacement. Other procedure commands are read-only. Unsupported
schemas report `PROCEDURE_SCHEMA_UNSUPPORTED` or `PROCEDURE_INVALID` as appropriate.

Built-in catalog commands expose only `sw-dev-v2` and `bug-fix-v2`. `start` accepts
one preset or one safe worktree-local procedure path, an optional expected procedure
digest for file sources, a nonempty task title, optional v2 goal inputs, and the
documented replacement/dry-run controls.

## Mutation rules

Automation mutations require explicit identity and revision preconditions and an
idempotency key. Human mode may obtain current fences from the daemon, but the same
closed transition evaluator applies. `--detach` returns a durable admission receipt;
otherwise the CLI waits for and renders the immutable terminal result. Unknown
outcomes are reconciled with `job lookup --idempotency-key` before retry.

`retry` remains on the active action node. `rework --to <node>` uses the Procedure
v2 manual-rework contract. Decisions, goals, and criterion assessments use their
typed commands. Cursor changes occur only through declared graph effects.

## Output and exits

Successful JSON output uses `podway.output/v3`; failures use `podway.error/v1`.
Machine clients consume schema IDs, command names, stable fields, and error codes,
never text. Text mode is advisory rendering of the same result. Exit 0 is success;
1 is a valid negative authoring/check result; 2 is usage; 3 is configuration; 4 is
daemon communication; 5 is state/precondition rejection; and 6 is internal failure.

Every JSON invocation emits exactly one bounded newline-terminated object on stdout.
Diagnostics go to the structured envelope in JSON mode and stderr only where the
command contract explicitly has no JSON response.
