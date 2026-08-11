# CLI Specification

## Command name and binaries

The public command is `podway`. The daemon binary is `podwayd` and is normally managed through `podway daemon ...`.

This document describes the implemented v0.1.2 grammar, including its automation
options and result surfaces.

## Global options

```text
--json                         emit the versioned JSON contract
--dev                          use the isolated contributor daemon and state tree
--worktree <path>              target an explicit Git worktree
--socket <absolute-path>       target the only permitted daemon endpoint
--timeout <duration>           bound daemon connection or wait time
--no-color                     disable color in text output
--quiet                        suppress nonessential text output
--idempotency-key <string>     override generated mutation key
--detach                       return after durable job admission
--if-workspace-uuid <uuid>     require an exact workspace identity
--if-session-id <uuid>         require an exact session identity
--if-session-revision <n>      require an exact session revision
--if-attempt <uuid>            require an exact active attempt
--if-item-revision <n>         require an exact item revision
--if-goal-revision <n>         require an exact positive goal revision
--yes                          approve a required destructive confirmation
```

Not every option applies to every command. Inapplicable options are usage errors rather than silently ignored.

`--dev` is a contributor and packaged-conformance surface, not the normal installed
service workflow. It is mutually exclusive with `--socket` and with every
`podway daemon ...` service lifecycle command.

Durations accept `ms`, `s`, and `m`, for example `500ms`, `30s`, and `2m`.

## Output modes

- Human-readable text is the default.
- `--json` emits exactly one success or error JSON object to stdout. The compact
  `version --json` success form is the deliberate exception to the common output
  envelope and is exactly `{"name":"podway","version":"v0.1.2"}` for this release.
- Diagnostics not represented in the JSON object go to stderr only for process-level failures before JSON can be produced.
- Color is used only on a TTY and never in JSON.
- Text wording is not a stable API. JSON fields, schemas, error codes, and exit codes are the stable integration contract.

## Static commands

The following do not require a worktree or daemon unless explicitly noted:

```bash
podway help [topic]
podway version
podway preset list
podway preset show <name>
podway preset explain <name>
podway procedure validate <file>
podway procedure show <file>
podway procedure format <file> [--check] [--write]
podway procedure vet <file>
podway procedure graph <file> --format <json|mermaid|puml|dot>
podway procedure preview <worktree-relative-file>
podway procedure lint <file> [--warnings-as-errors]
podway procedure check <file> [--warnings-as-errors]
podway procedure scaffold [--template minimal]
podway procedure convert <file>
podway daemon ...
```

`procedure validate`, `procedure show`, `procedure format`, `procedure vet`, `procedure graph`, `procedure preview`, `procedure lint`, `procedure check`, `procedure scaffold`, and `procedure convert` use the same Rust schema and canonicalization library as the daemon.
`podway version --json` emits only the compact `name` and `version` summary. The
target machine identity form is `podway version --json --identity`; it retains the
versioned output envelope and requires no worktree, daemon, `HOME`, or `TMPDIR`.
Its closed result includes `schema: podway.version-result/v1` and is identical
after JSON parsing to the daemon binary's identity result.

The daemon binary follows the same version grammar:

```text
podwayd version
podwayd version --json
podwayd version --json --identity
```

Its compact form is `{"name":"podwayd","version":"v0.1.2"}`. The identity
form retains the versioned output envelope and is the interface used by service
installation and release qualification to verify the embedded contract. Those
runtime probes validate the complete typed envelope and reject bare or malformed
identity results before using any reported field.

## Procedure v2 contract grammar

The status and next shapes below are executable for Procedure v2 sessions.
Procedure v2 authoring and preview commands are executable and are specified in
their dedicated sections below. Shared item mutations, `session.complete`,
`session.decide`, `session.rework`, `goal.define`, and `goal.revise` are executable
for their matching session states and return their registered v2 result families.
Goal-bearing start and replacement and all three goal mutation routes are
executable through the typed Procedure v2 request boundary. For an active
Procedure v2 session, the CLI uses
the standard status projection to supply omitted command-specific fences. An
invocation may instead provide every required fence explicitly and skip that
preflight.

