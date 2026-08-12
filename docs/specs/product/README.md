# Product Overview

## Goal

Podway is a local procedure guard for one task being performed in one Git worktree.

It ensures that the current task follows an explicit ordered procedure. It shows the active stage, records the small set of required stage inputs, prevents accidental stage omission, and forces affected stages to be repeated after retry or return.

## Core lifecycle

The primary loop is:

```text
start one task session
  -> inspect the current stage
  -> satisfy the stage's required items
  -> advance exactly one stage
  -> retry or return when work must be repeated
  -> complete the final stage
  -> reset the worktree for the next task
```

Podway answers five operational questions:

1. Which procedure governs this task?
2. Which stage is active now?
3. Which required items are still missing?
4. What Podway action is valid next?
5. Which later stages must be repeated after rework?

## Product composition

Podway consists of two binaries:

- `podway`: the user-facing CLI;
- `podwayd`: a user-scoped daemon and the sole normal writer of Podway state.

The CLI submits mutations as durable jobs. The daemon processes one mutation at a time per worktree. Independent worktrees may progress concurrently.

Each worktree stores its Podway state in:

```text
.podway/runtime/state.sqlite3
```

Deleting the worktree deletes the task session and all task-local operational data. Podway deliberately creates no global copy of task state.

## Confirmed product decisions

| Area | Decision |
|---|---|
| Unit of work | One current task session per Git worktree |
| Procedure concurrency | Exactly one active attempt per session: a v1 stage attempt or v2 graph-node attempt |
| Write authority | `podwayd` is the sole normal writer |
| Queue | Durable FIFO per worktree |
| Cross-worktree behavior | Different worktrees may process mutations concurrently |
| Workspace requirement | Git worktree required; workspace commands fail closed otherwise |
| State location | Inside `.podway/runtime/` in the worktree |
| Initial database state | schema-0/uninitialized; initialize or migrate transactionally to canonical schema-v3 |
| Implementation | Rust |
| Release and support platform | Native Apple Silicon macOS only (`aarch64-apple-darwin`, thin arm64 Mach-O) |
| Service lifecycle | User LaunchAgent, started at login |
| UI | CLI, versioned JSON, zsh/bash/fish completion |
| Procedure data | Built-in presets and worktree-local YAML |
| Stage requirements | Typed items, not a general evidence ledger |
| Artifact handling | Path/reference, SHA-256 digest, byte size, and media type only |
| Authentication | Same-user local trust; no worktree access key |
| External integrations | Generic CLI and JSON only |
| Built-in presets | Retained v1: `sw-dev`, `bug-fix`, `docs-only`, `analysis`; implemented pre-GA v2: `sw-dev-v2`, `bug-fix-v2` |
| License | MIT |

## Success model

Podway succeeds when it improves current-task discipline without becoming a second task-management system.

A successful user experience has these properties:

- `podway status` makes the task state understandable in one screen;
- `podway next` identifies every missing required item and a concrete command that can satisfy it;
- `podway complete` cannot advance an incomplete or blocked stage;
- `podway retry` creates a clean attempt of the current stage;
- `podway return` forces the destination and reached downstream stages to be performed again;
- concurrent CLI or agent requests cannot silently overwrite or reorder state;
- daemon failure never causes a mutation to be applied twice;
- deleting the worktree deletes the Podway task state;
- no Podway operation executes user commands, mutates Git, reaches the network, or stores artifact bytes.

## Principles

1. **The current task comes first.** Historical data exists only to operate the current session correctly.
2. **The next action must be explicit.** Users and agents should not infer the active stage from prose or chat history.
3. **Rework is normal.** Retry and return are first-class transitions, not exceptional recovery paths.
4. **Writes are serialized, not hidden.** The daemon provides deterministic ordering and durable admission.
5. **Definitions are data.** Procedures contain no executable expressions, plugins, remote includes, or shell commands.
6. **Automation consumes JSON.** Human text is informative; versioned JSON is the integration contract.
7. **Local trust is stated honestly.** Podway prevents accidents, not malicious same-user behavior.
8. **The worktree owns the task state.** There is no durable remote or global task ledger.

## Non-goals

Podway is not a workflow server, project manager, CI system, command runner, Git
automation layer, review database, artifact store, long-term evidence ledger, or
multi-user access-control system. It performs no network I/O, stores artifact
metadata rather than artifact bytes, and exposes no Git mutation API.

## Supported boundary

The `v0.2.0` candidate is a native Apple Silicon macOS product with:

- matching `podway` and `podwayd` binaries;
- LaunchAgent installation and lifecycle management;
- byte-compatible released v1 commands, JSON families, IPC, and Procedure behavior;
- additive Procedure v2 authoring and runtime contracts;
- transactional schema-0, schema-v1, and schema-v2 migration to canonical SQLite schema-v3;
- four retained v1 presets and two shipped pre-GA v2 presets;
- shell completion for zsh, bash, and fish;
- complete crash, concurrency, Git, service, compatibility, and preset conformance tests;
- MIT licensing and release packaging.

Normal v2 session admission remains closed in the candidate source: development
execution requires the isolated disposable unlock, and no public v2 support boundary
exists yet. V2REL-006 implements production public admission and qualifies those
unlock-free artifact bytes; V2REL-007 may publish only those unchanged bytes after
explicit release authorization. The released `v0.1.2` surface
remains the compatibility baseline.

Linux, Windows, Intel macOS, translated, universal, fat, and cross-built artifacts
are not Podway releases. Conditional non-macOS implementation code is internal and
does not create a support or compatibility promise.

Continue with [goals and non-goals](goals-and-non-goals.md), [terminology and
invariants](terminology-and-invariants.md), and [user workflows](user-workflows.md).
