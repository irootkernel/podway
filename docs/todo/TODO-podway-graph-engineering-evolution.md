# Podway Graph Engineering Evolution

## Status and authority

- Document state: `Candidate`
- Dossier type: research and design candidate
- Owning roadmap epic: none
- Target product release: undecided
- Candidate contract target: a possible successor to `podway.procedure/v2`
- Candidate scale: release-program scale, comparable to `PV2GA` or larger
- Repository scope: Podway only
- Research and planning baseline: August 17, 2026
- Related adopted dossier: [Podway External Check Assurance Typing](TODO-podway-assurance-typing.md), epic `V2AST`

This document is the single repository location for the research, working
decisions, rejected directions, unresolved questions, and promotion conditions
associated with a possible graph-engineering evolution of Podway. It preserves
planning knowledge so that later design work does not have to reconstruct the
same context.

It is not an adopted design dossier, an accepted architecture decision, a
roadmap commitment, or a specification of implemented behavior. The active
[roadmap](../roadmap/README.md) does not own this work. Accepted ADRs, canonical
assets, executable contracts, specifications, source, tests, and runtime evidence
remain authoritative for the current product. No command, schema, lifecycle,
storage table, adapter, or behavior described as a proposal below exists merely
because it appears in this candidate.

Promotion to `Adopted` requires the material open decisions in this document to
be closed, the necessary ADRs to be accepted, and one or more owning roadmap
epics to be registered. Until then, terms such as *Procedure v3*, *fan group*,
*work receipt*, and the illustrative command names are design vocabulary only.

The phrase "a possible successor to `podway.procedure/v2`" understates what is
proposed, so the scale is stated plainly here. Section 16.3 lists 26 material
open decisions and section 17 spans nine ownership areas including a new storage
generation, new protocol families, a new public CLI grammar, and several new
ADRs. That is comparable to or larger than the `PV2GA` release program of ten
epics that delivered Procedure v2 itself. This is Podway v3 in scale rather than
a schema revision. That describes the candidate's maximum breadth and not an
adoptable unit: section 21 bars wholesale promotion while material shapes remain
unexercised, so any promotion proceeds as a validated subset.

Section 3 records what the shipped product already permits, and section 20.1
names the single precondition experiment. Both exist because the argument in
sections 8 through 17 rests on a demand hypothesis this document has not yet
tested.

## 1. Context and goal

### 1.1 Context

The motivating observation is that AI-assisted software development appears to
be moving through a sequence of increasingly explicit control structures:

1. **Vibe coding** delegates an outcome to a model and relies heavily on the
   model's current context and judgment.
2. **Harness engineering** improves the environment around the model: tools,
   prompts, context selection, sandboxes, policies, and observation surfaces.
3. **Loop engineering** makes iteration, evaluation, recovery, and stopping
   behavior explicit rather than relying on one unbounded conversation.
4. **Graph engineering** lifts dependencies, parallel opportunities, joins,
   gates, rework, and durable progress into an inspectable graph outside any one
   model context.

This vocabulary is recent and unstable. The underlying ideas are not all new:
workflow engines, schedulers, build graphs, state machines, durable queues, and
dataflow systems have used them for decades. The useful new emphasis is applying
those ideas to nondeterministic AI workers whose context is temporary, whose
self-reports are not proof, and whose work may need independent verification.

Podway already addresses a significant subset of that problem. It externalizes
procedure state, preserves a durable graph trace, enforces declared progression,
and rejects stale mutations. The candidate asks how Podway could improve in three
directions without abandoning its product discipline:

1. integrate with executors instead of competing with them;
2. distinguish mechanically grounded verification from actor assertions; and
3. support constrained parallel graph work while preserving a single-writer
   principle.

### 1.2 Goal

The goal of this document is to capture what is known, what appears promising,
what has been rejected, and what still needs proof before any implementation is
adopted. It should let a future design effort resume from explicit evidence and
open questions instead of treating the current conversation or private memory as
authority.

## 2. Research questions

The investigation that produced this candidate asked:

- Is graph engineering a real emerging practice or only a private framing?
- Which graph-engineering properties does Podway already provide?
- Which products or frameworks could compete with Podway?
- Should Podway execute work, or should it remain a durable authority around
  external executors?
- How can a formal gate distinguish a claim such as "tests passed" from a result
  bound to an actual check and exact input?
- Can parallel reads and writes coexist without allowing concurrent authoritative
  writes or nondeterministic observations?
- Can useful fan-out and fan-in be introduced without turning Podway into an
  arbitrary workflow engine?
- What must remain unresolved before a Procedure successor can be adopted?

The research used current repository authorities and implementation evidence,
public primary documentation for agent frameworks, public project repositories
for adjacent tools, and published research. Public projects in this space change
quickly; all external comparisons are snapshots as of the planning baseline.

## 3. What the current product already permits

Condition 1 in section 21 requires concrete user workflows that Procedure v2
plus external orchestration cannot adequately represent. This section records
the current answer to that question so that the rest of the document is read
against the shipped baseline rather than around it. Nothing below is a proposal;
it describes released behavior.

### 3.1 Externally parallel work is already recordable

`podway record --stdin` consumes closed `podway.item-record-many-input/v1` JSON
bounded to 1 MiB and 1 to 64 unique current-attempt item operations, and returns
`podway.item-record-many-result/v1`. Requirement `AUT-ID-008` in the
[Automation Client Contract](../specs/interfaces/automation-client-contract.md)
requires the route to fence the workspace UUID, session ID, session revision,
active attempt, and every selected item revision before atomically changing any
selected item. Per [State Transitions](../specs/domain/state-transitions.md) and
[Daemon and Write Queue](../architecture/daemon-and-write-queue.md), the pure
transition applies the complete set or returns no successor, produces one
durable job effect, advances the session revision exactly once when at least one
item changes, and never advances the graph cursor.

An external executor can therefore already keep one action attempt open, run
many independent read-only agents or checks concurrently outside Podway, collect
their bounded results, and record the complete set atomically under exact fences.
That capability shipped with `V2AGT-003` and `V2AGT-004`.

Concurrent external work is not a new permission. [ADR-0002](../architecture-decision-records/0002-single-active-stage.md)
already decided that multiple humans or agents may perform external work
concurrently and update different items while Podway retains one authoritative
current placement, and the write queue is described as permitting independent
callers to submit updates safely without creating parallel graph execution.

### 3.2 The proposed fan group permits a similar concurrency shape

Sections 10.1, 10.4, and 10.5 propose at most one workspace-write lease, an
explicit dependency reachability relation between every pair of write work units,
and no live read overlapping a write except a whitelisted snapshot read. Those
three constraints compose into one achievable shape: many concurrent snapshot
reads and strictly serialized writes.

That is close to what an executor can already obtain by fanning out read-only
work itself and recording the collected result set atomically. The proposal's
value therefore cannot rest on a claim that Podway prevents parallel work,
because it does not.

### 3.3 The residual gap is executor-neutral authority, not parallelism

What the shipped path does not provide is executor-neutral authority over work
while it is in flight:

- no record of which units exist, which are claimed, and which are outstanding;
- no arbitration preventing two cooperating executors from taking one unit;
- no per-unit binding between a result and the exact input it observed;
- no defined recovery when an executor dies after finishing some units; and
- no formal readiness derived from unit-level dependencies.

The claim is not that fan-out state is lost. Several executors listed in section
7.2 do recover their own runs through checkpointing, replay, resume, or
incremental recomputation. The claim is that whatever state exists belongs to one
executor's store and one executor's run identity. Podway retains the attempt and
any already recorded items, but it never knew the fan-out existed, so it cannot
state what remains, cannot arbitrate between two executors, and cannot carry
recovery across an executor substitution or a human performing one unit.

Three explanations remain open: that the shipped path is already sufficient, that
a durable executor closes the gap and Podway needs at most a binding to that
executor's run identity, or that an executor-neutral record is genuinely
required. This document does not choose among them. Section 20.1 is the
experiment that must.

This is a narrower problem than "Podway cannot express a parallel graph," and it
may admit a considerably smaller design than sections 10 and 11 describe. A
successor proposal should be measured against this gap statement rather than
against an absence of parallelism.

### 3.4 Status of this comparison

This section is an argument, not evidence. It has not been tested against real
tasks. The experiment in section 20.1 is the precondition for the rest of this
document: until representative work has been attempted with the shipped path and
its actual failures recorded, the scope in sections 8 through 17 rests on an
unverified demand hypothesis.

## 4. Historical context and priority limits

Podway's initial architectural decisions are dated July 13, 2026. They defined
one current task, one active stage, a daemon as the sole normal database writer,
worktree-local authority, a same-user trust boundary, typed stage items rather
than a general evidence ledger, and generic CLI/JSON integration. The first
repository baseline commit, `88586d4`, was recorded on July 16, 2026.

The initial Procedure model was intentionally linear. [ADR-0002](../architecture-decision-records/0002-single-active-stage.md)
allowed multiple humans or agents to perform external work concurrently, but
retained one authoritative active stage attempt. This means that Podway's durable
control-plane instincts predate its explicit graph model, while its original
product did not yet express a general graph.

On August 4, 2026, [ADR-0015](../architecture-decision-records/0015-constrained-single-cursor-graph.md)
introduced the Procedure v2 graph. [ADR-0017](../architecture-decision-records/0017-single-cursor-convergence.md)
then permitted alternative routes to converge on one placement without permitting
parallel tokens or synchronization. The v2 work made action and decision nodes,
declared routes, evidence readback, rework invalidation, graph vetting, and goal
tracking explicit while deliberately rejecting parallel execution and joins.

The paper *From Agent Loops to Structured Graphs: A Scheduler-Theoretic Framework
for LLM Agent Execution* was submitted to arXiv on April 13, 2026, before the
Podway repository baseline. It describes agent loops as single-ready-unit
schedulers and proposes an immutable static DAG, explicit planning/execution/
recovery layers, and controlled recovery. It is a position paper and design
proposal, not a production implementation or empirical result.

The defensible historical conclusion is therefore limited:

- Podway independently converged early on several control-plane requirements now
  discussed under graph engineering: durable external state, explicit routes,
  bounded transitions, idempotent mutation, crash recovery, and inspectable
  evidence.
