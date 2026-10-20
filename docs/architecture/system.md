# System Architecture

## Context

Podway runs one command in the invoking process. The single `podway` binary
composes the CLI, Direct Engine and worktree-local Store. It exits after returning
one result; it has no resident process, socket, service or account state.
[ADR-0038](../architecture-decision-records/0038-remove-the-daemon.md) owns this
model and supersedes the daemon writer model.

```text
Human, script, or AI agent
          |
          v
       podway CLI
          |
          v
       Direct Engine
        /    |     \
  Config   Core   Native Git and filesystem boundaries
          |
          v
     Direct SQLite Store
          |
          v
  owning Git worktree/.podway/runtime/
```

Independent invocations share only their target worktree's filesystem locks and
SQLite state. SQLite serializes mutations within a worktree. There is no durable
admission, FIFO scheduler, detached job or global worktree catalog.

## Component responsibilities

### `podway` CLI

The CLI MUST parse command grammar, select the target worktree, decode bounded
inputs, construct typed Direct Engine requests and render validated public JSON
or human text. It supplies the command deadline, caller-owned idempotency key and
required workspace, session, attempt and item fences. Automatic fence reads remain
separate observations; transaction-time validation decides whether they are current.

The CLI MUST NOT access Store internals, mutate Git or execute Procedure work.
Static help, version, completion, preset and Procedure authoring commands may run
without workspace state. Stateful commands resolve a valid non-bare worktree.
The [CLI specification](../specs/interfaces/cli-specification.md) owns the exact
command list, input rules and retired-command guidance.

Implementation responsibilities live in private `command` modules: `grammar`
owns Clap and classification, `authoring` owns local Procedure/preset handlers,
`procedure_files` owns descriptor-relative reads and atomic replacement, `start`
owns interviews and independent fresh-source previews, and `render` owns output.
The entry module retains dispatch, bounded stdin, flag/shape validation, shared
failures/results, worktree targeting and local clock generation. File descriptors,
leaf names and modes stay private to file handling; authoring borrows bytes.

`command/preflight.rs` is the sole fence-facts replacement boundary for READS.
It owns missing-fact gating, fill-only-missing behavior and prepared-state
rejection, and obtains its facts from the Engine-owned typed preflight-facts
operation: the CLI passes one fresh request identity with the original
selector, fences, observation and deadline, and receives typed owned facts or
the same rendered failure the status read produced. No public status envelope
is fetched, built or parsed on this path; the retained status parser serves
only the later interactive-start consumer. It borrows the original Direct
`Invocation`, retaining its deadline and requested-command error attribution;
`Command::needs_preflight` stays in grammar because flag validation also uses
it. Shared error attribution remains with entry coordination.

Keep four observations distinct: fence preflight; dynamic completion's separate
best-effort 500 ms query; offline fresh-start preview versus stateful replacement
or reset preview; and the later interactive status read only after plain start
returns `SESSION_START_DECISION_REQUIRED`. Plain start still executes first,
retaining exact replay and the same key/deadline on an interview retry. Missing
workspace UUID alone never triggers fence lookup; stdin-owned record/delegation
identity remains independent. READS replaces only the first observation.

### Pure domain engine

`podway-core` owns immutable domain values, invariants, item satisfaction,
conditions, graph movement and state transitions. It receives validated state,
commands, IDs and times as explicit values and returns domain changes or a
structured domain error. It has no SQLite, native Git, filesystem, process or
service dependency. Recorded conditions evaluate bounded current values; they
perform no external lookup or execution.

### Direct Engine

`podway-engine` composes configuration, core, protocol, Store, Git and presets.
It owns read projections, Procedure and artifact preparation, canonical request
identity, typed command execution, public failure rendering and filesystem
maintenance. `direct_api.rs` is the shared in-process boundary; the CLI does not
construct an IPC request as an intermediate representation.

Expensive preparation occurs outside the mutation transaction. Native worktree,
artifact, Procedure and prepared-state identities are revalidated before commit.
A validated success envelope is persisted with the domain effect. Engine
maintenance resumes exact persisted intent through the owning Store lease.

### Procedure and configuration loader

