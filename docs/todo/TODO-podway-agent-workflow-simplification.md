# Podway Agent Workflow Simplification

## Status and authority

- Document state: `Candidate`
- Working label: CALLS
- Owning roadmap epic: none
- Target product release: undecided
- Repository scope: Podway only
- Original interaction-research baseline: 2026-10-19 at `98796e6a20467d1d072bd84f377fd9ddccccab93`

The working label reserves no roadmap ID or task. Source feedback retains its
own review context; the original research baseline is not a claim that later
feedback was inspected at that revision.

## 1. Context

Internal request reuse and external CLI interaction are separate design
questions. The adopted
[structural simplification program](../roadmap/README.md#structural-simplification-program)
preserves current CLI behavior; it does not authorize fewer observations,
combined mutation commands or changes to external consumers.

Existing `observe` may satisfy a caller that otherwise obtains status and next
separately, but it is not a substitute when complete Standard status or history
is required. Existing `record --stdin` already accepts 1-128 unique same-attempt
operations within its 1 MiB input bound as one atomic batch. These are current
capabilities to assess before proposing a new interface, not new features.

The actual fence flags include `--if-workspace-uuid`, `--if-session-id`,
`--if-session-revision` and `--if-attempt`; sketches must use the implemented
grammar rather than inventing `--expected-*` flags.

## 2. Goal

Explore whether callers can express the same intended work with fewer redundant
observations or request documents while retaining clear snapshot, fence,
atomicity, failure and replay meaning. Evaluate structural simplification rather
than elapsed time or claimed speedup.

## 3. Non-goals

No implementation order, task status, completion evidence or public contract is
established by this candidate. No consumer changes, new commands, schemas,
cross-project discovery, runtime service or automatic mutation are authorized.
Elapsed-time tests, latency percentiles, throughput benchmarks, performance
baselines and thresholds are excluded from the proposed investigation.

## 4. Rough Scope

The following subsets are independently selectable. The recommended first
selection is a small, coherent existing-interface improvement addressing observed
agent confusion. No selection must include all five source-feedback entries or
resolve the optional combined-API questions.

### Existing-interface guidance

- Clarify required and global options, missing-option messages and contextual
  help completion for the selected command flow.
- Explain the existing check-result shape, revision terminology, disposition
  choices and automation fence practice where agents have needed clarification.
- Preserve command semantics, fence authority and supported machine contracts
  unless a specific behavior or contract change is separately selected.
- Document when existing observation and atomic record interfaces already cover
  a caller's needs, including the information lost by replacing a richer read.
- Examine duplicated caller-side observations separately from internal Standard
  preflight, which still validates projection/schema/envelope failures.

### Optional interaction semantics

Potential directions, all non-contractual and requiring their own use case:

- Explore a combined record-and-complete request only with an explicit decision
  on transaction boundaries, intermediate failure and exact replay identity.
- Explore returning a next observation with a mutation only with an explicit
  decision on the observation's snapshot, bounds and replay meaning.

Separate terminal-start reset dry-run and interactive post-decision observations
remain intentional existing behavior until independently reconsidered. A shorter
call sequence alone does not establish semantic equivalence.

### Agent guidance and recovery

The source feedback below extends the interaction investigation to remaining
CLI usage, help-topic completion, check-result recording, revision terminology,
disposition and fence guidance. It includes the goal-tracking visibility and
regression-check prompt clarity recorded in DF-051. Preserve runtime-skill
guidance already delivered; DF-050's optional broader preset enrichment remains
independent deferred work.

Evaluate representative small-change and bug-fix flows, including stale fences,
lost mutation output and exact retry. Existing observation and atomic recording
are the baseline. A combined record-and-complete route or returned observation
needs its own closed snapshot, atomicity, error-order and replay decision before
adoption. Any preset wording change must account for canonical digests and
manifest identity rather than being treated as a free contract change.

### Source feedback

These entries retain the original source, reason for deferral and revisit
condition. Candidate grouping does not resolve, adopt or implement them.
Only the remaining work described by each entry enters this candidate;
already delivered subparts are preserved as context.

| ID | Source | Feedback | Deferral reason | Revisit condition |
|---|---|---|---|---|
| DF-044 | GUIDE design review | `podway help` topics have no shell completion: the help route completes no values, and multi-word command topics would need contextual completion. | GUIDE-003 resolves topics without changing completion route accumulation; completing multi-word positionals is an independent refinement. | Before changing help grammar or completion route accumulation, complete help topics from the same command tree and verify the bash, zsh and fish scripts. |
| DF-047 | GUIDE-012 UTM agent reruns | Per-command help usage lines omit `--idempotency-key` and global options such as `--contract-generation`, so agents learn the key requirement only from a `REQUEST_INVALID`, and "invalid command syntax" does not name a missing required option such as `--task`. | Help keeps its established per-route usage format and the closed JSON errors remain actionable; enriching usage lines and usage failures is a guidance improvement, not a contract defect. | Before changing help usage rendering or usage-failure messages, consider appending the always-available global options to per-command usage lines and naming the first missing required option in the usage failure. |
| DF-048 | GUIDE-012 UTM agent reruns | The nested `check_result` shape (`input_basis` with `descriptor` and `digest`, `executor` with `name` and `version`) appears only in the observation's `stdin_template` and not in the inspected agent guidance (help, the runtime skill and observation prose); `item.set` results call the session revision just `revision` beside item revisions; and nothing guides agents toward reproducible check-output digests. | The shapes are schema-valid and the observation carries the executable input. The runtime skill now names the required nested fields, uses stdin_template when present, distinguishes session/item revisions and recommends hashing retained check output. The remaining CLI-help and observation-prose guidance is independent. | When record help or observation prose is next revised, explain the nested check-result shape, session-revision field name and reproducible output digests; preserve the runtime guidance already added. |
| DF-049 | GUIDE-012 UTM agent reruns | Help and the runtime skill do not say when to choose `disposition handed-off` versus `not-required`, or what `--reference` holds; `allowed_actions` lists `session.start_replace` once while two prepared templates exist; and the templates omit the optional `--actor` and the `goal assess-criterion` `--evidence`/`--item` options. | The template shapes remain valid by design. Runtime lifecycle guidance now explains disposition choice and reference; shared execution rules permit supported optional/repeatable arguments, and goal guidance names citation flags. CLI-help explanation remains independent. | When lifecycle or goal help is next revised, explain disposition choice and reference, the shared start_replace action covering two variants, and supported optional actor/citation arguments. |
| DF-051 | GUIDE-012 UTM agent reruns on the final candidate build | The `begin` and `disposition` help say "Omitted fences are filled from a fresh status preflight" and their usage lines bracket the fences as optional, while the runtime skill says never to omit a required mutation fence (`774954a:skills/use-podway/SKILL.md:75`), so both agents read the two as conflicting; the envelope's `latest_workspace_sequence`, the init result's `revision` and the archive list's `archived_workspace_revision` are not related anywhere in the inspected guidance; `preset explain` (text and JSON) does not show that `bug-fix-v2` tracks a goal (`assets/presets/bug-fix-v2.yaml:7`); and the `bug-fix-v2` `regression-check` prompt (`:40`) can be read as the check to run later or as its result. | The runtime skill now distinguishes CLI preflight auto-fill from automation preserving observed fences. Both agents had preserved all fences, and the counters, preset explain output and prompt remain valid; their presentation improvements remain independent. | When command help or presets are next revised, explain automation fence practice, relate workspace revision and sequence, show goal tracking in human preset explain, and clarify the regression-check prompt. |

## 5. Open Decisions

- Which concrete caller needs lack an adequate existing interface, and what
  information and failure behavior must be retained?
- For the selected guidance subset, which existing behavior and user mistakes
  will it explain, what exact rendering will change, and how will that concrete
  user-facing improvement be verified?
- For a returned observation, is it tied to mutation commit state or a later
  snapshot? Does replay return the original observation or perform a new read?
- For a combined operation, is it wholly atomic, and which error wins if record
  and completion would both fail? How are caller keys and digests defined?
- Would changed output require new schemas/result generations, compatibility
  policy or an accepted ADR? Which independent consumers would need explicit
  authorization to change?

## 6. Roadmap Promotion Conditions

Promotion requires a concrete user-facing outcome, the selected feedback and
command flow, exact changes and focused verification, plus an explicit owner
decision on that bounded scope. A guidance-focused subset can be adopted using
accurate existing semantics without resolving unrelated combined-operation or
returned-observation design. Preserve the unselected findings under a continuing
owner according to the [TODO lifecycle](README.md#lifecycle).

Snapshot, atomicity, error precedence, request identity, replay, bounds and
compatibility decisions must be closed for changes that affect those properties.
Combined operations and returned observations remain optional future choices;
their selection requires their own concrete need and complete semantics. Use an
accepted ADR before changing an architectural invariant, and update affected
canonical contracts and compatibility checks through the adopted work.

Verification follows actual effects: CLI rendering and completion are executable
changes requiring focused checks and the development gate. Preset wording may
change canonical digests and manifest identity and needs the corresponding
contract checks. Documentation-only clarification uses documentation checks.
Neither rendering nor enrolled preset content is consequence-free prose.

Register an epic and tasks only after those decisions are complete. No
dependency or implementation commitment is implied by the working label CALLS.
Qualification must use correctness and structural evidence without timing or
throughput measurements.

## 7. References

- [TODO authority and lifecycle](README.md)
- [JSON contract](../specs/interfaces/json-contract.md)
- [Transaction contract](../specs/storage/transactions-concurrency-and-idempotency.md)
- [CLI source](../../crates/podway-cli/src/command/)
