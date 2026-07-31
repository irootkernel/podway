# Architecture

## Request flow

```text
user or local automation
  -> podway CLI
  -> length-prefixed JSON over a Unix-domain socket
  -> podwayd peer and request validation
  -> per-worktree durable FIFO admission
  -> one workspace worker
  -> pure domain transition inside a SQLite transaction
  -> versioned response to the CLI
```

Read commands return coherent committed views. Mutation commands are admitted with
an idempotency key and preconditions before the daemon executes them. A response
may be synchronous or detached, but admission and terminal results remain durable.

## State ownership and concurrency

Each Git worktree owns `.podway/config.yaml` and
`.podway/runtime/state.sqlite3`. `podwayd` is the sole normal writer. It executes
one mutation at a time for a workspace identity while bounded worker resources
allow different worktrees to progress concurrently.

SQLite transactions cover state changes, job results, and revision increments.
Compare-and-set preconditions prevent stale clients from silently overwriting a
newer session or item. Recovery converges interrupted jobs and ownership markers
without applying a mutation twice.

## Worktree boundary

The Git resolver distinguishes main and linked worktrees, records stable identity
inputs, validates containment component by component, and rejects symlink escapes.
Podway never mutates Git. A move can be repaired when identity still matches;
copied workspace UUIDs fail closed. Deleting a worktree deletes its local task
state because no global state copy exists.

## Domain boundary

The pure core receives explicit IDs, timestamps, hashes, and prepared inputs. It
validates one transition and returns the next immutable state. Infrastructure
performs I/O before entering the transition and persists the result atomically.
This keeps lifecycle rules deterministic and testable without SQLite, Git, sockets,
or macOS services.

## IPC and compatibility

The implemented local protocol uses bounded frames and versioned request, success,
and error envelopes. Same-user peer checks prevent accidental cross-user access to
the socket; they do not defend against malicious processes running as the same
user. The current generic response envelope tolerates additive fields. The accepted
v0.1.0 [automation contract](reference/interfaces/34-automation-client-contract.md)
replaces that policy for integration-critical result and error-detail objects with
versioned closed schemas.

## macOS service and observability

The CLI installs `podwayd` as a per-user LaunchAgent. The implementation uses the
OS-account-derived `~/.podway` root, a fixed per-user lock, an explicit no-fallback
socket option, and the verified daemon's actual absolute path. Structured logging
remains local and bounded. Podway emits no telemetry and performs no network I/O.

Detailed contracts are available for the
[system](reference/architecture/10-system-architecture.md),
[daemon and queue](reference/architecture/11-daemon-and-write-queue.md),
[worktree boundary](reference/architecture/12-git-worktree-and-filesystem.md),
[macOS service](reference/architecture/13-macos-service.md), and
[storage transactions](reference/storage/41-transactions-concurrency-and-idempotency.md).
