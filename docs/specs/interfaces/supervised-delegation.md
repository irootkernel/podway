# Host-Neutral Supervised Delegation

## Authority and availability

This is the implemented host-neutral foundation accepted by
[ADR-0035](../../architecture-decision-records/0035-host-neutral-supervised-delegation.md)
and retained by [ADR-0038](../../architecture-decision-records/0038-remove-the-daemon.md).
The single CLI's in-process Engine serves package, structural acceptance, receipt,
and current projection routes. The [roadmap](../../roadmap/README.md) owns task and
release status; the source capability does not attest an installed release,
external adapter, model run, or production rollout.

The [integration candidate](../../todo/TODO-podway-supervised-delegation-integration.md)
records separate external Dolgorae/managed Primary integration work. No adapter or
neighboring checkout is required for deterministic Podway tests.

## SDG-001: Responsibilities and scope

Keep the Procedure v2 Action/Decision distinction and exactly one authoritative
outer cursor and active attempt. A Direct action is performed by the Primary;
a Delegated action is performed by a Specialist under that Primary's
supervision. A Primary may be externally hosted or runtime-managed. Supervisor
is a responsibility, not a separately required agent. No placement creates a
Primary or requires one new Run per node.

Podway owns formal readiness and transactional progression validation. The
Primary owns semantic acceptance. The external runtime owns Roles, configurations, worker execution,
round accounting, task delivery, and operational recovery. Neither worker
completion nor receipt acceptance advances the cursor.

Exclude fan-out, fan-in, child work units, claims, leases, dynamic maps,
continuation generations, driver election, live Primary replacement, a web
console, network access, process execution, and same-user authentication.

## SDG-002: Explicit opt-in declaration

An action placement may contain one optional closed `delegation` object. It is
not a node-definition property. Decision placements, including session-goal
assessment decisions, reject it. A placement with delegation rejects
`skip.allowed: true`; the foundation has no waiver or direct fallback.

| Field | Required contract |
|---|---|
| `binding_ref` | Logical integration binding; ASCII `[a-z][a-z0-9]*(?:-[a-z0-9]+)*`, 1 to 64 bytes |
| `role_ref` | Opaque nonempty executor-owned reference, at most 256 UTF-8 bytes, no control characters |
| `role_snapshot_digest` | Existing lowercase `sha256:` digest representation |
| `max_rounds` | Integer from 1 through 32; no implicit default |
| `after_accept` | Exactly `retain` or `release`; no implicit default |

Every field is required when the object exists. Reject unknown fields, nulls,
wrong types, executable command/path fields, or a second execution-mode
declaration. Compare
opaque values exactly; do not trim, case-fold, normalize Unicode, or infer a
Role, Profile, provider, or model. The binding reference is data, not a command
or a Podway configuration lookup.

The declaration participates in canonicalization, snapshots, and the Procedure
digest. Omission must not materialize an empty object or a default value and
must leave old canonical bytes and digests unchanged. Procedures without the
field keep their canonical bytes and Direct behavior; runtime projections use the
current common direct generations. No built-in preset,
tracked Aquarium Procedure, default selection rule, or installed skill is
converted to delegation by this epic.

A future integration supplies the binding externally. Failure to resolve it
blocks that integration's explicitly delegated work, while unrelated Direct
sessions and ordinary local recovery remain available. Do not scan for
installed agents, read their homes, or check their availability on ordinary
Podway commands.

## SDG-003: Host-neutral execution identity

Do not equate Podway workspace/session/attempt identity with executor
workspace/aggregate/Run/task identity. The acceptance payload carries a closed
`execution` object with these required fields:

| Field | Meaning |
|---|---|
| `binding_ref` | Exact declared logical binding |
| `adapter_id`, `adapter_version` | Bounded opaque adapter identity, not a command |
| `primary_host` | `external` or `managed`, describing where the Primary runs |
| `scope_kind`, `scope_ref` | Opaque executor scope kind and identity; not specifically an external engagement |
| `supervisor_ref` | Non-secret provenance of the Primary that accepted the result |
| `worker_ref`, `task_ref` | Exact external member and final accepted task references |
| `role_ref`, `role_snapshot_digest` | Exact declared Role requirement |
| `agent_configuration_digest` | Executor-owned immutable configuration identity |
| `accepted_task_digest` | Digest identifying the final external task as accepted by its executor |

