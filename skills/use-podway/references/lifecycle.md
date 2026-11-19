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
are `analysis-v3`, `bug-fix-v3`, `small-change-v3` and `sw-dev-v3`; select from the
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
variants. Preserve the fences and explicit-authority conditions of the offered
recipe. Generation 1 serves the same lifecycle guidance in v7 without executable
templates.

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

Require `podway.successor-session-start-result/v1`. The new session is prepared
at revision 0 with no active attempt or goal. A known first submission of plain
`start --with-observation` may supply the next fenced `session.begin` template
in the embedded v8 observation. `begin` opens the first base attempt. Provide
an initial goal only with explicit intent. A goal-tracking Procedure permits
one later `goal define` if begin omitted it. Replacement starts, previews and
terminal disposition do not support `--with-observation`. Use the shared
[rules for refreshing state](../SKILL.md#command-execution).

## Activate, return and replan

1. At the base anchor, record the required plan items and resolve blockers. In
   generation-2 `observe`, copy the `group.activate` stdin template and exact
   `result.plan`. Use `source: {"kind":"default"}` with the catalog's exact
   `result.group_digest` from `preset show` as `expected_definition`. For custom
   source, use `kind: "document"`, `format: "yaml"` or `"json"`, and the exact
   group document text in `source.source`. Use the group preview's canonical
   `result.digest` as `expected_definition`. Supply the actual working-copy basis descriptor
   and digest and a new key. First-ever activation uses no predecessor and
   `prior_work: []`. The input is `podway.group-activate-input/v1`; the
   successful transaction opens one fresh scoped group entry attempt.
2. Work through the selected group's declared actions and decisions. Once its
   terminal action is ready, use the fenced `group return` template. The atomic
   return closes the group's terminal attempt and opens the base's declared
   successor. It does not finish the session. Follow base guidance through any
   goal assessment and the base terminal. A successful group can remain selected
   while the base evaluates its returned outputs.
3. For a real failure, use the actual per-decision, per-execution `decision_entry`
   (or `next_decision_entry` when projected for a preceding assessment Action).
   The shipped policy uses first-entry ordinary rework only with a viable
   correction, second-entry lightweight screening, and third-entry or later
   scored assessment and a current Coordinator choice. A declared serious
   structural defect can justify scoring earlier. Default assessment items
   preserve actual verdict and stage and use zero as the unscored placeholder.
   For default scores, evaluate structural need (50), lack of viable local
   repair (30) and demonstrated lack of progress (20) from current evidence.
   Below 80, keep the plan and apply a justified local correction or remain
   blocked. At 80 or higher, continue with a viable local correction, make an
   explicit replan choice, or retain a blocker with the missing facts. Record
   the reason for the choice. Advance immediately on success. Never manufacture
   a failure, inflate a score, repeat a decision to raise its entry ordinal, or
   run an empty correction loop. When a real dependency prevents an admissible
   next step, record a blocker and resume only when the facts change.
4. From a current eligible replan decision use `group replan --stdin` with
   `podway.group-replan-input/v1`. Include the observed option, reason, current
   working-copy basis and bounded feedback. Current declared guard evidence
   decides eligibility; the request has no score override. Settle affected
   descendants and reconcile uncertain external effects before closing the
   selection. When a new topology stays in scope, review the current plan under
   the original authority. Expanded scope or newly uncovered effects require
   their own approval. A base review must be explicitly declared for replan.
   Successful replan commits the decision, closes the selection and opens a
   new anchor. It retains previous attempts and never rewrites the working copy.
5. For later activation, use the fresh plan and basis. Link the latest prior
   execution as predecessor and include 1..128 justified prior-work dispositions
   against retained work of this session. A disposition can cite the group or
   the exact retained attempt or item allowed by the schema. State whether to
   `retain`, `revalidate`, `modify` or `revert` it and why. Those facts may come
   from an older same-session group even when a newer predecessor exists.
   Revalidate work under current inputs and records before using it at a gate.

The [worked continuation walkthrough](../../../docs/examples/continuation-v3.md)
is distributed with the paired source. Select it from the exact validated source
commit or matching published tag when running it. If only the skill is
installed, find `docs/examples/continuation-v3.md` in that source revision.

## Read retained history

Use `group list --session <id>` to discover retained execution identities and
`group show --session <id> --execution <id>` for one retained record. Read its
dispositions with `group prior-work --session <id> --execution <id>
--definition-digest <sha256>`.
Use `history dag` and `history attempts` with the retained session and definition
identity and the `--group-execution` when querying a group. Use `history item` and
`history blockers` with the exact retained `--attempt` JSON object. Add
`--archived` for these group/history queries on archived sessions. Keep the query
identity fixed when following an opaque `next_page_token`. These reads are
write-free and grant no eligibility to cite retained evidence as current work.

## Choose the lifecycle boundary

| Operation | Effect |
| --- | --- |
| `cancel` | Terminally abandons running work; retains session/history; it is not a pause |
| `archive` | Frees the current slot while retaining an eligible terminal session |
| `reset` | Deletes the current session and its group and attempt history |
| `archive purge` | Permanently deletes one selected inactive session and its retained group history |
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
and confirm the exact session is now inactive.
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