`podway-config` parses bounded YAML/JSON, rejects invalid structure and semantics,
resolves safe local Procedure files and computes canonical immutable snapshots.
`podway-presets` embeds the canonical YAML under `assets/presets/`.
A running session uses its persisted snapshot. Loading a Procedure never loads
remote includes, executable plugins or configured commands.

### Store

`podway-store` owns current Store initialization, coherent read snapshots,
private transactions, worktree binding, writer-contract identity, relational
Procedure state, atomic success outcomes and bounded journal retention.
SQLite relational state is authoritative; there is no event-sourced rebuild.

Ordinary opens hold a native shared maintenance lease. A cold read without the
required WAL/SHM state instead holds exclusive exclusion and uses an immutable
main file or heap WAL snapshot so reading creates no sidecars. Doctor and
maintenance use exclusive exclusion. The Store has a bounded process-local
connection gate to keep a read-only connection from inheriting another connection's
writable SHM mapping; this creates no persistent or per-user state.

### Supervised delegation

An explicitly authored action placement may require delegation under
[ADR-0035](../architecture-decision-records/0035-host-neutral-supervised-delegation.md).
It retains the ordinary single cursor and active attempt. Config constructs an
immutable work package; Engine exposes bounded package and receipt reads and
accepts a structurally valid receipt through the direct transaction boundary.
The Store atomically persists the immutable receipt, accepted item-state digest
and successful mutation outcome. Completion rechecks ordinary gates and receipt
currency in its own state transaction.

The external integration owns dispatch, feedback, executor identity, acceptance,
worker settlement and uncertain external effects. A host label or matching digest
grants no approval or permission. Direct Procedures require no adapter or receipt.
See the [interface contract](../specs/interfaces/supervised-delegation.md) and
[implementation guidance](../implementation-tips/supervised-delegation.md).

### Git worktree resolver

`podway-git` discovers main and linked worktrees without invoking `git`, rejects
bare or uncertain layouts, resolves lossless canonical Git paths and supplies
native replacement-race evidence. Engine validates that evidence against the
Store binding. The resolver exposes no Git mutation API.
The [filesystem model](git-worktree-and-filesystem.md) owns move, copy, path and
maintenance boundaries.

## Data ownership

| Data | Owner | Location |
|---|---|---|
| Tracked workspace config and custom Procedures | User/project | `.podway/config.yaml`, `.podway/procedures/` |
| Current and archived sessions, snapshots and receipts | Worktree | `.podway/runtime/state.sqlite3` |
| Successful mutation outcomes and operational journal | Worktree | same database |
| Maintenance locks and recoverable operation markers | Worktree | `.podway/runtime/` |
| Artifact bytes and external executor effects | External owner | original path or external system |

Podway neither reads nor creates `~/.podway`. Legacy installed services require
separate retirement by their original owner; the direct binary has no service
manager or account cleanup path.

## Trust boundaries

The CLI, binary, local worktree and caller assertions operate within the same OS
user's authority. Descriptor-safe paths, filesystem permissions, worktree binding,
writer identity and fences prevent accidental misrouting and unsupported writes.
They do not authenticate assertions or defend against malicious same-user code.
See [Security and Trust](../specs/operations/security-and-trust.md).

## Mutation path

```text
CLI parses bounded input, selects worktree and supplies fences/key/deadline
  -> Engine classifies Store, resolves identity and prepares outside transaction
  -> Store qualifies writer/schema/binding without a writable foreign open
  -> maintenance lease, process connection gate and SQLite BEGIN IMMEDIATE with one wait deadline
  -> same-key lookup returns original success or rejects a changed request
  -> required identities and prepared state are revalidated
  -> core computes transition; Engine constructs validated output/v4
  -> Store commits domain state, success outcome, sequence, journal and pruning
  -> CLI returns the committed response
```

Each acquisition is attempted once even after the wait budget is exhausted;
only retries and sleeps are deadline-bound. Preflight, preparation and human
confirmation consume the original budget. Successful acquisition allows the
operation to finish; reads distinguish connection-gate and SQLite read admission
from SQLite writer contention.

A rejection records no outcome. Only COMMIT I/O ambiguity maps to
`MUTATION_OUTCOME_UNKNOWN`; response loss is reconciled with `outcome lookup` and
an exact retry. Reset-all and removal are marker-governed maintenance protocols,
not ordinary single-transaction mutations.