`adapter_id`, `adapter_version`, and `scope_kind` use nonempty printable ASCII
up to 128 bytes. Other opaque reference fields use nonempty UTF-8 up to 256
bytes without control characters. Digests use the existing lowercase SHA-256
form. Unknown fields are rejected. No credentials, prompt transcripts, host
paths, controller tokens, or Role source bytes enter this object.

The adapter supplies these facts. Podway checks structural constraints and
required equality but cannot query or authenticate them. `primary_host` is
not a permission switch. No trusted-host identifier may be inferred from text
inside a model task. The real adapter must bind controller authority outside
model-controlled payloads.

A managed Primary must use its runtime's broker, not masquerade as an external
engagement. One host-neutral contract can represent both paths without claiming
they share authentication, wire shapes, or live capabilities.

### Adapter semantic mapping

Before enabling a real adapter, close the meaning of every required execution
field through a source-backed external/managed adapter mapping.
Include `result_digest`, `rounds_used`, and `worker_disposition` even though they
are outside the execution object. For each path, record the producing authority,
permitted response/read/host-binding surface, identity scope, exact
normalization, and response-loss recovery. A matching field name or an internal
Store column alone is not evidence of an accessible equivalent contract.

For each external digest, specify the immutable object and schema/version,
included/excluded fields, canonicalization, encoding, algorithm and textual
representation. Pin those rules to a versioned adapter mapping. Role-name
hashes do not identify Role content; prepared task bytes are not proof of
executor acceptance; an artifact ID or result summary is not the complete
accepted result. Exact derivation from permitted retained bytes is possible
only with a defined rule and authoritative linkage to the admitted Role,
configuration, task, or result. No digest may be replaced with another digest
merely because both are SHA-256 strings.

Classify each real-path mapping as contract-mapped, derivation-candidate,
needs-verification, needs-support, or incompatible-mapping. Any unresolved
external prerequisite keeps that integration path unavailable. It does not
block the defined Podway-side contract, deterministic testing, or Direct work.
An unresolved Podway-field meaning requires contract resolution before adoption.
Do not use placeholders, weaken required fields, scrape
private executor Stores, or switch a managed Primary to the external facade to
hide a gap. Podway still checks only structure and declared equality, not the
truth of caller-supplied external bytes.

The [adapter contract review](../../implementation-tips/delegation-adapter-contract-review.md)
contains the reviewed source baseline, field matrix, known mismatches, and
pre-reservation checklist. The review is offline design work, not a requirement
for a live model, installed adapter, or neighboring checkout in Podway tests.

## SDG-004: Canonical work package

Provide the read-only `delegation.package` query, surfaced as
`podway delegation package --json`. It resolves the selected current workspace
using the same workspace, session, and attempt rules as current direct reads.
It succeeds only for a running delegated action and writes no outcome or state.

The result `podway.delegation-package-result/v1` contains a closed package, its
`package_digest`, and the fresh ordinary mutation fences outside that package.
The closed package uses `podway.delegation-work-package/v1` and contains:

- workspace UUID, Procedure digest, session ID, placement ID, and attempt ID;
- the exact delegation declaration;
- the action title, intent, description, instructions, and item declarations,
  preserving validated snapshot content and existing absence semantics;
- the attempt-bound goal revision and statement/criteria, or null when absent;
- declared upstream evidence selectors and their frozen resolution, attempt and
  item identities, exact complete-value digests, and typed size metadata.

Each evidence entry keeps its declared selector and names `resolved`, `skipped`,
or `unresolved`. Resolved and skipped entries carry the source attempt ID,
attempt number, and frozen complete-item-set digest; unresolved entries omit
them. An omitted selector item list means all source
items, up to the Procedure's 128-item definition limit. Missing selected items
remain explicit in the package. Skipped and unresolved sources have only missing
selected items; present items retain their declared item type.
The goal projection uses the existing 1,000-character statement and up to 16
criteria with 300-character statements; those limits are part of the static
package budget.

Do not hash the current session revision, invocation timing, observation timestamp,
mutable current output values, or external Run identity into the package.
Recording the action's output items or replaying a read must not create a new
work assignment. Retry, rework, changed attempt-bound goals, or a different
Procedure changes the package identity.

Use the existing canonical JSON rules with the domain-separated digest input
`podway.delegation-work-package/v1\n` followed by canonical package bytes. UTF-8
bytes, not display character counts, determine encoded response limits. Reuse
existing typed field bounds; additionally prove that the complete package result
and envelope fit the 1 MiB complete-output budget. Reject a delegated document
whose worst-case projection cannot fit during static Procedure validation; do not
impose that rejection on a non-delegated document or discover a predictable
overflow after execution
has begun. Budget dynamic goal data against its existing maximum, not the
example goal used in a test.

