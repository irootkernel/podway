# Operate a Delegated Action

Read only for an authorized Procedure declaring `delegation`. Require the exact
compatible CLI/manifest and a qualified integration for its binding. Source
support, a Role label or valid YAML is not a working executor. Do not fabricate
provenance, use a fixture receipt for real work or bypass required delegation.

Podway guards structural progression. The Primary judges results; the integration
owns dispatch, feedback, durable task identities and worker recovery. External and
managed Primaries use their respective authorized integration paths;
`primary_host` is attribution, not authority to switch them.

## Package, supervise, record, accept, complete

1. Observe (`podway.observation-result/v5`) and confirm the active action/attempt,
   declaration and `status.delegation.state`. Ordinary Direct placements still use
   the normal item/progression rules.
2. Read `podway delegation package --json`; require
   `podway.delegation-package-result/v1`, retaining the complete package/digest.
   Read selected evidence fully with `evidence read`, continuing until not truncated.
3. The integration resolves the exact Role/configuration and dispatches the actual
   task. The Primary supervises feedback and explicitly accepts the final result.
   Delivery or an empty findings list is not acceptance.
4. Finish required items and any optional values used by the result, then resolve
   blockers. Verify that `after_accept: retain` settled the worker to `idle`, or
   `after_accept: release` completed release and settled to `released`.
5. Re-observe after the last mutation. Assemble the closed acceptance document from
   the sources below, retain its exact bytes/key and submit:

   ```bash
   podway delegation accept --stdin --json < acceptance.json
   ```

   Input supplies all mutation identities; do not also pass identity, revision or
   key flags. Keep it within 32 KiB and use unsigned integer JSON literals for
   revisions and round counts.
6. Require `podway.delegation-accept-result/v2`, then re-observe. Acceptance creates
   one immutable receipt per attempt but does not move the cursor. Complete only
   when current acceptance and ordinary readiness permit it, with fresh completion
   fences and a new key.

Keep the established owning worktree on every command.

## Acceptance field sources

| Fields | Authority |
| --- | --- |
| `schema` | `podway.delegation-accept-input/v1` |
| `workspace_uuid` | Observation `workspace.uuid` |
| `session_id`, `session_revision` | `result.status.session.id`, `.revision` |
| `attempt_id` | `result.status.current.attempt.attempt_id` |
| `idempotency_key` | Caller-generated unique key retained with this request |
| `package_digest` | Current complete package for this attempt |
| `execution.binding_ref`, `execution.role_ref`, `execution.role_snapshot_digest` | Integration-verified identities matching the declaration |
| Remaining `execution` fields | Reviewed integration mapping and durable acknowledgement |
| `rounds_used`, `worker_disposition` | Durable round accounting and settled worker state |
| `result_digest` | Immutable complete external result under that mapping |
| `summary`, `accepted` | Primary explanation and explicit decision; `accepted: true` |

The remaining execution fields are `adapter_id`, `adapter_version`, `primary_host`,
`scope_kind`, `scope_ref`, `supervisor_ref`, `worker_ref`, `task_ref`,
`agent_configuration_digest` and `accepted_task_digest`. Do not derive missing
identities from labels, task prose, summaries or private executor databases.
Exclude credentials, transcripts and host paths. Podway computes
`accepted_items_digest`; the caller does not supply it.

## Inspect and recover

Read `podway delegation receipt --json --session <session-id> --attempt <attempt-id>`;
require `podway.delegation-receipt-result/v1`. Compare its applicability with fresh
observation; a historical receipt does not satisfy another attempt.

| State/error | Action |
| --- | --- |
| `awaiting_acceptance` / `DELEGATION_RECEIPT_REQUIRED` | Finish supervision and recording before acceptance |
| `DELEGATION_BINDING_MISMATCH` | Compare actual execution against binding, Role, rounds and disposition; do not relabel old work |
| `accepted` | Complete only with ordinary readiness and fresh fences |
| `stale_acceptance` / `DELEGATION_RECEIPT_STALE` | Retry or declared rework, then revalidate work for the new attempt |
| `DELEGATION_ALREADY_ACCEPTED` | Inspect the receipt; another key cannot replace it |
| Response loss | Follow [recovery](recovery.md) with original request/key and outcome lookup |

Any committed item-state change after acceptance, even cosmetic/optional, makes the receipt
stale. Reverting content does not restore revision. Reads, true no-ops and rolled
back edits preserve applicability. Exact replay can return historical success
while current readiness remains stale.

Retry/rework does not redispatch external tasks. Reconcile uncertain external
results through their integration owner before reassigning. Reused artifacts need
revalidation against current inputs and acknowledged task identity, not a copied
receipt.

The [supervised review example](https://github.com/irootkernel/podway/blob/v0.2.14/docs/examples/supervised-delegation.md)
uses a synthetic executor and proves only isolated protocol behavior. Use source
and instructions from that published tag, or the exact validating release's tag.
It supplies neither a
live adapter nor permission to create a session.
