# Podway Execution Group Continuation

## 1. Status and authority

- Document state: `Adopted`
- Dossier type: shared release-program execution design
- Consumer epics: FRAME, GROUP, TRACE, ADOPT
- Planning release target: `v0.2.18`
- Repository scope: Podway only
- Delivery boundary: adopted design; product execution requires its separately approved workflow
- Architectural prerequisite: [ADR-0044](../architecture-decision-records/0044-versioned-execution-groups.md), Accepted

The [roadmap](../roadmap/README.md) alone owns epic/task identity, ordering,
dependencies and status. This dossier owns the closed future implementation
contract shared by its four consumers. It grants no implementation, test run,
commit, release, installation, runtime activation or external-repository
change. The owner selected all four Epics for the open `v0.2.18` release cycle.
Release targeting is not release acceptance or publication authority.

No product behavior changes through application of this dossier. Accepted ADRs,
canonical assets and current specifications remain authoritative until the
named implementation tasks change them. ADR-0044 accepts the successor
architecture and staged delivery; its ADOPT-001 cutover alone supersedes the
current v2-only product and fixed preset identity clauses.

## 2. Verified context and decision basis

Procedure v2 already permits external parallel work while keeping one formal
cursor and one active attempt. Its admitted topology is immutable. State-aware
start can preserve a superseded session and create a successor; it does not
bind multiple execution topologies to successive designs inside one session.

On 2026-11-17 the owner selected same-session execution-group replacement as a
direct product requirement. A group must retain its DAG and every recorded node
attempt after replacement, including successful work that remains in the
working copy. The owner selected new-session-only support, direct single-model
cutover, and Coordinator replacement within an already approved scope. Existing
native subagent and Herdr parallel work must remain usable without Dolgorae.

This is a narrow planning amendment to the continuation scope of
[PROOF](TODO-podway-graph-evolution-feasibility.md) and its [continuation
research](references/continuation-generations-research.md). The S7 empirical
comparison has not run and is not classified as Motivated, Removed or passed.
Its historical protocol and evidence limits remain intact. S1-S6 and their
fan/claim/lease/join mechanisms remain Candidate; no inference from this
owner-selected requirement admits them.

## 3. Goals, non-goals and release scope

Deliver a complete first product slice: author a group, activate it from a
design anchor, perform work, return to design, activate a new group, inspect
prior work, and finish the task through the base Procedure. Runtime replacement
and useful retrospective inspection belong in the same release.

Preserve Direct Engine transactions, success-only outcomes, write-free reads,
worktree-local ownership, explicit transitions, current typed items, native
path safety and the existing crate dependency direction. Podway records
structure and caller assertions; it does not judge plan quality or authenticate
an agent.

Exclude arbitrary in-place graph editing, multiple design anchors, nested
groups, parallel active Podway nodes, group restoration, per-group deletion,
new failed session lifecycle, claims, leases, dynamic map, fan groups, joins,
resident processes, automatic execution, snapshot creation, Git rollback,
artifact-byte or transcript storage, provider calls by Podway and external
adapter implementation. External agents may continue to work concurrently on
one action. One selected group or formal cursor imposes no worker-count or
external scheduling limit.

## 4. Design decisions and interfaces

### 4.1 Definitions, execution identity and selection

The successor base document is `podway.procedure/v3`. A group document is
`podway.execution-group/v1`. These identifiers are separate from the CLI's
presentation contract-generation selector; changing that selector must not
activate new runtime semantics.

The base snapshot remains immutable. It contains one design-anchor action
placement, a declared group-success return placement, final goal assessment
when goal tracking is enabled, and closeout. Exactly one anchor is mandatory,
including in successor simple presets; a zero-anchor variant is outside this
slice. The anchor owns the boundary, not its reusable node definition. Existing
finite action/decision topology remains available outside that boundary. Vetting
requires the anchor and declared group-success return placement to dominate
every terminal placement; no successful base path bypasses group execution.

Every group definition is immutable, canonical and content-digested. Activation
creates one distinct `group_execution_id` (the activation identity), including
when an identical definition digest is reused. Bind it to the base/session,
anchor placement and exact design attempt, current goal revision where tracking
is enabled, plan item-set digest and preceding activation/replan lineage.
Namespace node identity by base/group scope and group execution identity; the
same local node ID in two groups never identifies the same runtime placement or
attempt.

Selection and execution outcome are separate facts. At most one group is
selected active. Its internal execution may be running or completed while the
base reviews its current result. Returning to design closes that selection and
leaves zero active groups until a new activation. Session termination closes
any selection without erasing the final selected group's identity or accepted
task history. Only one formal node attempt is active, in either base or
selected group scope. The execution outcome is `running`, `completed`,
`abandoned` or `terminated_by_cancel`, independently of selected/inactive
status. Replan or goal-revision closure maps a running group to `abandoned`;
the separate closure cause distinguishes them. Cancellation maps only a running
group to `terminated_by_cancel`. Later closure, cancellation or session
termination never overwrites a group's `completed` outcome. Ordinary
complete/skip/decision routing cannot leave the anchor; activation is its only
successful execution exit. Cancel, retry and admitted goal revision remain
legal. Declared manual rework to a base-prefix placement is also legal before
first activation, as specified below.

