# Architecture

## Request flow

```text
user or local automation
  -> podway CLI grammar and bounded input
  -> typed Direct Engine request
  -> native Git/workspace classification and side-effect-free preparation
  -> worktree maintenance admission
  -> one SQLite IMMEDIATE transaction for an ordinary mutation
  -> fenced pure domain transition + validated success outcome + journal
  -> COMMIT, then the complete Output v4 response
```

A read opens one WAL read transaction and returns one committed snapshot without
writing or waiting for a queue. An ordinary mutation prepares Procedure/artifact
inputs before locking, revalidates them inside the transaction and commits state
and success together. No durable admission, detached execution, socket or resident
process remains. Filesystem maintenance uses exclusive leases and multistage
marker recovery; one SQLite transaction alone cannot cover directory replacement
or complete workspace removal.

## State ownership and concurrency

Every Git worktree owns `.podway/config.yaml` and
`.podway/runtime/state.sqlite3`. Independent command processes serialize writes
through SQLite IMMEDIATE transactions and share the process-independent maintenance
lock for ordinary opens. Creation, reset-all, removal and declared predecessor
migration take the exclusive lease. Lock admission is deadline bounded; stale
workspace/session/attempt/item/goal fences fail closed without an outcome.

Only successes record idempotency outcomes atomically with their effect. Exact-key
replay returns the complete original response before fresh fences or preparation
can reinterpret it. Rejections record nothing. FIFO order and execution after the
caller exits are retired. Reset/removal recover through descriptor-safe markers
only on an exact retry of the original canonical operation.

## Worktree boundary

Native Git resolution distinguishes main and linked worktrees, validates canonical
identity and component containment and rejects symlink escapes. Podway never
mutates Git. Repair accepts a supported moved-worktree identity; a copied or
incompatible UUID fails closed. There is no global task copy or workspace registry.
Legacy generations 1–13 are classified without mutation; only an explicitly
confirmed reset discards their history and creates fresh direct state.

## Domain boundary

The pure core receives explicit IDs, timestamps, hashes and prepared inputs. It
validates one transition and returns a successor. Infrastructure owns I/O,
transaction admission and persistence; external work and semantic judgment stay
with the caller. Procedure v2 retains one cursor and one active attempt. Eight
crates preserve the established dependency direction; the CLI reaches native
Store/Git behavior through the Engine.

## Public contracts and compatibility

Current success/error envelopes are Output v4 and Error v2. Command-selected
results and details are closed manifest-bound schemas with named input/output
budgets. The local integration surface is CLI/JSON and typed Engine APIs; there is
no IPC framing or daemon compatibility shim. Version-result v2 and manifest v2
identify the single binary, and the Store binds its writer-manifest digest.
Unknown/newer/undeclared writer state is refused rather than silently reinterpreted.

## Observability and trust

Diagnostics are command envelopes and the Store's bounded operational journal.
Podway neither reads nor creates per-user runtime state, logs or service metadata.
It performs no network I/O and trusts same-user local processes; filesystem safety
checks prevent accidental targeting errors, not a malicious same-user adversary.
A future foreground console needs its own accepted decision and the same Engine
boundary; resident/login-start/global authority is not an extension of this design.

Detailed authorities:
[system](system.md),
[repository structure](repository-structure.md),
[worktree boundary](git-worktree-and-filesystem.md),
[transactions and idempotency](../specs/storage/transactions-concurrency-and-idempotency.md),
[recovery and maintenance](../specs/storage/recovery-retention-and-maintenance.md),
and [ADR-0038](../architecture-decision-records/0038-remove-the-daemon.md).