- Podway did not invent workflow graphs, scheduling theory, or graph engineering.
- Podway's first released model was linear, and its explicit Procedure v2 graph
  postdates the cited graph-execution paper.
- Any public positioning should say *early independent convergence on durable
  graph-control problems*, not *the first graph-engineering system* or *the
  inventor of graph engineering*.

## 5. Current Podway product baseline

### 5.1 Product identity

The current product is a local procedure guard for one current task in one Git
worktree. [Goals and Non-Goals](../specs/product/goals-and-non-goals.md) defines it
as neither a project manager, CI system, command runner, Git mutation layer,
arbitrary workflow engine, plugin host, remote collaboration service, artifact
store, long-term evidence archive, AI runtime, nor same-user security boundary.

Podway does not execute configured commands, access the network, mutate Git, or
store artifact bytes. External actors perform the work. Podway records formal
state and permits only declared transitions.

### 5.2 Procedure v2 strengths

Procedure v2 currently provides:

- a finite, immutable, content-digested procedure snapshot;
- action and decision placements with declared routes;
- one authoritative cursor and exactly one active attempt;
- bounded typed items and item-local revisions;
- declared readback from prior attempts with freshness and dominance rules;
- explicit retry and declared or manual rework;
- conservative trace invalidation after rework;
- optional goal revisions, criteria, citations, and goal assessment;
- deterministic authoring validation, graph vetting, linting, and projections;
- generated JSON, Mermaid, PlantUML, and DOT views from the canonical YAML or
  JSON Procedure; and
- stable machine envelopes and structured recovery guidance.

These are substantial graph-engineering capabilities even though only one route
is traversed at a time. The graph is durable authority rather than an ephemeral
plan in a model's context.

### 5.3 Persistence and mutation strengths

[ADR-0003](../architecture-decision-records/0003-daemon-single-writer.md) makes
`podwayd` the sole normal writer of every live worktree database. Mutations are
durable jobs. One FIFO queue orders admitted mutations within a worktree, while
independent worktrees can progress concurrently.

[Transactions, Concurrency, and Idempotency](../specs/storage/transactions-concurrency-and-idempotency.md)
distinguishes workspace sequence, session revision, and item revision. It binds
idempotency keys to canonical request digests, performs state changes and terminal
receipts atomically, fails closed when an outcome cannot be proven, and guarantees
one logical state effect per idempotency key and request rather than one network
exchange.

This design is already suitable for concurrent callers. Parallel external work
does not require multiple SQLite writers. It requires a way to represent several
external work units while continuing to serialize their claims, renewals, reports,
and graph transitions through the one daemon writer.

### 5.4 Current assurance boundary

[ADR-0016](../architecture-decision-records/0016-recorded-item-workflow-memory.md)
and the [Domain Model](../specs/domain/domain-model.md) deliberately separate
formal validity from semantic truth. Podway can enforce that a required item has
the correct type and a recorded value. It cannot decide that recorded text is
true, that a confirmation corresponds to work actually performed, or that two
external schemas are semantically compatible.

This is honest, but it leaves an important gap. A confirmation that says "tests
passed" and a structured report from a known check, bound to the exact attempted
input, are not the same kind of evidence. Today they can collapse into similar
recorded assertions.

Unlike the parallelism boundary described next, this is a defect in the shipped
product rather than a limit whose cost has yet to be demonstrated. It is
therefore owned by the adopted dossier
[Podway External Check Assurance Typing](TODO-podway-assurance-typing.md), so
that its remedy does not depend on the topology proposed here.

### 5.5 Current parallelism boundary

[ADR-0017](../architecture-decision-records/0017-single-cursor-convergence.md)
permits several alternative routes to enter one placement, but only one route is
traversed. It explicitly excludes simultaneous forks, multiple active tokens,
parallel attempts, background graph execution, and synchronizing joins.

The current SQLite model reinforces that boundary with one active attempt per
session. The Procedure v2 schema contains only action and decision definitions;
it contains no fan group, map, join, executor, lease, or receipt contract.

Parallel work therefore requires a new architectural decision and new contracts.
It must not be smuggled into the closed Procedure v2 schema.

## 6. What graph engineering means for Podway

For this candidate, graph engineering is the discipline of moving orchestration
facts out of transient model context and into an explicit, reviewable, durable,
and recoverable work graph. A credible graph-engineering control plane should make
the following properties inspectable:

- goal and graph identity;
- nodes and dependency edges;
- which work is ready, claimed, blocked, stale, or terminal;
- which input version each result observed;
- where parallelism is safe and where order is required;
- fan-out and fan-in semantics;
- assertion, verification, and approval provenance;
- retry, rework, failure, and termination behavior;
- idempotency and crash recovery; and
- resource bounds and human-readable projections.

Podway already covers identity, durable state, declared routing, evidence
readback, rework, bounds, idempotency, and crash recovery well. Its gaps are
executor coordination, assurance typing, and durable authority over work that is
in flight.

The third gap is stated deliberately. It is not "Podway cannot run work in
parallel," because external concurrent work has always been permitted and, as
section 3 shows, its results are already recordable atomically. It is that Podway
holds no record of the individual units while they are outstanding, so it cannot
say what is claimed, what remains, or what an interrupted executor should resume.
Section 10 proposes one answer to that gap; it is not the only possible answer,
and the gap statement should outlive it.

The candidate thesis is that Podway should own the *authority graph*, not the
*compute graph*. Executors should run models, commands, tools, sandboxes, and
isolated worktrees. Podway should decide which declared work may be claimed, bind
results to exact state, derive formal readiness, and preserve the durable record.

## 7. External landscape

### 7.1 Research signal

The scheduler-theoretic paper frames a conventional agent loop as a scheduler
with one opaque ready choice and argues for immutable plan versions, explicit
dependencies, bounded recovery, and separate planning, execution, and recovery
layers. This supports Podway's separation of immutable procedure snapshot,
external work, and durable transitions. Its proposal is static and intentionally
trades expressiveness for control, which is closer to Podway than an unconstrained
agent-generated workflow.

The study *Towards a Science of Scaling Agent Systems* reports that the value of
multiple agents depends strongly on task decomposability. Independent parallel
work can benefit, while sequential work can degrade when split among agents. Its
reported error-amplification results also favor centralized coordination over
uncoordinated peer propagation. The design lesson is not "use more agents"; it is
"make real independence explicit and give the merge one authority."

### 7.2 Adjacent products and frameworks

| System | Primary role | Graph and parallelism | Durability and recovery | Verification model | Relationship to Podway |
|---|---|---|---|---|---|
| Graphene | Durable work-graph authority | Claimable DAG nodes, dynamic `for_each`, human gates | SQLite truth, leases, durable claims, stale belief propagation | Typed node outputs and belief provenance; does not execute models | Closest direct conceptual competitor; broader task-graph and belief model |
| AI Taskflow | Multi-agent DAG compiler and executor | Agent, map, reduce, gates, parallel rails | Resume, replay, cache, incremental recompute | Compiled IR, structured output, executable gates | Competes as an executor; Podway should integrate rather than reproduce it |
| Graph Skill | Host-neutral graph skill and local runtime | Concurrent dependency graph with selective retries | Local cache and node-level rerun | Local lint, type, and test gates | Skill/runtime competitor with lighter authority and deployment model |
| `graph-engineering` plugin | Claude Workflow design discipline and executor | Typed DAG, fan-out, adversarial verification, isolated worktrees | Workflow-specific caching and resume | Rerunnable harnesses, citations, independent voting | Strong evidence patterns; coupled to host execution rather than a general durable guard |
| headsign | Small phase-gate state machine | Primarily sequential phases and retry/escalation | Local state | Shell exit status rather than LLM self-report | Useful reference for mechanical gates, narrower graph semantics |
| LangGraph | Agent workflow runtime | Nodes, edges, parallel super-steps, reducers | Checkpoints, stores, replay and resume | Application-defined node and state validation | Executor/runtime to compose with, not replace |
| AutoGen GraphFlow | Multi-agent runtime | Sequential, parallel, conditional, and cyclic flows | Framework-managed execution state | Agent messages and application logic | Executor with richer scheduling and weaker Podway-style procedural authority |
| Strands Graph | Agent orchestration runtime | DAGs, cycles, nested graphs, custom nodes, output propagation | Runtime state and execution limits | Application-defined nodes and conditions | Executor framework; graph can be embedded or dynamically created |
| OpenAI Agents SDK | Agent harness and orchestration primitives | Manager, handoff, code-driven parallel orchestration | Sessions, tracing, and evolving sandbox snapshot support | Guardrails and structured outputs | Execution/harness layer that could consume a Podway work protocol |

The comparison is architectural, not a maturity ranking. Some projects are new,
experimental, host-specific, or rapidly changing. Their current feature claims
must be revalidated before an adopted design uses them as compatibility targets.

### 7.3 Competitive interpretation

Graphene is the closest existing analogue to a durable, non-executing graph
authority. It tracks readiness, claims, leases, typed outputs, humans, and
premise invalidation without calling a model or executing a node. That overlap is
worth watching.

It should not be read as proof that a category exists. One public repository is
evidence that one other group found the idea worth building, and nothing more.
The opposite reading is equally available from the same table: mature, widely
adopted systems cluster on the executor side, and the durable non-executing
authority niche is occupied mostly by small and recent projects. That pattern is
weak evidence either that the niche is small or that executors absorb the
function as they add durability, checkpointing, and resume.

This document should not treat a competitor's existence as validation and a
competitor's absence as opportunity, because that pair of rules cannot be
falsified by any observation. The honest statement is that the category is
unsettled and that neither the presence nor the scarcity of adjacent projects
currently tells us whether the demand in section 3 is real.

The durability and recovery column above is load-bearing for section 3.3 and
constrains what may be claimed there. Checkpointing, replay, resume, and
incremental recomputation are already present in several listed executors, so no
unconditional statement that fan-out coordination state is lost on a crash is
admissible. Any Podway durability claim must be stated as executor-neutral
authority and must be tested against an arm that uses a durable executor rather
than only against a stateless fan-out.

Podway's current advantages are different:

