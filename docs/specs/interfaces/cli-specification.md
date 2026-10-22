# CLI Specification

`podway` is the only supported user and automation executable. Each runtime invocation
resolves the owning Git worktree and executes through the typed `podway-engine`
Direct API in that process; static commands run locally without a Store. Mutations commit synchronously; reads observe one
committed snapshot without writing. There is no resident process, socket, service,
queue, runtime mode, or user-global Podway state.

Human error rendering may include the exact Procedure path supplied to `start`
and a visible truncation summary after a bounded diagnostic window. JSON mode
emits the machine response only; clients never parse human guidance.

## Command groups

- Local inspection: `help`, `version`, `completions`, and `preset list|show|explain`.
- Procedure authoring: `procedure validate|show|format|vet|graph|preview|lint|check|scaffold`.
- Workspace: `init`, `doctor`, `workspace show|repair|remove`, and `reset --all`.
- Session reads: `status`, `next`, `observe`, `evidence read`,
  `delegation package|receipt`, `archive list|show`, and `outcome lookup`.
- Session mutations: `start`, `begin`, `complete`, `skip`, `retry`, `block`,
  `unblock`, `cancel`, `disposition handed-off|not-required`, `reset`, `archive`,
  `archive purge`, `decide`, `rework`, `goal define|revise|assess-criterion`,
  `delegation accept --stdin`, `check|uncheck|set|add|remove|attach|clear`, and
  `record --stdin`.

The [command catalog](../../../assets/specifications/command-catalog.yaml),
[route contract](../../../contracts/command-routes.json), and CLI clap grammar
own exact routes, result families, and options. `podway -h` and `podway --help`
are aliases for the overview from `podway help`; they do not enable subcommand
help flags. JSON help retains command identity `help`. Help topic words are
joined with single spaces in the JSON `topic` field, which is null when no
topic is supplied.

## Grammar and global options

Angle brackets denote required values; brackets denote optional syntax. Global
options may surround the command, but their identities and confirmation flags are
accepted only by applicable routes. Non-applicable or duplicate stdin identity
fields return `REQUEST_INVALID`. The worktree request options — `--worktree`,
`--timeout`, `--idempotency-key`, `--if-workspace-uuid`, `--if-session-id`,
`--if-session-revision`, `--if-attempt`, `--if-item-revision`,
`--if-goal-revision` and `--yes` — apply to workspace routes only: the local
families `help`, `version`, `completions`, `preset` and `procedure` run
in-process, open no workspace, and reject every one of them with exit-2
`REQUEST_INVALID` naming each supplied option.

| Global option | Meaning and applicability |
|---|---|
| `--json` | Emit one machine JSON response |
| `--worktree <PATH>` | Select the Git worktree explicitly |
| `--timeout <DURATION>` | Combined direct lock-wait budget described below |
| `--no-color`, `--quiet` | Human rendering controls; quiet never enables prompts |
| `--idempotency-key <KEY>` | Caller-owned mutation key or `outcome lookup` key |
| `--if-workspace-uuid <UUID>` | Expected workspace generation on supported runtime routes |
| `--if-session-id <UUID>` | Expected current session on applicable reads/transitions |
| `--if-session-revision <N>` | Session transition/record-many/archive-purge revision; not a single-item fence |
| `--if-attempt <UUID>` | Current action/decision/evidence identity; not start/begin/disposition/reset |
| `--if-item-revision <N>` | Single-item mutation revision; not another route's fence |
| `--if-goal-revision <N>` | Positive goal revision on decide, goal revise, or assess-criterion |
| `--yes` | Explicit destructive confirmation; rejected on ordinary non-destructive commands |

