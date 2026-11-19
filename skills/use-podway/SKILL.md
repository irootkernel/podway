---
name: use-podway
description: Operate an explicitly authorized Podway Procedure v3 and Execution Group v1 session in one Git worktree. Covers scoped progression, history, goals, delegation and recovery. Use create-podway-procedure for source authoring.
---

# Use Podway

## Scope and entry

Use this runtime skill when the user explicitly requests a Podway operation or
an approved workflow names it. This runtime skill applies the grant to that
operation and worktree.
Inspection alone gives no authority to record items, advance or delete state.
Podway records caller assertions and formal progression. It cannot perform work
or establish the truth of a result. Route source changes to `$create-podway-procedure`.

1. Resolve the owning Git worktree and `.podway/config.yaml`. Read `podway
   version --json --identity` and match the exact build and contract manifest to
   the applicable source or release. This edition requires Procedure v3,
   Execution Group v1 and Store generation 16. To continue older work use its
   original writer and guidance. See [recovery](references/recovery.md#older-writers).
2. Run `podway --contract-generation 2 --worktree <absolute-root> --json observe`.
   Require `podway.output/v4` with `result.schema` equal to
   `podway.observation-result/v8`. Generation 1 is the deprecated default and
   serves v7 with no executable templates. Both selectors use the same writer,
   routes and runtime model. Failures use `podway.error/v2`. Read
   `SESSION_NOT_FOUND` as no current session and `WORKSPACE_NOT_INITIALIZED` as
   no initialized workspace. Start or initialize only under explicit intent.
3. Read `result.lifecycle`, `result.cursor`, `result.selection`,
   `result.guidance`, `result.items` and `result.goal`. Use `guidance.ready`,
   `guidance.available_operations` and `result.open_blockers` to plan the current
   work. `result.workspace` is the UUID. Use `result.workspace_sequence` to
   identify the observed workspace snapshot and `cursor.session_revision` as
   the session revision. Each scoped attempt binds the session, base digest,
   local node and unique attempt, plus `group_execution_id` and
   `definition_digest` for group scope. A prepared session has no active attempt. For terminal state,
   read `lifecycle_guidance` and establish disposition before archive or replacement.
4. Once per workflow entry, read `archive list`. If the inactive count exceeds
   ten, report it and offer the two exact deletion boundaries in
   [lifecycle](references/lifecycle.md#archive-and-purge). This advisory grants
   no deletion authority. Report a failed count read and continue authorized work.

## Command execution

Use JSON and an explicit `--worktree <absolute-root>` for workspace operations.
For executable recipes use `--contract-generation 2`. Local `help`, `version`,
`completions`, `preset` and `procedure` need no workspace flags or mutation fences.
Consult `podway help <route>` for route grammar and use the closed schema tied to
that binary. Machine callers branch on stable fields and error codes.

- For argv mutations preserve concrete template fences. For an active attempt,
  use the complete observed `result.cursor` as one JSON argument to
  `--if-cursor`, together with `--if-workspace-uuid`. Supply the observed item
  revision as `--if-item-revision` on single-item writes. Two writers can record
  different items across session revisions when both item revisions, attempt and
  selection still match. Cursor-changing and batch mutations also require the
  exact session revision. For prepared or terminal lifecycle routes, follow
  their identity/revision templates. Fill only declared placeholders with real
  facts and a caller-owned unique stable idempotency key. Several CLI routes
  can fill omitted fences from fresh state. Automation should always target the
  state it inspected.
- `record --stdin`, `goal assess-criteria --stdin`, `group activate --stdin`,
  `group replan --stdin` and `delegation accept --stdin` own the request identity
  in their closed JSON input. Start with generation-2 `stdin_template` when
  supplied. Their `context` contains `workspace_uuid`, `idempotency_key` and
  `cursor` with `attempt`, `session_revision`, `goal_revision` and `selected_group`.
  The attempt already contains `session`. Omit the observation cursor's extra
  top-level `session` when constructing a stdin cursor. Preserve null goal
  or selection values. These routes have no automatic fence preflight and reject
  argv `--if-*` or `--idempotency-key`. `set <item> --stdin` reads text only and
  retains ordinary argv fences.
- `--with-observation` is a boolean flag available on plain non-preview `start`,
  `begin`, `record --stdin`, `complete`, `decide` and `goal assess-criteria
  --stdin`. Both selectors return `podway.mutation-observation-result/v2` with
  the normal `result.mutation_result` and a commit-bound v8 `result.observation`.
  Use it to derive the next mutation only when submission history is known,
  the request was sent once, it exited 0 and the observation is complete. After
  replay, outcome lookup, lost stdout, retry, conflict, unknown submission history
  or an unsupported/unwrapped response, obtain a fresh `observe` before deriving
  any new mutation. Keep the original wrapper preference when reconstructing a
  request. A new key alone cannot prove a first submission.

`--timeout` defaults to `5s`. Unsigned `ms`, `s` or `m` values are bounded by
`60m`. It supplies one admission-wait budget across maintenance and SQLite
boundaries, not an execution timer.

## Record and advance

Perform the active node's work before recording its actual results. Read
`result.items` for record revisions and `guidance.items` for effective
requirements. Read the current guidance again when a mutation can change its
readiness, guards or evidence. Keep actor attribution when the route supports it.

| Type | Operation |
| --- | --- |
| `confirm` | `check` / `uncheck` |
| `text`, `choice`, `integer` | `set` (text accepts `--stdin`) |
| `list` | `add` / `remove` |
| `artifact` | `attach` |
| `check_result` | `record --stdin` with the authored operation identity |
| Any recorded value | `clear` |

For `record --stdin`, use `podway.scoped-item-record-many-input/v1`. Each of
1..128 unique operations supplies `item_id`, `expected_item_revision` and either
a typed `record` or `clear: true`. Validation and commit cover the entire batch.
For `check_result`, fill the declared `operation_id` and `operation_digest`,
`input_basis` (`descriptor`, `digest`), `executor` (`name`, `version`),
`outcome`, `summary` and `output_digest` from the actual check. Hash retained
bytes with a reproducible method. Artifact attachment records metadata and
rechecks required local files at completion. `ARTIFACT_CHANGED` requires fresh
attachment of the identified, verified file.

Use an allowed ordinary decision option and a nonblank evidence-based reason.
Read `guidance.allowed_option_ids`, `guidance.replan_option_ids` and guarded
`guidance.option_guard_statuses`. Use `group replan` for an eligible replan. Use `retry`
or declared `rework` only when the current work supports a fresh attempt. Use
`skip` only on an eligible nonterminal placement and supply a reason when its
policy requires one. Block for a real dependency and unblock when it is resolved. Advance promptly when
ready. For tracked sessions, read [goal assessment](references/goal.md).

## Move through the group boundary

- `start` pins the base. `begin` opens its first attempt. The base design anchor
  records the current plan inputs. When `group.activate` is available, use its
  closed stdin template with the exact plan, expected group digest and actual
  working-copy basis. A catalog proposal stays inactive until explicit activation.
- One group execution is selected at a time. The same definition digest can
  produce different execution IDs and fresh attempts. Complete ready group
  actions. Use `group return` at the selected group's terminal to close it and
  open the base return placement atomically. The base uses that group's declared
  outputs and owns final goal assessment and session completion.
- Replan from a declared, currently eligible decision only, using `group replan
  --stdin` with option, reason, working-copy basis and feedback. Settle affected
  workers and uncertain external effects first. This closes the selection and
  opens a new anchor attempt, preserving prior work. A replacement activation
  links the predecessor, records scoped prior-work dispositions and uses the
  new plan and basis. For details and the staged failure policy, read
  [group lifecycle](references/lifecycle.md#activate-return-and-replan).

## Evidence and recovery

Use `evidence read --source <selector> --item <item-id>` for a selected value
available to the current attempt. Select a declared local node, `@plan` or
`@execution-result` as allowed by current evidence. Use the consumer attempt
fence and follow `next_page_token` until `truncated: false`. After a stale token,
re-observe and restart paging. For earlier group/design/attempt records use
`group list`, `group show`, `group prior-work` and `history` queries. Retained
values help explain or revalidate work. Only current declared sources satisfy
a gate. Consult [lifecycle](references/lifecycle.md#read-retained-history).

On resume, re-observe and compare workspace/session identity before recording.
For an uncertain result retain the exact request and use original-key lookup
before any exact replay. See [recovery](references/recovery.md). Read
[delegation](references/delegation.md) before any explicitly delegated action.
Initialization, start/replacement, cancellation, archive/purge, reset/removal,
repair and completed-session reactivation need explicit authority for the exact
effect. See [lifecycle](references/lifecycle.md) before acting.