The package contains evidence identities, not a claim that previews or paths
are complete evidence. Full content is obtained through existing bounded
`evidence.read` requests under matching identities. An adapter must assemble
its task from the required full evidence, preserve provenance, and fail rather
than silently truncate required material. It records the digest of the actual
accepted external task separately from Podway's package digest. Podway does not
store or validate the executor's full prompt or its task bytes.

## SDG-005: Structural acceptance

Provide `delegation.accept`, surfaced as
`podway delegation accept --stdin --json`. Input uses bounded stdin decoding,
workspace/session revision, exact active attempt, and idempotency conventions,
plus a closed `podway.delegation-accept-input/v1` body containing:

- the package digest;
- the complete execution identity from SDG-003;
- `rounds_used`, an integer from 1 through the declared maximum;
- `result_digest`, the digest of the accepted external result;
- `summary`, nonempty UTF-8 without NUL, at most 4096 bytes;
- `worker_disposition`, exactly `idle` or `released`; and
- `accepted`, exactly true.

Reuse the canonical direct mutation identity and success-only outcome contract;
do not create a second idempotency protocol or a job/admission receipt. The
complete input is at most 32 KiB, including ordinary identity fields. Summary and
external attribution are excluded from default status even though an
explicit receipt read may expose them. `session_revision` and `rounds_used` require
unsigned integer JSON literals; `1.0` is rejected. The stdin document owns its
workspace/session/revision/attempt/key identities; duplicate CLI identity flags
are rejected. Acceptance is synchronous and records only a committed success.
Lost stdout requires original-key `outcome lookup` before exact-request replay;
COMMIT I/O uncertainty alone may emit `MUTATION_OUTCOME_UNKNOWN`.

At execution, in the ordinary worktree transaction, validate normal fences,
that the active action is delegated, exact declaration and package equality,
round bounds, and disposition. `retain` requires `idle`; `release` requires
`released`. Require the action's normal item-satisfaction and blocker conditions
before acceptance. The Primary must record the final action evidence before
submitting acceptance; the package remains stable because it excludes those
mutable outputs.

The Store computes `accepted_items_digest` from the current state inside the
acceptance transaction and verifies the receipt field before storing it. The
caller cannot choose this value. The Engine projects and supplies the expected
package digest to the Store. Store one immutable receipt for the attempt,
including the declared binding, package digest, execution object, result,
summary, round count, disposition, and accepted item-state digest. The mutation
advances the session revision once and never changes the cursor or active
attempt. Success uses `podway.delegation-accept-result/v2` with receipt identity,
current fences, package/result digests, and acceptance state.

For v1, sort the active attempt's complete item slots by item ID and encode each
as `item_id`, numeric `revision`, `disposition` (`recorded` or `unset`), and
`value_digest` (the existing canonical recorded-value digest, or `null` when
unset). Hash the canonical JSON array with SHA-256 after the UTF-8 prefix
`podway.delegation-accepted-items/v1\n`. Include optional and unset slots. This
digest excludes the session revision, blocker state, and read activity. Check
the receipt's digest against current slots again inside the completion write
transaction; a preflight observation cannot authorize completion on its own.

Exact-key replay returns the original result and does not re-evaluate external
state. Same-key changed input is the existing idempotency conflict. A second
non-replay acceptance for the same attempt is rejected; it cannot overwrite the
first receipt. This deliberately avoids a second per-round history or mutable
acceptance ledger.

All supplied execution facts are structural claims, not independently verified
execution, authorization, honest Role resolution, or proof of semantic quality.
The maximum round check validates a reported count; it cannot stop a rogue
executor from issuing more tasks.

## SDG-006: Readiness and subsequent changes

Direct action readiness and completion remain unchanged. For a delegated action,
normal completion additionally requires a current receipt whose package and
accepted item-state digest still match. An empty receipt slot is not success.
A malformed, mismatched, or stale receipt is not silently discarded or treated
as Direct execution.

The V2SDG-005 Store increment defines package currency as the immutable receipt
remaining bound to the live workspace, session, attempt, placement, procedure,
and declaration; it rechecks that binding and the accepted item-state digest in
the completion transaction. The public package/accept routes supply the
Engine-projected package digest at acceptance. Explicit custom-Procedure
use retains the complete native route and Store contract.