### Typed mutation callback failures

Store's `mutate_resolving_identity_typed` is the single ordinary mutation
transaction implementation. `mutate_typed` supplies fixed identity; existing
Store-only methods delegate to that same runner. Identity and execution
callbacks return their caller's error type. `DirectMutationFailureV1` keeps
callback errors separate from Store qualification, transaction and persistence
failures; Engine maps either branch without placeholder Store errors or outer
error slots. Bootstrap keeps its distinct private-publication protocol.

The nine typed Engine callback sites are:

| Module | Callbacks |
| --- | --- |
| `direct_execution` | `execute_cursor_v1`, `begin_session_v1` |
| `direct_items` | `record_many_v1`, `execute_item_v1` |
| `direct_delegation` | `accept_delegation_v1` |
| `direct_procedure` | `execute_procedure_v1` |
| `direct_sessions` | `execute_session_v1` |
| `direct_start` | `start_session_v1` |
| `direct_workspace` | `existing_mutation` |

Outcome lookup still precedes identity resolution, which precedes resolved-key
and digest checks. Exact replay runs identity resolution but bypasses execution,
clock and IDs. Start recovers the original pinned digest on replay; failed fresh
source preparation is consumed only when no retained outcome exists. Callback
failure drops the transaction without a success outcome. Only the existing
COMMIT I/O classification yields an unknown mutation outcome.

Equivalent session/attempt fences and graph success construction share
`direct_mutation` helpers across cursor, begin, item, batch, procedure and
acceptance commands. Single-item commands retain item-local revision checks;
prepared, goal, reset/archive/purge, source-backed start and workspace bootstrap
keep their distinct validation and envelope ordering.

### Pure graph state and persisted admission

Core owns [graph/session transitions](../../crates/podway-core/src/graph_v2.rs),
[workflow memory](../../crates/podway-core/src/workflow_v2.rs), and
[goal state](../../crates/podway-core/src/goal_state_v2.rs). Time and identifiers
remain explicit inputs. Store reexports these same values while retaining native
row decoding, independent persisted validation, successor admission and
changed-row writes; it has no second domain transition implementation.

`ProcedureRuntimeV2` owns exact canonical bytes, digest, bounded graph metadata
and the retained workflow-declaration result. `ProcedureSnapshotV2` adds the
stored snapshot ID, source label and creation time. Config's validated-source
adapter (`ValidatedProcedureV2::runtime_v2`) and Store's independently
shape-admitted snapshot adapter construct the same Core runtime value. The
Engine retains the validated source until the existing late snapshot boundary;
bounded canonical preparation, supplied IDs/time and source diagnostics keep
their ordering.

Both adapters bind identity through `CanonicalProcedureDocumentV2`. Runtime
construction reuses its decoded document for memory declarations and associates
placements by their admitted index. Node accessors share action topology, skip
policy and delegation presence without demanding the whole workflow model.
Malformed declarations remain retained failures, rather than earlier aggregate
rejections; delegation presence is not declaration validation. Goal
interpretation stays independent and on demand. Historical runtime defaults
never regenerate stored canonical bytes or rotate their digest.

Store retains canonical/digest/schema admission on loading, relational row
verification, and a separate shape check before inserting a newly supplied Core
aggregate. That INSERT check still parses the supplied canonical bytes; a pure
constructor cannot bypass persisted admission. It does not admit a retained
derived-model error earlier or alter successor writes. No schema assets or SQL
enter Core.

Snapshot construction can succeed with a retained declaration error. Convenience
`prepared` construction demands the model, while explicit empty-workflow
Prepared construction can defer it until `begin`; a stale revision still wins
there. Cold Store reconstruction independently rejects malformed declarations
as unreadable persisted state. `ProcedureStateErrorV2` carries the pure
structural reason; Store/Engine conversions preserve the existing public error
categories, details and exit status.