```text
podway status --verbose [--history-before <trace-sequence>]
```

All Procedure v2 authoring commands use the shared structured diagnostic family
for failures. `status --verbose` returns the six trace-sequenced history windows
defined by `status-result/v2`; `--history-before` applies the exclusive cursor to
each window. Standard status does not return history windows.

## Procedure v2 preview and confirmation

`procedure preview` never admits a job, creates a session, or writes state. A
preview first applies the same bounded parse and semantic validation as
`procedure validate`, then the same structural, liveness, and resource-budget
analysis as `procedure vet`, and finally the same advisory rules as the lint
command. Its file spelling must be UTF-8 and satisfy the same worktree-relative,
no-parent rule as `start`, so every emitted start suggestion is accepted by the
command grammar. The result always reports `admissible`, the three check outcomes, and
bounded ordered diagnostics. Validation or vet failure makes the preview
inadmissible and exits 1. Lint warnings set the lint check false but remain
advisory: they do not make an otherwise valid and vetted Procedure inadmissible
or change the successful exit status, and preview has no warnings-as-errors mode.

Only an admissible success reports Procedure identity, summary, normalized graph,
Mermaid, and the SHA-256 digest of the same canonical semantics that a custom
`session.start` admission validates. It also returns exactly this structured start
suggestion, leaving only the caller-owned title as a placeholder:

```text
podway start --procedure <file> --expect-procedure-digest <digest> --task <title>
```

Human output renders the complete identity, checks, goal policy, summary,
normalized nodes and edges, Mermaid, digest, and a POSIX-shell-safe spelling of
that same suggestion.

An inadmissible result returns no digest, graph, Mermaid, or start suggestion.
Preview remains unconditionally read-only on both paths: it never admits a job,
creates or resumes a session, mutates the source, or persists workspace state.

Starting a custom Procedure v2 requires `--expect-procedure-digest <digest>`
equal to the validated canonical digest. Semantic edits invalidate an earlier
confirmation; formatting and ordering changes that preserve canonical semantics
do not.
Built-in v2 presets use their shipped digest and do not require interactive
confirmation, but a shipped-digest mismatch fails closed. Preview remains
read-only regardless of confirmation or admissibility.

## PATH invocation and runtime environment

Automation may invoke `podway` by command name through a controlled `PATH`,
including when that entry is a symlink. Static and daemon-backed commands operate
from arbitrary working directories without `HOME`, `TMPDIR`, or `XDG_*` when the
required absolute worktree and socket are supplied.

`podway daemon install` resolves `podwayd` in this order: an explicit
`--daemon-path`, a sibling of the canonicalized current CLI executable, and then
the controlled `PATH`. The selected daemon is canonicalized and identity-checked,
and the LaunchAgent receives its absolute path rather than relying on a login
shell or interactive environment.

## Help topics

```bash
podway help workflow
podway help rework
podway help automation
podway help procedures
podway help daemon
podway help artifacts
```

Help MUST include complete examples and must not require internet access.

## Daemon commands

### Foreground dev mode

```bash
podwayd --dev
podway --dev --worktree /absolute/worktree status
podway --dev terminate
```

The daemon uses `~/.podway/dev/` by default. Contributors may set
`PODWAY_DEV_HOME` to an absolute private directory; the variable is read only in
dev mode. Dev `run`, `state`, and `logs` are isolated from production, while the
production singleton lock remains shared so both modes cannot run simultaneously.
`terminate` is dev-only, idempotently removes a safe stale dev socket, waits for an
orderly daemon shutdown, and succeeds only after the dev socket is absent. Registry
and log files are preserved.

### Install

```bash
podway daemon install [--daemon-path <path>] [--socket <absolute-path>]
```

Installs or updates the user LaunchAgent and waits for health.

### Uninstall

```bash
podway daemon uninstall [--purge-logs] [--yes]
```

Does not remove worktree-local state.

### Lifecycle

