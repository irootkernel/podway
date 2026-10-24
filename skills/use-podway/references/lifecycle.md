# Workspace and Session Lifecycle

Read for an authorized initialization, start/replacement, archive/purge,
cancel/reset, workspace removal or repair. Preserve the exact target and deletion
boundary; one operation does not authorize another.

Apply the shared [command execution](../SKILL.md#command-execution) rules to the
command forms below; fenced templates do not supply semantic decisions.

## Inspect and initialize

- When identity matters, run `podway version --json --identity`. Require Output v4,
  version-result v2 and contract-manifest v2; record build/source/manifest identity,
  not semver alone.
- In an initialized worktree, use `podway observe --json`, `podway workspace show`
  and, when health diagnosis is needed, `podway doctor`. Consult `podway help
  <route>` for unfamiliar operations. Diagnostic failure is not repair authority.
- `podway init` requires explicit initialization intent for that worktree. Do not
  initialize a side-work fixture as part of a session owned elsewhere. It creates
  the reviewable `.podway/config.yaml`, Procedure directory and
  `.podway/.gitignore`; that ignore rule keeps `runtime/` untracked, and the
  authoritative task state and maintenance locks under `.podway/runtime/` are
  never tracked. A new-key init refuses an initialized workspace; diagnosed
  repair is a separate operation.
- Preserve configuration, existing state and unsupported Stores on failure.
  Follow [recovery](recovery.md), not manual database or marker edits.

## Prepare and start

Inspect `podway preset list` and `podway preset explain <name>`. Supported built-ins
are `analysis-v2`, `bug-fix-v2`, `small-change-v2` and `sw-dev-v2`; select from the
actual task. For custom source, use `$create-podway-procedure`, then copy preview's
exact start suggestion and `--expect-procedure-digest`. That suggestion is a bare
handoff: retain its source and digest, fill the task, add caller options and a
stable key, and obtain current applicable start fences before execution. Missing
custom digest fails with `DIGEST_CONFIRMATION_REQUIRED`; presets do not use it.

Under `--contract-generation 2`, observation offers each lifecycle window as
fenced templates: a prepared session offers `session.begin`, `session.reset`
and two replacement starts; an undisposed terminal session offers the two
`session.terminal_disposition` variants; a disposed terminal session offers
`session.archive`, `session.reset` and two fenced plain `session.start`
variants. Execute them under the shared caller-option rule above. The deprecated
generation 1 keeps the bare v5 lifecycle templates.

An unfenced plain `start --dry-run` previews only the Procedure. With paired
`--if-session-id` and `--if-session-revision`, it also verifies the current
session identity, revision and lifecycle read-only; a prepared, running or
undisposed terminal session is refused. A policy dry run with
`--on-existing preserve|delete` inspects that existing-session policy. The real
start always revalidates; a dry run is not authorization.

Human TTY start interviews only for an existing session. Enter or EOF continues
that session without creating one. Prepared state offers continue/delete; running
offers continue/preserve/delete; an undisposed terminal state offers continue or
a supplied handed-off/not-required disposition. Do not invent a summary, reference,
reason, actor or disposition.

JSON, quiet and non-TTY starts never prompt:

- `--on-existing preserve --supersede-reason <text>` retains an identified running
  session only when authorized. Add `--actor` only when supplied.
- `--on-existing delete` deletes identified prepared/running work only when
  authorized. Running deletion also requires `--progress-summary <text> --yes`.
- Plain start automatically archives a completed/cancelled current session only
  when its disposition matches the current revision. The next-session request
  permits this retention transition; it never permits evicting inactive history.

Require `podway.session-start-result/v4`: the new session is prepared at revision
0, with no attempt or goal. Re-observe, then use its fenced `session.begin`
template. Provide an initial goal only with explicit intent; a goal-tracking
Procedure permits `goal define` once later when begin omitted it.

## Choose the lifecycle boundary

| Operation | Effect |
| --- | --- |
| `cancel` | Terminally abandons running work; retains session/history; it is not a pause |
| `archive` | Frees the current slot while retaining an eligible terminal session |
| `reset` | Deletes the current session and its session-scoped history |
| `archive purge` | Permanently deletes one selected inactive session |
| `reset --all` | Replaces all workspace runtime state; keeps configuration and Procedures |
| `workspace remove` | Deletes the entire `.podway` tree; keeps the Git worktree and outside paths |

A terminal disposition applies to one exact session revision. `disposition
handed-off` and `disposition not-required` make completed/cancelled sessions eligible;
`superseded` is produced only by atomic running-session preservation at start.
Choose `handed-off` when terminal work has been delivered: supply its actual
summary and `--reference` to the commit, artifact or handoff target. Choose
`not-required` when no handoff is needed and state why. Neither choice invents
delivery or authorizes an external handoff.
Inactive sessions are immutable and cannot reactivate. Completed current sessions
can reactivate only under authorized rework or goal revision.

### Archive and purge

Re-read terminal identity/revision before `session.archive`, preserve every fence,
and require `podway.session-archive-result/v2` with `activity: inactive`.
Use `archive list` or `archive show --session-id <uuid>` for retained state.
Show includes disposition and successor identity for superseded work; request
bounded verbose history only when needed.

Purge needs explicit permanent-deletion intent, the exact inactive session ID and
revision from fresh list/show, workspace UUID, `--yes` and a stable explicit key.
Read `podway help session.archive_purge`. After response loss, reconcile through
outcome lookup; absence alone does not identify which caller deleted a session.
Re-list after success before selecting another candidate.

Above the entry advisory threshold of 10, offer exactly these alternatives without
executing or blocking work:

```bash
# Reinitialize all workspace runtime state.
podway --contract-generation 2 --worktree <absolute-root> --json reset --all --yes --if-workspace-uuid <workspace-uuid> --idempotency-key <new-key>

# Purge only the oldest inactive session from the same archive-list envelope.
podway --contract-generation 2 --worktree <absolute-root> --json archive purge --session-id <oldest-session-id> --if-session-revision <revision> --yes --if-workspace-uuid <workspace-uuid> --idempotency-key <new-key>
```

Both commands use `workspace.uuid` from the same archive-list envelope; purge also
uses `oldest.session_id` and `oldest.session_revision`. Summarize all-runtime
history loss before authorized reset-all.
Neither the threshold nor an error recipe authorizes deletion.

### Discard the current session

1. Observe and summarize the Procedure, lifecycle, current node/attempt, session
   ID/revision, disposition and recorded progress. `SESSION_NOT_FOUND` means there
   is nothing to discard; stop without escalating to reset-all.
2. Match the user's intent to the table. Delete with reset, retain with archive,
   abandon with cancel. Do not cancel merely to enable deletion or archival.
3. Read `podway help session.reset`; preview `reset --dry-run` with current workspace,
   session and revision fences. A dry run accepts no key, `--yes` or progress summary.
4. Prepared state is eligible for default reset. Terminal state may require a
   caller-supplied current disposition. An ineligible running session requires the
   force-mode history-loss boundary, a bounded progress summary and explicit intent.
5. After any disposition, re-observe and use the current `session.reset` template.
   Eligible reset needs no `--yes`; force mode uses the authorized summary and
   `--yes`. Supply a unique stable key and preserve every template fence. Check help
   rather than inventing a force flag.
6. Reconcile an uncertain result with the original key/request. On success,
   observation must return `SESSION_NOT_FOUND`; workspace initialization remains.

### Reinitialize runtime state

`reset --all --yes --idempotency-key <key>` discards all current/inactive sessions
and runtime history without backup or conversion, preserving reviewable configuration,
Procedures and outside files. It creates a fresh workspace UUID. Use it only for
the identified worktree with explicit destructive intent.

For a readable Store admitted by the current writer, retain its validated UUID
with `--if-workspace-uuid`. For writer, schema or binding mismatch, corrupt or
unsupported older state, a UUID-fenced reset is refused. Restore access first for
inaccessible state under separate access-repair authority. To preserve history
after a binding mismatch, use the original worktree or its original location
instead of discarding. I/O, permission and contention failures are not discard
authority. For these discard-eligible states, after read-only diagnosis, exact-target
identification and explicit discard authorization, use:

```bash
podway --contract-generation 2 --worktree <absolute-root> --json reset --all --yes --idempotency-key <new-key>
```

This is a new unfenced discard request, not a weakened retry of a failed fenced
mutation. Confirm that no other Podway writer still uses the target before discard;
retiring one needs its owner's separate authority.
Keep the original request/key after response loss. A newer unsupported Store stays
refused; use a binary supporting its generation instead. For the reset route,
`--force` is accepted only with `--all` and is unnecessary. Never edit markers or
databases to bypass admission.

## Remove Podway from one workspace

Removal needs an explicit request accepting deletion of the complete `.podway`
tree in one absolute existing Git worktree. Reset, purge or Git-worktree deletion
authority does not cover it.

1. Resolve and state the absolute root. Read `podway help workspace.remove`, then
   inspect workspace identity/health. Obtain the UUID from supported output.
2. Disclose loss of configuration, ignore rules, custom Procedures, current/inactive
   sessions, runtime state and unknown `.podway` content. Inspect `git status --short
   -- .podway` and `git ls-files -- .podway` so tracked/modified deletions are visible.
3. After authorization, re-read and require the same root and UUID. JSON/non-TTY use:

   ```bash
   podway --contract-generation 2 --worktree <absolute-root> --json workspace remove --force --if-workspace-uuid <workspace-uuid> --yes --idempotency-key <original-key>
   ```

   Unfenced human TTY removal must type the exact resolved root even with `--yes`;
   a UUID-fenced request can use `--yes` instead. Do not invent a missing UUID;
   invalid current-generation state needs human-terminal removal with exact-root
   confirmation. For that human-terminal path, omit `--json` and do not invent a
   UUID. Unsupported older or unrecognizable-header state requires an authorized
   reset-all first.
4. For JSON success, require `podway.workspace-removal-result/v2`. Report root,
   prior UUID, `podway_directory_removed` and `already_absent`; verify only the selected
   `.podway` tree is absent and its Git worktree remains.
5. For response loss/interruption, retain the original request/key/UUID. Follow
   marker-backed recovery for that same operation, root and filesystem identity.
   Do not regenerate a key, delete residual state or weaken a fence. Stop on an
   ambiguous target, identity or deletion boundary.

Do not combine lifecycle effects with Git cleanup, installation, commits or
publication unless separately requested. After each mutation, inspect current
observation or workspace status; report only the supported state change.