### 4.2 Group graph and boundary records

A group has one entry, one terminal success exit, ordinary action/decision
nodes, declared local rework targets and the existing typed recorded-item
model. It has no session lifecycle, independent goal revision, group activation
or nested continuation authority. Final session-goal assessment belongs to the
base.

Required references must dominate their consumers; all forward paths terminate,
forward cycles fail vetting, and local rework remains explicitly declared and
attempt-invalidating. Node instructions are data, never execution hooks.

Boundary inputs and outputs use declared item IDs, existing item types and
bounded evidence selectors, not a general output-schema or scripting system.
The anchor declares its own current-attempt items that a group may read and the
terminal item contract required by its base successor. Boundary compatibility
requires equal canonical item declarations after defaults, excluding only the
presentation-only `prompt` field: IDs, types, required/conditional rules,
constraints, operation identity/digest and accepted outcomes must match.
Unresolved controller/evidence references fail admission. Both activation and
terminal return enforce this comparison before effects. Activation checks
compatibility and freezes the exact current anchor attempt, item revisions and
complete-value digests. A group may refer only to those admitted plan inputs
and current local sources. The group terminal node must supply the
base-declared required items with matching types before normal completion
returns the cursor to the base. The base successor resolves its declared
execution-result evidence against that exact completed current group, never
another activation with similar node IDs.

Direct base-to-group or group-to-base edges outside these ports, references to
other groups, and historical values used as current guards are rejected. A
group replan is an explicit boundary operation, not an undeclared graph edge.

### 4.3 Activation, completion and replanning

`group activate` is a closed stdin mutation carrying the group document and
expected digest, plan identity/digest and normal exact fences. It is legal only
at the current anchor after its required items (including declared confirm/text
approval records), evidence and blocker gates are satisfied. Goal-tracked
sessions must have a defined current goal revision before any activation; an
untracked session has no goal binding. The caller records actual authorization
under its existing workflow; Podway checks the declared formal conditions and
does not authenticate Coordinator or human approval.

Preparation validates the complete canonical document, topology, boundary,
limits and expected digest outside the writer. Inside one Direct transaction,
revalidate prepared identity, current base/session/design attempt, goal
revision, prior activation and budgets; then store any new immutable
definition, create the activation, close the design attempt and create the
group's entry attempt. There is no durable partially admitted group or separate
proposal lifecycle.

Every activation records a caller-supplied `working_copy_basis`: a nonempty
bounded descriptor (at most 512 Unicode scalars) and SHA-256 digest of the
caller's declared source snapshot. This supports dirty/untracked working
copies; Podway neither creates the snapshot nor verifies filesystem truth. The
same basis shape is required by closure feedback. A fresh activation may record
a new basis after further planning; it does not rewrite the earlier closure
basis.

An activation with any earlier group execution in the session requires
`prior_work_dispositions`: 1..128 bounded native provenance records. They may
reference any retained earlier group in that same session, optionally an exact
retained attempt/item digest, a retain/revalidate/modify/revert choice, and a
nonempty current-plan rationale. Only the first-ever activation requires an
empty array. Goal-revision closure and completed-session reactivation do not
reset the predecessor history. A separate predecessor link always identifies
the latest closed selection; a disposition need not name that predecessor when
it has no work relevant to the current plan. G3 can therefore cite successful
G1 work even if G2 performed nothing.

Validate referenced identities/digests against retained state and bind records
to the fresh design attempt/plan digest. Missing records when history exists,
nonexistent/cross-session references or records submitted for an older plan
reject activation. An old group's provenance is permitted; an old plan's
submission is not. These fixed lineage metadata are not a new item type or
arbitrary output schema. Records need not enumerate every historical node, and
Podway does not judge whether the rationale is complete or true. FRAME defines
this admission contract; GROUP enforces it and TRACE exposes it. The
working-copy basis and dispositions are explicit Coordinator assertions made in
the activation request, covered by its canonical request digest and immutable
activation record; they are not represented as independently reviewed parts of
the plan item-set digest. Current plan review and actual scope authority remain
consumer obligations.

Normal group-terminal completion enforces its items/evidence/blockers and
boundary contract, completes that node and creates the declared base-successor
attempt in one transaction. It does not finish the session or silently record a
successful goal assessment.

`group replan` is legal only at a current ordinary decision placement with an
explicitly declared replan option, in either the selected group or an admitted
base review. There is no escape from an arbitrary active Action. The option is
an explicit boundary route, not a local forward edge or unrestricted jump. Its
failure predicates must be met against current declared evidence; missing or
unevaluable evidence is not failure permission. Base review sources must also
appear in the base anchor-return policy. Authors must make declared success and failure conditions exclusive. Vetting
rejects every replan option without at least one eligible guard over a required,
explicitly selected evidence source, including custom Procedures. Native checks
source/guard eligibility and current truth; whether the predicate expresses a
real failure remains the author's semantic obligation. Default success verdicts
cannot meet the default failure guard.

The closed replan request names that option and carries bounded reason,
working-copy basis and feedback. It does not supply an independent score.
Default replan options use three existing AND guards over a required, selected,
current assessment evidence source: failure verdict, assessment-stage equals
scored, and integer score at_least 80. All three guarded source items are
unconditionally required. Their ordinary types and bounds remain unchanged.

