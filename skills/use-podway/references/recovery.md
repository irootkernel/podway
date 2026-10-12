# Mutation and State Recovery

Read this reference after a stale precondition, uncertain mutation outcome, maintenance conflict, or unreadable workspace state.

## Branch on the error contract

Require `podway.error/v2` and branch on the stable `code`, its exit class (0 success, 1 domain, 2 usage, 4 conflict, 5 workspace, 6 internal), and `retryable`. Exit class 3 is retired. Never branch on message text, which may change without a schema change.

When `details.recovery` is present, validate RecoveryRecipe v2: exactly `action`, `command`, `argv`, `reason`, and `requires_explicit_authorization`. Read-only recipes name `session.observe`, `outcome.lookup`, `workspace.doctor`, or `version` and require `requires_explicit_authorization=false`. Initialization resumes name `workspace.init`; destructive recipes name `workspace.reset_all` or `workspace.remove`. Both require `requires_explicit_authorization=true`. Validate the command/argv combination against its canonical schema, preserve the established exact worktree, and fill only declared placeholders from supported facts. Reject open fields, unsupported commands, or weakened fences. A recipe is guidance, never user authorization; execute a destructive recipe only within an applicable explicit grant.

## Stale state

1. Preserve the rejected request and its idempotency key.
2. Run `podway observe --json` again.
3. Compare session, attempt, goal, and item identities and revisions with the rejected request.
4. If another mutation already achieved the intended state, do not repeat it.
5. Otherwise derive a new request from current state with new applicable preconditions. Reuse an idempotency key only for the identical canonical request; use a new key for a changed request.

In Procedure v2, `EVIDENCE_REFERENCE_STALE` and `GOAL_REVISION_STALE` are retryable exit-4 conflicts resolved by re-reading state and re-deriving the request. `DIGEST_CONFIRMATION_REQUIRED` is a non-retryable exit-2 error.

Never omit or weaken a precondition merely to make a stale mutation succeed.

## Unknown mutation outcome

`MUTATION_OUTCOME_UNKNOWN` means an I/O failure during COMMIT prevented Podway from determining whether the transaction committed. Process termination can also leave the caller without a success response; it does not promise continued execution.

1. Retain the original idempotency key, canonical request with its resolved fences, and exact owning worktree. Preserve the workspace UUID when known. Repeating CLI arguments that originally autofilled fences may derive a different canonical request.
2. Run:

   ```bash
   podway outcome lookup --idempotency-key <original-key> --json
   ```

3. Require `podway.outcome-lookup-result/v1`. If `found` is true, inspect the retained successful outcome and compare it with the original request and intended effect. Lookup reports historical success, not current readiness.
4. If `found` is false, it establishes no retained success within the retention boundary. Workspace-scoped outcomes retain at most 100 entries or 30 days. Session-scoped outcomes survive archival and archive purge; session reset, runtime reset, or workspace removal ends their retention. Absence after retention or deletion does not disprove an earlier effect. Reconcile current state; replay only the identical canonical request with the same key when it can be reconstructed and its scope is still authorized. Otherwise report the recovery gap before another mutation.
5. Re-read `podway observe --json` before deriving a new mutation. Exact recovery replay uses the original fences, not replacements from the new observation.

An `IDEMPOTENCY_KEY_REUSED` error means the retained key is bound to a different canonical request. Do not overwrite that identity; inspect the original outcome and choose a new key only for a genuinely new authorized request.

## Maintenance or workspace failure

- Follow the validated read-only recipe before proposing a mutation. Invalid-Store and writer-contract failures use `podway --json version --identity`, which does not open the rejected Store. Use `podway doctor` and `podway workspace show` when Store admission permits them.
- `WORKSPACE_BUSY` reports an expired admission wait. `WORKSPACE_MAINTENANCE` reports unavailable maintenance admission or an observed maintenance marker; a marker may be reported before the wait budget expires. Read `details.boundary` to distinguish the maintenance lease, process-local connection gate, SQLite read/write admission, and observed `intent_marker`, `reset_marker`, `removal_marker`, or `predecessor_migration` boundaries. Each boundary tries once even after the original invocation budget expires; prompts do not renew it. These errors do not authorize reset or fence weakening. Inspect any structured recovery recipe before acting.
- On `LEGACY_RUNTIME_STATE_UNSUPPORTED`, preserve the byte-identical unsupported
  state. Classification performs no migration, checkpoint or SHM creation. Before
  any reset, establish separately authorized retirement of every old writer,
  including an Aquarium-managed service through its original controller. Then,
  with explicit destructive authorization for the exact worktree, use
  `podway reset --all --yes --idempotency-key <new-key>`. It creates fresh current
  state without backup/conversion, discards all old runtime history, and preserves
  configuration unrelated to retired mode, Procedures and unrelated worktree files.
  `--force` is unnecessary; the CLI accepts it only with `reset --all`.
- On `WORKSPACE_CONTRACT_MISMATCH`, compare exact writer/current manifest identities.
  Do not substitute a matching semver, retry through another binary or invent a
  predecessor. Only an explicitly declared release-owned predecessor permits
  migration; v0.2.13 declares none. Development with changed contract bytes uses a
  disposable worktree rather than a runtime mode.
- Initialization resumes preserve the original idempotency key and `--repair` policy. Foreign writer/root markers and reserved predecessor intents provide read-only writer diagnosis; the current writer declares no production predecessor. Resume only with the original writer and owning worktree, under an applicable grant.
- Maintenance-lock contention with a Store provides `observe` after the owner finishes. Store-less final-removal or residual contention provides `version --identity`, then an exact retry after the owner finishes. If removal reports a repopulated parent, preserve its current contents and inspect the writer and workspace before considering a new authorized request; this error does not establish invalid state.
- Init/reset/removal retain descriptor-safe intent and operation markers. A marked
  workspace returns `WORKSPACE_MAINTENANCE`; ordinary reads never resume it. Resume
  only the original canonical operation, idempotency key, applicable UUID and exact
  root/fences from the recipe. A newly generated key is not recovery. Do not remove
  `maintenance-intent.json`, `reset-all.marker` or `removal.marker` to bypass it.
- Do not edit runtime databases, maintenance markers, or filesystem identities manually.
- Do not repair, reset, or remove anything unless an applicable user authorization covers the diagnosed lifecycle action.
- If state remains unreadable, report the exact error, affected worktree, checks performed, and the smallest supported recovery action. Preserve existing task state until that action is authorized.
