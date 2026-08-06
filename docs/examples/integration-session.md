# Integration Session

Parallel tasks in [separate worktrees](../specs/product/user-workflows.md#parallel-tasks-across-worktrees) produce branches that must eventually merge, and merging divergent changes is real work: conflicts must be reconciled, each branch's intent preserved, and the result verified. Podway does not perform the merge — it mutates no Git state — but the merge can be governed like any other task: as its own session, with the recorded state of each branch session as its input.

## Capture branch state before reset

Session state is worktree-local and disposable. Before a finished branch worktree is reset or removed, capture what the merge will need. From the worktree where the integration will run:

```bash
mkdir -p branch-state
(cd ../retry-backoff && podway status --json) > branch-state/retry-backoff.json
(cd ../login-race && podway status --json) > branch-state/login-race.json
```

A completed session remains inspectable until `reset`, and its recorded items — the goal, the fix summary, the verification notes — are exactly the context a merge needs: not just what each branch changed, but why.

## An integration procedure

A worktree-local custom procedure shapes the merge ([authoring tutorial](authoring-procedures.md)):

```yaml
schema: podway.procedure/v1
id: integrate-branches
version: "1"
name: Integrate Parallel Branches
stages:
  - id: collect
    title: Collect branch outcomes
    items:
      - id: branch-summaries
        type: list
        prompt: Record one line per branch with its goal and outcome.
        required: true
        min_items: 2
  - id: reconcile
    title: Reconcile the changes
    items:
      - id: conflicts
        type: list
        prompt: List each conflict and how it was resolved.
        required: true
        min_items: 1
      - id: intent-preserved
        type: confirm
        prompt: Each branch's recorded intent survives in the merged result.
        required: true
  - id: verify
    title: Verify the merged result
    items:
      - id: merged-verification-passed
        type: confirm
        prompt: Verification covering both branches' behavior passed.
        required: true
      - id: verification-note
        type: text
        prompt: Summarize the verification of the merged result.
        required: true
        min_length: 1
  - id: land
    title: Land the integration
    items:
      - id: ready
        type: confirm
        prompt: The merged result is ready to land.
        required: true
rework:
  allow_return_to:
    - collect
    - reconcile
    - verify
```

## Run the merge as a session

```bash
podway procedure validate .podway/procedures/integrate-branches.yaml
podway start --procedure .podway/procedures/integrate-branches.yaml \
  --task "integrate retry-backoff and login-race"
podway next
```

Work the stages as usual: record one summary per branch from the captured status payloads, perform the merge with your normal Git tooling, record each conflict and its resolution, verify the combined result, and land. When verification exposes a bad resolution, `podway return --to reconcile --reason "..."` makes the redo explicit instead of leaving it in shell history.

Only after the integration session completes do the branch worktrees and their captured state stop being needed.