```bash
podway daemon start
podway daemon stop
podway daemon restart
podway daemon status
podway daemon logs [--follow] [--lines <n>]
```

`daemon status --json` returns service, socket, protocol, and queue summary.
The target result also returns the configured/effective socket, daemon executable,
process UUID and numeric PID, start time, and contract-manifest identity. Live process
fields are `null` when the installed service is stopped or cannot answer the status
probe; static installed-binary identity and configuration remain present.

## Workspace commands

### Initialize

```bash
podway init [--repair]
```

Creates or validates `.podway/` and initializes the worktree database. It never starts a task.

### Doctor

```bash
podway doctor [--deep]
```

Checks:

- daemon reachability and version;
- Git worktree validity;
- path containment and permissions;
- workspace configuration;
- runtime ignore status;
- database schema and integrity;
- workspace UUID conflicts;
- queue recovery state.

`--deep` additionally revalidates the Git-to-Store workspace binding. Doctor remains read-only; the store layer's deep SQLite integrity mode is not wired into this command in v0.1.0.

### Workspace inspection and repair

```bash
podway workspace show
podway workspace repair
```

Repair updates a moved worktree's minimal daemon registry after proving identity. It does not adopt conflicting copied state or change session semantics.

## Procedure commands

```bash
podway procedure validate <file> [--warnings-as-errors]
podway procedure show <file> [--canonical]
podway procedure format <file> [--check] [--write]
podway procedure vet <file>
podway procedure graph <file> --format <json|mermaid|puml|dot>
podway procedure preview <worktree-relative-file>
podway procedure lint <file> [--warnings-as-errors]
podway procedure check <file> [--warnings-as-errors]
podway procedure scaffold [--template minimal]
podway procedure convert <file>
```

`--canonical` prints Podway Canonical JSON v1. With `--json`, validation returns digest, warnings, normalized metadata, and errors.

`procedure validate` dispatches on the schema the document declares, and the two versions report different families. A Procedure v1 document is unchanged in every respect: the same `podway.output/v1` envelope, the same `podway.procedure-validation-result/v1` result carrying digest, canonical JSON, normalized metadata and warnings, the same one-line text summary, and the same `podway.error/v1` failures — a document that declares no schema, declares an unknown one, or does not decode at all is a Procedure v1 input and reports exactly what it always did. A Procedure v2 document reports `podway.output/v2` with `podway.procedure-diagnostics-result/v1` and `operation: "validate"`: an admissible document carries its `digest`, `valid: true`, and no diagnostics with exit code 0, while a rejected one carries no `digest`, `valid: false`, and the single diagnostic that describes the rejection with exit code 1. Validation is a two-stage single-error pipeline — parsing maps the document into the model and closed-reference validation resolves what it declares against what it uses — so a v2 result carries zero diagnostics or exactly one, never more. Each diagnostic names its catalog code and the authored field to edit; when the rejection carries the offending value, the location pins the source line it sits on, and a structural rejection with no single offending value degrades deterministically to the nearest containing line. `--warnings-as-errors` is accepted for a Procedure v2 document and does nothing: every diagnostic validate can emit is catalogued as an error, so there is no warning for the policy to promote — `procedure lint` and `procedure check` are the advisory stages the flag exists for.

