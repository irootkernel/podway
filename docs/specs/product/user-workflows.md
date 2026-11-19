# User Workflows

Initialize one Git worktree with `podway init`, inspect `podway preset list`, and
start `analysis-v3`, `sw-dev-v3`, `bug-fix-v3`, `small-change-v3`, or a validated
worktree-local Procedure v3 base. Use `analysis-v3` for source-backed research or
technical analysis that needs evidence-gap and phase-owner review rework. Use
`small-change-v3` for bounded changes with inspection, assertion-based
verification, review and closeout without a tracked goal. Use
`--dry-run` before a destructive replacement or other supported preview.
Starting creates a prepared session without a cursor or goal. Use `podway begin`
to create its first attempt, optionally defining the initial goal at the same time.

`podway init` defaults to `sw-dev-v3`. A preset start pins the embedded base
and makes its default group available. After recording the design inputs, use
the generation-2 observation template to activate an exact group explicitly.
A successful group returns its declared results to the base. A guarded replan
retains the closed group's work and starts a fresh design attempt. A running
session continues from its admitted definition snapshots.

During a task, read `podway observe` (or the commit-bound `result.observation`
returned by `--with-observation` on a known first submission of a supported
mutation), perform the actual work outside Podway, then record only supported
evidence on the active attempt. Use typed item commands or atomic `record
--stdin` and `complete` to follow an action's declared edge. Use `decide` on a
decision node and `goal assess-criteria --stdin` or `goal assess-criterion` for
goal-tracked sessions.

Record Procedure definitions, prompts, goal statements and criteria, narrative
evidence, check-result descriptors and summaries, and transition reasons in
English. Preserve exact commands, paths, hashes, identifiers, enumerated values,
and product names verbatim. Summarize non-English logs or source passages in
English and retain a digest or stable reference. This is an operator and agent
policy; Podway remains locale-neutral and accepts bounded Unicode.

Use explicit selected evidence items so each consumer reads only what it needs.
Use `required_when` for same-node typed conditional requiredness and complementary
guards for evidence-based routing. Manual rework is a declared target allowlist;
a decision route with `effect: rework` owns one option-specific target. For a
large selected value, use `evidence read` and follow its opaque
`next_page_token` until `truncated` is false. Use the scoped group and history
queries to inspect retained work. The
[Procedure v2 workflow](../../examples/v2-workflow.md) remains an example for
the original writer.

If the current attempt must be repeated, use `retry --reason`. If prior work must be
revisited, use `rework --to <node> --reason` only for a declared manual-rework
target. Podway creates a fresh attempt and applies the procedure's evidence
invalidation rules. Blockers prevent advancement until explicitly resolved.

Automation uses `--json`, carries the workspace UUID and exact observed cursor
for scoped mutations, and assigns a stable idempotency key to each request.
Scoped batch recording, criterion assessment, activation, replan and delegation
acceptance submit their own fences and key in one closed JSON stdin document.
Use the current `mutation_templates` and the
[scoped Direct contract](../interfaces/json-contract.md#scoped-direct-execution)
to construct them. A replan recipe appears only when a replan option is
currently eligible. The first activation uses an empty `prior_work` array.
For a replacement activation, replace `<prior-work-dispositions-array>`
with a nonempty array of dispositions for closed work from this session.

Six mutations support `--with-observation`: plain `start` without `--dry-run`
or `--on-existing`, `begin`, `record --stdin`, `complete`, `decide`, and
`goal assess-criteria --stdin`. The v2 wrapper pairs the mutation result with
a commit-bound v8 observation. Use that observation to construct an eligible
next mutation after a verified first submission. Record and complete are
separate transactions, as are criterion assessment and decision.

Each command runs synchronously. Callers may derive the next mutation directly
from `result.observation` only on a known first submission (no hidden retries)
that exits `0` with complete observation facts; a new key alone does not prove
submission history. On response loss or `MUTATION_OUTCOME_UNKNOWN`, use
`outcome lookup --idempotency-key <original-key>` and preserve the exact canonical
request. A retained success replays its original historical response; a rejected
request retains no outcome. Not-found after response loss alone does not prove
rollback: reconcile any still-running original writer before an identical-request
retry. There is no detached execution, job status, or queue barrier. Reads return
one committed snapshot; after any replay, `outcome lookup`, lost stdout, retry,
uncertain submission history, contention/conflict, or unsupported/unwrapped/incomplete
response, obtain a fresh `observe` before deriving any new mutation. See the
[transaction contract](../storage/transactions-concurrency-and-idempotency.md).

Actors can hand off within a worktree using the current scoped observation. Parallel tasks belong in separate Git
worktrees and therefore separate Podway workspaces. Podway does not transfer a
session between worktrees.

Cancel a task that will not continue. A prepared session can be reset immediately.
Before default reset or replacement of a completed or cancelled session, record a
handoff or not-required disposition for its exact current terminal revision. Force
deletion of running or undisposed terminal work requires explicit authorization and
a bounded progress summary. If opening old runtime state returns
`LEGACY_RUNTIME_STATE_UNSUPPORTED`, follow the separately authorized
[ordered cutover](../operations/release-and-packaging.md#upgrade) before confirmed
`podway reset --all --yes --idempotency-key <new-key>`. Reset itself makes no
backup or migration. Newer-generation refusal requires a supporting binary;
an older binary cannot reset or remove that Store.
