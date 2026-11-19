# Worked execution-group consumer examples

These examples apply the [version 1 consumer contract](../specs/interfaces/execution-group-consumers.md)
to an owned Git fixture. They use Procedure v3 and Execution Group v1 through
the scoped Podway CLI. Native and Herdr handles below are illustrative
identifiers for deterministic workers. A fixture can exercise the caller's
decisions and Podway's admission without launching a live adapter or adopting
anything in the external executor.

## Two contributors on one Action

The Coordinator starts with approval `approval/compatibility` and its
unconsumed grant to run the approved checks. The current observation selects
workspace `W`, session `S`, base digest `B`, group execution `G1`,
group definition digest `D`, and the `implement` Action attempt `A1`.
Both assignments bind this target and the same inspected working-copy digest
`K1`. They retain dispatch-time session revision `N`, selected group `G1`
and the untracked preset's null goal revision. These symbols stand for the
actual observed identities. The executor triples below are synthetic scope,
worker and task references.

| Responsibility | Consumer assignment and executor references | Role and configuration | Contribution |
| --- | --- | --- | --- |
| Task Operator on the native fixture | `fixture-assignment/native-1`, scope `fixture-runtime-session`, worker `fixture-native-worker`, task `fixture-original-task` | Role `fixture-work`, digests `R1` and `C1` | Checks the selected bytes and prepares the implementation summary. |
| Affected Evaluator on the Herdr fixture | `fixture-assignment/herdr-1`, scope `fixture-herdr-session`, reviewer `fixture-herdr-observer`, task `fixture-herdr-check` | Role `fixture-evaluator`, digests `R2` and `C2` | Independently checks the evidence against the required outcome. |
| Designated Writer | `fixture-writer`, current attempt's Writer record under `approval/compatibility` | Writer authority stays with the consumer | Reconciles each report against the common scope and basis and records the Action items. |
| Coordinator and Primary for this fixture | `fixture-coordinator` | Original authority and selected route | Selects the route and accepts the synthetic delegation result after reviewing the settled work. Technical repair belongs to the Writer. |

```text
R1 sha256:622fcab4f31a6b5f53f2afa479e4219bf471cf75eed2fb199ef0fe7297839606
C1 sha256:634fde2bbb641ceb3d9d010b978ebbd7b4433cdc8170f0cc0d62d54f22780c66
R2 sha256:efda81421d473eab68507592cd11376620b5d66a8c271d20e9cdeb1af21ee449
C2 sha256:d52ceb8d76913981afedd7a23ff0dca65158666f71f5510ce1d08d6e62fb6fe7
```

The Role preimages are the UTF-8 JSON strings `"fixture-role-v1"` and
`"fixture-evaluator-role-v1"`, including the quote bytes. The configuration
preimages are the UTF-8 text `fixture worker config v1` and
`fixture evaluator config v1`, without trailing newlines.

Each contribution preserves its original assignment reference and digest. The
fixture's native acceptance takes the accepted worker, Role, configuration and
task references from `fixture-assignment/native-1`. Its `accepted_task_digest`
binds that exact deterministic task record. The receipt's result digest covers
the accepted native fixture task's output bytes. The Evaluator's report keeps
its own assignment and digest in the consumer record.

The external contributions can overlap. They share one active Podway attempt.
Workers submit reports to the consumer. The first accepted item record
advances the session revision. The Writer then reconciles the second
contribution at the fresh revision, checking the unchanged workspace, session,
base, selected group, attempt, goal and working-copy basis. Both original
assignments still retain revision `N`. Only the authorized Writer records
items with the fresh native fences. Disagreement about the inputs or result
goes to the affected assessor, with a blocker if required evidence is missing.