The active delegated projection reports `awaiting_acceptance`, `accepted`, or
`stale_acceptance`, with a machine-readable reason and safe next action. These
are delegation states, not new session lifecycle values. Do not report a
complete-ready action or emit a `complete` suggestion while its delegation gate
is unsatisfied. Do not fabricate a copyable successful receipt filled with
example external identities, successful outcomes, or a preapproved decision.
The delegated `safe_next_action` describes the delegation gate only; clients
must also honor ordinary `readiness` and `allowed_actions` before completing.

Ordinary item edits remain permitted, but a changed item revision/disposition
or recorded value makes an earlier acceptance stale. Restoring identical text
does not restore its old revision. A stale acceptance requires ordinary retry
or rework and a fresh attempt; the initial foundation has no same-attempt
receipt replacement. This is a deliberate bounded-history choice, not a claim
that immutable receipts could never support append-only reacceptance. Cosmetic
or optional-item edits receive no special exemption: Podway does not judge
whether an item change is semantically important.

### Post-acceptance behavior matrix

The following cases apply while the accepted delegated attempt remains active.
Each case must be tested with the ordinary session/attempt fences and existing
error precedence, not by bypassing them to reach a delegation error.

| Operation or observation | Required acceptance behavior |
|---|---|
| Status, next, package, receipt, or evidence read | No invalidation or mutation |
| True item no-op with identical stored value, disposition, and item revision | Preserve the receipt; reuse existing no-op semantics rather than inventing new revision rules |
| Actual item value, disposition, or revision change, including a cosmetic or optional-item change | Preserve the immutable receipt as stale; suppress completion readiness |
| Restore the previous text/value after an intervening change | Still stale because the item revision is not restored |
| Failed or rolled-back item mutation with no committed item-state change | Preserve the previous applicability; failure alone does not invalidate it |
| Block/unblock without an item-state change | Receipt unchanged; an open blocker still prevents completion |
| Session revision changes without a package or accepted-item-state change | Not an invalidation trigger by itself; it still matters to ordinary mutation fencing |
| Exact acceptance-key replay or lookup after an intervening item edit | Return the stored original outcome, not a new acceptance; a fresh observation still reports stale acceptance |
| Different-key second acceptance, or changed-input reuse of the old key | Reject under the existing duplicate-acceptance or idempotency-conflict contract; never repair a stale receipt |
| Retry, rework, or goal revision producing a fresh attempt | No current receipt on the new attempt; old receipt remains historical under existing retention rules |
| Item operation on a Direct action | Preserve existing semantics, result shape, and confirmation behavior; introduce no delegation gate |

Session revision and acceptance applicability are distinct. Acceptance itself
increments the session revision without invalidating its own receipt. Compute
accepted item state and enforce completion inside their state transactions.
An advisory warning or preflight observation cannot replace those checks when
item edits and completion contend for the worktree SQLite write lock. There is
no FIFO scheduling guarantee.

### Operator sequence and diagnostics

The supported normal sequence is: review the worker result; finish all action
items and corrections; clear ordinary blockers; verify the declared worker
disposition through the adapter; submit acceptance; then explicitly complete.
Use the completion fences from the acceptance outcome only while they remain
current. Do not add routine item-recording steps after acceptance.

Delegated guidance must explain before acceptance that later item changes,
including small corrections, require a fresh attempt. While a receipt is
current, guidance must identify the consequence of editing accepted items.
The bounded `delegation.guidance` member carries this explanation in delegated
views even when the action already has the maximum number of authored
instructions; authored instructions remain intact.
After a committed edit, report `stale_acceptance`, its item-state reason, and
explicit retry/rework recovery. Retrying or reworking is an operator/Primary
choice, not an automatic response to the edit. A successful edit must not be
reported as rejected merely because it invalidated acceptance.

Use the delegation-aware projections and existing permitted diagnostic
carriers. Do not widen a released item-mutation result, append prose to JSON,
add a new confirmation to ordinary edits, or alter Direct output to deliver the
warning. Exact historic replay may contain the original success state; clients
must read current state before using it as completion guidance. No warning,
replay, block/unblock, or new idempotency key can make stale acceptance current.

Decision, goal, retry, rework, cancellation, disposition, archive, and reset
commands retain their existing authority and confirmation rules. Declared
mandatory delegation is not bypassable through skip, but retry, rework,
cancellation, and deliberate destructive recovery must remain available under
their ordinary conditions. They never wait synchronously for Dolgorae.

## SDG-007: Runtime lifecycle and external recovery

