# Podway Host-Neutral Supervised Delegation Foundation

## 1. Status and authority

- Document state: `Adopted`
- Owning roadmap epic: `V2SDG`
- Adoption date: September 16, 2026
- Plan refinement: September 17, 2026; offline adapter mapping and post-acceptance recovery coverage
- Scope: Podway-side contracts, implementation, and isolated conformance only
- Contract target: optional `podway.procedure/v2` placement extension
- Acceptance target: development completion through the full `make test` gate
- Product release: conditional `v0.2.12` target after `V2SDG-006` through
  `V2SDG-009` and separately authorized stable-release qualification
- Decision authority: [ADR-0035](../architecture-decision-records/0035-host-neutral-supervised-delegation.md)
- Behavioral target: [supervised-delegation specification](../specs/interfaces/supervised-delegation.md)
- Execution order and status: [V2SDG roadmap](../roadmap/README.md#v2sdg-host-neutral-supervised-delegation-foundation)

This is the decision-complete implementation dossier for the bounded Podway
foundation. It replaces the former candidate at this path. No capability is
implemented, released, installed, or live-qualified merely by this adoption.
The roadmap is the only task-status owner. This dossier adds no competing
status table, automatic task start, or deployment authority.

Current checkpoint: `V2SDG-001` through `V2SDG-005` are completed internal
foundations, while `V2SDG-006` through `V2SDG-009` remain incomplete. The
`v0.2.11` source and release candidate therefore do not provide a supported
end-to-end delegation capability. Reserved contracts, package projection,
receipt persistence, and completion gates must not be reviewed as equivalent to
truthful public projections, native routes, coherent admission, lifecycle
qualification, or final epic acceptance.

The owner explicitly narrowed the work from the earlier live-Dolgorae research
candidate. ADR-0035 records why local implementation may proceed without that
candidate's external comparative experiment. No experiment is claimed to have
run. Real integration, external-versus-managed Primary qualification, and the
remaining comparison are retained in the separate
[integration candidate](TODO-podway-supervised-delegation-integration.md).
Graph engineering, continuation generations, and the Web console remain
unadopted and are not dependencies of this epic.

## 2. Verified context and baseline

### Podway implementation baseline

The inspected checkout was committed at `1e81399`, following the local
`0.2.10` source baseline and the opening of an unreleased `0.2.11` cycle.
That commit removes the former inactive-session cap under
[ADR-0034](../architecture-decision-records/0034-unbounded-inactive-session-retention.md).
Store generation 11, not 10, is current. Do not copy the old 32-session cap or
old schema generation from the original research candidate into new tests.

The following authorities and implementation points support this plan:

| Evidence | Consequence |
|---|---|
| [Placement model](../../crates/podway-core/src/procedure_v2/graph.rs) | Action and Decision remain node kinds; delegation is a placement attribute |
| [Closed wire parser](../../crates/podway-config/src/procedure_v2_wire.rs) | The current release rejects unknown delegation fields; a prose instruction is not a supported extension |
| [Session trace](../../crates/podway-core/src/session_v2.rs) | Preserve one active outer attempt and existing retry/rework invalidation |
| [State transitions](../../crates/podway-store/src/v2_state.rs) | Reuse item gates, blockers, goal rework, and explicit completion |
| [Batch regression tests](../../crates/podway-store/tests/int_v2_workflow_memory.rs) | Keep atomic item updates, session revision behavior, and cursor stability |
| [JSON contract](../specs/interfaces/json-contract.md) | New payload shapes require correctly versioned closed result families |
| [Store schema](../../crates/podway-store/src/schema.rs) | Plan from generation 11 and preserve stored bytes during any additive migration |
| [Named runtime authority](../architecture-decision-records/0031-keyed-runtime-modes-and-managed-development-service.md) | Do not touch production or Aquarium runtimes to qualify a feature |

`V2SCL`, `V2AST`, `V2GRD`, `V2REF`, `V2SKL`, `V2RDY`, `V2RET`, `V2DVC`,
and `V2FRT` are completed prerequisite foundations in the roadmap. Do not
reimplement paging, check results, typed guards, archive/reset, or the service
manager in this epic.

At planning inspection, six unrelated working-tree changes existed in
`CHANGELOG.md`, daemon execution/production code, and three daemon test files.
They are not V2SDG changes. Preserve them and take a fresh baseline before
implementation; this paragraph is inspection context, not permanent authority
over a later checkout. Do not reset, stash, stage, commit, or absorb them without
explicit authorization.

### External context, not an implementation prerequisite

A local Dolgorae inspection found completed role/task separation and an
external v3 accepted-task object with inline contexts and criteria. The internal
Primary orchestration tool still uses a different task shape. Its live Primary
bridge remains separately planned. These observations justify keeping the
Podway work contract host-neutral; they do not qualify either real adapter.

The September 17 [adapter contract review](../implementation-tips/delegation-adapter-contract-review.md)
pins the inspected Dolgorae source and maps required receipt fields across both
paths. In particular, the inspected external aggregate binding hashes Role
reference bytes while the Broker hashes an installed Role object. Those fields
are not interchangeable Role-content identities. Internal persistence alone
also does not prove that a permitted adapter can recover a digest or identity.

No developer needs that neighboring checkout, a credential, subscription,
installed provider, running model, or live agent to complete V2SDG. The offline
mapping review must nevertheless close the generic field meanings before
V2SDG-002 reserves them. Use the pinned source observations, mark unavailable
real paths explicitly, and leave their missing interfaces with the integration
candidate. A future integration must revalidate its own then-current canonical
contracts and exact build rather than treating this summary as API authority.

## 3. Goal, scope, and non-goals

### Required outcome

An explicitly authored delegated action can expose one deterministic work
package, accept one structurally valid attempt-bound receipt, and complete only
when its ordinary evidence and the receipt agree. The same contract represents
an external Primary or a runtime-managed Primary. Non-delegated Procedures,
existing sessions, and ordinary daemon operations keep working without any
executor integration.

### Included work

- An optional closed action-placement declaration with immutable Role identity
  and a logical binding reference, independent of a concrete facade.
- Canonical package and accepted-item-state digests with clear input/output
  separation and existing evidence-read reuse.
- Bounded acceptance recording, completion gates, truthful guidance, and
  historical receipt summaries.
- New versioned CLI/JSON contracts, exact capability registration, and additive
  worktree-local persistence.
- Direct compatibility, crash/replay, migration, and isolated cross-worktree
  continuity qualification using controlled executor fixtures.
- Development documentation and a separate release/integration handoff.

### Excluded work

- Dolgorae external-facade or internal-Broker adapters, real agent execution,
  Profile/credential lookup, or provider installation.
- Podway-created Primary Runs, automatic Run-per-node creation, live Primary
  replacement, driver ownership generations, or cross-host handoff.
- Fan groups, parallel cursors, claims, leases, dynamic maps, timers, join,
  continuation generations, and the Web console.
- Per-round task storage, transcripts, arbitrary artifact bytes, or a second
  evidence ledger.
- Changes to default/built-in/Aquarium Procedure behavior, release metadata,
  installed tools, runtime modes, LaunchAgents, or active runtime state.

## 4. Accepted design and implementation constraints

### 4.1 Responsibility model

Follow SDG-001 through SDG-003. A Direct action is Primary-owned. A Delegated
action has Specialist execution and Primary supervision. Decision and goal
assessment remain Primary-owned. One continuing Primary may drive many
placements; a compatible worker may execute multiple tasks without making
node identity equal Run identity.

Keep procedure requirement and execution binding separate. `binding_ref` names
an externally supplied integration choice; it is not a runtime command or a
new global registry. Role and Role digest are requirements. Actual adapter,
Primary host, executor scope, worker, task, and accepted-task identity are
bounded provenance on the receipt. Do not hard-code an external Dolgorae
engagement ID into the core receipt shape.

The foundation adds no authentication. Any adapter must keep trusted identity
and Controller authority outside model-controlled payloads. The core must not
mistake an `external`/`managed` label for authority, select a provider, grant
write permission, or satisfy a missing human approval.

### 4.2 Opt-in and admission sequencing

Absence of `delegation` must be byte-stable and behavior-stable. There is no
empty declaration, second execution flag, provider availability check, or new
setup requirement on the Direct path. Delegation rejects skip-enabled action
placements and every decision placement. No default or tracked workflow is
silently converted.

Authoring/domain work may be implemented before runtime integration, but
public runtime admission must reject delegated Procedures until storage,
readiness, acceptance, recovery, and truthful projections are all coherent.
Use the existing contract registration/admission pattern to separate reserved
shapes from executable support. Do not introduce an undocumented environment
flag that lets partial code enter live execution.

Only V2SDG-007 may activate the complete local feature through explicit
custom-Procedure opt-in, after it verifies all earlier invariants together.
That source capability is still not an instruction to deploy it.

### 4.3 Input, accepted task, and output identities

The package binds admitted Procedure and attempt input, not mutable current
output items or the current session revision. It includes exact selected
upstream evidence identities. The package read uses fresh fences outside its
digest. Repeated package reads and recording final action evidence do not
create new assignments.

The integration resolves complete evidence through current paging and creates
its own executor request. Podway package digest and actual accepted-task digest
are separate. Do not claim that matching an opaque digest proves the external
request contained all required bytes; that is an adapter conformance obligation.

Before receipt acceptance, the Primary records the action's final evidence and
corrections. Acceptance requires ordinary item satisfaction and no open blocker,
and the Store computes and freezes the accepted item-state digest in the
acceptance transaction. An actual output edit
after acceptance, including a cosmetic or optional-item correction, makes that
receipt stale. Preserve the immutable receipt and require explicit retry/rework
before another acceptance; do not add receipt-replacement or round history.

Implement the complete [post-acceptance matrix](../specs/interfaces/supervised-delegation.md#post-acceptance-behavior-matrix),
not a blanket invalidation on any session-revision change. True no-ops, reads,
failed/rolled-back edits, and block/unblock without item changes retain the
previous receipt applicability. An edit followed by restoring identical text
remains stale because its item revision changed. Exact acceptance replay returns
the stored result, not a fresh grant to complete; current observation must still
report intervening staleness.

Use the supported sequence of final evidence, blocker clearance, confirmed
worker disposition, acceptance, then explicit completion. Guidance must make the
fresh-attempt cost visible before acceptance and after any accepted-item edit.
A local retry is neither automatic external redispatch nor permission to copy
an old receipt to a new package. Explicit artifact reuse/revalidation belongs to
a qualified integration, not a shortcut added to this foundation.

### 4.4 Storage and mutation model

Use one additive per-attempt receipt table, one immutable receipt per attempt,
and existing session/attempt foreign keys. The provisional next schema
allocation is 12, rebased if another adopted change has consumed it. Admission,
receipt persistence, state revision, and terminal job envelope must share the
existing atomic state transaction.

Do not rewrite old snapshots or reserialize old item/goal/trace/terminal data.
Record nothing new for Direct attempts. Reconstruct with the same fail-closed
integrity discipline as existing stored state. A fabricated receipt or receipt
attached to a Direct attempt is corruption, not an alternate execution mode.

A schema bump is not automatically reversible merely because its table is
additive. Qualification must prove migration and failure recovery on isolated
copies, and establish older-binary rejection. No implementation task modifies
production data or promises a downgrade without a tested compatible restore.

### 4.5 Observation and history

Follow the exact target family allocation in SDG-008. Preserve all old result
families and stored terminal envelopes. A non-delegated snapshot keeps its old
result shape. A delegated snapshot uses coherent new prepared, running,
terminal, graph, and archive projections, including when its current placement
is Direct.

The Primary must be able to distinguish ordinary item readiness from delegation
readiness. No `complete` suggestion may bypass missing or stale acceptance.
Default status, logs, and progress views reveal only safe identity/status
summaries, not complete packages, result summaries, credentials, or transcripts.
Complete explicit reads and bounded history must satisfy the public frame
contract. Never treat a history receipt as current progression evidence.

### 4.6 Failure handling

The target specification owns exact error and replay behavior. In particular:

- Resolve idempotency before considering a lost response to be new work.
- Keep acceptance and completion as distinct explicit mutations.
- Do not clear open blockers, repair an unknown external result, or skip required
  delegation to make a workflow appear successful.
- Retry/rework/cancel and confirmed maintenance remain locally available under
  existing rules; none waits for an external executor.
- A local cancellation is not proof of worker quiescence. The adapter must
  resolve external writers before dispatching replacement work.
- Preserve the original failure, evidence, and receipt on mismatches; no broad
  reset or silent state deletion is an acceptable repair.
- Test external Primary restart and managed Primary restart using the same
  stable identity. A different Primary/host is an unsupported handoff, not a
  restart that may reuse authority automatically.

## 5. Non-disruption contract

These are blocking acceptance conditions, not performance or quality goals.

| Gate | Required protection |
|---|---|
| Existing Direct behavior | Canonical bytes/digests, supported paths, default selection, old result shapes, and domain/error meanings remain unchanged |
| Existing sessions | Prepared, running, completed, cancelled, and inactive data survive; no admission-time reset, rebind, or snapshot rewrite |
| Missing external runtime | Cannot affect daemon startup/readiness, ordinary workspace discovery, Direct actions, or local recovery |
| Mixed worktrees | A delegated action waiting for acceptance cannot stall unrelated Direct work or create provider-driven queue work |
| Development isolation | No production or Aquarium runtime path, service, socket, registry, mode, or active session is used as a fixture |
| Migration | Additive data preservation, transaction/crash recovery, exact newer-state rejection, and explicit backup/restore evidence |
| Strict peer identity | Preserve manifest/mode agreement; no compatibility bypass or fallback to an installed daemon |
| Intermediate commits | Every commit leaves the established Direct path usable; delegated admission stays closed until coherent |
| Full acceptance | Complete development gate and independent review; no accepted known regression or waived supported-path test |
| Deployment | Not authorized by a completed task, epic, test, commit, or published development bundle |

A detected regression blocks the current task and the epic. Do not defer it to
`docs/deferred-feedback/`, remove the relevant test, weaken the expected result,
or ask the owner to reset their active work as a workaround. Investigate in an
isolated fixture and restore the preserved Direct behavior before proceeding.

## 6. Roadmap ownership and commit-sized work

The roadmap owns order and state. The task sections below own implementation
scope and acceptance, not status. Execute tasks numerically, one active task at
a time. One task is one logical implementation-and-test commit. Commit
operations themselves still require owner authorization.

### V2SDG-001: Freeze the continuity baseline and qualification matrix

Purpose: turn the non-disruption requirement into executable comparisons before
changing product behavior.

- Record the exact implementation baseline and the selected previous stable
  contract/schema baseline; do not treat a dirty checkout as a release receipt.
- Inventory existing Procedure/preset canonical identities, result families,
  Store generation, supported reference paths, and named-mode behavior.
- Add reusable isolated fixtures for Direct and future delegated scenarios,
  including a populated Store with current and inactive sessions.
- Define negative cases, fault points, proof locators, and capability-admission
  checks before introducing the feature, including every SDG-006 post-acceptance
  case with unchanged Direct controls.
- Record the external source/schema baseline and review inputs from the adapter
  worksheet for the next task. No credential, live call, or runtime dependency
  is required; this task does not certify either real adapter.
- Preserve unrelated edits and keep all qualification state outside real
  account/runtime roots.

Done when baseline tests pass for the correct reasons and expected deltas are
explicit. No product behavior or delegation admission changes in this task.

### V2SDG-002: Reserve the closed contracts and traceability

Purpose: close field semantics before encoding the specification, without
advertising unavailable runtime work.

- Before any public schema reservation, complete the SDG-003 semantic mapping
  review using the [source-backed worksheet](../implementation-tips/delegation-adapter-contract-review.md).
  Cover every required execution field, result digest, count and disposition
  for both Primary-host paths, including authority, permitted access and replay.
- Close exact digest domains and normalization. Do not equate Role-name hashes
  with Role snapshots, prepared requests with accepted tasks, or internal
  Store columns with supported adapter getters. Record each unresolved real
  path as unavailable pending a named integration prerequisite.
- An unresolved external implementation does not block a defined Podway
  contract. An unresolved meaning of the generic contract does block its
  reservation. Do not weaken required fields, fabricate values, read private
  executor state, or introduce an external runtime/test dependency to proceed.
- Reserve the declaration, package, acceptance, receipt, projection, job-wrapper,
  and error families from SDG-002 through SDG-009 in canonical assets.
- Update bounded decoding, manifest/catalog registration, known-answer fixtures,
  and compatibility/requirement traceability through existing reservation rules.
- Preserve each old closed result's fields and exact stored terminal envelopes.
- Add decoder negatives for unknown fields, wrong types, over-bound values,
  secret-shaped prohibited fields, duplicate selectors, and identity mismatch.
- Keep executable capability advertisement and runtime admission disabled.

Done when the offline field/digest review is recorded, every local field meaning
is closed, unsupported real mappings have explicit dispositions, schema/contract
tests and old fixtures pass, the manifest delta is explicit, and unavailable
runtime routes reject before mutation. Mapping known answers are not claims
that Podway authenticates external bytes or that a real adapter was tested.

### V2SDG-003: Implement placement semantics and canonical work packages

Purpose: separate execution responsibility from graph type and make input exact.

- Add core values and configuration mapping for the optional declaration.
- Preserve omitted-field bytes and all old preset digests.
- Reject delegation on decisions/goal assessments, on skip-enabled placements,
  and on inputs whose delegated package cannot satisfy the frame budget.
- Implement the pure package projection, domain-separated digest, and tests
  proving output recording does not change input identity.
- Preserve one outer cursor and omit all worker/claim/scheduler abstractions.

Done when YAML/JSON equivalence, bounds, digest sensitivity/stability, and
Direct compatibility pass. Delegated runtime admission remains closed.

### V2SDG-004: Add immutable per-attempt persistence and migration

Purpose: store a receipt without rewriting existing authority.

- Implement the additive table/migration and closed receipt codec with current
  session/attempt linkage, uniqueness, immutable bytes, and reconstruction checks.
- Integrate atomic receipt/state/job outcome persistence and ordinary replay.
- Preserve old snapshots, item values, goal/trace data, archived state, and job
  terminal envelopes byte-for-byte where already canonical.
- Verify crash/transaction failures, orphan rejection, purge/reset lifecycle,
  populated-Store migration, and unsupported downgrade in isolated fixtures.
- Register the new crash boundaries in the canonical quality inventory.

Done when native Store tests prove data preservation and atomicity. No live
migration, managed-generation activation, or public delegated admission occurs.

### V2SDG-005: Enforce acceptance and completion semantics

Purpose: prevent declaration-only supervision from becoming a bypassable label.

- Implement pure acceptance validation, ordinary preconditions, declaration and
  package matching, round/disposition checks, and Store-computed item-state digest.
- Enforce one immutable acceptance per attempt and unchanged cursor on success.
- Add the delegated completion gate and stale-after-edit behavior; do not add
  these conditions to Direct attempts.
- Cover exact replay, same-key drift, different-key duplicate acceptance,
  retry/rework/goal invalidation, skip rejection, and existing error precedence.
- Test every SDG-006 matrix row: cosmetic/optional-item edits, true no-op,
  edit-and-revert, failed/rolled-back writes, block/unblock, ordinary revision
  changes, and replay after an intervening edit. Acceptance's own session
  revision increment must not invalidate it.
- Exercise acceptance/edit/completion ordering at the transaction boundary;
  use current fences for semantic tests and retain old fence-error precedence
  in stale-request cases. A warning is never the completion gate.
- Keep local cancel/retry/rework/reset behavior free of external waits and
  automatic external redispatch. Never repair staleness by replacing a receipt.

Done when native state/transaction tests prove valid and adversarial paths,
including unchanged Direct results. Public delegated admission stays closed.

### V2SDG-006: Implement truthful projections and bounded history

Purpose: make every client-visible view agree with the completion gate.

- Implement the new package/acceptance results and delegated snapshot versions
  of next, observe, status, prepared view, preview, and graph.
- Mark Direct versus Delegated responsibility without implying a live worker
  observation or authenticated actor.
- Suppress unsafe complete suggestions and expose exact missing/stale reasons
  and supported local recovery. Explain the final-record-before-accept sequence
  and the fresh-attempt cost of later item edits before acceptance.
- For a current receipt, explain that an actual accepted-item change will make
  it stale. For a stale receipt, distinguish a successful item edit from failed
  completion and point to explicit retry/rework, not automatic acceptance or
  redispatch. Current observation must not inherit an old replay's success.
- Keep warnings in permitted carriers and planned delegated projections; no
  new confirmation, extra JSON prose, or Direct result-field changes.
- Extend current/retained attempt history with bounded immutable receipt
  summaries and the explicit single-receipt read, while preserving old Direct
  result shapes and existing evidence-paging scope.
- Cover byte limits, truncation/total honesty, Unicode, privacy, and projections
  across prepared/running/terminal/inactive states.

Done when projection/schema tests prove cross-surface consistency. No user-facing
capability claims precede the next integration task.

### V2SDG-007: Integrate CLI/daemon routes and coherent opt-in admission

Purpose: expose the complete local feature without a Dolgorae dependency.

- Add the package, accept, and receipt public routes/CLI commands, help/completion, bounded stdin,
  native dispatch, versioned results, and durable job lookup/wait behavior.
- Tie runtime admission to the fully implemented contract set. Activate the
  explicit custom-Procedure path only after all storage, gates, views, and
  recovery surfaces pass native CLI/daemon tests together.
- Keep ordinary startup, workspace access, presets, Direct paths, and command
  selection free of adapter discovery and new setup requirements.
- Exercise missing receipt, stale receipt, response loss, strict manifest/mode
  mismatch, and unrelated Direct worktree progress.
- Verify native help and guidance teach final evidence/corrections and worker
  settlement before acceptance, then explicit completion. Test stale-after-edit
  diagnostics, historic-replay versus current-state distinction, and unchanged
  exit/result semantics for successful edits and Direct commands.
- Do not add a bundled provider adapter, a mock success command, or a mechanism
  that turns sample receipts into real-work acceptance.

Done when a controlled test client can complete the whole delegated path and the
baseline Direct suite still passes. This is source implementation, not rollout.

### V2SDG-008: Qualify lifecycle, migration, and cross-host invariants

Purpose: challenge the integrated path without contacting a real provider.

- Run a deterministic external-runtime fixture through both `external` and
  `managed` Primary scenarios using identical procedure semantics.
- Exercise feedback counts, retained/released receipts, incompatible Role,
  configuration provenance, executor uncertainty, Primary restart, and late results.
- Inject crashes and lost responses around receipt commit and explicit complete;
  never rerun external work merely because a Podway response was lost.
- Prove the full post-acceptance matrix through native calls, including
  cosmetic/optional-item corrections, true no-op, edit-and-revert, failed
  transaction, block/unblock, acceptance-response loss followed by an edit,
  exact lookup/replay, and fresh-attempt history. Use ordinary fences.
- Count external assignments/effects in the controlled fixture and prove that
  reads, Podway response-loss recovery, edits and local retry do not cause an
  automatic redispatch. Keep a relabeled old receipt invalid under the new
  package; explicit artifact revalidation is an integration obligation, not
  evidence that a real executor was exercised here.
- Repeat populated-Store migration and failure recovery through actual binaries
  in isolated named runtimes, including concurrent unaffected Direct work.
- Record real adapter and live model qualification as not performed, not passed.

Done when every local acceptance scenario has native proof and no supported
Direct regression remains. No private runtime is left running after qualification.

### V2SDG-009: Close documentation, full gate, and independent acceptance

Purpose: establish development readiness without implying operational activation.

- Promote implemented behavior into canonical architecture, CLI/IPC/JSON/error,
  storage, trust, testing, and requirement-traceability owners.
- Update source-distributed authoring/runtime guidance only for implemented
  capabilities, preserving opt-in and the lack of live adapters. Do not install it.
- Run the complete `make test` gate through the repository's prescribed test
  tooling and obtain an independent whole-epic review against the baseline.
- Resolve every valid blocking finding; do not turn continuity failures into
  deferred work or replace runtime proof with mocked success.
- Replace all dossier-only durable claims/links, remove this dossier and its
  adopted index entry at completion, and update roadmap evidence.
- Hand off exact tested source/manifest/Store identities, the reviewed adapter
  field/digest matrix and unavailable-path prerequisites, the documented
  post-acceptance correction cost, and separately required full release/upgrade
  qualification. Preserve the reviewed matrix outside the retiring dossier.

Done when development acceptance and independent review are recorded against
one coherent candidate. Release selection, tagging, publication, installation,
Aquarium activation, and live use remain outside this task and epic.

## 7. Requirements and verification traceability

| Requirement | Primary implementation owners | Required proof |
|---|---|---|
| SDG-001 responsibilities | 002, 003, 009 | No new node/token/worker model; host-neutral fixture |
| SDG-002 declaration | 002, 003, 007 | Closed authoring negatives, omitted-field byte stability, coherent opt-in |
| SDG-003 identity | 001, 002, 005, 008 | Pinned offline field/authority/digest matrix, permitted sources, unavailable real paths, distinct IDs, both host fixtures, no privilege from attribution |
| SDG-004 package | 003, 006, 008 | Stable input digest, complete-evidence adapter contract, maximum frames |
| SDG-005 acceptance | 004, 005, 007 | Native atomic commit, replay, exact declaration/input/result linkage |
| SDG-006 readiness | 005, 006, 007, 008 | Complete post-acceptance matrix, no-op and rollback preservation, edit/revert staleness, replay versus current state, explicit correction cost and safe guidance |
| SDG-007 lifecycle | 004, 005, 008 | Restart, late result, retry/rework/goals, no automatic external redispatch, no receipt relabeling, external uncertainty without local deadlock |
| SDG-008 public versions | 002, 006, 007 | Frozen old payloads, closed new branches, mode/manifest mismatch |
| SDG-009 persistence | 004, 008 | Byte-preserving migration, crashes, corruption, downgrade refusal |
| SDG-010 continuity | 001, 007, 008, 009 | All baseline paths, cross-worktree progress, full gate, independent review |

Numbers in this table refer to `V2SDG-<number>` with three-digit suffixes.
Tests and durable evidence locators belong in canonical quality and traceability
assets, not references to this temporary dossier.

## 8. Release and handoff acceptance

Use the [non-disruptive implementation guidance](../implementation-tips/supervised-delegation.md)
for isolation, test ordering, and deployment exclusions. `v0.2.12` is the
conditional release target, not an assertion that the feature is complete or
published. Its release owner must rebase against the actual previous stable
Store and contract generation, obtain separate release authorization, select
the full distribution gate, and execute the existing upgrade QA plus V2SDG
scenarios on copies. A new schema may prohibit the ordinary Aquarium
managed-generation activation path; do not bypass that restriction.

The supported claim at epic completion is: the Podway foundation is implemented
and passed local/native deterministic conformance at the exact tested revision.
It is not: Dolgorae works end to end, Primary handoff is supported, rollout has
zero risk, installed state was upgraded, or old clients can talk to a new daemon.
Only a later successful release workflow may turn that completed foundation into
a `v0.2.12` shipped claim.

## 9. References

- [Documentation authority](../README.md)
- [Dossier lifecycle](README.md)
- [ADR-0035](../architecture-decision-records/0035-host-neutral-supervised-delegation.md)
- [Target interface specification](../specs/interfaces/supervised-delegation.md)
- [Implementation and continuity guidance](../implementation-tips/supervised-delegation.md)
- [Adapter field and digest review](../implementation-tips/delegation-adapter-contract-review.md)
- [Remaining real integration](TODO-podway-supervised-delegation-integration.md)
- [Graph engineering candidate](TODO-podway-graph-engineering-evolution.md)
- [Release upgrade QA](../implementation-tips/release-upgrade-qa.md)
- [Testing and conformance](../specs/quality/testing-and-conformance.md)
- [Requirements traceability](../specs/quality/requirements-traceability.md)