`procedure format` renders a Procedure v2 document in canonical authoring form. The default mode and `--check` print to stdout and never write the file; only `--write` replaces it. Authoring successes — including the structured findings that describe a document Podway cannot render — use the `podway.output/v2` envelope: a rendered document carries `podway.procedure-source-result/v1`, and findings carry the shared `podway.procedure-diagnostics-result/v1` family with the exit code 1 that a document-level error implies. Process failures keep `podway.error/v1`: a missing or unsafe path, a Procedure v1 input (`PROCEDURE_SCHEMA_UNSUPPORTED`), and invalid usage all report there. `--check` reads the file and never writes it: an already-canonical source reports `podway.procedure-source-result/v1` with `mode: "check"`, `changed: false`, and exit code 0, while a drifted source reports the diagnostics family with exactly one `FORMAT_NOT_CANONICAL` error located at the first line that differs and exit code 1. `--write` rewrites the named file and only that file. Every stage that can refuse a document — the hardened path walk, the parse, validation, the supported-construct scan, and the projection bound — completes in memory before the filesystem is touched, so a refused document is byte-identical afterwards and no staging file is left behind. Supported full-line comments are carried into the rewritten document. An already-canonical source is not written at all: the result is `podway.procedure-source-result/v1` with `mode: "write"`, `changed: false`, exit code 0, and an unchanged modification time. A drifted source is replaced atomically inside its own directory — the canonical bytes are staged in a sibling temporary file that carries the original's permission bits, flushed to the device, then renamed over the target — and reports the same family with `changed: true` and exit code 0. When the filesystem refuses the write before the rename, the original survives intact, the staging file is removed, and the failure reports `INTERNAL_ERROR` in `podway.error/v1`; a failure to flush the directory entry after the rename reports the same error even though the replacement already took effect. `--check` and `--write` are mutually exclusive.

`procedure vet` is an unconditionally read-only Procedure v2 graph gate. It parses and validates before running the complete graph-wide structural, liveness, and resource-budget analysis. A parse or validation failure stops the pipeline and reports that single error in `podway.procedure-diagnostics-result/v1` with `operation: "vet"`, `valid: false`, no `digest`, and exit code 1. Once validation succeeds, the result always carries the validated canonical digest; a clean document has no diagnostics, `valid: true`, and exit code 0, while any vet finding is a catalogued error and produces `valid: false` and exit code 1. Findings are deterministically ordered, bounded at 256 diagnostics, and retain the pre-truncation total. Process failures keep `podway.error/v1`: a missing or unsafe path and a Procedure v1 input (`PROCEDURE_SCHEMA_UNSUPPORTED`) report there.

`procedure graph <file> --format <json|mermaid|puml|dot>` emits the deterministic canonical JSON, Mermaid review, PlantUML state, or Graphviz DOT projection of a Procedure v2 graph and never writes anything. The command opens the source through the hardened read-only path walk, then parses, validates, vets, and projects those exact bytes. A parse or validation failure reports `podway.procedure-diagnostics-result/v1` with `operation: "graph"`, no `digest`, and exit code 1. A vet or projection-budget rejection reports the same family with the validated canonical `digest` and exit code 1, so no invalid graph can be projected. Success reports `podway.procedure-graph-result/v1` in `podway.output/v2`, binding the selected `projection` to both its `procedure_digest` and exact-byte `projection_digest`; text mode writes the projection followed by exactly one newline. Mermaid, PlantUML, and DOT carry the canonical procedure digest as metadata, distinguish actions, decisions, and goal assessments, label entry, terminal, skippable, and manual-rework-target placements, label decision routes with option and effect, and never invent evidence or manual-rework flow edges. The CLI spelling `puml` serializes the public result format as `plantuml`; `plantuml` itself is not a CLI alias. Podway emits DOT text without invoking Graphviz. Process failures keep `podway.error/v1`: a missing or unsafe path and a Procedure v1 input (`PROCEDURE_SCHEMA_UNSUPPORTED`) report there.

`procedure lint` reports advisory authoring findings for a Procedure v2 document and never writes anything. It parses and validates first: a document that fails either stage reports that single error in `podway.procedure-diagnostics-result/v1` with `valid: false`, no `digest`, and exit code 1, and is not linted, because every rule reads a resolved model. A document that validates is linted and reports the same family with `operation: "lint"`, the validated `digest`, `valid: true`, and the findings sorted by source position. Every lint finding is a warning: severity is bound to the diagnostic code, so lint can never make a document invalid, and a clean document reports zero findings and prints one summary line. `--warnings-as-errors` is a policy about the invocation rather than a statement about the document — it changes the exit code from 0 to 1 when at least one finding is present, and changes nothing in the result body, so the same document produces byte-identical results with and without it. Process failures keep `podway.error/v1`: a missing or unsafe path and a Procedure v1 input (`PROCEDURE_SCHEMA_UNSUPPORTED`) report there.