In particular, `GraphMutationErrorV2::InvalidState` remains `INTERNAL_ERROR`
(exit 6, nonretryable, internal diagnostic details), while
`DomainError::InvalidState` remains `REQUEST_INVALID` (exit 2). Corrupt persisted
canonical/schema/digest or relational state remains Store-owned and maps to
`WORKSPACE_STATE_UNREADABLE` (exit 5, nonretryable, workspace recovery details).
Source admission retains Config diagnostics and `PROCEDURE_INVALID`.

### Unpublished batch draft

`WorkflowMemoryStateV2::mutate_active_items_v2` sorts the batch and applies every
item through one pure `UnpublishedAttemptDraftV2`. The draft is an in-memory
value: it owns no connection, transaction, or Store error, and its rules remain
separable from storage and error mapping. Single-item calls and nested item
calls use that same primitive. The outer `GraphSessionStateV2` wrapper still
builds one graph successor after the memory draft succeeds.

Input bounds, item-ID ordering, duplicate detection, and fences stay first.
Each sorted operation then keeps its item revision, timestamp, value, and
attempt-content checks. An earlier sorted item that crosses the attempt-content
ceiling is rejected even when a later item would shrink the final total. One
memory value is published only after every item succeeds, through the existing
changed-row persistence and atomic success outcome. Failure discards the draft
and records no outcome.

A changed item advances its own revision. A changed batch advances the session
and workspace revision once. A successful all-item no-op leaves item, session,
and domain revisions unchanged, still commits its success outcome, and advances
the committed workspace sequence. Recorded payloads stay immutable shared
values, and the snapshot keeps its parsed memory model.

## Read path

A read classifies and qualifies the Store, takes one committed read snapshot and
projects bounded state without changing bindings, outcomes, journal or sidecars.
It reports committed sequence/revisions rather than queue activity. A read is not
a reservation: a later writer may commit before the caller's next request.
Doctor performs explicit integrity and graph/outcome checks under exclusion;
ordinary opens do not run those expensive scans.

### Request-scoped read composition

`ReadScope` borrows the coherent view the caller already owns, either
`DirectWorkspaceViewV1` or an existing `GraphWorkspaceViewV2`. It does not clone
`GraphSessionStateV2`. Public wrappers remain. Nested consumers of the same
request reuse the scope.

Declaration parsing and validation stay lazy. The first use that needs
declarations parses and validates the Config-owned `ValidatedProcedureV2` once,
at the admission point that use already had. A branch that returns before
declarations are required still returns before validation.

The scope covers seven Direct entries: `project_direct_status_v1`;
`project_direct_next_v1` with running and prepared next;
`project_direct_observation_v1` and its nested next;
`project_direct_archive_show_v1` with nested status and archive-show;
`project_direct_evidence_v1` on branches that need Procedure declarations;
`project_direct_receipt_v1`; and `project_direct_delegation_package_v1` for
work-package construction and package-result projection. Archive-show and
prepared or running nested projections share that one scope.

Reuse follows the current request's snapshot, session, Procedure snapshot
identifier, and stored digest. Core keeps the immutable `SnapshotMemoryModelV2`
distinct from Config's authoring representation.
The read scope does not attach the Config-owned Procedure to
`ProcedureSnapshotV2` or the session cache, and `from_snapshot` returns the
retained memory model without reparsing it.

Schema selection remains a separate owner from the borrow. Canonical projection
and output validation, lazy schema compilation, and response admission stay in
place. Counters, ordering, exact totals, truncation flags, page-token identity,
evidence freshness, delegation readiness, and byte budgets stay in place.
Terminal observation guidance remains null.

Projection computation inside one admitted projection is single-owner. The
current projection derives the selected evidence readback once and stores it;
next, readback and delegation work-package consumers reuse that selection for
the same admitted snapshot, preserving each consumer's reachable error
precedence and consistency guards; mappings that existed only in the removed
recomputation are gone. Observation builds guidance without materializing
evidence preview text it would immediately remove, while still evaluating the
same explicit-selector, remaining-preview-slot, preview-existence and
truncation gates for exactly the original items at their original logical
stage. Preview ownership is split: the plan decides preview presence, size and
truncation, while the readback path owns the explicit-selector and
remaining-slot gate and consumes the preview slot; the payload stage then
renders only admitted text, so discarded display payloads are never built. The
fallible truncation-time checks are retained exactly: when a plan truncates,
resolved source-snapshot presence, the active consumer and page-token
constructibility still run under their original gates and consumer-attempt
binding; only the payload/map construction and the final infallible encode
are bound to publication.