- a deliberately narrow one-task/one-worktree product boundary;
- immutable procedure snapshots and explicit compatibility contracts;
- strong stale-revision and attempt fencing;
- exactly-once logical mutation effects through durable idempotency;
- conservative rework invalidation;
- goal revision and goal-assessment memory;
- deterministic authoring projections; and
- release-level contract and crash qualification.

Taskflow, LangGraph, AutoGen, Strands, and the OpenAI Agents SDK demonstrate why
Podway should not become an executor. They already own model invocation,
sandboxes, parallel compute, callbacks, framework state, and provider integration.
Replicating those capabilities would increase surface area, couple Podway to
volatile runtimes, and create two competing authorities.

If the precondition experiment in section 20.1 supports it, the position this
candidate would defend is a durable, executor-neutral authority and verification
boundary that can outlive any one agent framework. That is a hypothesis to be
earned rather than an established position, and nothing in this section
establishes it.

## 8. Improvement theme A: integrate without competing with executors

### 8.1 Working direction

Podway should continue to avoid model invocation, configured command execution,
Git mutation, and sandbox management. An external executor should:

- obtain work that Podway declares ready;
- materialize or resolve the required isolated input snapshot;
- run an agent, command, tool, review, or human interaction;
- produce bounded structured results; and
- submit those results to Podway using exact fences and idempotency.

Podway should:

- expose a bounded ready frontier;
- issue and expire exclusive work claims;
- prevent claims that violate declared dependencies or effect rules;
- bind reports to the procedure, parent attempt, work unit, lease generation,
  operation definition, and input basis;
- accept the report atomically or reject it as stale;
- derive formal join readiness and outcome; and
- leave graph movement to an explicit mutation rather than treating worker
  completion as automatic stage completion.

This retains the principle in [ADR-0010](../architecture-decision-records/0010-generic-cli-json-integration.md):
generic CLI and versioned JSON are the canonical integration surface, and worker
completion does not silently advance the procedure.

### 8.2 Candidate integration surface

A future design may need operations equivalent to:

- observe ready and leased work;
- claim one eligible work unit;
- renew a finite lease;
- report a result and optional output basis;
- abandon a lease with a reason; and
- explicitly close or join a fan group.

Names such as `podway work observe`, `podway work claim`, `podway work renew`,
`podway work report`, `podway work abandon`, and `podway join` are illustrative.
They are not reserved public commands.

The canonical protocol should remain local CLI/JSON unless a later ADR changes
the integration boundary. MCP, agent skills, and executor-specific packages
should be stateless or thin adapters over that protocol. No LangGraph, AutoGen,
Strands, OpenAI, Claude, Codex, Orca, or Dolgorae concept should enter the core
domain model merely to make one adapter convenient.

### 8.3 Claim and lease considerations

A claim is coordination, not authentication. It prevents two cooperating
executors from owning the same work unit. In the current same-user trust model it
cannot prevent another same-user process from forging a request or mutating the
worktree outside Podway.

#### What the product does not have today

Neither concept exists in the current product. `lease` appears in no
specification. `claim` appears only as the daemon scheduler transactionally
changing the lowest queued job to `running`, which is internal, immediate, and
never expires.

The consequence is larger than parameter selection. In the shipped model every
domain state transition is caused by an admitted request carrying an idempotency
key, and no domain transition is caused by the passage of time. The only job
transition not caused by a request is restart recovery returning `running` jobs
to `queued`, described in
[Daemon and Write Queue](../architecture/daemon-and-write-queue.md); retention
pruning touches job rows and receipts rather than domain state.

A wall-clock lease expiry would be the first state change in Podway that occurs
without a client request. That is a new architectural property, not a new
constant, and it reaches:

- a timer or reaper that must run per workspace inside the daemon;
- the workspace FIFO, which currently has no notion of an entry that is not a
  submitted job, and no defined ordering for one;
- idempotency, which guarantees one logical state effect per key and request and
  has no representation for an effect with no request;
- restart recovery, which currently reconstructs state from durable rows alone;
  and
- the fail-closed principle, since an expiry decision is an assertion about
  elapsed time rather than a proven outcome.

#### Lazy expiry as the preferred alternative to evaluate first

A lease can carry a stored deadline that is evaluated only when a state
transaction next touches the affected work unit. Expiry then becomes a
derivation performed inside an ordinary mutation rather than an autonomous
event. Everything below is a preferred working direction, not an accepted
design.

What this preserves is narrower than first supposed. It removes the timer, it
introduces no state change without a request, and it adds no new kind of queue
entry. It does not remove time from the system. It relocates time from a
background thread into the mutation path, where it becomes an input to a
transition layer that
[Transactions, Concurrency, and Idempotency](../specs/storage/transactions-concurrency-and-idempotency.md)
treats as pure. That addition should be stated rather than denied.

The preferred evaluation rules follow the existing artifact completion race,
which already handles an external fact that is only true at an instant:

- **Evaluate at execution.** The deadline is read only inside the state
  transaction, never at admission. Admission remains a queue operation that
  allocates a sequence and inserts rows.
- **Daemon-supplied recorded instant.** The daemon reads the instant once inside
  that transaction and records it in the durable row and the terminal receipt.
  The instant is not part of the canonical request, so a transport retry can
  neither refresh it into a different request digest nor bind evaluation to a
  stale caller timestamp.
- **Stored-outcome replay.** Replay returns the recorded outcome rather than
  re-evaluating the deadline, consistent with lookup by idempotency key
  returning a stored terminal envelope instead of replaying a request.
- **Fail closed on divergence.** A claim whose target lease is still live at
  execution fails with a conflict rather than adapting, on the model of the
  stale-cursor rule and `ARTIFACT_CHANGED`. It does not take the unit merely
  because the lease appeared expired earlier.
- **Generation fencing.** Renewal carries an expected lease generation,
  mirroring item revision and attempt fencing, so a renewal that races a reclaim
  fails instead of resurrecting an expired lease.
- **Advisory read observation.** A read-only observation may derive expiry but
  never commits it. The derived value is marked as evaluated at the read instant,
  two reads may legitimately disagree, and a read reporting expiry does not
  guarantee that a later claim succeeds. This mirrors the existing rule that
  callers must inspect pending mutations when they require a quiescent view.

The honest limit should be stated the way the artifact race states its own. This
gives freshness at the transaction instant as far as local observation permits,
and nothing stronger.

The cost is that a stalled fan group makes no progress until an executor or
observer asks. That still looks cheaper than a reaper.

This alternative was not considered in the original exploration. It should be
evaluated before any timer-based design, because if it holds, several of the
questions below collapse.

#### Remaining lease questions

A credible lease design must still decide:

- default, minimum, and maximum lease duration;
- renewal fences and maximum renewal behavior;
- whether expiry creates a new lease generation or a new work attempt;
- how many expired or explicitly abandoned attempts are retained, and whether an
  expired lease that no request ever touches is bounded by a retention policy or
  accumulates indefinitely;
- whether claim capacity is caller-supplied, executor-registered, or purely
  pull-based;
- how writer starvation is prevented;
- under lazy expiry, which mutations are obligated to evaluate deadlines; and
- what a backward movement of the system clock means for a stored deadline. No
  specification states a clock policy today. Treating the transaction instant as
  authoritative and rejecting when it precedes the last recorded instant for the
  workspace would fit the existing fail-closed principle, but it is undecided.

These values are intentionally unresolved. The earlier discussion used ten
minutes and a one-hour maximum as examples, not accepted constants.

## 9. Improvement theme B: separate assertion from mechanical verification

This theme is owned by the adopted dossier
[Podway External Check Assurance Typing](TODO-podway-assurance-typing.md), which
holds epic `V2AST` and targets v0.3.0.

The reason for the split was evidence status, not lack of importance. The gap it
describes exists in the shipped product, as section 5.4 records: a `confirm` item
asserting that tests passed and a result bound to a named check against a named
input can occupy the same formal slot and satisfy the same completion
requirement. Nothing in that problem depends on fan groups, work claims, leases,
or a parallel frontier. Keeping it inside this document would have made the
cheapest and best-evidenced improvement wait behind the most expensive and
least-evidenced one.

That dossier owns the assurance classes, the receipt binding, and the honest
statement of what a receipt does not prove. It has also settled the question this
document previously left open. A `check_receipt` binds to a declared operation
identity and an exact input basis with no work-unit, map-instance, or lease
identity, so the receipt is useful without any of them and the two documents do
not merge back together.

The fan-specific identities are therefore a conditional extension rather than a
prerequisite. They enter scope only if the topology in section 10 is adopted:

- binding a receipt to fan-group, work-unit, and dynamic map instance identity;
- binding a receipt to lease identity and generation;
- deriving freshness from an input basis published by a preceding write work
  unit, as described in section 10.6; and
- how required and optional receipts compose into a group outcome, as described
  in section 11.3.

Each is governed by the evidence-to-scope rule in section 20.1 exactly like any
other mechanism proposed in this candidate.

## 10. Improvement theme C: effect-aware fan graphs

### 10.1 Single writer has two meanings

Two related but distinct principles must not be conflated:

1. **State writer:** `podwayd` remains the sole normal writer of Podway's SQLite
   authority. Every claim, renewal, result, and join is serialized through it.
2. **Workspace writer:** at most one cooperating external work unit may hold a
   declared right to mutate the owning worktree at a time.

The first is enforceable by Podway's architecture. The second is enforceable
only among clients that follow Podway's claim protocol; Podway is not a filesystem
security boundary.

Parallelism should apply to external work and snapshot reads, not to competing
database writers.

### 10.2 Composite fan group rather than a global multi-frontier

The preferred working direction is a composite fan group inside a single outer
procedure cursor:

- the root procedure has exactly one start and one end;
- a running session retains one authoritative outer cursor and parent attempt;
- when the cursor is on a fan-group placement, the group owns a bounded internal
  dependency graph;
- the fan group also has exactly one start and one end;
- internal work units may be ready, leased, reported, or abandoned concurrently;
- child work units are subordinate to the parent attempt rather than independent
  procedure cursors; and
- only an explicit join closes the internal frontier and moves the outer cursor.

This is less general than converting the whole Procedure into a multi-token DAG.
That constraint is intentional. It isolates concurrency semantics, preserves the
outer task narrative, and makes rework invalidation more tractable.