Retry creates a new attempt, no current acceptance, and a fresh external round
budget. Rework and goal revision invalidate affected attempts through existing
rules. A late acceptance with the old attempt or old fences is rejected. Receipts
remain attached to their historical attempts while that session is retained.

Executor retention, cancellation, release, and outcome-unknown reconciliation
remain integration responsibilities. A Podway cancellation or retry is not
proof that a worker stopped or relinquished filesystem write access. The
integration must establish quiescence before dispatching conflicting new work;
it may record an ordinary blocker when it cannot. Podway must not auto-release,
auto-reissue, or guess that no task ran after a lost external response.

A retry after a cosmetic correction creates a fresh Podway attempt, not an
instruction to repeat implementation or automatically dispatch a new external
task. Conversely, the old receipt cannot be copied or relabeled as acceptance
of the new package. The Primary/integration must inspect existing work, settle
unknown execution and writer state, and choose the minimum justified new work
or explicit revalidation. Reuse of an existing artifact requires current input
and evidence validation under a qualified adapter contract; a matching digest
alone is not permission to adopt an old task/result. If that revalidation path
is unavailable, report the integration limitation rather than inventing a new
task identity, silently repeating effects, or weakening the receipt gate.
Podway performs none of these external actions and keeps local lifecycle
recovery and unrelated Direct work available.

Release-before-accept and accept-before-complete are not cross-system atomic
transactions. Required recovery cases are:

| Boundary | Required result |
|---|---|
| Release succeeded, acceptance response absent | Recover executor terminal state; look up the same Podway idempotency key before exact replay |
| Acceptance committed, response lost | Lookup/redelivery returns the original receipt; do not assign another external task |
| Acceptance committed, completion response lost | Look up the original completion key and reread the cursor before another transition |
| Primary process restarted with the same runtime identity | Recover existing tasks/results externally and reread Podway; no new Primary ownership is inferred |
| Executor outcome is unknown | Leave the delegated action incomplete; expose existing blocker/recovery controls without freezing Direct work elsewhere |
| Primary is replaced or host changes | Out of scope; no in-place ownership change or attribution rewrite |
| Worktree removed or reset/reinitialized | Preserve existing workspace-generation fencing; an old receipt cannot apply to the new identity |

Retention of a worker is possible only within the executor's allowed scope with
exact Role, configuration, and access compatibility. A Role match alone does
not permit reuse, cross-aggregate transfer, or a write escalation. Podway never
selects the worker itself.

## SDG-008: Public contract generations

Canonical assets and the manifest/catalog workflow own current identifiers. The
output envelope is `podway.output/v4`; failures use `podway.error/v2`. Result
families below are emitted, not reserved placeholders.

| Family | Current selection and compatibility |
|---|---|
| `podway.delegation-work-package/v1`, `podway.delegation-package-result/v1` | Closed immutable package and read result |
| `podway.delegation-accept-input/v1` | Closed bounded structural acceptance input |
| `podway.delegation-accept-result/v2` | Synchronous committed acceptance success |
| `podway.delegation-receipt/v1`, `podway.delegation-receipt-result/v1` | Immutable receipt value and explicit exact-attempt read |
| `podway.status-result/v5`, `podway.compact-status-result/v5` | All Direct and delegated session snapshots |
| `podway.next-result/v5`, `podway.observation-result/v5` | Current running guidance and prepared/running/terminal observation |
| `podway.prepared-next-result/v3` | Cursor-free prepared guidance |
| `podway.procedure-preview-result/v2`, `podway.procedure-graph-result/v2` | Delegated documents; non-delegated documents retain v1 |
| `podway.session-archive-show-result/v3` | All retained session snapshots, with conditional delegated receipt summaries |
| `podway.delegation-error-details/v2` | Five closed code-bound delegation failure branches |

The Procedure identifier remains `podway.procedure/v2`: delegation is an opt-in
placement extension, not a second graph model. Direct and delegated runtime
projections converge on the current generations; optional delegation fields remain
truthful to the immutable snapshot and current placement. No field is added to an
older closed result in place. Detached admission and all job wrapper generations
are retired and are not supported decoding paths for current execution.

For delegated snapshots, projection must remain truthful at both Direct and
Delegated placements. Prepared/terminal views expose no invented active worker.
Bound summary collections by count and encoded bytes, report exact totals and
truncation, and keep explicit full receipt reads within their named output budget.
Use existing status/archive selection and paging infrastructure where applicable;
do not expand current-evidence paging into arbitrary stale-history retrieval.