In one transaction, revalidate decision/option, resolve and freeze the exact
assessment source attempt, item revisions/value digests and failure/score guards,
then complete the decision attempt, record its replan choice/closure, close
selection and create the fresh anchor attempt. Bound report references in the
assessment are caller assertions; Podway verifies retained source identities,
not external report bytes, authorship or independence. A completed selected
group keeps its completed outcome; a running group becomes abandoned. Ordinary
`decide` cannot execute a replan option and directs the caller to the full replan
request without effects. Replay creates no second decision, group or attempt.
No request-carried total can replace the guarded recorded score.

A separately authorized session-goal revision while a running or completed
group remains selected must atomically revise the goal, close that selection
and create a fresh design-anchor attempt under the new goal revision. It must
not retarget an existing activation or retain its results as new-goal
readiness. At the anchor, goal revision creates a fresh anchor attempt and
invalidates its old plan basis. A completed session additionally requires the
existing explicit reactivation authority and always lands at a fresh anchor
attempt; a cancelled session cannot reactivate. Once any group has been
activated, `goal revise --rework-to` accepts only the anchor (including from a
terminal completed session); before first activation it may use existing
declared base rework targets. Every goal-revision or reactivation closure
records the same reason, working-copy basis and feedback as replan, with
`goal_revision` or `reactivation` as its cause. It preserves the latest
predecessor link even when terminal completion already closed selection. The
next activation follows the history-exists disposition rule above. Ordinary
rework to a pre-anchor node after first activation is deliberately unsupported.
Invalid revision/replan fences reject the complete operation, leaving both goal
and selection unchanged.

Return from a base review to the anchor is available only through `group replan`
from a placement in the declared anchor-return policy. Vetting rejects the
anchor as an ordinary decision/manual rework target and rejects ordinary base
decision rework edges crossing its boundary. Declared guarded replan options are
the explicit boundary-operation exception, never ordinary rework edges. A base
manual target allowlist
may include prefix placements: before first activation those targets remain
available, including from the anchor; after any group has existed, a typed
runtime rejection forbids them. Base-suffix decision/manual rework targets only
declared suffix placements, retaining selected output and normal suffix
invalidation. Inside a group, ordinary rework targets only declared local
placements; vetting rejects external group targets. Each successor preset keeps
its required decision-rework coverage inside its default group; a base review
needing redesign uses the explicit closure operation instead. No additional
payload is added to `decide`.

The caller must settle modifying descendants and reconcile uncertain external
operations before replanning or goal-revision/reactivation closure. Feedback
records the caller's current working-copy basis, original group/attempt
references and reason. Podway neither terminates processes nor verifies
settlement, working-copy truth or external effects.

### 4.3.1 Repeat assessment and Coordinator decisions

CO means the external Coordinator. Use the existing decision kind for
conditions. Count entries separately for
(session, group_execution_id, scope, placement); base review counts bind to the
selected group. A new attempt at that decision, including retry, increments the
entry ordinal. Reads, item edits, exact replay and reconnect do not. Count all
entries, not just failures; a fresh group starts its own counts while prior
counts remain historical. Persist/reconstruct these facts and expose the current
ordinal in observe/next and history.

| Current entry ordinal | Handling only if current verdict is failure |
|---|---|
| 1 | Ordinary bounded rework |
| 2 | Lightweight screening of repeated cause, progress and remaining local alternatives |
| 3 and later | A scored assessment and an explicit CO routing judgment at every failure: continue, replan or retain a blocker |

Success immediately takes the admitted forward path, regardless of ordinal.
Three entries are not a quota to fill or a minimum before every replan. A clearly
serious structural defect can receive a scored assessment earlier, but only at
a declared replan decision and under the same 80-point default threshold.

The native ordinal is observable authoritative data, never a predicate or
required_when input. The consumer enforces the entry-2 screening and entry-3+
scoring schedule before choosing ordinary continuation; Podway does not reject
ordinary rework solely because its ordinal is high. Assessment Actions receive
the upcoming decision ordinal from current projections/counters before entry.
A retry at the Decision without a new assessment cannot manufacture new evidence.

Default assessment Actions declare unconditional verdict, assessment-stage
(not-scored, lightweight, scored) and score integer 0..100 items. The score is
0 when not-scored or lightweight, explicitly a placeholder rather than an
assessment. Existing same-definition required_when makes screening text required
for lightweight and scored reasons/components/report references required for
scored. Conditional fields are not guard inputs. The three unconditional source
items above are selected by required evidence references, preserving existing
guard eligibility and avoiding unevaluable optional sources. Optional source
items themselves are not forbidden by the existing guard contract; optional
evidence references are. Consumer checks stage
against actual ordinal and failure verdict, score component ranges/sum and
current report fit. Success uses not-scored at any ordinal.
An applicable reused assessment is explicitly re-recorded as a current
composition with provenance; historical items do not become current guards.