`procedure check` is the aggregate authoring gate and never writes anything. It runs every authoring stage over one document — the canonical formatting comparison `format --check` performs, closed-reference validation, graph vetting, and lint — and reports their findings merged into one `podway.procedure-diagnostics-result/v1` with `operation: "check"`. Vet enforces reachability, terminal-route existence, the advance-only cycle rule, assessment coverage, dominance, and both wire-budget proofs through the same analysis used by `procedure vet`. Only the absence of a model stops the pipeline: a document that fails parsing or validation reports that single error, with `valid: false`, no `digest`, and exit code 1, and is neither formatted nor vetted nor linted, while a document that validates is always vetted and linted even when it has drifted or uses a source construct canonical authoring form cannot represent — a stale format must not hide a graph finding. The `digest` is present exactly when validation produced the canonical model, whatever later stages found. Findings are reported in pipeline order — formatting, then validation, then vetting, then lint — and within a stage by source position, so the report reads the way the pipeline runs regardless of the order the stages had to execute in; the drift finding is produced by the same constructor `format --check` uses, so the two commands can never disagree about whether a file has drifted or about where. The result is bounded at 256 diagnostics with `diagnostics_total` counting before truncation. `valid` is the absence of an error-severity finding and nothing else: because `FORMAT_NOT_CANONICAL` is catalogued as an error, a document whose only defect is its formatting reports `valid: false` and exit code 1. Exit code 0 means no finding at all; `--warnings-as-errors` is a policy about the invocation rather than a statement about the document — it moves the exit code from 0 to 1 when any finding is present and changes nothing in the result body. Process failures keep `podway.error/v1`: a missing or unsafe path and a Procedure v1 input (`PROCEDURE_SCHEMA_UNSUPPORTED`) report there.

`procedure scaffold` writes a Procedure v2 authoring starting point to stdout. It is the only procedure command that reads nothing: there is no file argument, no path to resolve, and no failure path, so it always exits 0 and always reports `podway.procedure-source-result/v1` with `operation: "scaffold"`, the selected `template`, the `document`, and its `target_digest`. The result names no file, carries no `mode`, and reports no `changed` flag, because none of the three has a meaning for a document that did not come from one. `--template` selects from a closed list whose only member today is `minimal`, and it defaults to that; an unknown template is a usage failure with exit code 2 rather than a runtime rejection. Text mode writes the document bytes exactly, so `podway procedure scaffold > new.yaml` produces a file that is already in canonical authoring form and already passes every authoring stage — `procedure format --check new.yaml` and `procedure check --warnings-as-errors new.yaml` both report nothing. The `minimal` template is two action nodes, one text item, one confirm item, and a manual rework target, annotated with full-line comments that explain each region and that `procedure format` preserves; every value in it is guidance telling the author what belongs there, so the scaffold contains no invented workflow.

`procedure convert` renders a Procedure v1 document as a Procedure v2 authoring candidate on stdout. It is deterministic and review-required: the same v1 document always produces the same bytes, and the two values Procedure v2 requires that Procedure v1 has no field for — the procedure `purpose` and each action `intent` — are synthesized from fixed templates and marked with full-line review comments so the reviewer can see exactly what Podway supplied. The command reads only. It never writes a file, never rewrites the v1 source, and never starts a session; redirecting stdout is how a candidate becomes a file. Each v1 stage becomes one action node definition placed once in a linear chain, keyed by the stage identifier, with the last stage terminal; `skip: {allowed: false}` is omitted, because an absent v2 skip policy already means not skippable; and `rework.allow_return_to: any_previous` expands to every graph node, which is the faithful static form of a cursor-relative rule because §9.5 still requires a manual rework target to have a valid attempt on the current execution trace. Every v1-effective item field is written explicitly, including the ones v1 defaulted, because the v2 defaults differ. Success reports `podway.procedure-source-result/v1` with `operation: "convert"`, `source_schema: "podway.procedure/v1"`, the `source_digest` `procedure validate` reports for the same file, the `document`, and its `target_digest`; the result names no file, carries no `mode`, and reports no `changed` flag, because the candidate came from no v2 file and was written to none. A v1 value Procedure v2 cannot hold is never truncated: every one of them is reported at once in `podway.procedure-diagnostics-result/v1` with `operation: "convert"`, `valid: false`, no `digest`, and exit code 1, and each diagnostic names the v1 field path and the v1 source position so the author edits the document they still have. Process failures keep `podway.error/v1`: a missing or unsafe path, and a document that already declares Procedure v2 (`PROCEDURE_SCHEMA_UNSUPPORTED`), report there. A malformed Procedure v1 document reports exactly the failure `procedure validate` reports for it, because both admit v1 through the same parser.