Provide the read-only `delegation.receipt` route as
`podway delegation receipt --session <id> --attempt <id> --json` for one exact
attempt in the selected worktree's current or retained session. It returns
`podway.delegation-receipt-result/v1` containing the full closed receipt, its
session/attempt identity, and derived `accepted`, `stale_acceptance`, or
`historical` applicability. The complete result and envelope must fit 64 KiB.
This explicit read may include the bounded summary and non-secret attribution;
default status still may not. It cannot mutate, reactivate, or import
historical evidence. A Direct attempt returns `DELEGATION_NOT_APPLICABLE`; an
existing delegated attempt without a receipt returns
`DELEGATION_RECEIPT_REQUIRED`. Missing workspace and session identities use the
existing read-error categories. An unknown attempt in the selected session
returns non-retryable exit-1 `ATTEMPT_NOT_FOUND`. No opaque cursor, arbitrary
history-value query, or provider access is introduced.

New closed public errors are `ATTEMPT_NOT_FOUND`, `DELEGATION_NOT_APPLICABLE`,
`DELEGATION_RECEIPT_REQUIRED`, `DELEGATION_RECEIPT_STALE`,
`DELEGATION_BINDING_MISMATCH`, and `DELEGATION_ALREADY_ACCEPTED`.
Map malformed inputs and stale workspace/session/attempt/idempotency identities
to their existing error categories rather than replacing those categories.
New delegation errors are non-transient: the client must reread/correct state
or deliberately retry/rework, not blindly resubmit with new keys. Error details
name safe expected/actual identities or a reason enum, never full evidence,
credentials, or arbitrary external output.

A future material field change must allocate a coherent new closed identifier
across specification, schema, catalog, tests, and manifest. Reusing a retired or
published identifier with another meaning is not permitted.

## SDG-009: Persistence and isolation

Current Store v14 retains the immutable receipt side table keyed by session and
attempt, canonical-path workspace identity, and a writer-manifest identity.
The [SQLite model](../storage/sqlite-model.md) owns DDL and generation behavior.
Receipt persistence must not rewrite unrelated snapshots, items, trace, goals,
outcomes, or inactive-session values. Keep one receipt per attempt, referential
integrity, immutable canonical bytes, and one direct transaction for acceptance
and its validated successful outcome. No worker/lease/round table or global agent
registry belongs in the Store.

Reconstruction validates receipt schema, canonical identity, parent declaration,
and accepted state. Corruption follows fail-closed integrity behavior; it must
not become a fabricated missing receipt. A current receipt on a non-delegated
attempt is invalid. Direct attempts need no receipt. Purge/reset/removal retain
confirmed cascade and marker recovery; no orphan receipt survives deletion.
Session success outcomes have their separate retained lookup lifetime, including
after archive purge, as defined by the automation contract.

Daemon-era generations 1–13 and markers are refused without conversion or deletion.
Confirmed reset-all is explicit destructive recovery, not a forward migration of
old work. Only declared direct writer predecessors may migrate under exclusive
maintenance; the initial cutover declares none. Qualify strict writer refusal,
read-only non-mutation, direct transaction/reconstruction, interruption and exact
marker retry on isolated fixtures. Never weaken manifest or integrity checks to
keep a partial implementation running.

## SDG-010: Operational continuity and acceptance

No adapter is required to build or pass deterministic tests. Use a controlled
fixture outside production state to model both Primary-host cases. The fixture
is test machinery, not a bundled executable adapter or a source of evidence
that can be used to complete real work.

Compatibility gates cover unchanged canonical bytes/digests, built-in paths,
current prepared/running/terminal/inactive snapshots, item/check recording,
guards, retry/rework/goals, lost responses, success-only lookup/replay, identity
fences, receipt retention, and reset/removal. Include simultaneous Direct work
in one isolated worktree while another delegated worktree lacks acceptance or
observes executor uncertainty. Direct work must continue normally.

Any reproducible regression in a supported path blocks adoption. Do not defer a
known regression, change a default Procedure to hide it, skip required tests, or
reset the owner's worktree. Passing mocks does not replace real direct CLI,
Store, cold-process reconstruction, contention, failure, and legacy-refusal
scenarios. Development acceptance requires the complete existing `make test`
gate and independent review.

Release selection, distribution qualification, installation, external consumer
adoption, Aquarium activation, historical service retirement, and user data
restoration retain their separate authority. The foundation does not assert
cross-version compatibility or a live external adapter. The
[operational guidance](../../implementation-tips/supervised-delegation.md) owns
the host integration sequence.