Evaluator reasoning belongs to the current due assessment Action before the
Decision. Failure is a recorded assessment outcome, not a failed CLI invocation
or an invented extra decision submission. Lightweight screening is a short
addition to the existing failure report, not another full panel or mandatory
provider call. At ordinal 3 and later, CO makes a current routing judgment for
each failure; this need not request a fresh review when an earlier assessment
remains applicable. Record its current applicability and changed failure facts
without relabelling an old assessment as newly performed. If required judgment
or information is absent, use declared assessment/investigation recovery or stay
blocked; do not start arbitrary new work inside a Decision.

The applicable external Evaluator supplies reasoning. For Aquarium consumers,
Task/Epic use the affected Task/Epic Evaluators and Release uses its Release
Evaluator (EV); these are examples, not Podway roles or dependencies. Obtain
only actually affected assessment scopes. CO owns
the final route, support assignment and authority check, not technical repair
or arbitrary changes to an Evaluator's score. Resolve material disagreements
with that affected assessor. Basis, report availability and role independence
remain consumer responsibilities, not authenticated Podway facts.

Default scored assessments sum the following components, each in increments of
10 with explicit factual reasons. These are a decision rubric, not probabilities.

| Component | Maximum | Scoring anchors |
|---|---:|---|
| Need to change DAG structure | 50 | 0: no structural defect; 10/20: preference or inefficiency; 30: materially unsuitable boundaries; 40: missing required dependency/verification route; 50: current topology makes required delivery impossible |
| Difficulty resolving within current design | 30 | 0: viable local correction; 10: alternatives not yet investigated; 20: bounded corrections failed but alternatives remain; 30: no viable correction under current topology |
| Demonstrated lack of progress | 20 | 0: no repeated failure or meaningful improvement; 10: repeated cause with partial progress; 20: three or more failure results show no meaningful progress |

The consumer checks the component ranges and sum; Podway checks the admitted
option's guards over recorded verdict/stage/score without a scoring engine. Below
80, default Procedures forbid replan. At 80 or above, CO may continue with a
concrete viable next alternative, replan or retain a blocker, recording why.
A repeated failure alone scores at most 20 and never authorizes replacement.
No viable next alternative means blocked rather than aimless repetition when
replan is unavailable. A genuine separately authorized goal change/reactivation
retains its existing administrative contract; it is not a substitute for ordinary
replan and may not be fabricated to evade this policy.

Blocked is an existing blocker on the active decision attempt, not a routed
Decision option or a new terminal node. Use block with the actual missing facts
or lack of a viable alternative; resume with unblock after resolution, or use separately
admitted cancel/goal-revision operations. Missing information never produces a
fabricated success or a new forward route.

The entry schedule and rubric are default Procedure/consumer policy; Podway
provides authoritative counters and enforces declared sources, options, guards,
guarded score thresholds and exact records. It neither initiates reviews nor
invents failures or verifies that an external agent followed the reasoning policy. Preserve
current trust boundaries and ordinary recorded-item/predicate types.

### 4.4 Recorded work, feedback and new-plan reuse

Keep each group's definition, activation and closure relationships, design
attempt, node attempts, final retained item slots/revisions and digests,
blockers, decisions, rework/assessment records, timestamps and available actor
attribution. A replaced successful node remains completed; it is not relabelled
failed or never-run. Retain partial recorded values on an abandoned active
node. Unvisited nodes have definitions but no fabricated attempts.

Retaining final item slots per attempt does not introduce item-edit event
sourcing, every overwritten intermediate value, raw conversations or artifact
bytes. Inactive group state is immutable. Rejected stale submissions create no
new state or retained rejection outcome.

A new design records the present working-copy basis and its decision to retain,
revalidate, modify or revert prior owned work, citing exact historical group,
attempt and item/value digests where applicable. A reverted change is future
external work, not a Podway effect. The new plan can produce a group containing
only remaining work, so completed external work need not be performed again.

Historical references are provenance, not automatic readiness. The new plan
records fresh current items and its own review/check decisions; these provide
new group inputs. A historical check result, matching node name or same graph
digest alone never satisfies a new attempt. Artifact metadata/digests remain
inspectable even if external bytes disappear; report that absence honestly.

### 4.5 Public reads and mutation contracts

Provide closed commands/results for group activation and replanning, group
list/show, historical DAG projection, node-attempt listing and historical item
read. Standard authoring operations validate, canonicalize, digest, vet,
preview and render groups. Use new result schema discriminators for changed
semantics; never widen a released closed result family in place. Preserve typed
errors and the shared Output v4 and Error v2 envelopes.

`observe` and `next` expose only current actionable state, scope/activation
identity, current decision entry ordinal and bounded history summaries. Current
`evidence.read` retains its
current-evidence role. Separate history queries select exact session/group/
attempt/item identities, including archived sessions. They are write-free and
never make a stale value current.

List groups in activation-sequence order and attempts in trace order, with
exact totals, explicit truncation and stable continuation. History summaries
return at most 64 rows per page. Historical item data uses the existing typed
paging scale: at most 256 KiB encoded data and 320 KiB complete response,
scalar-aligned text and whole list entries. Larger DAG documents must also be
fully reachable through bounded canonical-document pages, not silently
truncated graphs. Other complete public envelopes retain the 1 MiB/depth-64
ceiling and lookup headroom rules.