## Preset commands

```bash
podway preset list
podway preset show <name>
podway preset explain <name>
```

`show` emits source YAML or structured JSON. `explain` provides purpose, stage outline, and common rework examples.

## Session commands

### Start

```bash
podway start --preset <name> --task <title>
podway start --procedure <worktree-relative-file> --task <title>
podway start --procedure <worktree-relative-file> --expect-procedure-digest <sha256:hex> --task <title>
podway start ... --replace --yes
podway start ... --goal <text> --criterion <id>=<statement>... [--actor <text>]
```

Exactly one of `--preset` or `--procedure` is required. `--replace` deletes an existing session before start and requires confirmation. `--dry-run` validates and shows the first stage without creating a session.

An initial goal requires one to sixteen ordered, uniquely identified criteria.
`--criterion` and `--actor` are invalid without `--goal`. Goal-bearing start and
start replacement use the executable typed Procedure v2 request boundary and
atomically create immutable goal revision 1 with the new session. The Procedure
must opt in to goal tracking. A retained v1 start contains none of those fields
and preserves its released wire shape.

A non-dry-run replacement with explicit `--if-workspace-uuid`, `--if-session-id`, and
`--if-session-revision` sends those complete identity fences directly without a status preflight.
This preserves the original replacement identity across an exact idempotent retry. If any fence is
omitted, the CLI continues to preflight status and fills the missing identity from the observation.

### Status

```bash
podway status [--verbose [--history-before <trace-sequence>]] [--wait-for-idle] [--compact] [--after-job <uuid>]
```

Reports:

- task and procedure;
- session lifecycle and revision;
- current stage and attempt;
- all stage statuses;
- active-stage item states;
- blockers;
- queued and running jobs;
- whether the stage is ready to advance.

For Procedure v1, `--verbose` includes previous attempt summaries for the current
session. For Procedure v2, it adds the six bounded history windows defined by
`status-result/v2`. `--history-before` requires `--verbose`, accepts a positive
trace sequence, and applies that exclusive cursor to all six windows. Verbose
status does not provide an audit export.

`--compact` requires `--wait-for-idle` and returns the closed, bounded authority
projection defined by the automation client contract.

### Next

```bash
podway next [--wait-for-idle] [--after-job <uuid>]
```

Reports:

- current stage title and instructions;
- missing required items;
- open blockers;
- structured Podway command suggestions;
- whether complete, skip, retry, return, or cancel is currently allowed;
- next stage after completion.

### Complete

```bash
podway complete
```

Requires all required items and no blockers. The CLI automatically supplies current revision and attempt unless explicit precondition flags are given.

### Skip

```bash
podway skip --reason <text>
```

Fails when the active stage is not skippable. A reason is always accepted and is mandatory when the procedure requires it.

For a Procedure v2 session, skip is executable only on an active action placement
whose skip policy allows it. Required items and blockers do not prevent the
transition. A supplied reason is non-blank and limited to 2,000 characters; the
placement policy may require it. The CLI automatically supplies the current
session revision and attempt unless explicit precondition flags are given. A fully
fenced skip is sent directly without a status preflight.

### Retry

```bash
podway retry --reason <text>
```

For a Procedure v2 session, retry is executable on the active action or decision
placement. It abandons only the current attempt, preserves that attempt as stale
history, and creates a clean attempt of the same placement. The new attempt has
fresh item and blocker state and resolves its evidence references again. The CLI
automatically supplies the current session revision and attempt unless explicit
precondition flags are given. The v2 reason is required, non-blank, and limited to
2,000 characters.