Serialized length accounting is exact and incremental. A Protocol-owned
counting helper returns checked serde JSON lengths without retaining output
buffers; observation lists and history windows admit each candidate once from
its own measured length, folding separators, wrapper fields and trial trace
bounds without reserializing the admitted prefix. A full history window only
asks whether another filtered source entry exists; that lookahead entry is
never rendered, so a failing render there remains a truncation fact rather
than a surfaced error. The indivisible-value evidence charge is a separate
conservative worst-case policy, distinct from the exact serde length owner.

Display construction is bounded by admission. Status values format each value
only at its display-consideration point and stop at the first rejected
candidate while still reporting exact logical totals and truncation. Text
evidence pages end on Unicode scalar boundaries, allocate only the returned
page, and reuse the caller's logical size of the same admitted value instead
of recounting it. The stale-attempt summary counts every slot with a recorded
value and formats only the first three of those valued slots; empty slots
contribute neither an item nor a count.

Store admission, the per-item source-slot, revision and digest checks, and the
Standard preflight projection/schema/envelope-equivalent failures remain
independently owned trust boundaries; projection reuse merges none of them,
and archive-list keeps its summary-only exception for archived rows.

### Procedure identity boundaries

Sharing `ValidatedProcedureV2` shares computation. Revalidation can materialize
defaults, so the wrapper's canonical bytes and digest can differ from the stored
snapshot. The boundaries below keep their own sources.

| Boundary | Preserved source |
|---|---|
| Stored ordinary read identity | Status, next, observation, and archive use `state.snapshot().digest()` and the original stored canonical bytes. |
| Validated-wrapper package identity | Delegation work-package construction uses `ValidatedProcedureV2::digest()` as `procedure_digest`. |
| Package result mismatch rejection | Package-result admission compares that package `procedure_digest` with `state.snapshot().digest()` and rejects a mismatch. Workspace, session, attempt, placement, and package-digest checks remain. A constructed package is not yet an admitted public response. |
| Execution digest and bytes mismatch refusal | `rehydrate_procedure_v2_execution_v1` rejects a wrapper digest or canonical bytes that differ from the stored snapshot. This execution path sits outside the seven Direct read entries. |

Historical snapshots that ordinary reads already accept stay acceptable there.
Package-result admission and execution rehydration keep their mismatch refusals.

## Dependency direction

The eight-crate adjacency is owned by
[`contracts/cargo-adjacency.json`](../../contracts/cargo-adjacency.json).
Core has no infrastructure dependency; config, protocol, Store and Git depend on
core; presets depend on core/config; Engine composes those six crates; CLI invokes
Engine and uses core/config/protocol/presets for presentation and authoring.
CLI has no production Store or Git edge. Cycles and reverse edges are prohibited.

## Performance and scale envelope

[ADR-0024](../architecture-decision-records/0024-bounded-evidence-scale-and-paged-read-back.md)
and [ADR-0037](../architecture-decision-records/0037-bound-procedure-graphs-at-1024-nodes.md)
preserve these limits. Text lengths count Unicode scalar values:

- one current session and one active attempt per worktree;
- at most 1,024 node definitions and graph placements;
- at most 128 items per definition and attempt;
- 1 MiB direct input and output budgets, independent of IPC;
- text: 4,000 scalars by default, 65,536 hard maximum;
- list: 1,000 entries, 8,192 scalars per entry and 1,000,000 total scalars;
- check result: 32 KiB canonical JSON per item;
- recorded text/list content: at most 16,777,216 scalars per attempt.

Evidence readback uses bounded pages: at most 256 KiB encoded page data within a
320 KiB response. Progression views carry identities, digests, sizes and bounded
previews. Windowed collections report exact totals and truncation state.
Artifact hashing streams bytes. Changed-row persistence and lazy schema validation
bound per-invocation work; historical feasibility measurements do not qualify a
new candidate. The [performance qualification protocol](../implementation-tips/direct-cli-performance-qualification.md)
owns fixture matching, measurement and baseline limits.