| Route grammar | Command-specific constraints |
|---|---|
| `help [<TOPIC>...]` | One or more topic words: overview, a guide, a dotted route identifier, executable command words, a command-group word, or a retired family; the overview prints the complete topic index and unknown topics return exit-2 `REQUEST_INVALID` naming `podway help`; `-h`/`--help` alias only the top-level overview |
| `version [--identity]` | `--identity` requires `--json` |
| `completions <bash\|zsh\|fish>` | Emits the selected shell script; dynamic candidates use bounded read-only queries |
| `preset list`, `preset show <NAME>`, `preset explain <NAME>` | Offline built-in catalog |
| `procedure validate\|lint\|check <FILE> [--warnings-as-errors]` | Authoring diagnostics and warning policy |
| `procedure show <FILE> [--canonical]` | Source or canonical data |
| `procedure format <FILE> [--check\|--write]` | Check and write conflict; omission renders only |
| `procedure vet\|preview <FILE>` | Read-only semantic budget checks or preview; preview uses start-safe worktree-relative spelling |
| `procedure graph <FILE> --format <json\|mermaid\|puml\|dot>` | Required projection format |
| `procedure scaffold [--template minimal]` | `minimal` is the only accepted template |
| `init [--repair]`, `doctor [--deep]`, `workspace show\|repair` | Local initialization, diagnosis, or explicit repair; Doctor uses the retained generic result object |
| `workspace remove [--force] [--yes]` | Confirmed complete `.podway` removal and workspace fencing below |
| `start (--preset <NAME>\|--procedure <FILE>) --task <TITLE>` | Optional file-only digest, policy, reason/actor/summary, and dry-run controls below |
| `begin [--goal <TEXT> --criterion <ID=STATEMENT>...] [--actor <TEXT>]` | Goal and criteria require each other; actor requires goal |
| `status [--compact\|--verbose [--history-before <N>]]` | Compact conflicts with verbose/history; history requires verbose and a positive trace sequence |
| `next`, `observe` | `--verbose` is rejected; no read barrier or job wait |
| `evidence read --source <NODE> --item <ITEM> [--page-token <TOKEN>]` | Current declared evidence only |
| `delegation package` | Current delegated action package |
| `delegation accept --stdin` | Closed complete acceptance document supplies its own identities |
| `delegation receipt --session <UUID> --attempt <UUID>` | Exact receipt; not a current-session precondition override |
| `complete`, `skip [--reason <TEXT>]`, `retry --reason <TEXT>` | Complete/skip act on actions; retry repeats the active action or decision |
| `block --reason <TEXT>`, `unblock (<BLOCKER>\|--all)`, `cancel --reason <TEXT>` | Exactly one unblock selector |
| `decide --option <ID> --reason <TEXT> [--actor <TEXT>]` | Goal fence also applies to goal-assessment decisions |
| `rework --to <NODE> --reason <TEXT> [--actor <TEXT>]` | Declared manual rework |
| `goal define --goal <TEXT> --criterion <ID=STATEMENT>... [--actor <TEXT>]` | One or more criteria |
| `goal revise --goal <TEXT> --criterion <ID=STATEMENT>... --rework-to <NODE> --reason <TEXT> [--actor <TEXT>] [--reactivate]` | Current goal fence and explicit reactivation when required |
| `goal assess-criterion <ID> --status <satisfied\|unsatisfied\|not_applicable> --reason <TEXT> [--evidence <NODE>...] [--item <ITEM>...] [--actor <TEXT>]` | Typed applicability/assessment rules |
| `disposition handed-off --summary <TEXT> --reference <TEXT> [--actor <TEXT>]` | Current terminal revision |
| `disposition not-required --reason <TEXT> [--actor <TEXT>]` | Current terminal revision |
| `reset [--dry-run]` or `reset --yes --progress-summary <TEXT>` | Session eligibility or confirmed force path; dry-run rejects progress summary |
| `reset --all --yes [--force]` | Runtime replacement; rejects dry-run/progress summary; force is accepted only with all and is unnecessary |
| `archive`, `archive list` | Retain current disposed terminal session or list inactive summaries |
| `archive show --session-id <UUID> [--verbose [--history-before <N>]]` | Exact inactive read; history requires verbose |
| `archive purge --session-id <UUID> --if-session-revision <N> --yes` | Confirmed inactive deletion |
| `check\|uncheck\|clear <ITEM>` | Typed single-item operation |
| `set <ITEM> (<VALUE>\|--stdin)` | Exactly one text input form |
| `add <ITEM> <VALUE>`, `remove <ITEM> <VALUE> [--ignore-missing]` | List mutations |
| `attach <ITEM> <PATH> [--media-type <TYPE>]` | Local artifact preparation |
| `attach <ITEM> --reference <REF> --digest <SHA256> --size <BYTES> --media-type <TYPE>` | Complete caller-supplied reference metadata; excludes local path |
| `record --stdin` | Complete closed atomic multi-item document; no check-result single-item alias |
| `outcome lookup --idempotency-key <KEY>` | Read-only retained successful outcome |