### Decide and rework

```bash
podway decide --option <option-id> --reason <text> [--actor <text>]
podway rework --to <graph-node-id> --reason <text> [--actor <text>]
```

`decide` requires the exact active attempt and conditionally requires the exact
current goal revision when the active decision is a session-goal assessment.
The standard status preflight copies the current `goal_revision` fence whenever
the status exposes one and omits it when no goal is defined; the closed status
contract does not expose the active decision's assessment definition. A fully
fenced direct invocation may likewise provide `--if-goal-revision`. The daemon
requires that fence for a goal-assessment decision and validates it when it is
also supplied for a general decision, so stale values fail in either case.
`rework` requires the exact session revision and carries the active attempt when
the session is running. These
Procedure v2 commands are distinct from the retained v1 `return` and `reopen`
commands; neither retained command is an alias for `rework`.

The CLI obtains omitted workspace, session, revision, attempt, and goal-revision
fences from a version-aware standard status preflight. A caller may provide the
complete command-specific fences to skip that read. The preflight does not
reinterpret a Procedure v2 session as v1. Manual rework requires an exact active-
attempt fence while running, omits that fence when reactivating a completed
session, and rejects cancelled sessions.

### Goal

```bash
podway goal define --goal <text> --criterion <id>=<statement>... [--actor <text>]
podway goal revise --goal <text> --criterion <id>=<statement>... \
  --rework-to <graph-node-id> --reason <text> [--actor <text>] [--reactivate]
podway goal assess-criterion <criterion-id> \
  --status <satisfied|unsatisfied|not_applicable> --reason <text> \
  [--evidence <graph-node-id>]... [--item <item-id>]... [--actor <text>]
```

Definitions and revisions carry one to sixteen ordered, uniquely identified
criteria. Revision and criterion assessment require `--if-goal-revision`.
Assessment accepts at most four citations in total; `not_applicable` forbids
citations. The same version-aware preflight rule applies to all three goal commands.
Definition, revision, and criterion assessment are executable durable mutations.
Criterion assessment atomically binds the result to the active goal-assessment
attempt and current goal revision after validating its mode, reason, actor, and
bounded citations.

### Return

```bash
podway return --to <stage-id> --reason <text>
podway return --to <stage-id> --reason <text> --dry-run
```

Dry run shows the new destination attempt number and downstream stages that would become `redo`.

### Reopen

```bash
podway reopen --to <stage-id> --reason <text>
podway reopen --to <stage-id> --reason <text> --dry-run
```

Allowed only for a completed session.

### Block and unblock

```bash
podway block --reason <text>
podway unblock <blocker-id>
podway unblock --all
```

Exactly one blocker ID or `--all` is required.

For a Procedure v2 session, block and unblock preserve the active graph node
and attempt. Block requires a non-blank reason of at most 1,000 Unicode scalar
values and permits at most 64 simultaneous open blockers. An open blocker makes
readiness report `unblocked=false` and removes complete or decide from legal
actions, but it does not prohibit an otherwise eligible skip. Unblock resolves
one blocker owned by the active attempt or, with `--all`, every open blocker on
that attempt. Resolved blockers remain immutable history.

### Cancel

```bash
podway cancel --reason <text>
```

Cancels a running session. A cancelled session cannot reopen.

For a Procedure v2 session, cancellation abandons the active attempt, records
the reason in history, changes lifecycle to `cancelled`, and removes the active
cursor. The v2 transition result deliberately omits the reason. Subsequent
status reports `current` as null, and `next` fails with `SESSION_NOT_RUNNING`.

### Reset

```bash
podway reset [--dry-run] [--yes]
podway reset --all --force --yes
```

Normal reset deletes the session but preserves workspace initialization. `--all --force` recreates disposable runtime state and is also the corruption-recovery path.