The alternative — a global multi-frontier in which every placement may be active
simultaneously — was rejected as the initial direction because it would require a
complete redesign of session status, attempt identity, evidence dominance,
decisions, goals, rework, terminal state, CLI guidance, and storage invariants.

### 10.3 Effects

The working model uses two declared effects:

- `snapshot_read`: work reads an immutable input basis and does not intentionally
  mutate the owning live worktree.
- `workspace_write`: work may mutate the owning worktree and must hold the one
  workspace-writer lease.

The names are provisional. A later design must decide whether approval,
snapshot publication, or external side effects require additional effect kinds.

Effect declarations are contracts with cooperating executors, not sandbox
enforcement. Podway can reject conflicting leases and inconsistent receipts. It
cannot detect an executor that declares a read and then writes outside the
protocol.

### 10.4 Write ordering

Every pair of possible write work units inside one fan group should have an
explicit dependency reachability relation. If neither write precedes the other,
graph vetting should reject the candidate rather than let a scheduler, priority,
node identifier, or claim race silently choose semantic order.

This gives write nodes a total order while allowing read nodes to occupy parallel
branches. It also makes a generated Mermaid projection explain why one write
must precede another.

Implicit priority ordering and runtime executor choice were rejected because they
would make the visible graph incomplete and make replay dependent on scheduling
accidents.

### 10.5 Read/write overlap

Reading a mutable live worktree while another agent writes can observe a partial,
non-repeatable state. Live reads therefore should not overlap a write.

A read may overlap a write only when it is bound to an immutable snapshot that
predates and does not depend on that write. The preferred authoring mechanism is
a fail-closed whitelist on the write work unit. A whitelist entry is admissible
only if graph vetting can prove that:

- the target is a `snapshot_read` work unit or map template;
- its input basis was fixed before the write;
- it has no data dependency on the write's result; and
- the relation remains closed and unambiguous after dynamic expansion.

The rule must be symmetric at claim time. A writer cannot be claimed while an
incompatible read lease is active, and an incompatible read cannot be claimed
while the writer lease is active.

A blacklist was rejected. Missing a blacklist entry would silently permit unsafe
concurrency, whereas missing a whitelist entry merely reduces concurrency.

### 10.6 Snapshot ownership

The preferred direction is executor-owned isolated snapshots:

- an external executor creates or resolves the immutable snapshot;
- Podway stores a bounded descriptor and content digest, not snapshot bytes;
- all parallel reads that claim the same basis must echo its exact identity;
- a write result produces a new output-basis descriptor for downstream reads;
  and
- no downstream read that depends on the write becomes ready before that basis is
  accepted.

A descriptor might eventually contain a provider, opaque snapshot identifier, and
content digest. Its exact wire shape, resolver ownership, lifetime, cleanup,
cross-executor portability, and handling of dirty worktrees remain open.

Two alternatives were rejected as defaults:

- requiring every fan group to operate only on a clean Git commit is strong but
  too restrictive for reviewing in-progress software changes; and
- having Podway hash and freeze a live dirty worktree would pull filesystem
  snapshotting, race detection, large-file policy, and lifecycle management into
  the control plane.

### 10.7 Dynamic map and reduce

Dynamic fan-out is useful when an earlier node identifies files, packages,
findings, competitors, or other bounded items that can be processed independently.
The preferred source is an already recorded, immutable upstream `list` item.

This keeps expansion deterministic and reviewable. The procedure identifies the
source node and item. When the fan group activates, the list value is frozen under
the parent attempt and expands one map instance per entry.

The following rules are promising but not adopted:

- worst-case fixed work units plus mapped instances must fit one bounded group
  budget, tentatively aligned with the existing 64-placement bound;
- a mapped source's declared maximum must allow that budget to be proven before
  session admission;
- a downstream dependency on a map template acts as an all-instance barrier;
- `snapshot_read` map instances may be parallel;
- `workspace_write` map instances must be explicitly ordered, for example by
  source-list order, rather than treated as parallel writes; and
- zero-entry map behavior must be deterministic and visible in the join record.

Executor-submitted runtime manifests and Podway-owned filesystem glob expansion
were rejected as defaults. The former moves graph authority into the executor;
the latter makes Podway interpret the filesystem as a workflow engine.

### 10.8 No semantic lane cap

An earlier exploration considered a maximum of three lanes because small graph
plans are easier for humans and LLMs to reason about. That should be an authoring
heuristic, not a runtime contract.

The graph's ready frontier and the executor's available capacity should determine
actual concurrency. Podway still needs hard collection, queue, lease, frame, and
work-unit bounds, but it should not encode "three agents" as product semantics.

The authoring skill should encourage a small number of genuinely independent
lanes, often two or three for ordinary software work, while rejecting fake
parallelism. Sequential work should stay sequential, and shared-context work
should not be split merely to increase agent count.

## 11. Join, optional work, and failure

### 11.1 Result versus execution failure

`pass`, `fail`, and `inconclusive` should all be usable reported results. A failed
verification is information that a reducer or decision may need. It should not be
confused with an executor crash, malformed report, expired lease, or missing
result.

This distinction permits a fan group to collect all diagnostics before deciding
whether to advance, rework, or fail.

### 11.2 Required and optional work

The working interpretation is:

- a required work unit must have a usable result before join;
- an optional work unit may contribute a result but is not allowed to block join
  indefinitely; and
- joining explicitly abandons any unfinished optional unit and rejects its later
  report as stale.

An explicit join creates a cutoff and records what was accepted and what was
abandoned.

These two statements are in tension, and the conflict is a design defect rather
than a storage detail. The stated intent is that optional work should not
disappear merely because of timing, but under the rule above an optional unit's
inclusion depends entirely on whether its report was admitted before the join
mutation. That is timing dependence, and it is the same non-repeatability that
section 10.4 rejects for write ordering: two runs of one procedure over one input
can produce different accepted result sets because of scheduling, and a replay
cannot reconstruct which outcome was correct.

The document does not currently resolve this, and resolving it requires
separating two properties that the earlier discussion conflated.

**Inspectability** is a property of the record. Because admitted mutations are
totally ordered within a workspace, the accepted and abandoned partition for any
one run is durable, attributable, and reconstructible afterwards. This already
holds and needs no new mechanism.

**Repeatability** is a property of the semantics. The accepted set is a function
of declared procedure data and durable state, and does not depend on the arrival
order of concurrent reports. Ordering mutations cannot supply this, because
ordering records what happened rather than constraining what may happen.

A two-step join that first closes group admission and then aggregates does not
provide repeatability. Closing is itself a mutation contending in the same queue
as an incoming report, so the construct relocates the race from
report-against-join to report-against-close. It is bookkeeping within the
declared non-deterministic family below, not a separate deterministic family.

Four families are available. None has been chosen, and the choice is product
semantics rather than storage design:

1. **Completeness.** Join is illegal until every optional unit is terminal. This
   makes inclusion timing-independent, because the accepted set becomes exactly
   the set of units that reported. It redefines optional as "may report no usable
   result" rather than "may be outstanding at join". It delivers that property
   only if reaching a terminal state is itself timing-independent; see the lease
   coupling below.
2. **Irrelevance.** Optional results are recorded but formally excluded from the
   group outcome and from any reducer or decision input. This does not make
   inclusion timing-independent, because the recorded set still varies between
   runs. It prevents that variable record from affecting the formal outcome.
   Section 11.3 currently permits optional results to be supplied to a reducer or
   decision, which is the mechanism this family would have to close.
3. **Elimination.** There is no optional category. Work is required, or it
   belongs to a different placement or a later group. Inclusion is
   timing-independent by construction, at the cost of expressiveness.
4. **Declared non-determinism.** Arrival order decides inclusion, the cutoff is
   recorded as an explicit fact, and the contract states that repeated runs may
   accept different optional sets. This family promises inspectability and does
   not promise repeatability. It is legitimate when declared and is a defect only
   when denied, which is the current state of the text.

Independently of the family chosen, an optional report admitted after a cutoff
need not be discarded. Recording it as arrived after the cutoff and not counted
improves inspectability without altering determinism in either direction.

The completeness family couples to the lease design in section 8.3. If a unit can
become terminal through time-based expiry, then whether it expired or reported is
decided by the clock, and the timing dependence this family exists to remove
returns by another route. Choosing completeness would require abandonment to be
an explicit mutation, or would require expiry to be excluded from the paths that
make a unit terminal for join purposes. The two decisions must not be taken
independently.

Transaction ordering remains a storage question for the families that make
inclusion timing-independent. For the declared non-deterministic family no
ordering makes it repeatable, and none is required to.

### 11.3 Mechanical group outcome

A simple proposed aggregation for required results is:

- `fail` if any required result is `fail`;
- otherwise `inconclusive` if any required result is `inconclusive`;
- otherwise `pass` when all required results are present and `pass`.

Optional results can be supplied to a reducer or decision but do not implicitly
override the required aggregate. A semantic reducer may still make an assertion;
the observation surface must preserve whether the final conclusion was mechanical
or asserted.

It remains open whether aggregation is a universal built-in rule, a small set of
closed join policies, or a declared reducer contract. Arbitrary expressions are
outside the current direction.

The routing of `inconclusive` is not yet consistent across this document. Section
11.1 defines it as a usable reported result that is deliberately distinct from an
executor crash or a missing result, which implies that the natural responses are
retry, escalation, or a decision. The sketch in section 13 instead routes it
directly to terminal failure. A group outcome of `inconclusive` most often means
the evidence was not obtained rather than that the work was judged bad, so
terminal failure is a poor default and retry has no route in the sketch at all.
Either the aggregate must not have a terminal-failure default, or `inconclusive`
must be redefined; the current pair of statements cannot both stand.

### 11.4 Rework and terminal failure

A required failure should be able to follow a declared rework route to an earlier
outer placement. Rework must create a fresh attempt and make prior fan claims,
leases, bases, and receipts ineligible for the new attempt unless a future rule
explicitly preserves independently valid evidence.

[Rework and Lifecycle](../specs/domain/rework-and-lifecycle.md) currently defines
a Procedure v2 session as running, completed, cancelled, or absent. A `failed`
state is therefore an addition to a small closed set that every observation,
projection, receipt, and consumer already switches on.