Opaque tokens bind query kind, session/group/attempt/value or definition
digest, logical offset and snapshot sequence where needed. Changed bound state
is an explicit stale-token result; never combine pages from different
snapshots. Frozen historical objects remain readable without dependence on a
mutable source file, short-lived journal or outcome-retention row.

Cursor/selection mutations, including activation, return, replan, complete,
decide, rework and goal changes, fence workspace, session ID/revision, current
activation where applicable, attempt and applicable goal revision. Activation
additionally fences the plan basis and preceding activation. Single-item writes
preserve the current concurrency exception: fence workspace, session ID,
selected group execution, active attempt and that item's revision, without a
session-revision fence. Different-item writes can succeed across intervening
session revisions; same-item races reject. Atomic record-many keeps its current
session-revision plus per-item fences. Replan or goal change invalidates old
activation/attempt fences for both forms without serializing external agent
work. Idempotency lookup precedes live-fence validation and exact replay
returns the original stored success; original-key lookup is the first
response-loss recovery path. No fresh operation may use an old success
observation as current state.

### 4.6 Bounds, retention and lifecycle

| Dimension | Initial hard maximum |
|---|---:|
| Group activations/executions per session | 64 |
| Placement definitions per group | 1,024 |
| Directed edges per group | 8,192 |
| Cumulative placement definitions per session, including base and distinct group definitions | 16,384 |
| Cumulative node attempts per session, including base and all groups | 65,536 |

Repeated use of one definition consumes a new activation and new attempts but
not another copy of its placement-definition budget. Existing per-document,
item, text/list, reference, instruction, input and output limits remain in
force; new aggregate limits do not enlarge them. One Core-owned set binds
authoring, Store admission, transitions, projections and schema consistency
checks.

A mutation that would exceed a bound fails atomically without deleting data.
Cancel/archive and read-only inspection remain available at exhausted creation
budgets. `observe`/`next` expose remaining activation and attempt creation
budgets and the concrete limit preventing the next operation. Refusal at an
exhausted anchor is an intentional fail-closed result, not permission to evict
history. Group replacement, ordinary successful-mutation pruning and history
reads never prune group/attempt/item history. There is no automatic eviction,
individual group purge or restoration in this first slice. Existing explicit
session reset/archive purge/worktree-reset authority remains destructive and
must disclose its group-history effect. Archive retains all group content.

### 4.7 Ownership and consumer handoff

Core owns pure group values, graph invariants and transitions; Config owns
source admission, boundaries, canonicalization and projections; Protocol owns
closed wire families and budgets; Store owns relational persistence and
independent admission; Engine composes prepared/current facts and atomic calls;
CLI owns commands and rendering. Reuse existing crate boundaries and native
adapters.

Aquarium's binding/assignment maps its roles and actual native/Herdr handles to
session, group, node, attempt and target basis. Podway does not acquire role
slots, launch agents, guarantee reviewer independence, enforce agent
exclusivity or authenticate Writer authority. A group may have many external
agents on one action, with the consumer's existing single-Writer rules
unchanged.

Supply a versioned consumer contract and bounded examples for original work,
replan, response loss, settled and uncertain operations, changed bindings and
late results. Role configuration does not prove execution or settlement. The
consumer retains the original approved scope and effect grants as the authority
basis. A new DAG inside that scope requires its current plan review and formal
anchor gates, but no new Master approval solely because its topology changed. A
changed goal/scope or uncovered effect needs actual additional approval before
that effect; replan never renews consumed grants. The native caller-supplied
authority reference and required approval items prove shape and binding only.
ADOPT-003 covers both in-scope replacement and rejected/unapproved scope
expansion. Grant consumption is consumer-owned state, not a Podway grant
registry. Examples record the caller's original authority reference, current
scope classification and any newly required approval reference; Podway treats
them as bounded assertions. No native semantic scope detector or approval
credential is introduced. Actual Aquarium source changes, compatibility
adoption, rollout and installed/live qualification need their own consumer
owner and authority.

## 5. Compatibility and failure handling

The release uses direct single-model cutover to the successor for new sessions.
Do not convert or reinterpret existing v2 sessions, snapshots, values or
outcomes. Settle v2 work with its original writer before upgrade. A successor
encountering old state must refuse before mutation, migration or SHM creation;
no automatic reset, archive deletion or source rewrite. Recovery uses
original-writer access or separately authorized explicit destructive reset. No
v2/v3 coexistence or transparent migration is claimed. A new Store supports new
successor sessions; no predecessor declaration is implicit.

Preserve future same-writer reopening and complete new-group history. Later
writer/manifest upgrades must declare their own compatibility/migration
contract; this dossier promises no universal future upgrade compatibility.

Use existing prepared/running/completed/cancelled session semantics.
Group-level assessed failure or inconclusive results are recorded data routed
through declared actions/decisions/replan, not a new failed-session state.
Invalid activation, stale reports, changed plan basis, conflicts, resource
limits and unsupported writer state have distinct typed, no-effect failures.
Clock/process failures do not create a resident recovery service or imply an
external task was stopped.

### 5.1 Cutover inventory

