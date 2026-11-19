# Operate a Delegated Action

Read for an authorized v3 base or v1 group action that declares `delegation`.
Use the exact compatible CLI and a qualified integration for its binding. The
Primary judges and accepts results. The integration owns dispatch, feedback,
settlement, durable task identity and worker recovery. Podway fences the exact
scoped attempt and persists the caller's reported acceptance. Source support
alone supplies no executor or semantic proof.

## Package, supervise, record, accept, complete

1. Observe v8 and check the active placement, `result.cursor.attempt`, selected
   execution and delegation declaration/state. Use ordinary Direct commands
   for a placement with no delegation. Read `delegation package --json` on the
   current delegated attempt and retain `podway.delegation-package-result/v2`,
   its complete `package` and `package_digest`. Resolve selected evidence with
   `evidence read` and page each needed value to completion.
2. The authorized integration resolves the exact Role and configuration,
   dispatches the work and supervises feedback. The Primary explicitly evaluates
   and accepts the external result. Finish all required and used optional items
   before acceptance. Confirm that `after_accept: retain` settled the worker
   to `idle`, or `after_accept: release` settled it to `released`.
3. Re-observe after item writes. Match the current active scoped attempt and
   package digest, then prepare `podway.delegation-accept-input/v2`. Use the
   [stdin context](../SKILL.md#command-execution) with the newest session revision
   and keep the exact original key/body. Submit the closed document (at most
   32 KiB) via `podway --contract-generation 2 --worktree <absolute-root> --json
   delegation accept --stdin < acceptance.json`. Use unsigned integer JSON
   literals for revisions and rounds. Do not supply argv preconditions or a key.
4. Require `podway.delegation-accept-result/v3`. Acceptance creates an immutable
   receipt and leaves the cursor in place. Re-observe, then `complete` when the
   current receipt and ordinary readiness permit it. `delegation accept --stdin`
   has no `--with-observation`. Keep receipt settlement and completion distinct.

## Acceptance field sources

| Fields | Authority |
| --- | --- |
| `context` | Workspace UUID, active scoped stdin cursor from current observation and caller-owned idempotency key |
| `package_digest` | Complete current package for that exact scoped attempt |
| `execution.binding_ref`, `execution.role_ref`, `execution.role_snapshot_digest` | Integration facts matching the declaration |
| Other `execution` fields | Reviewed integration identity and durable acknowledgement |
| `rounds_used`, `worker_disposition` | Accounted feedback rounds and settled worker |
| `result_digest` | Immutable complete external result under that identity |
| `summary`, `accepted` | Primary explanation and explicit acceptance (`accepted: true`) |

The other execution fields are `adapter_id`, `adapter_version`, `primary_host`,
`scope_kind`, `scope_ref`, `supervisor_ref`, `worker_ref`, `task_ref`,
`agent_configuration_digest` and `accepted_task_digest`. Do not invent them from
local labels or private databases. The integration's `scope_kind` and `scope_ref`
do not replace Podway's scoped attempt. Exclude credentials, transcripts and host
paths. Podway computes the accepted item identity from the recorded item state.

## Inspect and recover

`delegation receipt --session <session-id> --attempt <attempt-id>` is read-only.
`podway.delegation-receipt-result/v2` reports the full scoped `target`,
`acceptance_state` (`missing`, `stale` or `accepted`), `receipt` and `applicability`
(`current` or `historical`). Compare them with the current attempt and selection.
A historical receipt is evidence of its original acceptance only.

| Result or error | Action |
| --- | --- |
| `missing` or `DELEGATION_RECEIPT_REQUIRED` | Finish current recording and verified worker settlement before acceptance |
| `DELEGATION_BINDING_MISMATCH` | Reconcile actual Role, binding, round and worker disposition against the current declaration |
| `accepted` with `current` applicability | Complete only with ordinary readiness and fresh fences |
| `stale` or `DELEGATION_RECEIPT_STALE` | Use retry or declared rework, then revalidate work for the new attempt |
| `DELEGATION_ALREADY_ACCEPTED` | Inspect the original immutable receipt |
| Lost response | Use original-key recovery in [recovery](recovery.md) |

Any committed item-state edit after acceptance makes the receipt stale,
including a cosmetic, optional or reversing edit. Reads and true no-ops preserve
it. Retry or rework does not launch another external task. Before replan or goal
revision, settle affected descendants and reconcile uncertain effects with the
integration. A prior scoped receipt cannot be moved to a new activation.
