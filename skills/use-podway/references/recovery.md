# Mutation and State Recovery

Read this reference after a stale precondition, uncertain mutation outcome, maintenance conflict, or unreadable workspace state.

## Branch on the error contract

Require `podway.error/v2` and branch on the stable `code`, its exit class (0 success, 1 domain, 2 usage, 4 conflict, 5 workspace, 6 internal), and `retryable`. Exit class 3 is retired. Never branch on message text, which may change without a schema change.

When `details.recovery` is present, validate RecoveryRecipe v2: exactly `action`, `command`, `argv`, `reason`, and `requires_explicit_authorization`. Read-only recipes name `session.observe`, `outcome.lookup`, or `workspace.doctor` and require `requires_explicit_authorization=false`. Destructive recipes name `workspace.reset_all` or `workspace.remove` and require `requires_explicit_authorization=true`. Validate the command/argv combination against its canonical schema, preserve the established exact worktree, and fill only declared placeholders from supported facts. Reject open fields, unsupported commands, or weakened fences. A recipe is guidance, never user authorization; execute a destructive recipe only within an applicable explicit grant.

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
4. If `found` is false, it establishes no retained success within the retention boundary. Workspace-scoped outcomes retain at most 100 entries or 30 days; session-scoped outcomes are removed with their owning session or workspace state. Absence after retention or deletion does not disprove an earlier effect. Reconcile current state; replay only the identical canonical request with the same key when it can be reconstructed and its scope is still authorized. Otherwise report the recovery gap before another mutation.
5. Re-read `podway observe --json` before deriving a new mutation. Exact recovery replay uses the original fences, not replacements from the new observation.

An `IDEMPOTENCY_KEY_REUSED` error means the retained key is bound to a different canonical request. Do not overwrite that identity; inspect the original outcome and choose a new key only for a genuinely new authorized request.

## Maintenance or workspace failure

- Use `podway doctor` and `podway workspace show` to diagnose before proposing a mutation.
- `WORKSPACE_BUSY` and `WORKSPACE_MAINTENANCE` report bounded SQLite or maintenance contention. They do not authorize reset or fence weakening. Inspect any structured recovery recipe before acting.
- On `LEGACY_RUNTIME_STATE_UNSUPPORTED`, preserve unsupported state. No automatic daemon-era conversion is supported. After explicit authorization for that exact worktree, use `podway reset --all --yes --idempotency-key <new-key>`; it preserves configuration, Procedures, and paths outside runtime state. `reset --all` has no `--force` option.
- Resume an interrupted reset or removal only with its original canonical request, idempotency key, and applicable workspace UUID. A newly generated key is not recovery.
- Do not edit runtime databases, maintenance markers, or filesystem identities manually.
- Do not repair, reset, or remove anything unless an applicable user authorization covers the diagnosed lifecycle action.
- If state remains unreadable, report the exact error, affected worktree, checks performed, and the smallest supported recovery action. Preserve existing task state until that action is authorized.