- Keep the mandatory one-anchor model. All four built-ins become `analysis-v3`,
  `small-change-v3`, `bug-fix-v3` and `sw-dev-v3`, each with one base anchor and a
  catalog-supplied default group under canonical `assets/presets/`. Default groups
  are proposals, not automatically activated work. Preserve each preset's goal-
  tracking policy; small-change remains untracked. Explicitly lift the v2
  feature-matrix prohibition on option guards for small-change-v3 to support its
  guarded replan decision. Keep its assertion-based verification and absence of
  typed external check results; ordinary integer/choice items suffice. Move final
  session assessment
  to the base only where enabled. Old preset IDs/digests are retired explicitly;
  custom v2 documents/default selections require explicit caller adoption, never
  source rewriting or transparent conversion. Successor init defaults to sw-dev-v3.
- Preserve opt-in supervised delegation on base and group action placements,
  never decisions. New package/receipt families additionally bind base digest,
  group definition digest and group_execution_id where scoped, node and attempt.
  Re-activation of the same definition digest cannot reuse an old receipt. Keep
  the structural-only trust boundary, required completion gate and external
  adapter qualification limits; no live adapter is added. FRAME owns the contract, GROUP persistence
  and fences, ADOPT its public families and deterministic conformance cases.
- Bound current projections over the complete base plus selected group; archived
  groups are history-only. Charge combined counters/guidance and all wrapper/
  lookup headroom against each applicable response cap before activation and
  before creating the base-successor attempt. The combined counter schema is a
  new closed family, not widening ADR-0037's released 1,024-counter family.
  Maximum node counts do not guarantee admission if exact projected bytes exceed
  a cap.
- Retain presentation selectors 1 and 2, default 1, and their existing presentation
  distinctions. Both operate the same successor model, routes, Store and manifest;
  neither selects v2 semantics. Version every changed group-aware result/detail
  family while retaining the shared Output v4 and Error v2 envelopes. Generation-1
  deprecation is not silently converted into retirement by this program.
  Consumers must re-pin the complete successor manifest. Preserve explicit
  lifecycle, archive and disposition rules
  on new group history; old v2 state remains refused.
- Accepted ADR-0044 records its relationship to ADR-0019's preset identity
  clauses and v2-only rule, and to the affected ADR-0027, 0035, 0037 and 0039
  contracts. Reciprocal links preserve the earlier decision bodies; the product
  and preset supersession takes effect only at ADOPT-001.

### 5.1.1 Default Procedure quality and authoring examples

ADOPT-001 delivers all four successor presets and their default groups with
real work, assessment Actions and explicit decision routes. Preserve each
preset's original purpose, goal policy and verification coverage; do not copy
Aquarium-specific role bindings, tools, publication effects or dependencies into
a generic preset. Use the inspected Aquarium Epic/Release plans as reference
patterns for author/evaluator/Coordinator separation and failure recovery, not
as proof of implemented or installed behavior or an imported runtime owner.

Every work/assessment node states its purpose, accountable external role, input
basis, required output and failure-report obligations. Decisions state the
current evidence, success/failure predicates, continue/rework/replan choices and explicit blocker handling, selected targets and reasoning to retain. Provide negative reports
through declared paths without fabricating successful work. A node that cannot
produce required failure evidence remains blocked; there is no hidden escape.

The happy path advances as soon as requirements are satisfied. Instructions
must forbid manufactured failures, count-filling retries, inflated scores and
replanning merely to demonstrate the feature. In particular, do not retry a
decision without a new assessment merely to raise its ordinal. Rework needs actual failure facts
and a viable correction. ADOPT-002 supplies clearly separate worked scenarios:
normal completion, one correction, second-entry lightweight screening,
third-entry and later failure assessment, sub-80 plan preservation, 80-plus CO
choice, and early serious structural replan. Each failure scenario states its
concrete assumed defect; no instruction tells an agent to cause it in real work.
Intentional fault/count manipulation belongs only in isolated test fixtures.

### 5.2 Internal qualification and atomic exposure

FRAME/GROUP/TRACE remain internal and non-admitted through the shipped CLI. Use
a test-only Engine/Store driver, compiled only into the registered integration
suite/test-support path, launched as real child processes against owned fixture
Stores. It invokes the actual typed Engine and persistence transactions; it is
not a mock or product CLI mode. GROUP-005 uses it for contenders, response
loss, kills and cold reopen; TRACE uses it for exact history/paging/retention.
No new resident process, production unlock flag or public alternate writer is
introduced.

Candidate DDL, Store generation and candidate writer-contract admission remain
isolated to that test-only fixture constructor until cutover. The production v2 Store constructor/catalog and
public admission remain unchanged before ADOPT-001. Intermediate candidate
generations are unreleased, declare no release predecessor and establish no
upgrade support; do not open a real current workspace with them. Test-only
entrypoints must be absent from release artifacts.