The canonical command name in JSON uses dotted routes (for example
`session.observe`, `goal.assess_criterion`, `item.record_many`) even when CLI words
use spaces or hyphens. State-aware `start` resolves to `session.start_replace`.
The hidden `__complete [KIND]` route supports completion scripts; it is a bounded
best-effort query and emits no candidates on failure.

## Worktree selection and deadlines

`--worktree <path>` selects an explicit Git worktree; omission discovers it from
the current directory. Automation should supply the explicit path and observed
workspace UUID. Workspace binding uses canonical Git paths, not replaceable Git
filesystem object identity. A matching numeric revision does not substitute for
workspace, session, or attempt identity.

`--timeout <duration>` uses an unsigned integer followed by `ms`, `s`, or `m`,
with a maximum of 3,600,000 ms. The default is 5,000 ms; `0ms` is a valid
immediate deadline. One absolute invocation deadline bounds maintenance leases,
the process-local connection gate, and SQLite read admission or write-lock waits.
Each boundary makes one non-blocking attempt even if the budget is exhausted.
Only retries and sleeps are deadline-bound. Elapsed preflight, preparation and
human confirmation consume the remaining wait budget; no later step receives a
fresh budget. Acquired locks allow the operation to finish past the deadline.
This is not a hard timer for stdin, human confirmation, preparation, execution,
filesystem cleanup, or response rendering.
Lock expiry returns `WORKSPACE_MAINTENANCE` or `WORKSPACE_BUSY` with no partial
mutation or retained rejection. Busy details distinguish `connection_gate`,
`sqlite_read_admission`, and `sqlite_write_lock`; a read never reports a write lock.
Reads have no queue or completion barrier.

## Retired surface

The [retired-command table](../../../assets/specifications/retired-commands-v1.json)
is executable authority. `daemon *`, `terminate`, `runtime reset *`,
`job list|status|wait|cancel|lookup`, and `workspace mode *`, together with
`--mode`, `--dev`, `--socket`, `--detach`, `--wait-for-idle`, and `--after-job`,
return `COMMAND_RETIRED` before workspace resolution, stdin consumption, or Store
access. `job.lookup` names `outcome.lookup` as its replacement; other null
replacements mean no replacement exists. Ordinary unknown syntax remains a usage
failure. Literal values after `--` or in value positions are not retired syntax.
Retirement classification stops at ordinary unknown syntax; a retired flag
appearing later remains part of that usage failure.
The `job *` table entry supplies `help job` guidance; unknown job children remain
usage failures. Family help topics are `daemon`, `job`, `runtime`,
`runtime.reset`, and `workspace.mode`.

## Workspace lifecycle

`workspace remove [--force] [--if-workspace-uuid <uuid>] [--yes]` removes the
complete `.podway` tree and preserves the Git worktree and user files. Unfenced
human TTY
use must confirm the exact resolved absolute root, including when `--yes` is
supplied. A UUID-fenced TTY request may use `--yes` instead of that prompt. JSON and
non-TTY use require `--yes`, the workspace UUID fence, and an idempotency key.
JSON use also requires `--force`. Human removal may omit `--force` when no
prepared or running session exists; a recovery recipe preserves the original
force policy rather than changing the canonical request.

`reset --all --yes --idempotency-key <key>` replaces runtime state with a fresh
Store while preserving Procedure sources and configuration unrelated to retired
legacy mode; legacy-mode configuration/markers are intentionally retired.
`--force` is not required for reset-all. Store generations 1–13 and daemon-era
markers are refused without conversion or deletion; explicit reset-all is the
supported destructive recovery. A Store newer than this binary supports is
refused even for reset and removal; use a Podway that supports it. A recipe alone
grants no reset authority.