The discussion favored adding a distinct `failed` session lifecycle for a
procedure that reaches a considered negative terminal outcome. Reusing
`cancelled` would confuse an assessed failure with an operator stopping the task.
Using `completed` with a nested outcome would reduce lifecycle expansion but make
success and failure less direct.

The candidate preference is therefore:

- `completed` means successful terminal completion;
- `failed` means declared terminal failure with reason and evidence;
- `cancelled` means externally stopped; and
- a failed session may eventually be eligible for explicit rework in the same way
  a completed session can be reactivated, while a cancelled session remains
  terminal.

This is not adopted. Goal-assessment readiness, terminal receipt shape, reactivation
rules, and compatibility consequences require a separate decision.

## 12. Integrated conceptual model

The improvement themes reinforce one another. Assurance typing appears here
because the combined picture is easier to read with it, but it is owned by the
adopted [Podway External Check Assurance Typing](TODO-podway-assurance-typing.md)
and does not depend on the rest of this diagram:

```text
canonical Procedure and immutable snapshot
                    |
                    v
       Podway derives a ready frontier
                    |
          claim / finite lease
                    |
                    v
     external executor performs the work
                    |
     assertion or basis-bound work receipt
                    |
                    v
       sole daemon writer records result
                    |
     deterministic join / rework / failure
```

Executor neutrality prevents Podway from competing with fast-moving runtimes.
Receipt typing gives formal gates something stronger than self-report without
pretending to prove semantic truth. Effect-aware fan groups introduce useful
parallelism while one daemon remains the authoritative writer and write work
remains explicitly ordered.

The central conceptual distinction is:

- **authority cursor:** one outer procedure placement owns the current task
  narrative;
- **execution frontier:** a bounded set of subordinate work units inside the
  active fan group may be claimable; and
- **mutation order:** all authoritative state changes remain totally ordered by
  the daemon for the worktree.

This distinction avoids treating "single writer" as "only one worker may do
anything" while still preventing multiple authoritative writes.

## 13. Illustrative authoring shape

The following fragment is intentionally non-contractual. It exists to make the
working concepts concrete and to expose unresolved schema questions. It must not
be copied into a Procedure or fixture as if it were supported.

```yaml
schema: podway.procedure/v3-candidate
id: review-and-repair
version: "1"
name: Review and repair
purpose: Inspect independent concerns, apply ordered changes, and verify the result.

graph:
  start: plan
  end: finish
  placements:
    - id: plan
      use: plan
      next: inspect

    - id: inspect
      type: fan
      group:
        start: group-input
        end: synthesize
        work:
          - id: group-input
            role: basis

          - id: inspect-area
            use: inspect-area
            effect: snapshot_read
            required: true
            basis_from: group-input
            map_over:
              node: plan
              item: areas
              mode: parallel

          - id: repair
            use: repair
            effect: workspace_write
            required: true
            needs: [inspect-area]
            overlap_reads: [independent-research]

          - id: independent-research
            use: independent-research
            effect: snapshot_read
            required: false
            basis_from: group-input

          - id: synthesize
            use: synthesize
            effect: snapshot_read
            required: true
            basis_from: repair
            needs: [repair, independent-research]
      outcomes:
        pass:
          effect: advance
          to: finish
        fail:
          effect: rework
          to: plan
        inconclusive:
          effect: finish_failed
          to: finish

    - id: finish
      type: end
```

Unresolved questions exposed by this sketch include:

- whether fan is a definition, placement, or both;
- whether basis publication is a visible work unit or group metadata;
- whether `needs` and `basis_from` are separate edges;
- how a mapped template supplies per-instance typed input;
- whether optional predecessors may appear in a required `needs` list. The
  sketch makes this concrete: `independent-research` is `required: false` while
  `synthesize` is `required: true` and names it in `needs`. Either the dependency
  makes the optional unit effectively required and contradicts its own
  declaration, or `synthesize` consumes whatever happened to arrive and feeds a
  timing-dependent input into the unit that determines the group outcome. This is
  the section 11.2 choice seen from the authoring side, not a separate question;
- how a single end represents success and failure;
- whether fan outcomes route directly or through a normal decision placement;
- whether reducer work must always be read-only;
- how rework targets interact with output bases and map instances;
- whether `finish_failed` is a third route effect, and how it relates to the
  `failed` session lifecycle proposed in section 11.4; and
- why the sketch offers no retry route for `inconclusive`.

The sketch also silently extends a released closed contract. The current
`route.effect` enumeration in
[`procedure-v2.schema.json`](../../assets/schemas/procedure-v2.schema.json) is
exactly `advance` and `rework`. `finish_failed` is therefore a third effect, not
a spelling choice, and it is the point where a route stops describing movement
within the graph and starts describing session termination. Whether those belong
in one enumeration is itself an open question.

These questions must be answered before a public schema is proposed.

## 14. Mermaid and authoring guidance

Podway already supports deterministic Mermaid projection through
`podway procedure graph <file> --format mermaid` and includes Mermaid in
`procedure preview`. The canonical Procedure YAML or JSON and its digest are the
authority. Mermaid is a generated review projection.

That rule should remain unchanged. Accepting Mermaid as an authoring or registration
source would create a second grammar, lose procedure metadata, and risk divergence
between a diagram and executable contracts.

If this candidate advances, the generated projection should make at least these
facts visible:

- fan-group boundary and unique start/end;
- fixed work and dynamic map templates;
- dependencies and reduce barriers;
- required versus optional work;
- `snapshot_read` versus `workspace_write` effects;
- the total order among write work units;
- whitelisted read/write overlap;
- input-basis lineage;
- pass, fail, inconclusive, rework, and terminal routes; and
- worst-case expansion or another boundedness summary.

The `use-podway` authoring guidance should eventually teach an LLM to:

1. classify tasks by dependency and effect before drawing lanes;
2. keep sequential work sequential;
3. identify the single owner of every write and merge;
4. use immutable inputs for parallel reads;
5. author canonical YAML or JSON;
6. run format, validate, vet, lint, check, and preview;
7. inspect the generated Mermaid for false independence or hidden ordering; and
8. start only from an explicitly reviewed digest.

No skill change is authorized by this candidate.

## 15. Compatibility and versioning direction

Procedure v2 is a closed, released, compatibility-sensitive contract.

The original exploration asserted breakage at bundle granularity: that adding fan
placements, effects, work claims, receipts, map expansion, joins, or a failed
lifecycle to v2 would be a breaking semantic and schema change. That statement is
not wrong for the bundle, but it is not useful, because the listed features do
not carry the same cost and do not all break the same contracts. Deciding them
together forces the cheapest change to inherit the compatibility consequences of
the most expensive one.

Each candidate feature needs its own verdict before a version decision is made:

| Feature | Contracts it touches | Breaking for existing v2 consumers? |
|---|---|---|
| Assurance kind on recorded evidence | Item contracts, recording route, observation results | Decided by `V2AST`: additive through a new `check_receipt` item type, with the Procedure schema remaining `podway.procedure/v2` |
| Mechanical receipt record | Existing `item.record_many` route, SQLite v5 item discriminator | Decided by `V2AST`: additive, with no separate result family and no second receipt table or lifecycle |
| Work claim, lease, and report routes | New routes, new result families, daemon behavior | Additive at the protocol level; no v2 document changes |
| Fan placement and effects | Procedure schema topology, vetting, cursor semantics | Breaking; changes what a placement is |
| Dynamic map expansion | Procedure schema, vetting, resource budgets, storage | Breaking; changes admission-time boundedness proofs |
| Join and group outcome | Cursor semantics, transitions, terminal derivation | Breaking; changes when and how the cursor moves |
| `failed` session lifecycle | Session status set, observation, receipts, consumers | Breaking; extends a closed enumeration |

The first two rows are no longer open. They are owned by the adopted dossier
[Podway External Check Assurance Typing](TODO-podway-assurance-typing.md), which
settled them additively without requiring a procedure generation. The remaining
rows record the shape of the question rather than answers, and filling them in is
prerequisite work.

For whatever subset does require a new procedure version, three transition
policies remain open. This document prefers none of them:

1. **Direct cutover.** The successor replaces v2 as the only supported model in
   one release, on the pattern ADR-0019 used to remove Procedure v1.
2. **Bounded coexistence followed by cutover.** Both models are supported for a
   declared migration window with a stated end, after which the successor becomes
   the only supported model.
3. **Indefinite coexistence.** Both models remain supported with no planned
   removal.

Coexistence is not a neutral default in this repository. Podway 0.2.0 shipped
Procedure v2 additively beside the released linear Procedure v1 model, and
[ADR-0019](../architecture-decision-records/0019-procedure-v2-only-product.md)
then removed that arrangement because keeping both models duplicated parsing,
domain, persistence, protocol, runtime, preset, documentation, and conformance
paths. The `V2CUT` epic shipped that removal in v0.2.1. Policies 2 and 3
therefore propose returning to an arrangement this project built, operated, and
deliberately dismantled, and the cost recorded then is evidence about the cost it
would incur again.

The repository's only worked transition precedent is direct cutover with
fail-closed legacy rejection: no automatic conversion, no lossy semantic
conversion, `LEGACY_PROCEDURE_STATE_UNSUPPORTED` when legacy state is opened, and
recovery through an explicit confirmed `reset --all`. That precedent was set for
removing v1 rather than for introducing a third model, so it disqualifies
coexistence as a stated preference without establishing cutover as one.

Each policy carries an exact ADR obligation, and none may be adopted by
implication. Policy 1 requires an accepted ADR extending ADR-0019's precedent to
the successor. Policies 2 and 3 require an accepted ADR that supersedes or amends
ADR-0019's decision that Podway has one supported authoring and runtime model.

Either coexistence policy would additionally require that:

- existing v2 documents, snapshots, sessions, results, and error semantics remain
  unchanged;
- no active v2 session is automatically converted;
- authoring dispatch selects behavior from the explicit schema identifier;
- successor success envelopes and observation results receive new schema
  discriminators;
- storage migration retains v2 state and adds new state rather than rewriting v2
  history; and
- replacement between versions remains an explicit lifecycle operation.

`podway.procedure/v3` is a convenient working name, not an accepted identifier.
The eventual design must decide which transition policy applies, whether the
change is large enough to justify a full procedure generation, and what migration
window, if any, a coexistence policy would declare.