Successor Procedure/group schemas live under canonical `assets/schemas/`.
FRAME-001 accepts their existing automatic manifest enrollment, ordinary schema
checks, offline handoff-registry loading and Protocol's embedded schema registry,
regenerating the development manifest accordingly. This changes the manifest
digest bound by development v2 writers and may refuse
earlier development Stores; it grants no migration, installed-workspace access,
new public parser/runtime admission or live Dolgorae qualification. No reserved
schema exclusion is introduced. Candidate DDL remains a canonical asset under
`assets/specifications/`, unenrolled in the active manifest's static DDL selection
until ADOPT-001; the public v2 constructor still selects its existing DDL.
FRAME/GROUP tests directly verify schema/type agreement and instantiate the
candidate DDL. ADOPT-001 selects/enrolls that same DDL and switches public
admission/families/catalogs atomically, with no derived source copy.

ADOPT-001 is the sole atomic public cutover Task: successor CLI/parser/runtime,
active Store DDL/writer and manifest identities, closed result/generation
catalogs, v2-only route/model removal, successor preset catalog/defaults and
new group-bound delegation contracts switch together in one coherent
candidate. ADOPT-002 then owns guidance and additional examples, not a later
preset flip. ADOPT-005 re-proves the required process/history flows with the
actual successor CLI; earlier internal driver evidence does not replace this.
No intermediate scaffolding or partial preset/runtime candidate is released.

## 6. Roadmap ownership, dependencies and task contracts

The closed dependency chain is `FRAME -> GROUP -> TRACE -> ADOPT`. Within each
Epic, execute Tasks in numeric order. Each final Task includes its own focused
integration, applicable documentation and complete development gate before Epic
acceptance. The roadmap alone maintains lifecycle/status. A Task's acceptance
below includes its relevant positive, rejection and boundary checks.

| Task | Title and bounded delivery | Dependencies | Acceptance |
|---|---|---|---|
| FRAME-001 | Define successor schemas and domain types, including root/group ports and identity | Accepted ADR-0044 | Canonical assets/types agree; successor schemas enroll in the development manifest and offline registry without runtime admission; declared replan options, score records and entry counters; input/output item contracts, exact declaration compatibility, prior-work metadata and scopes are closed; no public runtime admission |
| FRAME-002 | Validate graph, boundary, canonicalization and budgets; produce deterministic projections | FRAME-001 | Valid/invalid references, termination, replan-option/guard/source validation, rejection of unguarded replan options; unconditional default verdict/stage/score items, local rework, terminal dominance, rejected anchor/cross-boundary decision routes, conditional prefix manual rework, exact bounds, canonical equivalence and digest changes are covered |
| FRAME-003 | Implement pure activation, group return, replan and applicability transitions | FRAME-002 | Single active selection/attempt, decision-only scored replan, exact entry counting, correct base return, same-digest new run, required goal before tracked activation, goal-revision/reactivation closure and outcome mapping, exact anchor targets, scoped delegation and history-preserving invalidation hold |
| FRAME-004 | Integrate model verification and durable ownership | FRAME-003 | Focused tests, dependency-direction checks, affected docs and make test pass; public v2 runtime remains coherent |
| GROUP-001 | Persist definitions, activations, designs and attempts with independent Store admission | FRAME-004 | Reopen reconstructs exact state; invalid identities/relations and tampered persisted values fail closed |
| GROUP-002 | Commit activation and normal group-to-base return atomically | GROUP-001 | No partial group/entry/return; plan/output boundary, basis/provenance admission, combined response budgets and original success replay are enforced |
| GROUP-003 | Commit replan, selection closure and fresh anchor attempts | GROUP-002 | Completed/partial records remain; the selected decision attempt completes with its replan choice and a running group becomes abandoned; declared failure-only decision replan, score threshold, selected decision/closure atomicity and Action/undeclared/success-source rejection work |
| GROUP-004 | Fence group mutations and recover uncertain outcomes | GROUP-003 | Missing/stale prior-work records and crossed rework boundaries reject; late/cross-group operations fail; exact replay/lookup preserves original result after state advances; different-item writes both succeed, same-item conflicts and stale scoped receipts reject; counter replay/reconnect rules and current observation rules hold |
| GROUP-005 | Verify concurrency, process faults and lifecycle integration | GROUP-004 | Test-only real-process Engine/Store contenders/kills/reopens prove atomic effects and no duplicate activation; focused and make test gates plus docs pass |
| TRACE-001 | Query bounded group/design/activation lineage | GROUP-005 | Ordered pages/totals/cursors expose decision ordinals, assessment/CO choice and complete lineage under current and archived sessions without writes |
| TRACE-002 | Read historical DAGs, attempts, recorded values and outcomes | TRACE-001 | Maximum admitted content is fully reachable; scalar/list/document paging and explicit missing artifacts are covered |
| TRACE-003 | Link current decisions to historical work and its applicability | TRACE-002 | Admission is FRAME/GROUP-owned; queried decisions preserve score, failure evidence, CO rationale and provenance and old results cannot directly satisfy current gates |
| TRACE-004 | Verify retention, archive, bounds and explicit deletion boundaries | TRACE-003 | Replan/reopen/archive preserve history; no auto eviction; explicit reset/purge effects are documented and verified; make test passes |
| ADOPT-001 | Atomically expose successor CLI, Store/writer, manifests, preset catalog and closed public contracts | TRACE-004 | No public parser/runtime/preset mismatch; all four default presets have explicit failure/replan routes, the 50/30/20 rubric, complete node guidance and bans on fabricated failures/loops/count-filling/inflated scores; small-change-v3 admits guards without typed external results; both presentation generations and scoped delegation are coherent; all routes/fences covered through actual CLI |
| ADOPT-002 | Align authoring/usage guidance, additional group examples and paired source skills | ADOPT-001 | Reference paths author/activate/replan/finish with staged assessment and no artificial loops; normal, sub-80, 80-plus and early structural scenarios are covered; no later preset flip, installed-copy update or tracked .podway/procedures managed-file edit |
| ADOPT-003 | Deliver native/Herdr consumer contract and examples | ADOPT-002 | Deterministic callers demonstrate parallel contributions, scoped identities, settlement, affected Evaluator reasoning, CO choice and applicable assessment reuse, staged failure review, in-scope replacement without repeat approval, scope-expansion approval and stale-result refusal; no live-adapter claim |
| ADOPT-004 | Qualify cutover and original-writer refusal/recovery | ADOPT-003 | Authentic prior Store fixtures are unchanged on refusal; unsupported downgrade and fresh successor operation are covered; no original workspace reset |
| ADOPT-005 | Qualify integrated CLI and prepare accepted durable closeout | ADOPT-004 | All requirements reconciled, make test and exact native CLI scenarios pass, whole-target review/internal validation complete; actual owner acceptance remains distinct |