For a supervised instance of `implement`, the Primary obtains the scoped
package with `delegation package` and binds the actual external task and
eligible worker. Record the final Action values, settle the worker as required
by the placement, and submit `delegation accept --stdin` using the current
package digest, scoped cursor and accepted task and result identities. The
package and receipt cover the supervised acceptance. Keep the other
contribution's exact source records in the consumer's evidence. With
`after_accept: release`, use the worker's observed released disposition.
Use `complete` only when the active Action and its receipt are ready.
[Scoped delegation](../specs/interfaces/supervised-delegation.md#scoped-packages-and-receipts)

## Recover a missing response and reconcile a changed binding

| Observation | Deterministic next step |
| --- | --- |
| The external dispatch acknowledgement is missing. The consumer cannot distinguish accepted work from an unsent request. | Keep the original task/dispatch identity and query that same external operation. Keep modification authority held while the operation is uncertain. A Podway lookup cannot establish its external outcome. |
| The original external worker completed and the consumer observed the accepted task, result and required released disposition. | Record that actual settlement and reconcile every other affected worker before a group boundary operation. |
| The expected role or executor binding changes before the pending result is accepted. | Leave the original result attached to its original assignment. Recover and settle it under that identity. Resolve the new binding and check its current authority before starting any new assignment. A changed role submitted against the old declared package is refused with `DELEGATION_BINDING_MISMATCH` in the fixture. |
| The bounded Podway acceptance was sent under key `K-accept`, but its stdout was lost. | Run `outcome lookup --idempotency-key K-accept` with the exact owning worktree and compatible writer. The fixture finds the original committed result. Exact replay of the original complete stdin returns that result and allocates no new acceptance. |
| A lookup finds no retained Podway outcome. | Verify the retention and workspace boundary and re-observe. The miss leaves an external operation's outcome unknown. Never resend external work because of that miss or recreate the Podway request with updated fences and the same key. |

## Assess staged failure and reuse current evidence

Consider the default compatibility example on `G1`. Three distinct checks
fail to retain a required independent compatibility result. Each local
correction was justified using the then-current facts. The affected Evaluator
records the evidence before each Decision. The latest selected execution and
the upcoming `next_decision_entry` come from `observe`.

| Entry | Current evidence and affected Evaluator's contribution | Coordinator decision |
| --- | --- | --- |
| 1 | The result record omits the independently required check. A known wrapper configuration might provide it. Record failure and an unscored zero placeholder. | Choose the ordinary correction with that concrete change. |
| 2 | The modified wrapper still emits only one retained result. The screening names the repeated omission and one remaining way to capture the second result under the current plan. | Choose that correction if it still has a viable path identified by the current evidence. |
| 3 | The new run again lacks the required independent result, with no qualifying improvement in three failures. The existing report's verified inspection of the route and alternatives still applies. The affected Evaluator checks the current digests, retains the original report's identity, and composes a new recorded basis with the fresh result and its own applicability reason. | Record the decision from the new assessment. At 40 for the missing required route, 20 for failed corrections with an alternative still under examination, and 20 for repeated failure without progress, the default guard admits replan. The Coordinator can still continue on a proven viable route, keep a blocker, or choose the eligible replan. |

If the runner configuration or the selected evidence changed, reassess the
affected claims. Preserve the earlier report with its original target and
outcome. Recording current applicability does not count as another execution
of that report. A prior successful work result or score never satisfies a new
group gate by itself. See the [staged failure walkthrough](continuation-v3.md#assess-actual-failures)
for the other default decision paths and the early serious-defect case.

## Settle and decide whether the replacement is covered

At the eligible Decision, one modifying descendant still has an uncertain
completion response. The Coordinator records the concrete missing settlement
as a blocker and keeps the selected group. In the deterministic case, both the
unknown state and the settled outcome refer to
`fixture-op/compatibility-check` on `fixture-native-verifier`. Recovery queries
that same handle and records `fixture-settlement/compatibility-check` with
released write permission before the caller considers a replacement.
Use the native unblock operation after the missing fact is resolved. Refresh
the observation and select only a route still allowed by the current evidence.

| Proposed plan | Scope classification and authority record | Outcome |
| --- | --- | --- |
| Add a verification placement inside the same approved compatibility objective and the same worktree. | `within-original`, original approval `approval/compatibility`, existing available `grant/verify-compatibility`. The previously used `grant/first-dispatch` remains consumed. | The Coordinator reviews the new plan, records the closure reason and prior-work dispositions, and binds fresh anchor inputs before activation. The topology change needs no repeat approval. |
| Modify `fixture-adapter.toml` in the owned worktree as well as verifying compatibility. | `scope-expansion`, original approval `approval/compatibility`, additional approval absent. `grant/verify-compatibility` covers verification only. | The caller refuses the new modification before any uncovered dispatch or effect. Podway cannot decide whether that configuration change is in scope. |
| Consider the same edit after the fixture authority owner grants it. | `scope-expansion`, original approval `approval/compatibility`, additional approval `approval/adapter-fixture`. New `grant/edit-fixture-adapter` covers only editing `fixture-adapter.toml` in the owned worktree. `grant/first-dispatch` remains consumed. | The toy caller accepts this authority classification for current plan review and formal anchor gates. Use a separately authorized goal revision if the goal changes. The fixture records the decision without editing the file. An actual consumer needs a real owner's approval before performing the effect. |

For an admitted in-scope replacement, use the current Decision's fenced
`group replan --stdin` request with the option, reason, working-copy basis and
feedback. After the fresh anchor, read the current plan binding and latest
predecessor. Supply concrete prior-work choices, for example `revalidate`
the captured `G1/A1` implementation under the new cases. Activate only after
the proposed document matches the current plan interface and verified source
digest. The [replacement walkthrough](continuation-v3.md#select-a-justified-replacement)
shows that path with a changed verification graph.

## Refuse a late result even with the same definition

A separate deterministic fixture activates the same compatible definition
`D` again after closing `G1`. The base digest and node label are unchanged.
The new assignment targets group execution `G2` and attempt `A2`.

| Input arriving at the consumer | Admission and evidence |
| --- | --- |
| A result asserts original target `(W, S, B, G1, D, implement, A1)` and old basis. | Keep it as historical evidence. The consumer declines to record it at `(W, S, B, G2, D, implement, A2)`. Matching the definition or node label does not authorize retargeting. |
| Negative control in the isolated fixture submits the old acceptance with a fresh key. | The CLI refuses it with `ATTEMPT_NOT_CURRENT`. The typed details identify `A1` and the actual current `A2`. Lookup for the rejected request's key finds no accepted outcome. The new cursor is unchanged. |
| Exact original key and body are replayed to recover the prior accepted response. | The success describes the original receipt only. Read the old receipt with `delegation receipt --session S --attempt A1`. Its applicability is historical. A fresh `complete` at `A2` still needs a current accepted receipt. |

Task-time verification used the matching development CLI with generation-2
presentation in owned fixtures. It covered original-key behavior, changed-role
refusal, distinct activation and attempt identities, isolated late-result
refusal and the current receipt gate. The
[registered scoped CLI checks](../../crates/podway-cli/tests/e2e_adopt001_scoped_cli.rs)
cover the related package, acceptance, replay and fence rules. Scope
classification, role routing and external settlement are deterministic fixture
assertions. Live external qualification has its own evidence.
