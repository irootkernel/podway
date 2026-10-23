# JSON Contract

## Direct CLI generation

The V2DFC-007 CLI emits `podway.output/v4` for retained commands and
`podway.error/v2` for failures. Their canonical closed schemas are
[`output-v4.schema.json`](../../../assets/schemas/output-v4.schema.json) and
[`error-v2.schema.json`](../../../assets/schemas/error-v2.schema.json).
`version --json --identity` also emits output v4, with version-result v2 and
manifest v2 identity. The existing
[CLI failure-rendering boundary](cli-specification.md#output-and-exits) applies
when a valid timestamp or rebuilt error envelope cannot be supplied: bounded
stderr and exit 6 replace JSON, without partial output or process abort. This
exception does not permit emitting an invalid Error v2 envelope.

Direct and delegated Procedures share status, compact status, running next,
and the default (generation 1) observation v5, with prepared next v3; under
`--contract-generation 2`, observation serves
`podway.observation-result/v6` instead (see
[Contract generations](#contract-generations)). The success envelope has no job or
admission fields. Retired commands and flags return `COMMAND_RETIRED` before
workspace resolution or effects. The
[command catalog](../../../assets/specifications/command-catalog.yaml) owns
each retained command's result families; the
[transition matrix](../../../assets/specifications/state-transition-matrix.csv)
owns mutation transitions and their result identifiers.

`outcome.lookup` is a read-only direct Engine command. It returns
`podway.outcome-lookup-result/v1` with retained committed success or a null
outcome. Input and output budgets are independently 1,048,576 bytes. Recovery
uses the original idempotency key and canonical request, including its resolved
fences; a lookup miss does not disprove an effect outside outcome retention.
RecoveryRecipe v2 describes read-only diagnosis or explicitly authorized
marker-backed maintenance. It never grants deletion authority.

The closed `workspace-removal-result/v2` reports absence separately from identity.
Removing a present invalid Store reports `already_absent=false`,
`podway_directory_removed=true` and `workspace_uuid=null` when that identity could
not be validated. An already-absent Store requires `workspace_uuid=null`;
`already_absent=false` requires an actual directory removal.

## Previous daemon generation

The following describes the previous generation superseded by the direct CLI.
These families were retired by V2DFC-008. They describe historical contracts
and do not define the current CLI output.

The previous daemon-based commands used `podway.output/v3`; its schema and
version-only identity bridge are retired. Failures used `podway.error/v1` and
IPC requests used `podway.ipc/v1`. These identifiers
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
`prepared-next-result/v1` for Direct Procedures. A Procedure with delegation
selects status and compact status v4 and prepared next v2. Running next and
observation use v3 for Direct Procedures and v4 for delegated Procedures. The
contract adds `evidence-read-result/v1` for the `evidence.read` query. These
result families are served. `session.archive_list`
emits `session-archive-list-result/v2` with the exact total, a newest-first
window, truncation, and the oldest summary; the v1 list family remains decodable.
Item and graph-transition
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

`item-record-many-input-v1` admits the new closed record kind while `item-record-many-result-v1` stays record-kind agnostic (ADR-0033 withdrew the reserved but never-emitted check-result entry), and `observation-result/v3` admits the declaration constraints, five-field value projection, conditional `stdin_template`, and the closed `unsatisfied_reason` on a stored but unsatisfied check result. These are closed
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

These fields were incorporated into the served `next-result/v3` and
`observation-result/v3` families under the exact manifest compatibility gate
before peers could interoperate with a different contract set. They do not create
v4 families or widen a manifest-matched peer silently. The closed
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

V2SDG-002 reserves a separate delegation generation: status and compact status
v4, running next and observation v4, prepared next v2, preview and graph v2,
archive show v2, and job status/wait/lookup v5. The older closed result families and
stored terminal envelopes retain their existing fields. A `job lookup` miss
remains v4 because no stored command identifies a v5 job. The new package,
acceptance, receipt, projection, and error-detail schemas are registered in the
contract manifest. Source implementation through V2SDG-007 makes
`delegation.package`, `delegation.accept`, and `delegation.receipt` executable
and admits explicitly supplied custom Procedures with delegated actions.
This source capability is not an installed stable-release claim. Native lifecycle
qualification and its limits are described by the
[conformance inventory](../quality/testing-and-conformance.md). The
authoring `procedure preview` and `procedure graph` commands select their v2
results for parseable delegated documents and validated delegated graphs,
respectively. The
[adapter mapping review](../../implementation-tips/delegation-adapter-contract-review.md)
defines the generic digest meanings and records both real Primary paths as
unavailable pending separately qualified mappings.
In the reserved delegated views, a `current_receipt` or stale receipt summary
must name the enclosing current attempt. Views without a current attempt report
`inactive`; retained receipt summaries remain historical data. The decoder
checks these correlations in addition to each closed JSON Schema.

The error envelope contains a catalogued `code`, summary, retryability, exit code,
request correlation, command, and closed code-specific details. Human messages are
not stable API. `LEGACY_PROCEDURE_STATE_UNSUPPORTED` means the opened runtime contains
Procedure v1 task state. Podway performs no automatic conversion or deletion; after
backup, `podway reset --all` is the supported recovery.

The runtime-reset families are `runtime-reset-plan-result/v1`,
`runtime-reset-result/v1`, and `runtime-reset-control-result/v1`, each prefixed
with `podway.`. Plan, confirmed single-mode and all-mode apply, and process-bound
control are executable. Their output branches omit workspace, job and session
projections.
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

## Direct generation compatibility

`V2DFC-003` reserved `podway.output/v4`, `podway.error/v2`, and RecoveryRecipe
v2. The direct Engine adopts the generation in `V2DFC-006`, and the CLI uses it
in `V2DFC-007`. The current direct validators enforce this generation.
`V2DFC-008` removed daemon-era decoding and superseded assets; those are no longer compatibility APIs.

The new result and detail generations remove admission, job, queue, endpoint,
registry, and runtime-mode metadata. Direct and delegated status, compact status,
next, and observation converge on v5; observation v5 is the default
generation-1 family (see [Contract generations](#contract-generations) for
observation v6), and prepared next converges on v3. Session
start becomes v4. Families without a daemon-dependent shape keep their current
identifier. Author-authored item content remains valid even when it contains
words or object keys such as `job`; removed metadata is rejected at protocol
positions, not by searching application content. Existing unnamed generic
results and diagnostic details retain their JSON-object compatibility boundary,
subject to the preserved byte and nesting-depth bounds.

`podway.outcome-lookup-result/v1` is emitted by `outcome.lookup`. A successful
lookup carries a committed mutation response; not-found carries a null outcome.
The executable route uses the `direct` execution class and `podway-engine` owner.
Input and output budgets
remain 1,048,576 bytes independently of IPC framing.

Recovery recipes are constrained by the owning error. `MUTATION_OUTCOME_UNKNOWN`
uses read-only `outcome.lookup` with the original idempotency key; its detail,
reconciliation field, and recovery argv must agree on that key. Lock contention
permits only read-only state refresh or workspace diagnosis. Legacy state requires
explicit reset-all authorization. Reset markers name their resuming command.
Invalid-Store and writer-contract failures can diagnose the writer through
`version --identity` without opening the rejected Store. The reason separates
restoring access or using the original writer from explicitly confirmed
unfenced discard; neither the recipe nor its read-only command grants destructive
authorization. The closed schemas retain their earlier doctor form for compatibility.
Maintenance reports the observed marker kind, including `intent_marker` for
init, predecessor and reset/removal intent-only windows. Initialization resumes
with its original key and repair policy. Removal without JSON authorization
uses a human-terminal recipe that preserves the original request; unfenced
removal requires exact-path confirmation. Foreign writer/root markers remain
`WORKSPACE_MAINTENANCE` with `diagnose_writer` / `version --identity`, which
identifies the current writer and directs recovery to the original writer and
worktree. Predecessor intents use that diagnosis while the compiled predecessor
registry is empty. Maintenance-lock contention uses read-only `session.observe`
after the owner finishes when a Store is present. Store-less final-removal or residual
contention uses `version --identity` without opening a Store and directs an exact
retry after the owner finishes. A repopulated removal parent retains
`WORKSPACE_STATE_UNREADABLE` with writer diagnosis and explicit preservation of
the current contents, rather than invalid-Store discard guidance.
No ordinary read resumes a marker.
Maintenance recipes retain the original key and the marker's original request
parameters. They include `--if-workspace-uuid` only when that original request
asserted a UUID, using that UUID even after reset creates a different generation.
Removal recipes preserve the original `--force` authority; a human unfenced
recipe omits `--json` and `--yes` so the CLI requires exact-path confirmation.
Reset-all uses `--all --yes`; `--force` is not required by the direct reset
contract. A legacy reset recipe includes the explicit
`<new-idempotency-key>` placeholder, which the caller must replace with its own
fresh key before execution. Producing the recipe never generates a key or
authorizes deletion.
Retained revision, attempt, session, Procedure digest, and stale-goal/evidence
conflicts keep read-only `session.observe` recovery. Workspace UUID mismatch
keeps read-only `workspace.doctor` recovery. Invalid-Store and writer-contract
failures emit `diagnose_writer` / `version --identity`; their closed detail
schemas retain the earlier doctor form for compatibility and exclude legacy
reset, maintenance retry,
and outcome lookup recipes; error/v2 applies the same owning detail constraints.
Newer Store generations use `workspace-schema-unsupported-details/v1` instead:
`store_schema_version` is the actual newer generation,
`supported_store_schema_version` is this binary's generation, and `guidance`
requests a Podway version that supports that Store. The closed object admits no
recovery recipe; older binaries refuse reset and removal as well as ordinary
Store commands. Human output renders the same compatibility guidance.

`podway.version-result/v2` and `podway.contract-manifest/v2` omit
`supported_ipc_ids`. The generated manifest is
`contracts/contract-manifest-v2.json`, and `version --json --identity` emits
version-result v2 inside output v4. The single-binary release verifier validates
the complete version envelope and manifest through the canonical schema registry.
Store v15 DDL is enrolled in the current
manifest with asset kind `store_schema` so its bytes affect writer identity.

## Contract generations

[ADR-0039](../../architecture-decision-records/0039-opt-in-contract-generations.md)
defines a contract generation as a closed, named selection of result and
error-detail families. All generations share the Output v4 and Error v2
envelopes, the routes, error codes, exit classes, retryability, messages, the
Store layout and the writer identity. The canonical declaration
[`assets/specifications/contract-generations.json`](../../../assets/specifications/contract-generations.json)
(`podway.contract-generations/v1`, enrolled in the manifest) declares the
`default` generation and, for each generation, its selection for every variable
point and its status from the closed set `reserved`, `supported`, `deprecated`
and `retired`. Generation 1 is the direct contract shipped since `v0.2.13`; it
is `deprecated` from the first release that ships generation 2 and remains the
default until removal
([ADR-0039](../../architecture-decision-records/0039-opt-in-contract-generations.md)
decision 7). No runtime warning is emitted. Generation 2 is `supported`: under
`--contract-generation 2`, `session.observe` serves
`podway.observation-result/v6`, `CRITERION_CITATION_INVALID` serves
`podway.criterion-citation-invalid-details/v1`, and
`WORKSPACE_CONTRACT_MISMATCH` serves
`podway.workspace-contract-mismatch-details/v2`. The error catalog keeps
naming the default generation's families in `details_schema`, so its bytes are
unchanged.

A global `--contract-generation <1|2>` option selects the generation for one
invocation. The default is 1; it changes only with the removal decision
(ADR-0039 decision 8). The value is
validated after parsing and before local execution or reading stdin; every
other value, including an empty value, fails with exit-2 `REQUEST_INVALID`
attributed to the parsed route, and retired syntax keeps `COMMAND_RETIRED`
precedence. Every command accepts the option, including the local families
where it has no effect and a top-level `-h`/`--help` that follows it, and human
mode honors the same selection. Selection is presentation only: it is never
part of canonical request identity, idempotency, stored outcomes, exact
replay, outcome lookup, Store state or writer identity. No environment
variable, workspace configuration or stored preference selects a generation,
and templates and recovery recipes never embed the option.

`tools/verify_contracts.py` checks the declaration against the Output v4 and
Error v2 selections in generation order, against the command catalog's
`session.observe` result schemas, and against the error catalog's
`details_schema` for the default generation. The
`podway.observation-result/v6` family specifies executable lifecycle templates:
running templates keep the v5 shapes, while the prepared and terminal templates
carry the session fences and the idempotency-key placeholder and use the closed
placeholder vocabulary of ADR-0039. Consumers re-pin on every release; the
manifest, the Output v4 and Error v2 schema bytes and the Dolgorae adapter
schema pins change with this reservation, so Dolgorae must re-pin when a release
ships it.

## Evidence paging

`V2SCL-002` reserved the route, the `podway.evidence-read-result/v1` family, and the error codes, and `V2SCL-004` serves them.

`evidence.read` is a `query` that reads one item selected by a currently resolved, declared evidence reference of the current consumer attempt. It cannot browse arbitrary attempts or stale history, it creates no durable job or session revision, and it is fenced by workspace UUID, session ID, and consumer attempt ID. Its closed result is `podway.evidence-read-result/v1` and carries consumer, source node, source attempt, and item identities; item type, item revision, complete-value digest, and total logical size; a typed page; `truncated`; a nullable `next_page_token`; and the page-token version with the logical page offset.

Text sizes and offsets count Unicode scalar values, list sizes and offsets count entries, and scalar item types use a single-page unit. Text pages end only at scalar boundaries and list pages contain complete entries, so no page splits an entry. Encoded page data is at most 256 KiB and the complete encoded response, including one token and its envelope, is at most 320 KiB. The Engine measures its result member against that bound and refuses to serve a response over it; the 64 KiB between the two bounds covers the envelope the output envelope adds, which a conformance test asserts on a maximal page.

The opaque page token is at most 256 base64url characters. Its canonical payload is fixed-order binary containing a one-byte version, 16-byte session UUID, 16-byte consumer attempt UUID, 16-byte source attempt UUID, length-prefixed item ID, 32-byte SHA-256 value digest, and unsigned 64-bit logical offset, encoded with unpadded base64url. It is not an authentication or authorization token, and the Engine validates every field against current authoritative state on each read.

Failure ordering is fixed. Malformed, oversized, cross-session, and wrong-binding tokens fail as `REQUEST_INVALID` before any state comparison. A no-token read of an unresolved or skipped reference fails non-retryably with `EVIDENCE_NOT_AVAILABLE`, while a genuinely stale reference retains `EVIDENCE_REFERENCE_STALE`. A previously valid token whose bound snapshot changed fails retryably with `EVIDENCE_PAGE_TOKEN_STALE` rather than silently returning another snapshot. An offset at or past the logical end fails non-retryably with `EVIDENCE_PAGE_TOKEN_EXHAUSTED`; the Engine never issues such a token. No failure changes state.

Storage may decode a bounded complete value internally, but no public response may exceed the page or output budget.

## External check result recording

[ADR-0025](../../architecture-decision-records/0025-structurally-bound-external-check-results.md)
adds `check_result` as a closed `item.record_many` record variant; it adds no
command route or error code. The request carries the complete bounded value and
the existing workspace, session, attempt, item-revision, and idempotency fences.
One request remains atomic and input-bounded. A node containing many maximal
values may require several requests, and those requests are not atomic as a
group.

An unsatisfied required check result exposes `item.record_many` in
`allowed_actions` and a suggestion whose argv is `podway record --stdin`.
`session.observe` may attach one closed `stdin_template`, at most 16 KiB, only to
the `item.record_many` mutation template. It targets the first unsatisfied check
result in declaration order, prioritizing required items, and supplies observed
identity and revision fields plus placeholders for caller-supplied result data
and the idempotency key. Accepted outcomes remain declaration constraints and are
not inserted into the mutation body. No other template may carry
`stdin_template`.

Next, observation, preview, and compact-item projections expose only
`operation_id`, `outcome`, `operation_digest`, `input_basis.digest`, and
`output_digest`. Those fields are pattern-constrained ASCII and are never
character-truncated. Descriptor, executor, and summary text is omitted from the
projection. `evidence.read` returns the complete check-result value in one
terminal page because its 32 KiB encoded ceiling is below the 256 KiB page-data
limit.

## Response budgets

`session.next` and `session.observe` each have a closed byte composition proved against the 1,048,576-byte output bound. Authoring validation charges every allocation separately and names the one an author exceeded, because the remedies differ. Every windowed collection reports its exact total and whether it was truncated; silent truncation is forbidden.

| `session.next` charge | Independent ceiling or aggregate |
|---|---:|
| Procedure and runtime static content | 256 KiB |
| Complete decision records | 264 KiB |
| Reference and item metadata, at most 128 items | 216 KiB |
| Preview data, at most 32 slots | 176 KiB |
| Up to 32 page tokens | 64 KiB |
| Complete graph counters (included in aggregate; not an extra reservation) | variable, at most 172,033 bytes |
| **Actual five charges plus counters** | **976 KiB aggregate** |
| Serialization and envelope reserve | 48 KiB |
| **Output** | **1 MiB** |

The five placement-local ceilings are independent limits, not simultaneously
available reservations. Authoring adds their actual worst-case charges to the
exact complete counter-array charge and admits the placement only when that sum
fits the 976 KiB aggregate payload. The preview and page-token ceilings hold the
widest selection a selector can request — thirty-two slots and one token per slot
— however many items the reference names. Authored text binds the static ceiling.
One complete goal-assessment record nearly fills its ceiling, so a placement may
read back from at most one goal-assessment decision source. A decision record
without an assessment is an order of magnitude smaller, so several of those fit.
Metadata binds through selector-less references: an omitted selector requests
every item of its source, so a reference to a 128-item definition charges far more
than the sixteen items an explicit selector may name, and two such references
exceed the metadata ceiling.

| `session.observe` component | Maximum |
|---|---:|
| Complete compact status and bounded `next-result/v5` guidance | 768 KiB shared |
| Active item window | 128 KiB |
| Mutation template window | 64 KiB |
| Serialization and outer-envelope safety | 64 KiB |
| **Total** | **1 MiB** |

A guarded decision reserves at most 96 KiB inside the existing 256 KiB
`session.next` static allocation: eight options, four predicates per option, and
bounded expected and actual values. An unguarded decision omits
`option_guard_statuses` and incurs no guard-status charge. A conditional active
item charges about 2.4 KiB for one predicate or 9.5 KiB for four predicates
inside the existing 128 KiB observation active-item window. The existing exact
totals and truncation fields remain authoritative when fewer active items fit.

Running guidance carries `next-result/v5` without its previews or page tokens. Prepared guidance carries
the cursor-free `prepared-next-result/v3`. Status and guidance
both carry the complete graph-wide counter set, so they share the 768 KiB
that their former 32 KiB and 736 KiB allocations occupied. This lets long graphs
spend more on counters while content-rich placements spend more on guidance.
They remain complete structured members rather than windows: there is no honest
way to return half of either one, so their combined encoded size is measured and
fails closed if it exceeds the shared allocation. The active-item and template
windows are cut and publish what they cut.

Authoring admission proves the same shared observation bound before a session can
start. For every placement it charges procedure-static guidance, decision records,
read-back metadata, two complete counter arrays, and a 32 KiB reserve for compact
status fields other than counters. A Procedure containing a delegated placement
also reserves 8 KiB for the delegated projection in next guidance and 8 KiB for
its observation copy. `OBSERVATION_STATIC_MEMBERS_BUDGET_EXCEEDED`
identifies a Procedure that is individually valid for `session.next` but cannot
produce a complete observation.

An omitted evidence item selector means metadata-only. Explicit selectors receive previews in declaration order for at most 32 items, and later admitted items remain metadata-only. At most 128 item metadata records may appear across all references in one response. Metadata costs the same for every item kind, so the byte allocation does not imply that ceiling: a wide selection of small items can stay inside the allocation and still exceed it. Graph vetting therefore counts records as well as bytes and rejects a larger declared selection, recommending the smallest useful selectors. Metadata remains complete for every item in an admitted `session.next` projection, so that response never marks its evidence metadata window truncated.

Observation composes its own budget rather than embedding an unbounded duplicate of `session.next`. Its guidance emits evidence metadata only, with no previews or page tokens, and its evidence metadata window may be truncated. The active-item and mutation-template members are the two byte-budgeted windows. Compact-status items are a bounded collection inside the measured shared status-and-guidance member, not a window of their own: a combined member that outgrows its allocation fails closed rather than cutting status or guidance. The active-item window admits in declaration order until its allocation is spent. The template window first reserves the first template of every command, so no action named by `allowed_actions` loses its template, and then fills the remainder, admitting a later template that fits after skipping one that does not. Active-item choice constraints expose at most eight choices plus the exact choice count and a truncation flag, while the complete declaration remains available from the Procedure inspection routes. Template argv elements are limited to 4,096 scalars.

Procedure preview is likewise all-or-nothing. Its normalized graph and Mermaid
projection are required details, not truncatable windows. If Mermaid text exceeds
1,048,576 characters, preview reports `GRAPH_PROJECTION_BUDGET_EXCEEDED`, marks
the vet check false, withholds all preview details, and remains inadmissible. Any
other normalization or projection failure keeps its own authoring diagnostic and
also withholds the required details.

Every derived collection was audited against the declaration it projects. A collection whose ceiling equals its declaration's own ceiling is complete and carries no truncation flag: decision options, evidence guidance, instructions, allowed actions, graph counters, and manual rework targets are all of this kind. A collection that can be narrower than what a declaration may produce is a window, and a window always publishes its exact total and whether it was cut. Raising items per definition to 128 left three collections in the standard status tier able to exceed their own published ceilings: `missing_required_item_ids`, `item_values`, and the `items_total` that describes it. `status-result/v5` retains the released bounds, so each was corrected by raising its numeric ceiling to what a definition can declare rather than by windowing it. Windowing would have required a new member, and a released family never grows one; a bounded numeric update changes no field set and no unknown-field closure, which is the only in-place change the contract permits.

`missing_required_item_count` remains exact while `missing_required_items` returns at most 64 entries plus a truncation flag. Suggestions remain capped at 128, expose exact total and truncation fields, and order currently actionable session-level suggestions before item suggestions in item declaration order.