## 16. Working decision ledger

Everything in this section remains subordinate to Candidate status. "Preferred"
means the current discussion favored the direction; it does not mean the product
has adopted it.

### 16.1 Preferred directions

| Topic | Current preferred direction | Reason |
|---|---|---|
| Product role | Durable graph authority, not executor | Preserves a narrow product and composes with mature runtimes |
| Integration | Versioned CLI/JSON with thin adapters | Keeps one generic public surface and avoids provider coupling |
| Assurance | Not a direction of this candidate; decided by the adopted [Podway External Check Assurance Typing](TODO-podway-assurance-typing.md) | Listed only as a pointer, because a Candidate holds no preferred direction over adopted authority |
| Trust | Structural and freshness assurance only | Matches the same-user trust model and avoids false security claims |
| Lease expiry | Lazy evaluation inside the state transaction that next touches the unit, with a daemon-recorded instant, rather than a timer | Keeps the property that no domain state changes without a request, fixes one explicit evaluation point instead of two, and makes replay stable through the stored outcome |
| Snapshot | Executor-owned immutable snapshot descriptor | Keeps snapshot bytes and execution infrastructure outside Podway |
| Parallel topology | Composite fan group under one outer cursor | Limits concurrency semantics and preserves the task narrative |
| Entry and exit | One start and one end for root and fan group | Keeps diagrams and lifecycle convergence reviewable |
| Effects | Snapshot read and workspace write | Makes concurrency safety explicit without arbitrary capabilities |
| Write concurrency | One workspace-write lease | Preserves the user's strong single-writer principle |
| Write order | Explicit dependency reachability between every write pair | Avoids hidden scheduler order |
| Read/write overlap | Fail-closed writer whitelist for independent snapshot reads | Missing declarations reduce concurrency instead of corrupting meaning |
| Map source | Immutable upstream typed list item | Makes dynamic expansion deterministic and bounded |
| Join | Explicit mutation; worker reports never auto-advance | Preserves existing authority. The claim that the optional cutoff is deterministic is withdrawn; see section 11.2 |
| Result semantics | Pass, fail, and inconclusive are usable results | Allows full diagnosis and reduction rather than conflating failure with crash |
| Terminal semantics | Distinct failed lifecycle | Separates assessed failure from cancellation and successful completion |
| Procedure compatibility | Open: direct cutover, bounded coexistence then cutover, or indefinite coexistence, with none preferred | ADR-0019 accepted one supported authoring and runtime model, so every option requires a named superseding, amending, or extending ADR |
| Visualization | Generate Mermaid from canonical Procedure | Prevents a second source of truth |
| Parallel width | No semantic lane cap | Lets dependency shape and executor capacity govern concurrency |

### 16.2 Rejected initial directions

| Direction | Reason for rejection |
|---|---|
| Podway executes verification commands | Competes with executors and reverses the command-runner non-goal |
| Podway invokes models or owns agent loops | Turns the guard into an AI runtime and duplicates framework responsibilities |
| Lease expiry bound to workspace sequence or session revision instead of time | Clock-free and maximally architecture-preserving, but a dead executor issues no further requests so its lease would never expire, while unrelated activity in the same workspace would expire a healthy one |
| Signed executors in the first design | Introduces key management and a security boundary before structural receipts are proven useful |
| Live-worktree reads during writes | Produces non-repeatable observations of partial state |
| Clean Git commit as the only fan input | Too restrictive for in-progress software work |
| Podway snapshots and hashes every dirty worktree | Pulls filesystem storage, race, and lifecycle complexity into the authority layer |
| Blacklist for unsafe read/write overlap | Fails open when an entry is omitted |
| Implicit write priority or scheduler ordering | Hides semantic order outside graph edges |
| Runtime-chosen write order | Makes replay and outcomes claim-race dependent |
| Fixed maximum of three lanes | Confuses an authoring heuristic with product semantics |
| Unbounded dynamic map or executor-created graph | Breaks boundedness and moves graph authority to the executor |
| Filesystem glob expressions in Procedure | Adds executable or environment-dependent interpretation |
| Global multi-frontier as the first parallel model | Requires a broad redesign of almost every v2 invariant |
| Mermaid as authoritative input | Creates a second incomplete grammar and conflicting truth |
| Breaking Procedure v2 in place | Violates released closed-contract expectations |
| Reusing cancellation for assessed failure | Conflates an outcome with an operator lifecycle action |

### 16.3 Material open decisions

One decision precedes all of the following and is not merely first in a list:
whether the demand established in section 3 and tested by the precondition
experiment in section 20.1 is real. A negative result does not reorder the list
below; it removes it. The numbered decisions are the ones that would matter if
that question is answered affirmatively.

Three decisions previously listed here have been removed because the adopted
[Podway External Check Assurance Typing](TODO-podway-assurance-typing.md) decided
them: the exact receipt schema, canonical digest, and size bounds; whether an
operation definition carries only an opaque identifier and digest; and whether
human approval is a third assurance class.

The following issues prevent wholesale promotion of this Candidate. They do not
all gate a validated subset extracted under condition 1 of section 21, which is
scoped per shape:

1. Exact Procedure successor grammar and normalized domain types.
2. Whether fan is a node definition, placement, composite subprocedure, or a
   distinct graph construct.
3. Exact unique-start and unique-end semantics, including how the end records
   success versus failure.
4. Whether fan group nesting is prohibited, supported once, or recursively
   bounded.
5. The complete work-unit state machine and distinction between lease generation,
   execution attempt, retry, and parent attempt.
6. Whether lease expiry can be evaluated lazily inside the state transaction that
   next touches the work unit, as section 8.3 proposes, thereby preserving the
   property that no domain state changes without a request. The open parts are
   divergence between admission and execution, replay stability, whether a
   read-only observation may report an expiry it does not commit, retention of
   expired lease rows, and backward system-clock movement. Only if lazy
   evaluation cannot hold: lease durations, renewal limits, starvation
   prevention, timer ownership, expiry recovery, and daemon restart
   reconciliation.
7. Snapshot descriptor schema, resolver ownership, creation protocol, cleanup,
   portability, and treatment of dirty or untracked files.
8. Whether every write must publish a new basis, and how a no-op write is
   represented.
9. The authoring and runtime meaning of `basis_from`, dependency edges, and
   result data flow.
10. Dynamic-map empty-input semantics, duplicate values, instance identities,
    ordering, and worst-case budgeting.
11. Whether ordered write maps are allowed in the first version or deferred.
12. Exact required/optional semantics. This is a semantic contradiction before it
    is a transaction race: the current rule makes an optional unit's inclusion
    depend on report timing while simultaneously claiming timing must not decide
    it, which is the same non-repeatability rejected for write ordering.
    Serialization already provides inspectability of the accepted and abandoned
    partition; it cannot provide repeatability, and a two-step close is
    bookkeeping rather than a deterministic construct. The decision is a choice
    among the four families in section 11.2, namely completeness, irrelevance,
    elimination, or declared non-determinism, and not a transaction-ordering
    question. Choosing completeness additionally constrains decision 6 above,
    because time-based lease expiry would become a path to terminal state and
    would reintroduce the timing dependence that family exists to remove.
13. Whether join aggregation is fixed, selected from closed policies, or delegated
    to a typed reducer.
14. How semantic reducer assertions and mechanical outcomes compose.
15. Exact pass, fail, inconclusive, rework, and terminal routing rules.
16. Failed-session goal readiness, reactivation, and durable receipt semantics.
17. Rework invalidation for fan instances, receipts, output bases, and optional
    results.
18. Resource bounds for work units, active leases, result collections, map
    expansion, history retention, and projection size.
19. The public CLI command grammar and JSON result versions.
20. Whether the canonical observation command is extended or a separate work
    observation is introduced.
21. Which adapters, if any, Podway ships versus documenting only a conformance
    protocol.
22. Whether an MCP facade belongs in this repository, a plugin, or an independent
    package.
23. Storage version, table ownership, migration rollback, and preservation of
    active v2 state.
24. Threat analysis for malicious or merely buggy same-user executors.
25. Performance targets and evidence required to show that claim and join
    concurrency remain bounded and recoverable.
26. Which transition policy in section 15 applies, namely direct cutover, bounded
    coexistence followed by cutover, or indefinite coexistence, and which
    superseding, amending, or extending ADR each would require; then the release
    target, the supported migration window if any, and whether the successor
    becomes the sole product model.

## 17. Rough candidate scope

This inventory is the maximum possible extent of the work, not an adoptable unit.
It is a scope inventory rather than an implementation order or task breakdown,
and its breadth is what the release-program scale in the status block describes.
Actual adopted scope is determined per shape by the evidence-to-scope rule in
section 20.1 and is bounded by condition 1 of section 21, which bars wholesale
promotion while material shapes remain unexercised.

Assurance and receipt items appear below only where a fan group changes them.
The base assurance work is owned by the adopted
[Podway External Check Assurance Typing](TODO-podway-assurance-typing.md) and
must not be counted twice.

- **Architecture authority:** new ADRs for executor neutrality and receipts,
  constrained parallel fan groups, and failure lifecycle. Fan groups require an
  accepted ADR superseding ADR-0017, which explicitly rejects synchronizing
  joins, multiple active tokens, and parallel node attempts. A coexistence
  transition policy requires an accepted ADR superseding or amending ADR-0019's
  one-model decision, while direct cutover requires an accepted ADR extending its
  precedent to the successor. ADR-0010 and ADR-0016 require explicit treatment
  where the integration surface and the recorded-evidence model change.
- **Core domain:** fan-group topology, work units, effects, immutable bases,
  receipt assurance, join outcome, and failure state.
- **Configuration:** successor schema parsing, semantic validation,
  canonicalization, digests, graph vetting, resource budgets, and deterministic
  projections.
- **Protocol:** closed observation, claim, lease, report, join, failure, job result,
  and error contracts.
- **Storage:** successor snapshots and sessions, fan work units, dependencies,
  leases, bases, receipts, optional closure, joins, and migration integrity.
- **Daemon:** ready-frontier derivation, claim arbitration, writer exclusion,
  lease recovery, report admission, join execution, queue scheduling, and
  observability.