Do not defer current correctness or an Epic's due checks to a later Epic.
Intermediate completion is not release readiness. Consumer source adoption and
publication do not become implicit Tasks of this Podway program.

## 7. Verification, release acceptance and dossier closeout

Use focused exact registered tests first, narrow Make targets during
development and make test before sharing executable/contract changes. Route
long/noisy checks through configured Gaori commands. Checks use the pinned
toolchain and --locked. Documentation-only adoption uses file readback,
references/authority validation, the documentation verifier and git diff
--check; it does not need make test.

The matrix additionally covers Action/undeclared-decision/success replan
refusal; unguarded custom replan-option vetting refusal; first/second/third/later entries with current failure versus success;
lightweight versus scored assessment and applicable report reuse; score 79
refusal, 80 admission and high-score CO continuation; early serious defects;
no-change repetitions becoming blocked; ordinary-decide bypass refusal;
entry counts across retries/replay/reconnect; atomic selected decision/closure; request-score substitution refusal; guarded
unconditional default score sources and missing/unevaluable evidence refusal; blocker/unblock recovery;
small-change guarded replan without typed external results; and unchanged retained work. Deterministic fixtures may inject failures to
verify these boundaries; default/example instructions must not prescribe them.

The integrated matrix must cover first activation, same/different definition
replan, replan from permitted base review, rejected anchor-crossing ordinary
routes and late prefix manual rework, rejected anchor-bypass terminal paths,
tracked activation without a goal, current goal changes/reactivation
with closure feedback and outcome mapping, cancellation after group completion,
G1-to-G3 provenance through empty G2, initial and revised working-copy basis,
successful work retained without Git rollback, partial work, terminal
group-to-base return, parallel external contributors and concurrent
different-item writes, same-item races, scoped delegation receipts, stale group
writes, bad digest/plan/interface, unsupported references, simultaneous
activation/replan/report contenders, response loss before/after commit, process
exit/kill and fresh-process reopen, archive and explicit deletion,
exact/one-over budgets, maximal paged values and old-writer/downgrade refusal.
Negative checks verify unchanged state and no stored rejection outcome. Use
owned isolated Git fixtures and bounded child cleanup.

Runtime truth needs actual CLI/persistence/concurrency/fault evidence, not only
compilation or mocked success. Deterministic native/Herdr caller examples prove
protocol behavior only; installed role startup, providers, real execution and
external semantic acceptance remain separately qualified. Introduce no timing,
throughput or cost acceptance thresholds.

All four Epics form the first feature slice for `v0.2.18`. Release gate selection
and qualification follow the repository release policy under the later explicit
release workflow, including its applicable upgrade checks. The selected version
does not itself select or waive a gate. No release/installation/consumer
activation follows a development pass automatically.

At each Epic closeout, promote its lasting contracts and replace only that
consumer's dossier link with Canonical Outcomes. Retain this shared dossier and
TODO entry while any of FRAME/GROUP/TRACE/ADOPT still needs it or canonically
references it. Only the last accepted consumer may retire it, after all durable
owners and links are settled and deletion is authorized. Keep no generated
reports or transient workflow receipts in this dossier.

## 8. References

- [Documentation precedence](../README.md#precedence)
- [TODO lifecycle](README.md)
- [Roadmap](../roadmap/README.md)
- [Accepted architectural decision](../architecture-decision-records/0044-versioned-execution-groups.md)
- [PROOF candidate](TODO-podway-graph-evolution-feasibility.md)
- [Continuation research](references/continuation-generations-research.md)
- [Graph research](references/graph-engineering-research.md)
- [Current Procedure and items](../specs/domain/procedure-and-item-specification.md)
- [Current lifecycle](../specs/domain/rework-and-lifecycle.md)
- [Current transaction contract](../specs/storage/transactions-concurrency-and-idempotency.md)
- [Current paging and JSON contract](../specs/interfaces/json-contract.md)
- [Current direct architecture](../architecture/system.md)
