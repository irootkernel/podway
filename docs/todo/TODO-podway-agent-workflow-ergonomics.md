# Podway Agent Workflow Ergonomics

## Status and authority

- Document state: `Adopted`
- Owning roadmap epic: `V2AGT`
- Intended release train: post-v0.2.1 unreleased work
- Product version in this epic: unchanged
- Contract target: Procedure v2, `podway.output/v3`, and additive public routes

This dossier is the decision-complete implementation authority for unfinished
`V2AGT` work. The active roadmap owns task order and status. Accepted ADRs,
canonical assets, executable contracts, and current specifications retain their
normal precedence.

## Verified context

Podway already exposes a stable CLI/JSON automation boundary, durable jobs,
explicit optimistic-concurrency fences, typed Procedure v2 items, deterministic
next-action guidance, and two full-feature presets. The source-distributed agent
skill currently reads both `status` and `next` after every mutation. Those reads
split the state facts needed to construct the next safe mutation: `status` owns
item revisions and recorded-value projections while `next` owns prompts, allowed
actions, and suggestions.

The existing `next` suggestion contract contains placeholders rather than the
complete type constraints and applicable mutation fences. Recording several
items therefore requires one durable job and two fresh reads per item. Terminal
sessions also have status but intentionally have no next projection, which makes
the unconditional two-read skill loop imprecise at closeout.

ADR-0010 remains accepted: the public CLI and versioned JSON are the sole normal
integration authority. This epic adds no adapter, plugin, command execution,
network capability, Git mutation, or second writer.

## Goals

1. Provide one bounded, self-contained observation for the current session.
2. Make every active item input and safe next mutation mechanically describable.
3. Record a bounded set of current-attempt items in one atomic durable mutation.
4. Return structured safe recovery recipes for common automation failures.
5. Ship a shorter built-in Procedure for small verified changes.
6. Harden the combined feature set before closing the epic.

## Non-goals

- Do not replace or remove `status`, `next`, or any single-item command.
- Do not execute work, commands, tests, Git operations, or network requests.
- Do not infer semantic item values, decision options, criterion results, or
  rework reasons.
- Do not weaken identity, revision, attempt, goal, artifact, idempotency, or
  one-writer checks.
- Do not add session history export, remote collaboration, MCP, or a plugin host.
- Do not advance the product version, build a distribution, publish, install, or
  activate a runtime in this epic.

## Accepted design

### Session observation

Add the query route `session.observe` and CLI command:

```text
podway observe [--wait-for-idle | --after-job <job-id>]
```

The route returns closed `podway.observation-result/v1` through the existing
`podway.output/v3` envelope. It accepts the ordinary optional session identity
guard. `--wait-for-idle` and `--after-job` retain the existing queue-barrier
semantics and are mutually exclusive.

One observation contains the bounded current-session subset automation needs:

- Procedure, session, goal, queue, node, attempt, readiness, and counters;
- active items with prompt, required/satisfied state, revision, current bounded
  value projection, and type-specific authoring constraints;
- blockers, evidence references, selected read-back, allowed actions, and manual
  rework targets;
- structured mutation templates with semantic placeholders, exact applicable
  workspace/session/attempt/item/goal fences, idempotency-key requirement, and
  explicit-authorization classification.

Templates never contain generated semantic values or idempotency keys. Endpoint
selection remains caller-owned. Session creation, replacement, cancellation,
reset, repair, daemon lifecycle, goal definition/revision, and completed-session
reactivation are classified as requiring an explicit user request.

Observation omits history windows. Existing `status --verbose` remains the
history surface. A running session returns current guidance. A completed or
cancelled session returns success with no current item guidance or mutation
templates instead of treating the absence of a next node as an error.

The complete serialized response remains within the existing 1,048,576-byte
frame limit. Maximum-item and maximum-value fixtures prove the bound.

### Atomic multi-item recording

Add the mutation route `item.record_many` and CLI command:

```text
podway record --stdin
```

Standard input is the sole batch input grammar. The CLI reads no more than
1,048,576 bytes and accepts only closed `podway.item-record-many-input/v1`.
The input contains 1 to 64 operations with unique item IDs. Each operation
contains its expected item revision and exactly one `record` value or `clear`
disposition.

`record` supports confirm, text, choice, integer, a complete list replacement,
and artifact input. Artifact input uses either a local path with optional media
type or complete reference metadata. Local paths follow the existing safe open,
hash, admission, and completion revalidation rules.

The request also carries the existing workspace, session, attempt, and
idempotency fences. Operations are canonicalized by item ID after duplicate
rejection. The daemon validates every target, type, constraint, artifact, and
expected revision before changing state. It then applies all values in one
state transaction or none. The mutation never advances the cursor.