- **CLI:** human and JSON work inspection, mutation commands, help, completion,
  rendering, and response-loss recovery.
- **Authoring experience:** scaffold, validate, vet, lint, preview, Mermaid and
  other projections, examples, and authoring skill guidance.
- **Qualification:** concurrency, crash, stale receipt, response loss, migration,
  compatibility, fuzzing, and packaged runtime scenarios appropriate to the
  eventual release claim.

No implementation dependency order is established by this Candidate.

## 18. Non-goals for the candidate direction

Unless a later accepted decision says otherwise, the exploration does not aim to
make Podway:

- an LLM runtime or model router;
- a shell, build, CI, or arbitrary command executor;
- a Git mutation or merge engine;
- a sandbox or worktree manager;
- a remote collaboration server;
- a provider-specific agent framework;
- an arbitrary expression, scripting, hook, or plugin host;
- an unbounded dynamically generated workflow scheduler;
- an artifact-byte or transcript archive;
- a cryptographic identity or same-user security boundary;
- a semantic judge of agent conclusions;
- a general project manager spanning multiple tasks;
- a replacement for LangGraph, AutoGen, Strands, OpenAI Agents SDK, Taskflow, or
  other execution frameworks; or
- a guarantee that more parallel agents improve quality, cost, or latency.

## 19. Risks and failure modes

### 19.1 Product-boundary risks

- Work coordination may gradually acquire execution hooks and turn Podway into a
  workflow engine despite the stated boundary.
- Adapter conveniences may leak provider-specific concepts into public contracts.
- Snapshot descriptors may become de facto artifact storage or remote resource
  management.
- A Procedure successor may become too complex for humans to author or review.

### 19.2 Correctness risks

- A false read-only declaration can permit an executor to mutate while another
  writer is active.
- An incomplete write-order proof can introduce claim-race-dependent results.
- A stale or ambiguous snapshot basis can make receipts appear fresher than the
  work they verify.
- Optional-result and join races can make repeated runs differ. Serializing the
  join, including as a two-step close, does not resolve this: it makes the cutoff
  inspectable without making inclusion repeatable. See section 11.2.
- Dynamic expansion can exceed resource bounds after a list changes.
- Rework may accidentally preserve results that depend on invalidated writes.
- Lease expiry can allow duplicate external effects even when Podway records only
  one accepted result.

### 19.3 Assurance risks

- Users may interpret a structured receipt as proof that Podway executed or
  independently verified the check.
- An agent may wrap a semantic judgment in mechanical-looking JSON.
- The same-user trust model permits forgery by local processes.
- A mechanically passing check may be insufficient, weakened, or scoped to the
  wrong behavior.
- Output digests prove identity only if the snapshot and operation resolvers are
  trustworthy.

### 19.4 Performance and usability risks

- Claim polling and lease renewal may create queue pressure or noisy state.
- Too many independent workers may duplicate large context and cost more than one
  persistent agent.
- A writer can starve behind a continuous stream of allowed or incompatible reads.
- A strict whitelist may serialize graphs that authors expected to be parallel.
- Mermaid projections may become unreadable for dynamic maps or nested groups.
- Keeping all historical work attempts and receipts may grow the local database
  beyond current assumptions.

## 20. Questions for experiments

### 20.1 Precondition experiment

One question ranks ahead of every other and should be answered before further
design effort is spent, because a negative or weak result removes the motivation
for sections 8 through 17: whether the gap described in section 3.3 survives a
comparator that includes a durable executor.

#### Preregistration

Before the first run, and not revised after any result is seen, record the task
selection, the shapes it covers, and the classification of every measure as
gating or descriptive. Tasks are drawn from this repository's completed work
rather than invented for the experiment, and each must be expressible as a
Procedure the author would actually run from a shipped preset or a small authored
procedure. No task count is prescribed. The set must cover every shape whose
mechanisms the experiment intends to license.

#### Arms

A single-arm trial cannot answer the question. Comparing the shipped path against
a stateless fan-out alone would show at most that a stateless fan-out loses
state. Three arms are used, and they are run per shape rather than once:

- **Arm A** is the shipped path described in section 3, driven by an executor
  that keeps no durable coordination state.
- **Arm B** is the same path with a durable executor of the checkpoint, replay,
  and resume class listed in section 7.2, where the executor owns coordination.
- **Arm C** is arm B with the minimum coordination metadata for the shape under
  test additionally recorded as ordinary items through `podway record --stdin`:
  the unit inventory, the claims, and the executor run identity in every shape;
  the declared effect of each unit in S2 and S3; the immutable input basis in S3;
  and the expansion source in S4. It approximates fan-group authority using
  shipped code only, and is the cheapest arm, so it should be run first.

Arm B must name the executor and its version, and should cover two executors of
different families. Otherwise its result is scoped to the one executor tested,
consistent with the snapshot rule stated in section 7.2.

#### Work shapes

Arms are run over each shape the preregistration covers:

- **S1 read-only fan-out:** every unit is a snapshot read.
- **S2 write-bearing fan-out:** at least one unit mutates the owning worktree.
- **S3 overlap:** a read must observe a fixed input while a write proceeds.
- **S4 expansion:** the unit count derives from an upstream recorded list.
- **S5 outcome combination:** units produce pass, fail, or inconclusive results
  that must combine into one group outcome.
- **S6 failure and return:** a required unit fails and work must return to an
  earlier placement.

S1 through S4 are constructible today with shipped code, S2 and S3 by having one
unit mutate a temporary worktree while others read it. Whether S5 and S6 can be
exercised without first building a join mechanism is unresolved. If they cannot,
they remain unexercised and the mechanisms they govern cannot be promoted on this
experiment's evidence.

#### Faults

The same faults are injected in every arm and every shape: an executor killed
before any result is recorded; an executor killed after some units finish but
before recording; a lost response on the record mutation; two executors started
against one task; and an executor replaced, or a human substituted for one unit,
on resume.

#### Gating invariants

These are binary and observable within the recorded state of the system under
test, and a failure is itself the finding. Podway does not observe external
processes or the filesystem, so no invariant below asserts anything about what an
executor actually did. Each is evaluated under each injected fault, in every
shape:

- no work unit had two concurrent owners, meaning no two live claims on one unit
  coexisted at any instant;
- at most one result was accepted per work attempt and lease generation;
- no unit was reissued after an accepted terminal result;
- the system under test had an authoritative answer for what work remained after
  the fault, whichever component owned that answer;
- resume did not redo units whose results had already been accepted; and
- recovery is achievable using only documented read-only commands and declared
  mutations.

Additionally in S2 and S3:

- no two workspace-write leases coexisted at any instant; and
- every accepted result bound to the immutable input basis its unit declared, and
  no incompatible lease was issued for the duration of that binding.

Two of these are deliberately weaker than they might appear. Duplicate external
execution is not gated, because Podway cannot prevent or observe it; section 19.2
records it as a standing risk, and gating on it would make the experiment
unrunnable. The remaining-work invariant is functional rather than
Podway-specific: an executor that can authoritatively answer the question
satisfies it. Requiring Podway to be the answering authority would make arm B
fail by construction and would decide the comparison in advance.

#### Descriptive metrics

Recorded but never gating: which component owned the authoritative remaining-work
answer in each arm; whether duplicate external execution was detected, which
Podway cannot prevent; operator steps to recover; context duplication; model
cost; merge work; and wall clock. A descriptive metric may narrow or order scope
after a gating invariant has already admitted a mechanism, but may never admit
one on its own. No threshold may be introduced after data collection.

Which component owned the remaining-work answer is the datum that separates the
arms once more than one of them satisfies the functional invariant. The
substitution fault is what prices it: an answer that does not survive executor or
human substitution is executor-local, and that cost appears in the substitution
result rather than in a failed gate.

#### Evidence-to-scope rule

Each mechanism is evaluated against the shape that governs it, and the result is
one of three states:

- **Motivated.** The shape was exercised and at least one gating invariant failed
  in arm C. Scope is limited to the specific invariant that failed, not to the
  whole section containing it.
- **Removed.** The shape was exercised and every gating invariant held in arm B
  or arm C. The mechanism leaves scope.
- **Unexercised.** The shape was not exercised. The mechanism is neither
  motivated nor removed, and no result obtained in another shape may be
  transferred to it.

The arm rules are per shape. Within one shape, an arm A that satisfies every
gating invariant means that shape has no gap. An arm B that satisfies every
gating invariant settles that shape in favour of composing with a durable
executor, and says nothing about any other shape.

| Shape | Mechanisms it can license |
|---|---|
| S1 | Executor-neutral unit inventory, claims, and readiness for reads: section 8.2 and the read half of section 10.2 |
| S2 | Workspace-write lease 10.1, effects 10.3, write ordering 10.4 |
| S3 | Read and write overlap whitelist 10.5, snapshot ownership 10.6 |
| S4 | Dynamic map expansion 10.7; ordered write maps require S2 and S4 together |
| S5 | Join and group outcome 11.1 through 11.3 |
| S6 | Rework and terminal failure 11.4 |

The rule applies to mechanisms whose absence produces an observable fault.
Section 10.4 states a static vetting rule, so its evidence is indirect: S2
producing order-dependent outcomes is what motivates it. Section 10.8 records a
non-decision and needs no evidence.

This experiment needs no new Podway code for S1 through S4. It is the cheapest
question in this document and the one the rest of the document depends on.

### 20.2 Design experiments

If the precondition experiment establishes a real gap, small disposable
prototypes or model-level experiments should answer questions such as:

- Can a generic fake executor consume only versioned CLI/JSON and complete a
  receipt-bound sequential work unit without Podway knowing the executor type?
- Can concurrent processes race to claim one unit without duplicate ownership?
- Does lease expiry and reclaim remain deterministic across daemon restart and
  response loss?
- Can the lazy expiry described in section 8.3 satisfy every claim, renewal, and
  join requirement without introducing a timer or any unrequested state change,
  and do replay, read-only observation, and arbitrary queue delay between
  admission and execution all remain correct under it?
- Can a model checker or property test prove that no two write leases coexist?
- Can it also prove the symmetric read/write whitelist rule under every claim and
  expiry ordering?
- Can map instance identities remain stable across equivalent YAML and JSON,
  daemon restart, and idempotent admission?
