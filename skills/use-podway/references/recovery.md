# Mutation and State Recovery

Use after stale preconditions, an uncertain result, maintenance conflict or an
unreadable Store. Require `podway.error/v2`; branch on code, retryability and exit
class: 1 domain, 2 usage, 4 conflict, 5 workspace, 6 internal. Success exits 0.
Message text is not an API.

## Validate recovery guidance

RecoveryRecipe v2 has exactly `action`, `command`, `argv`, `reason` and
`requires_explicit_authorization`. Validate its command/argv against the exact
schema; preserve the owning worktree and fill only declared placeholders from
supported facts. Reject extra fields, unsupported commands and weakened fences.

- Read-only recipes use `session.observe`, `outcome.lookup`, `workspace.doctor`
  or `version`, with `requires_explicit_authorization=false`.
- Initialization resumes use `workspace.init`; destructive recipes use
  `workspace.reset_all` or `workspace.remove`. Both kinds require
  `requires_explicit_authorization=true` and applicable user intent.

A recipe never grants permission. Retain an established exact operation grant;
do not expand its deletion boundary or switch targets.

## Stale request

Preserve the rejected request/key, re-observe and compare workspace, session,
attempt, goal and item identities/revisions. If current state already achieved
the intended effect, do not repeat it. Otherwise derive a new request from current
state with all applicable fences. A changed canonical request needs a new key;
an identical recovery replay keeps its original key and fences.

`EVIDENCE_REFERENCE_STALE` and `GOAL_REVISION_STALE` are retryable exit-4 conflicts.
`DIGEST_CONFIRMATION_REQUIRED` is a non-retryable exit-2 error. Do not remove a
precondition to bypass any conflict.

## Unknown mutation outcome

`MUTATION_OUTCOME_UNKNOWN` means COMMIT I/O prevented a definite commit verdict.
Termination may also leave a caller without a response; it does not establish
continued execution.

1. Retain the original key, canonical request with resolved fences, exact worktree
   and known UUID. Arguments that previously autofilled fences may reconstruct a
   different request on a later invocation.
2. Run `podway outcome lookup --idempotency-key <original-key> --json` and require
   `podway.outcome-lookup-result/v1`.
3. If `found`, compare the retained successful outcome with the request and intended
   effect. It proves historical success, not current readiness. Exact replay returns
   the complete original envelope and request ID.
4. If not found, reconcile current state before considering replay. Workspace
   outcomes retain at most 100 entries or 30 days. Session outcomes survive archive
   and archive purge; session reset, reset-all and workspace removal end retention.
   A miss outside retention does not disprove an earlier effect.
5. Replay only the identical reconstructible request/key within existing authority.
   If that cannot be established, report the recovery gap before another mutation.
   Re-observe before deriving any new request; never replace fences on an exact replay.

Only validated successful effects retain outcomes atomically. Rejections retain
none. `IDEMPOTENCY_KEY_REUSED` means another canonical request owns the key;
inspect that outcome rather than overwriting its identity.

## Workspace and maintenance

- Use the validated read-only recipe first. Invalid Store/writer-contract diagnosis
  uses `podway --json version --identity`, which does not open the rejected Store.
  `doctor` and `workspace show` apply only when admission permits them.
- `WORKSPACE_BUSY` means an admission wait expired. `WORKSPACE_MAINTENANCE` means
  unavailable maintenance admission or an observed marker, possibly before timeout.
  Inspect `details.boundary`: `maintenance_lock`, `connection_gate`,
  `sqlite_read_admission`, `sqlite_write_lock`, `intent_marker`, `reset_marker`, `removal_marker`
  or `predecessor_migration`. Each boundary tries once even after the original
  budget expires; prompts do not renew it. Neither error authorizes deletion.
- Preserve unsupported state byte-for-byte. For a supported destructive discard,
  follow [reset-all](lifecycle.md#reinitialize-runtime-state): confirm the exact
  root and that no other Podway writer still uses it; retiring one needs its owner's
  separate authority. Obtain explicit intent for the unfenced request with a new
  key. UUID-fenced reset refuses writer, schema or binding mismatch, corrupt or
  unsupported older state. Restore access first for inaccessible state under
  separate access-repair authority. To preserve history after a binding mismatch,
  use the original worktree or its original location instead of discarding. I/O,
  permission and contention failures are not discard authority. A newer unsupported
  Store remains refused. Never edit databases, markers or filesystem identity.
- On `WORKSPACE_CONTRACT_MISMATCH`, compare writer/current manifest identities.
  Semver alone is insufficient. Only a declared release-owned predecessor permits
  migration; do not invent one. For non-destructive continuation, use the original
  writer matching `actual_contract_manifest_digest`, not an arbitrary binary with
  the same semver. Its `version --json --identity` `contract_manifest_digest` must equal that
  recorded digest. For discard, use the separately authorized reset-all path above.
  Use disposable worktrees for development with changed contract bytes.
- Initialization resumes keep the original key and `--repair` policy. Foreign
  writer/root markers and predecessor intents require supported read-only writer
  diagnosis. Resume only with the original writer and owning worktree.
- Store-backed lock contention supplies observe after the owner finishes. Store-less
  final-removal/residual contention supplies version identity and an exact retry.
  A repopulated-parent error does not prove invalid state: preserve the new contents
  and inspect the writer before considering any newly authorized removal.
- Init/reset/removal retain descriptor-safe operation markers. Ordinary reads do
  not resume a marked workspace. Follow only the matching canonical operation,
  original key, applicable UUID/root/fences and exact recovery recipe. A new key or
  deletion of `maintenance-intent.json`, `reset-all.marker` or `removal.marker`
  bypasses supported recovery.

Repair, reset and removal always need applicable lifecycle intent. If diagnosis
cannot establish a safe operation, report the error, exact worktree, observations
and smallest supported recovery action while preserving existing state.