Normal reset accepts a running, completed, or cancelled Procedure v2 session.
Its successful v2 result reports `transition: "reset"`, `reset: true`, and the
terminal revision without inventing a cursor or session lifecycle. The graph
session no longer exists after the atomic reset; workspace initialization and
the reset job receipt remain available.

## Item commands

### Confirm

```bash
podway check <item-id>
podway uncheck <item-id>
```

### Text, choice, and integer

```bash
podway set <item-id> <value>
podway set <item-id> --stdin
```

For `integer`, the value must parse as a base-10 signed integer. For `choice`, it must exactly match a declared choice. `--stdin` is valid for text only and reads until EOF.

### List

```bash
podway add <item-id> <value>
podway remove <item-id> <value> [--ignore-missing]
```

### Artifact path

```bash
podway attach <item-id> <worktree-relative-path> [--media-type <type>]
```

The daemon computes size and digest. A supplied media type overrides the embedded extension mapping and must satisfy the item allowlist. Unknown extensions default to `application/octet-stream`.

### Artifact reference

```bash
podway attach <item-id> \
  --reference <opaque-reference> \
  --digest sha256:<hex> \
  --size <bytes> \
  --media-type <type>
```

The four reference options are required together.

### Clear

```bash
podway clear <item-id>
```

Clears the current item value.

## Job commands

```bash
podway job list [--state queued|running|succeeded|failed|cancelled]
podway job status <job-id>
podway job wait <job-id>
podway job cancel <job-id>
podway job lookup --idempotency-key <key>
```

Job commands are scoped to the current worktree. `job wait` honors `--timeout`.
`job cancel` only succeeds for queued jobs. `job lookup` is read-only and does not
submit or replay a mutation. After job-row pruning, lookup reconstructs the same
terminal job response from the retained receipt without exposing the canonical request.

## Automatic and explicit preconditions

For interactive use, the CLI reads current state immediately before submitting a mutation and includes the observed preconditions.

Automation SHOULD pass explicit values obtained from `status --json`:

```text
--if-workspace-uuid <uuid>
--if-session-id <uuid>
--if-session-revision <n>
--if-attempt <uuid>
--if-item-revision <n>
--if-goal-revision <n>
```

Workspace identity applies to session start and replacement, session-bearing
reads, session and item mutations, reopen, reset, and reset-all. Session identity
applies to session-bearing reads and commands that target an existing session;
plain start and reset-all reject it. Static, daemon lifecycle, workspace
maintenance, job, init, and doctor commands reject both identity options.

Explicit workspace and session identities take precedence over CLI preflight observations. The
CLI carries them through guarded reads and mutations. Ordinary mutation request identity binds
the resolved workspace UUID and, for commands targeting an existing session, the session ID.
`workspace.reset_all` instead binds stable Git common-directory and worktree-administration
fingerprints and excludes rotating workspace UUIDs so an exact retry can replay after the reset.
The command-specific required IPC combinations remain normative in the
[automation contract](automation-client-contract.md#14-workspace-and-session-identity-preconditions-aut-id-001007).

```bash
podway complete \
  --if-session-revision 12 \
  --if-attempt 6f8e... \
  --idempotency-key task-42-complete-verify \
  --json
```

Item updates additionally accept `--if-item-revision`. For an unset item, use revision `0`.

## Destructive confirmation policy

Commands requiring confirmation:

- `start --replace`;
- `reset` always, except for `--dry-run`;
- `reset --all --force`;
- `daemon uninstall` when service files will be removed.

On an interactive TTY, Podway prompts unless `--yes` is present. With `--json` or non-TTY input, missing `--yes` fails with `CONFIRMATION_REQUIRED`. Prompt text is never emitted in JSON mode.

## Shell completion

The release ships completion for:

- zsh;
- bash;
- fish.

Completion includes commands, Procedure v2 goal and decision flags and status
values, built-in preset names, active-stage item IDs, allowed return destinations,
open blocker IDs, and current-worktree job IDs where dynamic completion is safe
and fast.

Dynamic completion MUST use read-only daemon queries and MUST degrade silently when the daemon or workspace is unavailable.