- What is the smallest snapshot descriptor that two independent executors can
  resolve without Podway storing bytes?
- Can dirty-worktree snapshots be made immutable by an executor without requiring
  Git mutation in Podway?
- Can required, optional, fail, inconclusive, and join races be specified with a
  small state machine and exhaustively tested?
- Can rework invalidate exactly the dependent fan suffix without introducing an
  event-sourced architecture?
- Does a generated Mermaid graph allow a reviewer to identify all write order,
  snapshot lineage, and join conditions without reading raw JSON?
- Which external runtimes can implement a thin adapter without special cases?
- Can the assurance language prevent users from mistaking an external receipt for
  Podway-certified truth?

Experiments must use isolated fixtures or temporary worktrees. They must not
modify a user's active Podway runtime, installed daemon, Git state, or LaunchAgent.

## 21. Roadmap promotion conditions

This Candidate may be promoted only when all of the following are true:

1. The product problem and user workflows are written with concrete examples that
   cannot be adequately represented by Procedure v2 plus external orchestration.
   Section 3 is the current attempt at that comparison and concludes that the gap
   is executor-neutral authority over in-flight work rather than parallelism.
   This condition is met only when the precondition experiment in section 20.1
   has been run under a preregistration record that exists and was not revised
   after any result was seen, and the evidence-to-scope rule has been applied per
   shape. Within each shape, arm B must fail at least one gating invariant, since
   an arm B that satisfies every gating invariant settles that shape in favour of
   composing with a durable executor. Adopted scope must name, for every admitted
   mechanism, the shape and the gating invariant that admitted it, and must record
   which component owned the authoritative remaining-work answer in each arm,
   since that invariant is functional and an executor may satisfy it. Mechanisms
   whose shape was exercised and whose gating invariants held are removed from
   scope rather than carried forward. A failure confined to arm A establishes
   only that a stateless fan-out loses state and does not satisfy this condition,
   and neither does this document's reasoning.

   An unexercised shape cannot enter adopted scope. Because
   [TODO and Adopted Design Dossiers](README.md) requires an adopted dossier to be
   decision-complete for a closed set of work, this Candidate cannot be promoted
   wholesale while material shapes remain unexercised. Either the validated subset
   is extracted into its own adopted dossier with its own owning epic, or this
   Candidate is first narrowed to that subset while the unexercised shapes remain
   in Candidate state pending their own experiment.
2. The composite fan-group boundary is confirmed or replaced by a better bounded
   topology with explicit reasoning.
3. The work-unit, lease, basis, receipt, join, rework, and failure state machines
   are decision-complete.
4. Assertion and mechanical receipt terminology states exactly what Podway does
   and does not guarantee.
5. Read/write effect rules and total write ordering have a static validation
   strategy and runtime enforcement strategy.
6. Dynamic map expansion and resource budgets are closed and demonstrably bounded.
7. Snapshot ownership and interoperability are proven feasible without violating
   Podway's non-goals.
8. The transition policy is chosen from section 15, the superseding, amending, or
   extending ADR it requires is named, and Procedure v2 compatibility and storage
   migration behavior are explicit.
9. Public CLI/JSON interface sketches are closed enough to assign schema versions
   and error semantics.
10. At least one executor-neutral integration prototype validates the separation
    of authority and execution.
11. Failure, crash, response-loss, stale-result, and rework acceptance scenarios
    are enumerated.
12. Every change to an accepted decision is proposed as an explicit superseding or
    amending ADR and reviewed as one, rather than adopted by implication. No
    accepted decision is altered silently.
13. An adopted dossier defines implementation scope and the active roadmap
    registers owning epic or release-program tasks.
14. The target release and complete development or distribution gate are selected
    according to repository policy.

Promotion should turn the resolved parts of this document into accepted ADRs,
canonical schemas, specifications, executable contracts, and roadmap-owned work.
This candidate should not remain a competing normative specification after that
promotion.

## 22. Repository references

Current authority and implementation context:

- [Contributor Documentation](../README.md)
- [TODO and Adopted Design Dossiers](README.md)
- [Podway External Check Assurance Typing](TODO-podway-assurance-typing.md)
- [ADR-0001: Focus on One Current Task Session](../architecture-decision-records/0001-current-task-session-focus.md)
- [ADR-0002: Keep One Active Stage](../architecture-decision-records/0002-single-active-stage.md)
- [ADR-0003: Use the Daemon as the Sole Normal Writer](../architecture-decision-records/0003-daemon-single-writer.md)
- [ADR-0006: Use a Same-User Local Trust Boundary](../architecture-decision-records/0006-same-user-local-trust.md)
- [ADR-0007: Keep Typed Stage Items Instead of a General Evidence Ledger](../architecture-decision-records/0007-stage-items-not-evidence-ledger.md)
- [ADR-0009: Store Artifact Metadata, Not Artifact Bytes](../architecture-decision-records/0009-artifact-metadata-only.md)
- [ADR-0010: Integrate External Tools Through Generic CLI and JSON](../architecture-decision-records/0010-generic-cli-json-integration.md)
- [ADR-0015: Permit a Constrained Single-Cursor Graph](../architecture-decision-records/0015-constrained-single-cursor-graph.md)
- [ADR-0016: Use Recorded Items for Workflow Memory](../architecture-decision-records/0016-recorded-item-workflow-memory.md)
- [ADR-0017: Permit Single-Cursor Convergence](../architecture-decision-records/0017-single-cursor-convergence.md)
- [ADR-0019: Make Procedure v2 the Only Product Model](../architecture-decision-records/0019-procedure-v2-only-product.md)
- [Goals and Non-Goals](../specs/product/goals-and-non-goals.md)
- [Terminology and Invariants](../specs/product/terminology-and-invariants.md)
- [Domain Model](../specs/domain/domain-model.md)
- [Procedure and Item Specification](../specs/domain/procedure-and-item-specification.md)
- [State Transitions](../specs/domain/state-transitions.md)
- [Rework and Lifecycle](../specs/domain/rework-and-lifecycle.md)
- [Automation Client Contract](../specs/interfaces/automation-client-contract.md)
- [Transactions, Concurrency, and Idempotency](../specs/storage/transactions-concurrency-and-idempotency.md)
- [SQLite Model](../specs/storage/sqlite-model.md)
- [System Architecture](../architecture/system.md)
- [Daemon and Write Queue](../architecture/daemon-and-write-queue.md)

Canonical assets relevant to any future promotion:

- [`procedure-v2.schema.json`](../../assets/schemas/procedure-v2.schema.json)
- [`sqlite-v3.sql`](../../assets/specifications/sqlite-v3.sql)
- [`sqlite-v4.sql`](../../assets/specifications/sqlite-v4.sql)
- [`job-result-v3.schema.json`](../../assets/schemas/job-result-v3.schema.json)
- [`procedure-graph-result-v1.schema.json`](../../assets/schemas/procedure-graph-result-v1.schema.json)
- [`procedure-preview-result-v1.schema.json`](../../assets/schemas/procedure-preview-result-v1.schema.json)

## 23. External references

Research and conceptual framing:

- Wei Hu, [*From Agent Loops to Structured Graphs: A Scheduler-Theoretic
  Framework for LLM Agent Execution*](https://arxiv.org/abs/2604.11378), arXiv,
  submitted April 13, 2026. Position paper and design proposal; no production
  implementation or empirical results are claimed by the paper.
- Yubin Kim et al., [*Towards a Science of Scaling Agent Systems*](https://arxiv.org/abs/2512.08296),
  arXiv. Used here for the distinction between decomposable parallel work and
  sequential work and for coordination risk, not as proof that one topology is
  universally optimal.
- *Agent Harness Engineering: A Survey*, [OpenReview
  preprint](https://openreview.net/pdf/f358711a95aaaf61fdeffd4ef3fc60fba9b8da57.pdf).
  Used as context for the broader harness and orchestration category.

Durable graph authorities and graph-engineering tools:

- [Graphene](https://github.com/4tyone/graphene), a durable work-graph engine that
  tracks readiness, claims, typed outputs, humans, beliefs, and leases without
  executing models or nodes.
- [AI Taskflow](https://github.com/heggria/taskflow), a multi-agent DAG compiler
  and runtime with agent, map, reduce, gate, resume, replay, and incremental
  recomputation concepts. This is distinct from the C++ parallel programming
  project with the same name.
- [Graph Skill](https://github.com/gwaghmar/graph), a host-neutral dependency
  graph skill and local runtime with caching, validation, parallel nodes, and
  selective retry.
- [Graph Engineering plugin](https://github.com/ayaangazali/graph-engineering), a
  Claude Code workflow discipline using typed JSON handoffs, fan-out, isolated
  worktrees, and adversarial verification.
- [headsign](https://github.com/meganemura/headsign), a small phase gate that uses
  shell check outcomes to route advance, retry, escalation, or completion.
- [Graph Engineering Architectures](https://github.com/Mark393295827/graph-engineering-architectures),
  a recent collection of bounded static DAG contracts and validators. It is
  contextual evidence, not an implementation dependency.

Executor and orchestration frameworks:

- LangChain, [LangGraph graph and reducer documentation](https://langchain-ai.github.io/langgraph/how-tos/state-reducers/)
  and [persistence documentation](https://langchain-ai.github.io/langgraph/concepts/time-travel/).
- Microsoft, [AutoGen GraphFlow documentation](https://microsoft.github.io/autogen/stable/user-guide/agentchat-user-guide/graph-flow.html).
  The documentation labels GraphFlow experimental as of the research baseline.
- AWS, [Strands Graph multi-agent pattern](https://d3ehv1nix5p99z.cloudfront.net/pr-cms-2751/docs/user-guide/concepts/multi-agent/graph/).
- OpenAI, [Agents SDK orchestration](https://openai.github.io/openai-agents-js/guides/multi-agent/)
  and [Agents SDK overview](https://openai.github.io/openai-agents-python/).
- OpenAI, [*The next evolution of the Agents SDK*](https://openai.com/index/the-next-evolution-of-the-agents-sdk/),
  describing sandbox separation, snapshotting, durable execution, and parallel
  work across isolated environments.

All external references were last reviewed for this candidate on August 17,
2026. Their APIs, maturity, and positioning must be checked again before roadmap
promotion or compatibility claims.
