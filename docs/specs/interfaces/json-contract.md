# JSON Contract

All successful public commands emit `podway.output/v3`. The closed schema is
[`output-v3.schema.json`](../../../assets/schemas/output-v3.schema.json). Failures
emit `podway.error/v1`; IPC requests remain `podway.ipc/v1`. These identifiers
version different contracts and do not imply Procedure support.

The success envelope contains `schema`, `request_id`, `command`, `generated_at`,
`warnings`, and a command-bound `result`. Workspace, job, session, and admission
objects are present only where their command family permits them. Unknown fields in
closed result objects are rejected. The output schema binds every command to one
allowed result schema, including Procedure v2 session families and
procedure-independent service/workspace families whose own first-version IDs remain
`/v1`.

Procedure runtime results identify `procedure_schema: podway.procedure/v2`.
Prepared-aware routes use `session-start-result/v3`,
`session-begin-result/v1`, `terminal-disposition-result/v1`,
`session-reset-result/v1`, `status-result/v3`, `compact-status-result/v3`,
`prepared-next-result/v1`, and `observation-result/v2`. Running next retains its
v2 family until the separately adopted evidence-scale contract introduces
`next-result/v3`. Item and graph-transition families retain their current
versions, and decision, rework, goal, authoring, and platform results remain
first-version families where their shapes do not change.

The output-v3 `session.next` branch accepts the running next result or the
disjoint cursor-free prepared result. Prepared status uses null cursor, attempt,
goal, and readiness projections; prepared observation has no active items and
contains only bounded lifecycle guidance and applicable fenced templates.
Existing released closed schemas are not widened in place.
`job status`, `job wait`, and `job lookup` use their v3 wrapper schemas so an embedded
terminal success is a non-recursive `podway.output/v3` document.

The error envelope contains a catalogued `code`, summary, retryability, exit code,
request correlation, command, and closed code-specific details. Human messages are
not stable API. `LEGACY_PROCEDURE_STATE_UNSUPPORTED` means the opened runtime contains
Procedure v1 task state. Podway performs no automatic conversion or deletion; after
backup, `podway reset --all` is the supported recovery.

Canonical request identity excludes transport timing but includes all fields that
can change execution. The daemon stores bounded semantic terminal projections and
the exact public terminal envelope required for idempotent replay. A lost response
is reconciled read-only by idempotency key before a caller decides whether to retry.

Contract assets are canonical under `assets/schemas/`; the generated manifest under
`contracts/` records exact bytes and digests. New result families require a new
schema identifier. Additive envelope fields are allowed only where the schema leaves
the object open; automation must ignore unknown open-envelope metadata.