Normal opens refuse unknown writer-contract digests. A declared direct
predecessor may migrate under exclusive maintenance; the v0.2.13 cutover declares
none. Development uses scratch worktrees whose writer identity matches the binary.
Reset and removal publish recovery markers. An exact retry retains the original
key, canonical request, and applicable workspace fence; reads never resume them.
A pending removal resumes through its exact marker-backed request. Complete
removal destroys its outcome Store; an already-absent result confirms convergence,
not replay of the original response. Inspect the selected worktree read-only
after lost removal output. Podway does not control historical installed services.

## Procedure commands

All procedure commands accept only `podway.procedure/v2`. Validation and authoring
use the same parser, semantic validator, canonicalizer, diagnostics catalog, and
bounded source reader as session preparation. `format --write` is the only authoring
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
When a custom Procedure fails with `PROCEDURE_V2_SCHEMA_INVALID`, human output points
`procedure check --` at that exact path using shell-safe rendering; JSON output remains the
unchanged error envelope. Other start failures retain their code-specific remediation.

`begin` accepts optional initial goal inputs and actor attribution, fences the
prepared session, and atomically creates the first running attempt. Goal input is
allowed only when the validated Procedure enables goal tracking and retains the
existing goal and criterion bounds.

## Evidence reads

`evidence read --source <graph-node-id> --item <item-id> [--page-token <token>]` returns one bounded page of an item selected by a currently resolved, declared evidence reference of the current consumer attempt. It is a query: it creates no durable job, no session revision, and no state change, and it cannot browse arbitrary attempts or stale history.

Optional `--if-workspace-uuid`, `--if-session-id`, and `--if-attempt` fences are checked against the same coherent snapshot used for the read. `--if-attempt` identifies the consumer attempt; a changed consumer returns `ATTEMPT_NOT_CURRENT`, including on the first page without a continuation token.

A read without a token starts at the beginning of the value. A response carries the complete-value digest, the total logical size, the page, a `truncated` flag, and a nullable continuation token. Text offsets and sizes count Unicode scalar values, list offsets and sizes count entries, and scalar item types return one terminal page. The token is opaque, bounded to 256 base64url characters, and is not an authentication or authorization credential.

## Delegation commands

`delegation package` reads the current delegated attempt's immutable work package
and digest. `delegation receipt --session <uuid> --attempt <uuid>` reads one exact
immutable receipt from the current or retained session; it does not accept
session precondition flags. Both commands are read-only.

`delegation accept --stdin` reads one closed acceptance document of at most
32 KiB and commits acceptance synchronously with its success outcome. The
document alone supplies workspace,
session, revision, attempt, and idempotency identities; command-line identity
flags cannot override it. `session_revision` and `rounds_used` require unsigned
integer JSON literals; a zero-fraction float such as `1.0` is rejected even
though JSON Schema's `integer` type admits it. Record final action evidence and
settle the worker before acceptance. Acceptance does not complete the action:
the caller must run `complete` while the receipt remains current. Recover a lost
response with `outcome lookup` using the same idempotency key before exact replay.

## Mutation rules

Automation must retain an explicit idempotency key and the complete resolved
request, including original identity and revision fences. The CLI may fill omitted
fences from a fresh direct observation; automation should supply observed fences
to prevent that preflight from changing the intended target. Interactive human
mutations may generate a key and print it to stderr; automation mutations require
a caller-owned key. The same closed transition evaluator serves both paths.

An ordinary Store mutation records its validated success envelope atomically with
state. Filesystem reset/removal use their separate marker-backed protocol.
Rejected mutations roll back and record no outcome. Replay checks the workspace
UUID and writer contract before consulting the stored key, then returns the
original success for an identical canonical request. It does not refresh fences,
read changed Procedure/artifact sources, or apply another effect. Another request
using that key fails with `IDEMPOTENCY_KEY_REUSED`.

