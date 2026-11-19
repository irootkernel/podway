# Execution Group Consumer Contract

## Version 1 and applicability

This contract applies to consumers coordinating work against Procedure v3,
Execution Group v1 and the compatible Store generation 16 writer. It versions
the consumer's operating obligations. The closed Podway request and response
schemas continue to own wire shape and limits. A native caller, a Herdr
coordinator or another executor can implement the same obligations using its own
authority, assignment and operation records. Podway observes formal progression
and bounded caller assertions. Authentication, dispatch, role independence,
write access and external settlement are consumer responsibilities.

Use the [execution-group model](../domain/execution-groups.md) for scope and
lifecycle, the [scoped Direct JSON contract](json-contract.md#scoped-direct-execution)
for current requests and recovery, and the
[host-neutral delegation contract](supervised-delegation.md#scoped-packages-and-receipts)
when an Action declares delegation. The normative boundary comes from
[ADR-0044](../../architecture-decision-records/0044-versioned-execution-groups.md).
The [worked consumer examples](../../examples/execution-group-consumer-v1.md)
exercise deterministic choices and existing interfaces. Real executor setup and
qualification, consumer source adoption, and installed compatibility have their
own owners and evidence.

## Own the work and bind the assignment

Podway retains one active attempt and at most one selected execution in the
worktree. Multiple external workers can contribute to one attempt. The consumer
can give read-only investigation, verification and assessment to different
workers concurrently. Keep one authorized logical Writer for the task's
modifying work in the target worktree. Transfer that authority explicitly,
after settling the previous Writer and descendants. Reviewers and Evaluators
may return findings or patches for that Writer to apply. Preserve independence
required by the consumer's workflow. A delegated Action is an explicit source
choice governed by its package and receipt. Ordinary external contributions do
not create delegation implicitly or require a delegation receipt.

Maintain a recoverable assignment record in the consumer's system of record.
Before dispatch, pin who may act, what they may do, and the exact target. After
dispatch, retain the returned native operation or assignment handle. At result
admission, reconcile these facts against the original assignment and current
state before recording anything in Podway.

| Fact | Required binding |
| --- | --- |
| Authorized work and effects | Owning worktree, task and exact work boundary, allowed access/effects, responsible role, original approved scope and authority reference, plus any later grant that covers additional effects. Track consumption of each effect grant in the consumer. |
| Dispatch-time Podway identity | Workspace UUID and the observed cursor with session ID and revision, exact scoped attempt, goal revision or null, and selected group or null. The attempt contains session, base digest, scope, node ID and attempt ID. Group scope includes the execution ID and the definition digest. A base review can also have a selected group. |
| Exact target and inputs | Pin the design or working-copy basis and the actual reviewed or modified target, such as a commit and a staged tree or bounded changed-file inventory. Record the applicable definitions, inputs, evidence identities and relevant digests. Recheck the basis at adoption and before using the result for a decision. |
| External identity | Pin the external scope, assigned role and actual durable native handle, including supervisor and worker/task identities as applicable. For native or Herdr work, use identities returned by that execution surface. Record the role/configuration identity where it defines applicability. Retain original binding, receipt and settlement facts when the surface provides them. Labels or proposed roles alone establish no assignment. |
| Returned work | Identify the actual result, immutable target basis, producer, material limitations, observed settlement and any still-open external effects. Provide bounded evidence references and digests when available. Keep transcripts, secrets and bulk results in appropriate consumer-owned storage. |

The consumer keeps its external bindings in its own records. Use the closed
input schema for each Podway request. Admitted values include item values,
citations, blocker reasons, replan feedback, activation source and plan
bindings, prior-work dispositions and delegation fields. A role or access
statement records consumer intent. Filesystem access and authenticated roles
remain with the consumer.

### Concurrent contributions

Read a fresh `observe` before admitting a contribution. Compare workspace,
session, base digest, selected group, goal revision and exact scoped attempt.
Then check that the reported target and relevant inputs still apply. The
dispatch-time session revision identifies the original observation. An
intervening revision alone leaves an otherwise applicable contribution eligible.
Use the fresh fences required by the native write. If an intervening edit
changes the basis, revalidate affected findings against current inputs before
recording them or requesting a new assignment.

Keep the original producer identity and the Writer's responsibility for
applying changes. Results can arrive independently. The authorized progression
owner reconciles all required results and readiness before selecting a route or
changing the plan.

Single-item commands can record distinct item slots across intervening session
revisions when the workspace, session, selected execution, active attempt and
respective item revisions still match. Same-slot conflicts need fresh state
and a new decision about the value. `record --stdin` and cursor changes also
fence the session revision. These semantics protect Podway records. The consumer
still enforces one Writer for worktree changes and settles conflicts among the
external producers. [Scoped Direct execution](json-contract.md#scoped-direct-execution)

## Recover and settle the original assignment

External execution and Podway mutation have separate outcomes. Before repeating
any work, recover the original operation using the executor's supported read or
recovery surface and its durable handle. Compare exact work scope, role and
configuration, task/result identity and target basis. If the original
assignment is still running, recover or await that assignment within existing
authority. If the assignment is settled, review its actual result and remaining
worker access. Acknowledging a task, sending a cancellation request or receiving
a Podway result gives no proof that an external Writer has stopped.

An unchanged binding can support continued supervision of the original task
when the executor allows it. If the role, configuration, external scope, task
subject or underlying target changes materially, retain the original identity
and assess the change. Reuse only facts independently applicable to the new
binding. Require an executor-authorized new assignment or supported revalidation
when the prior binding no longer applies. Apply the same test on recovery after
a process restarts. An unverified replacement cannot inherit the old Writer's
permission, task acceptance, outstanding effect or receipt. For delegated work,
keep the exact attempt package, authored role and binding, and receipt
applicability. Any supported new assignment must satisfy the current declaration
and cannot relabel a prior acceptance. Resolve uncertain external operations
with the original owner before assigning conflicting work.

For lost Podway output or `MUTATION_OUTCOME_UNKNOWN`, retain the original
workspace and writer, key, exact scoped fences, semantic arguments, stdin and
observation-wrapper choice. Run same-workspace `outcome lookup` with the
original key before replay. A found outcome proves historical commit. A miss
requires reconciling the original writer and retention boundary before an
identical retry. A Podway lookup cannot answer whether external work executed.
Exact replay returns stored success and performs no second transition. Obtain a
fresh `observe` before deriving a new mutation. [Direct recovery](json-contract.md#scoped-direct-execution)

Before `group replan`, or any authorized goal-revision/reactivation closure,
settle affected assignments and descendants under the consumer's custody
policy. Confirm that modifying descendants have stopped or are quiescent, and
that conflicting write access has ended. Reconcile uncertain external effects
and preserve their actual outcomes and unresolved dependencies. A read-only
result arriving after a scope change still needs a current applicability
check. Hold the selection or withhold reactivation while modifying work or
write authority is uncertain. On an active applicable attempt, record the
dependency, owner and resumption condition in a blocker. A completed session
has no active attempt, so keep that hold in the consumer's records. Apply
the hold even if the CLI shows an eligible replan. The external assignment
and working copy remain the consumer's responsibility across cursor changes.

## Assess current failure and choose a route

An affected Evaluator supplies reasoning at the current declared assessment
Action. Identify the exact target and applicable evidence. Obtain the smallest
required independent assessment scope. For example, a consumer with Task, Epic
and Release Evaluators asks the corresponding affected assessor under its own
role policy. The Coordinator (CO) owns route selection, support assignment and
effect authority. The Task Operator or authorized Writer owns technical
repair. Resolve material disagreements with the affected assessor. Keep the
original assessment provenance and the Coordinator's reason. The Evaluator
owns the score used for the routing decision.

Read the native `decision_entry` or the preceding assessment Action's available
`next_decision_entry`. Ordinals count entries per Decision, scope and execution.
A base review is bound to the selected execution. Retry creates another entry.
Reads, item edits, exact replay and reconnect leave the count unchanged. The
consumer applies the schedule. Podway enforces the authored predicates over
recorded values and does not score or initiate assessment work.

| Current verdict and history | Default consumer handling |
| --- | --- |
| Success at any entry | Take the eligible forward path promptly. Record the normal success basis and the unscored stage. |
| First failure | Use the declared local correction when it has a concrete viable change. |
| Second failure | Add a lightweight account of repeated cause, actual progress and remaining local alternatives to the current report. |
| Third and later failure | Record a current scored assessment and make a fresh explicit CO routing judgment for each failure. Reuse the affected assessment when its reasoning still applies. |
| Serious structural failure earlier | Score early at a declared replan Decision only when current facts establish a serious defect. Apply the same default threshold and authority gates. |
| Evidence, judgment or a viable route is unavailable | Use the declared assessment or investigation path when appropriate, or retain a blocker and its owner and resumption condition. |

Reuse an earlier assessment only after checking the accepted role/scope,
reviewed target, exact input and evidence applicability, relevant changed facts,
and that its reasoning still answers the current decision. Retain the report
identity, original provenance and a current applicability explanation. Record
that composition through current declared items and citations. A reused report
remains attributable to its original assessment. Missing source bytes, a changed
finding or a mismatched scope requires an explicit gap and the applicable
focused assessment or blocker. Retained history, a matching digest or a reused
node name cannot satisfy current gates automatically.

The default rubric scores structural need up to 50, difficulty resolving
within the current design up to 30, and demonstrated lack of progress up to 20.
Use increments of ten, factual reasons per component and the true sum. The
consumer checks the components, eligible stage and provenance. The declared
guards check current verdict, stage and score. For `not-scored` and
`lightweight`, use zero as the required unscored placeholder. A score below 80
keeps the plan. Use a justified correction or remain blocked. A score of at
least 80 permits the CO to choose replan when all other conditions hold. The CO
can preserve the current plan and take a viable declared correction, or retain
a blocker, with current reasons. Never induce failure, repeat unchanged work to
increase an ordinal, inflate a score or use goal revision to evade a guard.
The [continuation walkthrough](../../examples/continuation-v3.md) shows the
rubric anchors and concrete scenarios.

## Authorize and activate a replacement

Classify the proposed replacement under the original retained grant. A new DAG
within the approved objective, work and unconsumed effects still needs current
plan review and the formal anchor gates. The topology change alone does not
require another approval. When scope, goal or effects move outside that grant,
refuse the expanded request at the caller. Identify the exact expansion and
obtain actual approval from the authorized owner before using it. A consumed
grant cannot be renewed by replan. Record the original authority reference, the
current scope classification, and the new approval reference when required.
Any Procedure item for approval is a bounded caller assertion. Podway has no
scope detector, external approval credential or grant registry.

After settlement, use a declared currently eligible option in `group replan
--stdin`. Include its exact scoped context, stable key, reason, working-copy
basis and feedback. On success Podway completes the chosen Decision, closes the
selection and opens a fresh base anchor. The earlier execution and its attempts
remain inspectable. On the new anchor, review the exact new plan under the
applicable authority and satisfy required items, blockers, evidence, goal and
any delegation gate. Keep accurate basis and feedback. The `group activate --stdin` request uses a
fresh cursor and key, exact plan binding, expected definition digest, compatible
item interface and `prior_work` dispositions. Link the latest predecessor
separately. Earlier retained work in the same session can be cited as
`retain`, `revalidate`, `modify` or `revert` with rationale and exact available
attempt/item identity. Record any current assessment or verification needed
before reusing the work. Changes or reverts to Git remain external effects.
[Activation and replan model](../domain/execution-groups.md#pure-execution-lifecycle)

## Reject late applicability and preserve history

A late result from the closed or superseded attempt must keep its original
workspace, session, execution, definition and attempt identifiers. Compare them
with current state. Reject results relabelled with a new fence, digest, role
or external identity. Podway checks the declared request target. Authenticating
the external content and its origin remains a consumer responsibility. Retain
the old result as historical evidence or use an authorized current revalidation
path. Read-only access to an old receipt provides its retained applicability.

A fresh native mutation using an old attempt or selection fails the applicable
fence, for example `ATTEMPT_NOT_CURRENT` or `GROUP_SELECTION_STALE` when the
corresponding check is reached. Identity, revision or package checks can refuse
first. Validate the exact actual code against the request and first failed
boundary. A rejection changes no state and retains no success outcome. An exact
replay of a previously committed request can still return its old response.
That recovery path never establishes current applicability. In an isolated
fixture, compare the before and after state and look up the rejected request's
key. Other admitted writes can advance live state between observations. Relate
a live refusal to its typed error and original key within the applicable
retention boundary. Reconcile any external effect separately. Read retained
group, history or receipt records for provenance and current `observe` or
`evidence read` for live gate evidence.
A deterministic caller fixture proves those bounded caller/native distinctions.
External authority, settlement and live adoption require their own evidence.
