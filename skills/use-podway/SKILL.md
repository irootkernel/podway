---
name: use-podway
description: Operate an explicitly authorized Podway Procedure v2 session or workspace lifecycle, including state inspection, progression, goals and recovery. Use create-podway-procedure for Procedure authoring.
---

# Use Podway

## Scope and entry

This runtime skill requires an explicit request to use or manage Podway, or an
approved workflow that names its operations. Availability, repository files and
existing sessions do not activate it. Carry authorization forward within the
same workflow; unrelated work needs no Podway inspection.
A request limited to inspection, diagnosis or one lifecycle operation authorizes
only that operation, not item recording or progression.

Podway guards one task in one Git worktree. It records caller results and formal
progression, not external execution or semantic truth. Use JSON fields and stable
error codes for decisions, and `podway help <route or command words>` for exact
command grammar.
Do not install it, change project guidance or author a Procedure through this
skill. Route Procedure source work to `$create-podway-procedure`.

This edition requires the generation-2 support introduced for `v0.2.15`, or a
verified matching development build. If the CLI rejects `--contract-generation`,
report the compatibility gap; do not silently fall back to generation 1. Recovery
with an older original writer uses that release's own guidance and grammar.

1. Resolve the owning worktree, available CLI and `.podway/config.yaml`. Report
   missing prerequisites without initializing or installing anything. Reviewable
   `.podway` content is `.podway/config.yaml`, Procedure files under
   `.podway/procedures/` and `.podway/.gitignore`, which ignores `runtime/`;
   authoritative task state and maintenance locks live under `.podway/runtime/`
   and are never tracked.
2. Run `podway --contract-generation 2 --worktree <absolute-root> --json observe`.
   Require `podway.output/v4` (exit code `0`; `podway.output/v4` has no `ok` field)
   and `podway.observation-result/v6`; the deprecated generation-1 default still
   serves `podway.observation-result/v5` to consumers that have not adopted
   generation 2. Error output uses `podway.error/v2`; `SESSION_NOT_FOUND` means
   no current session; `WORKSPACE_NOT_INITIALIZED` means this worktree has no
   workspace, so report it and initialize only with explicit intent.