`outcome lookup --idempotency-key <key>` is read-only. It returns the complete
retained success or `found=false`; it never submits a mutation. A miss proves no
record only within the retention boundary. Session outcomes survive archival and
archive purge until that session is reset, runtime is reset, or the workspace is
removed. Workspace outcomes retain at most 100 entries or 30 days, with pruning on successful writes.
After lost stdout or uncertain COMMIT I/O, look up the original key before exact
replay of a retained Store mutation. `MUTATION_OUTCOME_UNKNOWN` is emitted only
for uncertain COMMIT I/O; an external process kill or lost stdout is a caller-side uncertainty, not a job state.

`retry` remains on the active action or decision node. `block` and `unblock` also
apply to either active node type. `rework --to <node>` uses the Procedure
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
non-TTY, and policy invocations never prompt. Automation uses
`--on-existing preserve|delete`; preserve requires `--supersede-reason` and may
include `--actor`, while deleting a running session requires
`--progress-summary` and `--yes`. Missing policy returns
`SESSION_START_DECISION_REQUIRED` with no mutation.
An interactive continue result performs a fresh committed-snapshot read and renders
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
retains an eligible terminal session as inactive. `archive list` returns the exact
total, a newest-first window of at most 256 summaries, truncation, and the oldest
summary; `archive show --session-id <uuid>` returns one immutable status
projection plus its terminal disposition and accepts the normal verbose history controls. `archive purge`
requires the inactive session ID, `--if-session-revision`, and `--yes` and deletes
only that retained session. Purge is a fenced synchronous control-plane deletion with a
workspace-scoped success outcome. If its response is lost, look up the original key before exact
replay. `archive list` can additionally confirm absence, but absence alone does
not identify which caller completed deletion.
No archive command restores or mutates inactive state.

Plain `start --dry-run` without an existing-session policy remains a local
Procedure preview. A policy dry run reads the selected Store and returns the observed
session identity, revision, lifecycle, and proposed action without mutation.

`record --stdin` is the only multi-item mutation grammar. It reads at most 1 MiB
of closed `podway.item-record-many-input/v1` JSON. The document supplies the
workspace, session revision, active attempt, idempotency key, and 1..128 unique
item-local revision fences. The Engine canonicalizes operations by item ID and
records or clears the complete set atomically without advancing the cursor.
Identity, revision, and idempotency flags must not duplicate the stdin fields.
The same 1,048,576-byte aggregate ceiling covers the canonical complete input,
including fences and key, for typed Direct API callers as for stdin callers;
per-item limits do not replace it. Oversize input returns `REQUEST_TOO_LARGE`
before canonicalization, preparation, or mutation.

A `check_result` record uses this same stdin grammar and has no single-item CLI
alias. Observation may provide one copyable closed stdin template for the first
unsatisfied check-result item, but the caller must replace its result and
idempotency placeholders after performing the external check. Podway does not
invoke the operation named by the declaration. Human output calls the value a
recorded or structurally bound external check result and never describes it as
verified, trusted, attested, or secure evidence.

## Output and exits

Successful versioned JSON output uses `podway.output/v4`; failures use `podway.error/v2`.
`version --json` retains its compact `name` and `v`-prefixed `version` summary;
`version --json --identity` uses output v4 and `podway.version-result/v2` with
`podway.contract-manifest/v2`, no IPC identity, and the same version prefix.
Machine clients consume schema IDs, command names, stable fields, and error codes,
never text. Text mode is advisory rendering of the same result. Exit status follows
the canonical [exit-code table](errors-and-exit-codes.md#exit-codes).

Except for the failure-rendering escape boundary below, versioned JSON emits
exactly one bounded newline-terminated object on stdout.
Outside recognized value positions, usage failures honor a standalone `--json`
anywhere before the `--` delimiter, including after unknown syntax. Local internal
failures and local results that exceed the output budget return a validated error
envelope with a diagnostic UUID. If the clock cannot supply a valid timestamp or
the rebuilt error envelope cannot be validated, rendering falls back to a stderr
diagnostic and exit 6 without aborting.
Diagnostics go to the structured envelope in JSON mode and stderr only where the
command contract explicitly has no JSON response.
