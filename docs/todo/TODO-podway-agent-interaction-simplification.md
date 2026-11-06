# Agent Interaction Simplification

## 1. Status and authority

- Document state: `Adopted`
- Adoption date: 2026-11-06
- Owning roadmap epic: AGENT
- Target product release: unassigned
- Repository scope: Podway only
- Source baseline: `98b8de7da9ac086ea0839f5e62a29dd88dc874ae`

The owner approved this planning registration. Implementation has not started.
The [roadmap](../roadmap/README.md#agent-agent-interaction-simplification) alone
owns task order and status. [ADR-0043](../architecture-decision-records/0043-atomic-assessment-and-commit-bound-observations.md)
accepts the future contract design; current executable assets and specifications
continue to describe implemented behavior until the corresponding tasks deliver it.
This dossier is self-contained and does not depend on an untracked proposal.

## 2. Verified context and evidence

At the source baseline, mutations return command-specific Output v4 results;
the runtime skill requires observation after mutations. `goal assess-criterion`
records one assessment. Core's
`criterion_mutation_is_cursor_stable_and_normalizes_reverse_call_order` regression
pins cursor stability, declaration-order read-back and already-assessed refusal.
The transaction owner persists success and state atomically and replays the
original response. ADR-0039 excludes generation selection from persisted identity.
These are source-inspection facts, not newly executed behavioral evidence.

Source owners are Core graph/goal state, Protocol slice/result contracts, CLI
command grammar/preflight, Engine direct execution and `v2_read_service.rs`, and
Store direct transactions. [System architecture](../architecture/system.md) and
[repository structure](../architecture/repository-structure.md) define ownership.

## 3. Goals, non-goals and scope

Reduce normal assessment and record/advance invocations using atomic assessment
and optional commit-bound observations. Three prepared criterion assessments plus
decision should take two normal CLI calls instead of eight; record/complete
should take two instead of four. These counts exclude setup, external work,
contention and recovery and do not claim latency or LLM-turn savings.

Keep one cursor and active attempt, existing command-specific fences/error order,
independent Store admission, exact replay and bounded persistence. Core owns
pure transitions; Protocol owns closed contracts; Engine owns preparation and
composition; CLI owns grammar/rendering; Store owns persistence. Config retains
Procedure parsing. No reversed dependencies, general command framework or broad
Core/Store decomposition is selected. No assess-and-decide or record-and-complete
operation, provider execution, external consumer change, preset-content revision,
installation or runtime activation is included. CANON and the entire TYPED
candidate are not technical prerequisites.

## 4. Accepted design decisions and interfaces

[ADR-0043](../architecture-decision-records/0043-atomic-assessment-and-commit-bound-observations.md#decision)
fixes the batch spelling/families/fields, 1-16-entry limit, 1 MiB/depth-64 input,
partial batches, order-sensitive identity, declaration-order output, common actor
and transaction time, response flag and identity, wrapper and fixed observation
v6, exact route variants, coherent projection, output/lookup headroom and caller
recovery rule. There are no deferred interface choices for implementation.

AGENT-001 implements the accepted contract foundation, not a new design phase.
If assets are reserved before handlers exist, use the existing reservation
pattern and keep unsupported routes/families out of advertised runtime capability.

## 5. Failure handling and compatibility boundaries

One invalid entry or precommit projection/admission failure rejects the entire
mutation without partial assessments, successful outcome, committed sequence,
receipt or journal entry. Preserve existing evidence freshness, error precedence
and filesystem preparation outside the writer. Exact replay precedes fresh
mutation admission at the existing authoritative boundary and never applies a
second assessment. New-key duplicate assessments remain rejections, not no-ops.

The observation wrapper is opt-in. Existing request identity and result shapes,
generation defaults, writer upgrade/refusal and retention rules remain preserved.
A fitting initial response must also fit its future lookup wrapper and nesting.
Replay is historical; a caller with uncertain first-submission history refreshes.
No predecessor, migration, evidence archive or global settings read is introduced.

## 6. Roadmap ownership, dependencies and traceability

Task dependencies and execution order live in the roadmap. The following task
requirements carry implementation and direct verification together. Documentation
is updated by each affected task; AGENT-006 integrates guidance and qualification
rather than postponing all documentation until the end.

### AGENT-001: Establish Interaction Contracts

**Why**

Batching and returned observations change public request/result contracts. Observation inclusion also changes persisted success bytes. These choices must be coherent before implementation spreads across CLI, Engine, Store, and Protocol.

**What and how**

- Record the agreed ADR and compatibility decisions from Section 4.
- Define the batch input/result and selected observation-bearing mutation results.
- Encode ADR-0043's response-mode identity, default output and same-key conflict rules.
- Encode ADR-0043's aggregate input/output bounds and recovery headroom.
- Preserve the existing result shapes and behavior for requests that do not select the new capability.
- Update the relevant schema registrations, command/result membership rules, catalogs, and manifest through existing workflows.
- If contracts are reserved before handlers exist, use the established reservation approach and prevent unsupported routes/families from being advertised as executable.

**Acceptance**

- The exact route set and all material request, replay, failure, and compatibility decisions are explicit.
- Existing generation-1 and generation-2 behavior remains defined and tested.
- No released closed family gains an optional field in place.
- Contract assets and generated identities remain coherent at every accepted task boundary.
- No external consumer or installed-runtime change is implied.

### AGENT-002: Share Stdin-Owned Fence Classification

**Why**

The new batch introduces another mutation whose input document should own its identity, revisions, and key. Existing record-many and delegation acceptance already contain related CLI classification/rejection logic.

**What and how**

- Implement DF-014, whose original details were transferred at adoption to [the selected-feedback entry](#selected-feedback-df-014).
- Give CLI one small owner for classifying stdin-fenced mutation routes and their conflicting argv flags.
- Reuse it in grammar validation, direct dispatch, and the relevant preflight path.
- Preserve the distinction between stdin-owned fences and ordinary argv-owned requests.
- Retain Protocol decoding and Engine/Store admission at their existing boundaries.
- Keep the abstraction local and specific; no general command framework is needed.

**Acceptance**

- Existing record-many and delegation acceptance continue to reject conflicting identity/revision/key flags.
- Preflight never replaces document fences with newly observed values.
- No broad bypass applies merely because an unrelated command accepts stdin.
- Existing parse, stdin, preflight, replay, and fence error precedence is preserved.
- The new route joins this classification only under its accepted contract.

### AGENT-003: Implement Atomic Criterion Assessment Batches

**Why**

A checkpoint with several criteria currently needs one mutation per criterion and a fresh observation between mutations. One atomic request can record the already-known assessments together.

**What and how**

- Parse and bound the request before expensive preparation or duplicate processing.
- Require one current session, active checkpoint attempt, and goal definition revision.
- Reject an empty batch and duplicate criterion IDs within the request.
- Preserve the single-assessment rules for nonexistent or already-assessed criteria, assessment mode, required reasons, citation count, source eligibility, and item revision freshness.
- Do not turn an identical assessment submitted under a new key into an implicit no-op. Preserve current already-assessed semantics; exact replay remains separate.
- Prepare reusable pure input outside the writer where current ownership allows it.
- Compute one unpublished assessment successor. Repeatedly calling the public single-assessment mutation and then repairing revision counters is not an acceptable implementation.
- Normalize assessment read-back consistently with criterion declaration order. Implement the separately agreed canonical request-order policy.
- Compute completeness and determined goal outcome from the final batch state, including any assessments already present.
- Persist all accepted entries, one success outcome, and the associated state changes atomically.

**Acceptance**

- A successful nonempty batch advances the session/workspace mutation state once, preserves the cursor, and preserves the goal definition revision.
- One invalid entry rejects the whole batch with no partial assessments or successful outcome.
- Partial batches are handled according to the agreed contract without falsely declaring assessment completeness.
- Mixed modes, stale fences, stale citations, and previously recorded criteria retain the intended errors.
- Exact retries return the original stored result without another assessment or sequence allocation.
- Existing single-criterion behavior remains unchanged.

**Primary implementation owners**

Core owns the batch transition and assessment invariants. Protocol owns the closed input/result and bounded decoding. Engine composes preparation and transactional execution. Store persists and independently admits the resulting state. CLI owns grammar and rendering.

### AGENT-004: Extract Coherent Observation Composition and Budget Reuse

**Why**

A returned observation must describe the actual successor state, including readiness and lifecycle changes. Reusing only a graph while retaining an old receipt cache would produce an inconsistent view.

**What and how**

- Extract only the projection/budget components needed by the feature from `v2_read_service.rs`.
- Supply a coherent successor context: graph, workflow memory, current attempt, goal assessments, receipt/readiness facts, workspace sequence, and required view inputs.
- Keep ordinary read projection and mutation projection on the same reusable logic where their semantics agree.
- Handle the actual resulting lifecycle: a new prepared session after start, an active attempt after begin, and changed cursor or terminal state after complete/decide.
- Keep private or narrowly visible module boundaries. Do not widen public APIs simply to split a file.
- Replace the remaining temporary output-buffer size measurements with `serialized_json_len_v1` where equivalent.
- Preserve existing entry overhead, saturating arithmetic, serialization-error treatment, collection windows, and truncation disclosures.
- Change object construction to a typed Map when naturally helpful. Do not replace invariant failures with empty or default successful output.

**Acceptance**

- Existing observation v5/v6 and related read outputs retain their behavior.
- Successor projections do not reuse facts owned by the previous attempt or snapshot.
- No second database acquisition or post-commit read is introduced to manufacture the promised observation.
- Existing Unicode/escaping, exact-byte, and boundary tests remain valid.
- The refactor has a direct feature purpose and does not expand into decomposition of all large Core/Store files.

### AGENT-005: Return Commit-Bound Observations

**Why**

After a successful mutation, the caller needs the new readiness and templates. Returning those facts with the committed result removes a separate observation on the normal path.

**Initial scope**

Use the contract-approved branches of start, begin, record-many, complete, decide, and the new assessment batch. This does not automatically include every start replacement, reset, archive, removal, delegation mutation, or other maintenance operation.

**What and how**

- Make observation return explicit and opt-in under the agreed request/result contract.
- After authoritative replay lookup and normal admission, compute the mutation successor and its observation in the same transaction context.
- Validate the complete result/envelope, depth, byte budget, and recovery headroom before commit.
- Persist the exact observation-bearing success with its domain changes and outcome.
- Do not commit first and run a separate public observe command afterward.
- On projection or budget rejection, roll back the mutation. Do not return an error after silently committing an effect.
- Preserve record-many no-op semantics: a successful outcome/sequence may be recorded without advancing session/item revisions.
- Keep existing retention policy and bounded storage assumptions explicit. Optional large observations do not authorize a new evidence archive.

**Replay and recovery**

- Replay returns the original stored observation, even if later mutations changed the current state.
- Outcome lookup returns the same original success inside its own response.
- Never recompute a historical outcome to make its observation look current.
- A response that fits initially must remain recoverable through outcome lookup within the public limit. Include its extra envelope, identity metadata, and nesting depth in the design.
- Keep the distinction between exact stored canonical success bytes and invocation-specific outer metadata or stdout serialization.
- A fresh success is an as-of-commit snapshot, not a lock against later writers. Subsequent mutations still use fences.

**Acceptance**

Cover successful prepared/running/transition/terminal projections, cap boundaries, rollback on projection failure, lost output after commit, replay after later advancement, and lookup recovery. Same-key/different-mode behavior must match the adopted identity policy.

### AGENT-006: Align Skills, Examples, and Integrated Workflows

**Why**

The current runtime skill correctly requires re-observation after every mutation. The new capability provides a conditional replacement for that read, not permission to use historical state as current readiness.

**What and how**

- Update the repository's `skills/use-podway` and relevant examples/specifications.
- Teach a normal-success path that consumes the admitted embedded observation.
- Teach a recovery path that refreshes after replay, outcome lookup, or unresolved freshness.
- Teach ADR-0043's caller-tracked first-submission rule without altering stored success bytes; uncertain submission history requires re-observation.
- Continue adding the caller's selected generation and JSON/worktree options consistently.
- Preserve declared template fences and distinguish identities, session/item revisions, and goal definition revision.
- Keep the existing observation path for unsupported commands, older contracts, contention, or incomplete information.
- Keep semantic work and evidence production outside Podway.
- Verify complete small-change and bug-fix paths through existing native CLI test infrastructure.

**Acceptance**

- The supported checkpoint and action paths achieve the structural counts in [the adopted goals](#3-goals-non-goals-and-scope).
- Tests also cover recovery and contention paths where extra observations are necessary.
- Documentation does not claim measured speedups, guaranteed LLM-turn savings, or one-call assess-and-decide.
- Existing functional owners and normal development gates provide the evidence.
- Repository skill changes do not imply installation into user-global environments.
- Keep optional preset-content changes separately scoped from the interaction contract.

### Selected feedback: DF-014

This is the single detailed owner transferred from TYPED at adoption. Promotion
closes only the deferral; implementation and acceptance belong to AGENT-002.

| ID | Source | Feedback | Deferral reason | Revisit condition |
|---|---|---|---|---|
| DF-014 | V2SDG validation maintainability review | `record --stdin` and `delegation accept` duplicate identity-flag rejection and use different CLI preflight paths for stdin-owned fences. | Both commands currently reject conflicting flags and preserve document-owned identity; the duplication is a future maintenance risk. | Before adding an identity flag or another stdin-fenced mutation, share its CLI classification and flag-rejection logic, then verify that preflight never replaces document fences and conflicting flags remain rejected. |

DF-025 and DF-028 remain with their existing owners; changing their boundaries
requires an explicit scope update before claiming their promotion. DF-050 and all
other TYPED findings remain unselected. Existing citation resource bounds cannot
be removed as part of batch preparation.

## 7. Verification and release acceptance

| Task | Required behavioral evidence | Durable documentation owner |
|---|---|---|
| AGENT-001 | Old/new contract admission, invalid closed shapes, mode identity and unsupported reservations | JSON contract, CLI specification, schemas/catalogs and transaction contract |
| AGENT-002 | Existing stdin routes reject conflicting flags; preflight preserves document fences and error order | CLI specification and testing guide |
| AGENT-003 | Partial/full batches, one revision/sequence, stable cursor/goal revision, duplicate/unknown/already-assessed/mixed-mode rejection, stale fences/citations, rollback and exact replay including changed input order | Goal/state specifications and goal regression owners |
| AGENT-004 | Existing observation v5/v6 equivalence, successor-owned readiness/receipts, Unicode/escaping and exact-byte limits, no second database read | System architecture and affected module guidance |
| AGENT-005 | Prepared/running/transition/terminal and record no-op projection; projection/size/depth rollback; initial and lookup cap edges; lost output, historical replay/lookup, mode conflict | JSON/transaction/SQLite specifications and native transaction tests |
| AGENT-006 | Native small-change and bug-fix normal counts, uncertain/retried submission refresh and contention, older/unsupported route fallback | Runtime skill, examples, testing guide and requirements traceability |

Use registered crate `int_suite` tests and native CLI helpers, focused checks
first, then required `make test` for each executable/contract candidate under
repository gate routing. No test, build or timing result is claimed by this
registration. No new benchmark or verification framework is required.

After integrated development verification and independent whole-Epic review,
obtain separate owner acceptance. Before completion, promote lasting design,
regression owners and guidance to the documents above; replace all dossier
references and retire this dossier under the [TODO lifecycle](README.md#lifecycle).
Development completion does not establish stable distribution, release,
installation, activation or external consumer acceptance. Target release remains
unassigned; release gates require their own scope and authorization.

## 8. References

- [ADR-0039](../architecture-decision-records/0039-opt-in-contract-generations.md)
- [JSON contract](../specs/interfaces/json-contract.md)
- [Transaction contract](../specs/storage/transactions-concurrency-and-idempotency.md)
- [CLI specification](../specs/interfaces/cli-specification.md)
- [Testing guide](../implementation-tips/testing.md)
- [Continuing interaction candidate](TODO-podway-agent-workflow-simplification.md)
- [TYPED candidate](TODO-podway-typed-boundary-ownership.md)
- [Deferred register](../deferred-feedback/README.md)