3. Read identities and lifecycle from `status`, current work from `guidance`
   (including `guidance.goal` when goal tracking is enabled), declarations from
   `active_items`, and fenced requests from `mutation_templates`. Note that
   `podway.observation-result/v6` has no `workspace.uuid` field; read
   `workspace_uuid` from the outer `podway.output/v4` envelope (`workspace.uuid`)
   or from concrete `mutation_templates[].preconditions.workspace_uuid`.
   Apply [command execution](#command-execution) when using a template.
   Prepared state has no active attempt; null guidance denotes terminal state.
   An undisposed terminal revision needs disposition before archive or replacement.
   Use `status --verbose` for history and `next` for narrower guidance.
4. Once per workflow entry, run `podway archive list --json` and require
   `podway.session-archive-list-result/v2`. Above 10 inactive sessions, report the
   count and offer exactly two boundaries: reset all workspace runtime state, or
   purge only the oldest inactive session. Follow [lifecycle](references/lifecycle.md)
   for the exact requests. This is a nonblocking advisory, never deletion authority;
   if the count read fails, report it and continue the requested work.
5. Without a current session, continue external work. Create a session only when
   the workflow explicitly authorizes it.

## Command execution

Pass `--contract-generation 2` on every command with this compatible CLI. Use
`--json` for machine output and `--worktree <absolute-root>` on every workspace
command. Local commands (`help`, `version`, `completions`, `preset`, `procedure`)
accept the generation selector but reject worktree request options: `--worktree`,
`--timeout`, `--idempotency-key`, all `--if-*` fences, `--with-observation` and
`--yes`.

Six direct mutations (plain `start` without `--dry-run` or `--on-existing`,
`begin`, `record --stdin`, `complete`, `decide` and
`goal assess-criteria --stdin`) accept `--with-observation`. When passed, the
response result schema is `podway.mutation-observation-result/v1` (with
`result.mutation_result` holding the command's normal result,
`result.observation` holding an as-of-commit successor
`podway.observation-result/v6` computed in the same transaction without a second
database read, and `result.workspace_sequence`; both generation 1 and generation
2 embed `podway.observation-result/v6`, while omitting `--with-observation`
preserves the default unwrapped result schemas and request identities).
`--with-observation` is a boolean flag and takes no `=true` or `=false` value;
all other commands reject `--with-observation` with `REQUEST_INVALID` (exit `2`).

Templates and recovery recipes never embed `--contract-generation` or
`--with-observation`. Add the caller options above only where absent. Replace
declared placeholders with supported facts; preserve supplied options, fences and
concrete idempotency keys. Observation mutation templates and preview start
suggestions may append optional or repeatable semantic arguments admitted by
command help, including additional `--criterion`, citations (`--evidence` /
`--item`), `--with-observation` on the six supported mutations, and applicable
`--actor`, within the authorized work. Recovery recipes keep their semantic argv
unchanged: add only caller options and fill declared placeholders. A bare preview
start suggestion also needs a caller-owned key and current start preconditions
when applicable. Stdin-fenced mutations (`record --stdin`,
`goal assess-criteria --stdin`, `delegation accept --stdin`) own their workspace,
session, attempt, item or goal fences and `idempotency_key` inside the JSON stdin
body, never run implicit status preflight, and reject CLI `--if-*` and
`--idempotency-key` flags (`REQUEST_INVALID`, exit `2`).

`--timeout <duration>` defaults to `5s`; unsigned `ms`, `s` or `m` values
may be at most `60m`. It supplies one admission-wait budget across maintenance
leases, the connection gate and SQLite locks. Input, preparation and confirmation
consume that budget without renewing it; each boundary tries once even at `0ms`,
and admitted work may finish later. It is not an execution timer.

## Record and advance

1. Perform the work owned by the active node. Inspect `missing_required_items`,
   readiness and the observed suggestions; fill values only from actual results.
2. For each mutation, use the latest workspace, session, attempt, goal and item
   identities/revisions required by its template. Supply one unique stable
   idempotency key, retain the canonical request, and add semantic arguments from
   command help. Some CLI commands can fill omitted fences from an internal
   preflight, but automation must preserve the observed template fences so it
   acts on the state it inspected. Never drop a fence to make a request succeed;
   separately authorized unfenced maintenance is described in
   [lifecycle](references/lifecycle.md).
3. Record items with their typed command:

   | Type | Operation |
   | --- | --- |
   | `confirm` | `check` / `uncheck` |
   | `text`, `choice`, `integer` | `set`; text accepts `--stdin` |
   | `list` | `add` / `remove` |
   | `artifact` | `attach` |
   | `check_result` | `record --stdin` with the declared operation identity |
   | Any recorded value | `clear` |

   For `check_result`, start from the observed `item.record_many` template's
   `stdin_template` when present. Its `record` requires `type: "check_result"`,
   `operation_id`, `operation_digest`, `input_basis` with both `descriptor` and
   `digest`, `executor` with both `name` and `version`, `outcome`, `summary` and
   `output_digest`. Keep the declared operation identity; fill result fields from
   the actual check. Hash retained check-output bytes reproducibly and identify
   the checked inputs; never invent digests. Podway validates structure, not truth.

   Use `--actor` only on commands that admit it, such as goal-bearing `begin`,
   disposition, `decide`, `rework` and goal operations. Keep distinct actors distinct.
   Required local artifacts are checked again at completion; `ARTIFACT_CHANGED`
   requires fresh attachment of the verified file.
4. When one observation supports several item updates, or when reducing a
   satisfied action node's normal progression from four CLI calls (`observe`,
   `record`, `observe`, `complete`) to two (`record --stdin --with-observation`
   followed by `complete --with-observation`), prefer atomic `podway record
   --stdin`. Observation `mutation_templates` emits `item.record_many`
   (`stdin_template`) only when the active node has a `check_result` target item.
   On action nodes without a `check_result` target, construct the closed
   `podway.item-record-many-input/v1` document directly from the same
   observation: `workspace_uuid` from the outer `podway.output/v4` envelope's
   `workspace.uuid` (or a concrete template's `preconditions.workspace_uuid`),
   `session_id` and `session_revision` from `status.session.id` and `.revision`,
   `attempt_id` from `status.current.attempt.attempt_id`, and a new caller-owned
   `idempotency_key`. Its 1..128 unique operations each carry `item_id`, an
   `expected_item_revision` taken from that item's `active_items[].revision`
   (record templates show it as `preconditions.item_revision`), and exactly one
   typed `record` or `clear: true`. Do not also pass identity, revision or key
   flags on argv. Require `podway.item-record-many-result/v2` (or
   `result.mutation_result` of that schema when `--with-observation` is used);
   any rejected operation changes none of them. The mutation result's `revision`
   is the session revision, not an item revision; read current
   `active_items[].revision` from the returned `result.observation` (under the
   first-submission rule below) or from a fresh `observe` before another write.
5. Advance using either a fresh `observe` or a verified first-submission
   `result.observation`:
   - **Conditional first-submission rule**: When one of the six supported
     mutations is submitted with `--with-observation`, consume its embedded
     `result.observation` directly to derive the next mutation **only** when the
     caller knows the request was its first submission (no hidden retries), the
     command exited `0`, and `result.observation` is present and complete.
     Generating a new idempotency key alone does not prove submission history.
   - **Mandatory `observe` fallback**: After any idempotent replay,
     `outcome lookup`, lost stdout, retry, unresolved submission history, or
     contention/conflict requiring refresh (`WORKSPACE_BUSY`,
     `SESSION_REVISION_CONFLICT`, `ITEM_REVISION_CONFLICT`,
     `GOAL_REVISION_STALE`, `ATTEMPT_NOT_CURRENT`), or when running an
     unsupported command, receiving a compatible unwrapped response, or lacking
     complete observation facts, obtain a fresh `podway --contract-generation 2
     --worktree <absolute-root> --json observe` before deriving any new mutation.
     Never derive multiple mutations from a single unrefreshed observation, and
     never treat historical success as current readiness.
6. Use `complete`, `skip`, `retry`, `decide`, `rework`, `block` or `unblock` only
   when current evidence and `allowed_actions` permit it. Block only for a real
   external dependency that prevents progress.

An authorized active workflow permits supported item updates and progression.
Initialization, session creation/replacement, archive/purge, cancel/reset/removal,
repair, and completed-session reactivation require explicit user intent for their
exact effects. Starting the next session permits automatic archival of a disposed
terminal session, not deletion of current work or inactive history.

## Graph, goals and evidence

- `decide` needs a nonblank reason and a currently allowed option. `rework` follows
  a declared target. Retry/rework creates fresh attempts; history does not satisfy
  them. Completed sessions can reactivate through rework or
  `goal revise --reactivate` under explicit intent;
  cancelled sessions cannot.
- Skip is a separate disposition allowed only by placement policy. It clears the
  attempt's item values; required items and open blockers do not gate it. Terminal
  skip still requires the applicable goal and fresh-assessment readiness.
- Define or revise a goal only with explicit intent in a goal-tracking Procedure.
  Assess its criteria only at the active goal-assessment decision after the work,
  using `goal assess-criteria --stdin` (optionally with `--with-observation` so
  three prepared/ready criterion assessments plus `decide` take two normal CLI
  calls instead of eight `observe`+mutation calls) or single-criterion
  `goal assess-criterion`. These `4 -> 2` action and `8 -> 2` goal-checkpoint
  counts are structural normal-path CLI invocation counts excluding setup,
  external work, contention, recovery and independent verifier observations; they
  claim no measured speedup or LLM-turn reduction, introduce no fused one-call
  `record-and-complete` or `assess-and-decide` operation, and imply no
  user-global installation. No batch `goal.assess_criteria` template is emitted
  in `mutation_templates`; construct the batch stdin document from the observed
  `goal.assess_criterion` template's `preconditions` and `guidance.goal`, then
  choose the `decide` option via the Procedure's declared outcome mapping
  (`not_achieved` maps to `--option not-achieved` in `bug-fix-v2`). Read
  [goals](references/goal.md) before choosing, defining, assessing or revising a
  goal, including cooperation with a Codex goal.
- Check reference/readback states together with `items_satisfied`, `unblocked`,
  `goal_ready` and `can_advance`. Required evidence must resolve and be fresh;
  an unresolved optional reference does not block progression.
- Previews are excerpts. Read a complete selected value with `podway
  --contract-generation 2 --worktree <absolute-root> --json evidence read
  --source <node> --item <item>`. Pass `next_page_token` back as `--page-token` until
  `truncated: false`. On `EVIDENCE_PAGE_TOKEN_STALE`, re-observe and restart at page
  one. On stale required evidence, rework its owner only when the graph requires it.
- Before an explicitly delegated action, read [delegation](references/delegation.md).
  Require a qualified integration, final item recording, settled worker state and
  current acceptance before completion. Neither receipts nor retry/rework launch
  external work.

## Resume and recover

After context loss, re-observe and match the recorded workspace UUID/session ID.
An identity change needs authorization covering that new target; revisions within
the same authorized session do not. Derive new work from the current attempt,
not notes. Reuse external checks when their actual inputs still match and the
Procedure permits it; resuming does not repeat the archive advisory.

Retain an unresolved mutation's original request, key and recovery location.
Read [recovery](references/recovery.md) for stale state, unknown outcomes or Store
failures. Only successful mutations retain outcomes; exact replay returns the
entire original envelope and request ID. Do not blindly retry or change a key.
Structured recovery guidance never grants authorization or weakens fences.

Read [lifecycle](references/lifecycle.md) before authorized initialization, start,
replacement, archive, purge, cancellation, reset, removal or repair. For bounded
work without a goal, prefer `small-change-v2`; for source-backed analysis, prefer
`analysis-v2`. Select a fuller or custom Procedure only when its gates fit the work.
