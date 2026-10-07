# Operate a Delegated Action

Read this reference when the authorized Procedure declares `delegation`. It
supplements the normal observe, fencing, and recovery loop in `SKILL.md`.

## Establish availability

Require a compatible exact CLI/manifest and a qualified integration for
the declared binding. Source support, a Role name, and successful Procedure
validation do not establish a working executor. If the integration is absent,
report that prerequisite; do not fabricate an acceptance, run a fixture against
real work, or bypass delegation by direct execution.

Podway owns structural progression; the Primary judges the result; the
integration owns dispatch, feedback, durable task identity, and worker recovery.
An external Primary uses its external integration; a managed Primary uses its
authorized Broker integration. `primary_host` is attribution, not authorization
or permission to switch those paths.

## Read, supervise, record, accept, complete

1. Observe the current session and require `podway.observation-result/v5` for a
   delegated Procedure. Confirm the active action, attempt, declaration, and
   `status.delegation.state`. A Direct placement in that Procedure still follows
   the ordinary item and completion rules.
2. Read `podway delegation package --json`. Require
   `podway.delegation-package-result/v1`; retain the complete package and its
   `package_digest`. Read selected evidence with `podway evidence read --json
   --source <node> --item <item>` until `truncated` is false. Bounded previews
   are not full task input.
3. Have the qualified integration resolve the exact Role/configuration and
   dispatch the actual task. The Primary supervises feedback and explicitly
   accepts the final result. A delivered result or an empty findings list is
   not itself acceptance. Keep external task and result identities distinct
   from Podway's package and attempt identities.
4. Record the final required items and any optional values that will be used;
   an optional item may remain unset. Resolve blockers. Verify through the
   integration that `after_accept: retain` has settled to `idle`, or that
   `after_accept: release` has completed release and settled to `released`.
5. Observe again after the last mutation. Assemble the closed
   `podway.delegation-accept-input/v1` document from the sources below, retain
   its exact bytes and idempotency key for recovery, and submit:

   ```bash
   podway delegation accept --stdin --json < acceptance.json
   ```

   The document supplies all mutation identities. Do not also pass identity,
   revision, or idempotency flags. Keep the complete input within 32 KiB and
   encode revision and round counts as unsigned integer JSON literals.
6. Require `podway.delegation-accept-result/v2`, then observe again. Acceptance
   creates one immutable receipt per attempt and does not move the cursor.
   Only complete when the current receipt and ordinary readiness conditions
   permit it, using fresh observed fences and a new completion idempotency key.
   Follow the current command help and observation mutation template.

Preserve the workflow's explicit owning worktree on every command.
The command examples above omit those already-established global options.

## Acceptance field sources

| Fields | Producing authority |
| --- | --- |
| `schema` | Exactly `podway.delegation-accept-input/v1` |
| `workspace_uuid` | Current observation envelope's `workspace.uuid` |
| `session_id`, `session_revision` | Current `result.status.session.id` and `.revision` |
| `attempt_id` | Current `result.status.current.attempt.attempt_id` |
| `idempotency_key` | A unique caller-generated key retained with the exact request |
| `package_digest` | Current package result, for this exact attempt |
| `execution.binding_ref`, `execution.role_ref`, `execution.role_snapshot_digest` | Integration-verified identities matching the declaration exactly |
| Remaining `execution` fields | Qualified integration's versioned mapping and durable acknowledgement |
| `rounds_used`, `worker_disposition` | Integration's durable round accounting and settled worker state |
| `result_digest` | Complete immutable external result under the integration's mapping |
| `summary`, `accepted` | Primary's bounded explanation and explicit decision; `accepted` must be true |

The remaining execution fields are `adapter_id`, `adapter_version`,
`primary_host`, `scope_kind`, `scope_ref`, `supervisor_ref`, `worker_ref`,
`task_ref`, `agent_configuration_digest`, and `accepted_task_digest`.
Do not derive missing values from labels, task prose, a result summary, or private
executor databases. Do not include credentials, transcripts, or host paths.
`accepted_items_digest` is computed by Podway, not supplied by the caller.

## Inspect and recover

Read one receipt with `podway delegation receipt --json --session <session-id>
--attempt <attempt-id>`. Require `podway.delegation-receipt-result/v1` and use
its applicability together with a fresh observation. A historic receipt does
not satisfy a fresh attempt.

| Observation or error | Next action |
| --- | --- |
| `awaiting_acceptance` / `DELEGATION_RECEIPT_REQUIRED` | Finish external supervision and final recording before accepting. |
| `DELEGATION_BINDING_MISMATCH` | Compare the declared Role/binding, current package, round count, and disposition against the actual integration result. Do not relabel old work. |
| `accepted` | Complete only with current ordinary readiness and fresh fences. |
| `stale_acceptance` / `DELEGATION_RECEIPT_STALE` | Choose retry or an allowed rework, then revalidate work for the fresh attempt. |
| `DELEGATION_ALREADY_ACCEPTED` | Inspect the immutable receipt; a different key cannot replace it. |
| Lost acceptance response | Follow [recovery](recovery.md), retaining the original key and exact canonical request; use outcome lookup before replay. |

Any actual item edit after acceptance, including a cosmetic or optional edit,
makes the receipt stale. Reverting text does not restore its revision. Reads,
true no-ops, and rolled-back edits preserve applicability. Exact replay can
return historic success while current readiness remains stale.

Retry and rework do not redispatch external work. Reconcile uncertain external
effects through the integration owner before another assignment. Reusing an
artifact requires explicit revalidation against current inputs and acknowledged
task identity; copying an old receipt is never recovery.

## Learn with the isolated example

The [supervised review walkthrough](https://github.com/irootkernel/podway/blob/main/docs/examples/supervised-delegation.md)
links the canonical YAML and the real-CLI test command. Its executor and digests
are synthetic and prove only local protocol behavior. Use the example and
instructions from the same checkout or pinned revision. Installed skill files
do not include a live adapter or permission to create a session.