The closed `podway.item-record-many-result/v1` reports admission, active attempt,
session revision, and item-ID-ordered per-item changed, revision, and optional
value-digest outcomes. Detached admission, exact replay, key-reuse rejection,
receipt retention, response-loss reconciliation, crash recovery, and one-writer
sequencing use the established durable-job machinery. Existing item rows and
job/receipt storage are reused; no SQLite migration is introduced.

`V2AGT-003` establishes the bounded pure transition, deterministic per-item
outcomes, internal durable command and terminal projection, atomic Store commit,
rollback, replay, and restart behavior. It accepts already resolved artifact
values so the existing safe-open and completion-revalidation boundary remains
available to the daemon. Route registration, request decoding, public schema
assets, CLI input, and automation guidance remain owned by `V2AGT-004`.

### Recovery recipes

Keep `podway.error/v1`, stable error codes, retryability, admission facts, and
exit classes unchanged. Version only affected details schemas and add a bounded
closed recovery object containing an action, structured command/argv, reason,
and `requires_explicit_authorization`.

Recipes cover these implemented families:

- workspace/session/procedure identity mismatches;
- `SESSION_REVISION_CONFLICT`, `ATTEMPT_NOT_CURRENT`, and
  `ITEM_REVISION_CONFLICT`;
- `GOAL_REVISION_STALE` and `EVIDENCE_REFERENCE_STALE`;
- `MUTATION_OUTCOME_UNKNOWN` and `JOB_WAIT_TIMEOUT`;
- `DAEMON_UNAVAILABLE` and `DAEMON_CONTRACT_MISMATCH`;
- `WORKSPACE_STATE_UNREADABLE` and `WORKSPACE_SCHEMA_UNSUPPORTED`.

Recipes may recommend only `observe`, `job lookup`, `job wait`, `daemon status`,
or `doctor`. They never recommend weakening a fence or automatically performing
retry, restart, repair, reset, reinstall, or another mutation.

### Small-change preset

Add built-in `small-change-v2` with goal tracking disabled (the Procedure v2
contract represents this by omitting `goal_tracking`) and the graph:

```text
inspect -> implement -> verify -> review -> closeout
                         ^          |
                         `-- changes-requested
```

- `inspect` records one required bounded scope-and-constraint summary.
- `implement` records one required implementation summary.
- `verify` records the exact verification command and integer exit status.
- `review` reads the preceding evidence and routes `ready` to closeout or
  `changes-requested` back to implementation as rework.
- `closeout` records one required terminal note.
- Manual rework targets are `inspect`, `implement`, and `verify`.

The preset intentionally requires no source revision, log digest, goal,
criterion assessment, or artifact. It remains a formal verified change path,
not an abbreviated command runner.

## Failure and compatibility boundaries

- `status`, `next`, all existing result schemas, and all single-item commands
  remain byte- and behavior-compatible.
- Observation and batch recording are new closed routes with new schema IDs.
- Malformed, oversized, duplicated, stale, cross-attempt, unsupported, or
  artifact-invalid batch input fails closed with no partial state effect.
- A batch containing one invalid or stale operation rejects every operation.
- A lost batch response is reconciled by its original idempotency key before any
  new request is considered.
- Recovery additions expose no item values, procedure source, environment,
  credentials, or new filesystem paths beyond existing remediation contracts.
- Existing preset identities and digests do not change.

## Roadmap ownership

- `V2AGT-001`: adopt this authority and the ordered epic.
- `V2AGT-002`: implement self-contained observation, typed inputs, and fenced
  templates.
- `V2AGT-003`: implement the atomic domain/store recording core.
- `V2AGT-004`: expose the batch route, CLI, schemas, and automation guidance.
- `V2AGT-005`: add bounded structured recovery recipes.
- `V2AGT-006`: add, bind, document, and dogfood `small-change-v2`.
- `V2AGT-007`: review and harden the integrated feature set, pass the complete
  development gate, and archive this dossier.

Tasks execute in numeric order. Each task owns one commit and must be completed
before the next task starts.

## Verification and acceptance

Every executable task runs the narrowest relevant exact tests followed by the
complete `make test` development gate, preferably through configured Gaori
evidence compression. Passing logs remain unopened by default. `.gaori/` is
never committed.

Integrated acceptance requires:

- running and terminal observations, queue barriers, all item types, maximum
  bounds, fence accuracy, and authorization classification;
- atomic mixed-item success and all-or-none failure, deterministic request
  identity, artifact handling, replay, response loss, concurrency, crash, and
  restart evidence;
- exact recovery-schema fixtures with no mutating recipe or redaction leak;
- complete happy, review-rework, retry, and manual-rework paths for the new
  preset without changing existing preset identities;
- synchronized schemas, catalogs, manifests, help, completions, specifications,
  examples, README, roadmap, and source-distributed agent skill;
- clean architecture and documentation contracts plus a passing `make test`.

`make dist` is not part of this epic and no result claims release readiness,
publication, installation, or runtime activation.
