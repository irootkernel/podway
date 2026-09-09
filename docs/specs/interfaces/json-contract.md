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
`session-reset-result/v1`, `status-result/v3`, `compact-status-result/v3`, and
`prepared-next-result/v1`. Running next uses `next-result/v3` and observation uses
`observation-result/v3`, and the contract adds `evidence-read-result/v1` for the
`evidence.read` query. All three families are served. Item and graph-transition
families retain their current versions, and decision, rework, goal, authoring, and
platform results remain first-version families where their shapes do not change.

`next-result/v3` and `observation-result/v3` are new versions because their evidence
payloads change materially: an evidence projection carries source and item identity,
the complete-value digest, and the total logical size instead of the complete value.
`next-result/v3` adds bounded preview state and a continuation token; observation
guidance carries metadata only.
`observation-result/v3` retains the prepared-aware lifecycle and template semantics
while reducing its status member from the standard tier to a bounded, value-free
compact projection, because observation is the current-state guidance surface and its
own guidance and active-item windows already carry what a mutation needs. A caller
that wants the standard or verbose status projection, including trace history, uses
`session.status`. Evidence metadata total and truncation fields are always present,
are invariantly non-truncated for `session.next`, and may be truncated only in the
observation-specific projection; observation guidance carries no preview data and no
page token.

ADR-0025 adds a closed `check_result` declaration and complete recorded-value
variant. The complete value, five-field preview, read-back page, compact item,
and observation constraints are distinct schema definitions; a complete value is
never substituted where a bounded projection is promised. The complete
check-result and artifact value branches remain disjoint under exact-one `oneOf`
validation because both are closed and require non-overlapping field sets.

`item-record-many-input-v1` and `item-record-many-result-v1` admit the new closed
record kind, and `observation-result/v3` admits the declaration constraints,
five-field value projection, and conditional `stdin_template`. These are closed
kind additions within the exact manifest compatibility gate, not new command
families. Existing variants retain their field sets and unknown-field closure;
peers with a different manifest reject the complete contract set.

ADR-0028 adds closed typed-predicate definitions to the Procedure and preview
schemas. `required_when` and option `guards` are optional one-to-four-element
arrays with no materialized defaults. Running next keeps the complete authored
option array, adds authoritative `allowed_option_ids`, and conditionally adds
complete bounded `option_guard_statuses`. Observation active items expose authored
`required`, derived `required_now`, and bounded condition status. Text and list
statuses expose only derived counts, never recorded content.

These fields are added to the unreleased `next-result/v3` and
`observation-result/v3` families under the exact manifest compatibility gate.
They do not create v4 families or widen a released peer silently. The closed
`OPTION_GUARD_UNSATISFIED` details branch carries the selected option ID and the
same bounded predicate-status shape.

Adding a result family to the `output-v3` command-to-result selection is not widening
a released closed schema: each released result schema keeps its own identifier, field
set, and unknown-field closure. Within the manifest fail-closed gate, a released shared
family may also receive a bounded in-place numeric constraint update or an additional
closed `kind` variant, because neither alters an existing variant's field set or its
unknown-field closure, and a peer with a different manifest digest already fails closed
on the whole contract set rather than silently accepting a wider value. A materially
changed payload shape still requires a new version, as `next-result/v3` and
`observation-result/v3` do.

The output-v3 `session.next` branch accepts the running next result or the
disjoint cursor-free prepared result. The `evidence.read` branch is
procedure-independent and carries no durable job. Prepared status uses null cursor, attempt,
goal, and readiness projections; prepared observation has no active items and
contains only bounded lifecycle guidance and applicable fenced templates.
Existing released closed schemas are not widened in place, apart from the bounded numeric constraint updates and closed `kind` additions described above, which change no field set and no unknown-field closure.
Released v3 job wrappers remain closed over their original command set. `job status`,
`job wait`, and `job lookup` emit v4 wrappers, which add prepared-lifecycle commands
while keeping an embedded terminal success as a non-recursive `podway.output/v3`
document. The output envelope accepts both wrapper generations for compatibility.

The error envelope contains a catalogued `code`, summary, retryability, exit code,
request correlation, command, and closed code-specific details. Human messages are
not stable API. `LEGACY_PROCEDURE_STATE_UNSUPPORTED` means the opened runtime contains
Procedure v1 task state. Podway performs no automatic conversion or deletion; after
backup, `podway reset --all` is the supported recovery.

The runtime-reset families are `runtime-reset-plan-result/v1`,
`runtime-reset-result/v1`, and `runtime-reset-control-result/v1`, each prefixed
with `podway.`. Plan, confirmed single-mode apply, and process-bound control are
executable; account-wide apply remains reserved. Their output branches omit
workspace, job and session projections.
An incomplete reset is accepted only inside the closed
`podway.runtime-reset-error-details/v1` branch of an error, never a success output.
The reset-specific `retry` object requires explicit confirmation and is distinct
from the existing read-only `recovery` recipe. See the
[runtime reset specification](../operations/runtime-reset.md#public-results-errors-and-bounds).

Canonical request identity excludes transport timing but includes all fields that
can change execution. The daemon stores bounded semantic terminal projections and
the exact public terminal envelope required for idempotent replay. A lost response
is reconciled read-only by idempotency key before a caller decides whether to retry.

Contract assets are canonical under `assets/schemas/`; the generated manifest under
`contracts/` records exact bytes and digests. New result families require a new
schema identifier. Additive envelope fields are allowed only where the schema leaves
the object open; automation must ignore unknown open-envelope metadata.
