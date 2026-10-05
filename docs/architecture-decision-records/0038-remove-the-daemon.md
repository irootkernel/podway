# ADR-0038: Remove the Daemon and Commit Mutations In-Process

- Status: Proposed
- Date: 2026-10-05
- Acceptance owner: `V2DFC-002`, after the `V2DFC-001` feasibility gate passes
- Delivery owner: [V2DFC](../roadmap/README.md#v2dfc-daemon-free-core)
- Implementation plan: [adopted V2DFC dossier](../todo/TODO-podway-daemon-free-core.md)
- Supersedes on acceptance: [ADR-0003](0003-daemon-single-writer.md), [ADR-0012](0012-explicit-daemon-endpoint-and-canonical-per-user-podway-home.md), [ADR-0020](0020-managed-dev-runtime-isolation.md), [ADR-0022](0022-bounded-correlated-daemon-logs.md), [ADR-0023](0023-daemon-owned-bootstrap-log.md), [ADR-0029](0029-serve-phase-aware-daemon-readiness.md), [ADR-0031](0031-keyed-runtime-modes-and-managed-development-service.md), [ADR-0032](0032-retire-ordinary-user-runtimes.md)
- Supersedes in part on acceptance: [ADR-0004](0004-worktree-local-state.md), [ADR-0005](0005-rust-and-macos-first.md), [ADR-0006](0006-same-user-local-trust.md), [ADR-0024](0024-bounded-evidence-scale-and-paged-read-back.md), [ADR-0027](0027-state-aware-session-start.md), [ADR-0030](0030-retire-workspaces-and-prune-stale-registry-entries.md), [ADR-0035](0035-host-neutral-supervised-delegation.md), [ADR-0036](0036-use-canonical-git-paths-for-workspace-identity.md)
- Preserves: [ADR-0010](0010-generic-cli-json-integration.md), [ADR-0013](0013-native-apple-silicon-macos-only.md), [ADR-0021](0021-separate-session-preparation-from-execution.md), [ADR-0037](0037-bound-procedure-graphs-at-1024-nodes.md)

This ADR is a proposal. Until `V2DFC-002` accepts it, the ADRs listed above
remain the accepted authority for shipped behavior, and no linked record changes
status.

## Context

ADR-0003 made `podwayd` the sole normal writer because direct CLI writes "would
require complex file locking." It did not evaluate SQLite's own locking, which
the Store already relies on: every connection uses WAL with a bounded busy
timeout, and every write path runs in an IMMEDIATE transaction. The daemon adds
a second serialization layer, a durable job queue, above a database that
serializes writers by itself.

That layer is costly. About 18,000 production lines of daemon-only
infrastructure, about 8,300 lines in the service crate, and about 6,200
daemon-shaped Store lines exist to run it. Each mutation needs admission, an
encoded execution document, and a claimed execution step. Eight ADRs exist to
operate the daemon and its per-user runtime, and recorded startup, readiness,
and status incidents concentrate there. The LaunchAgent also ties the service
path to one platform.

No shipped feature and no documented candidate needs a resident process under
Podway's invariants: Podway executes no configured command and changes no
domain state without a request. The Graph Engineering candidate itself prefers
lease expiry evaluated inside the next state transaction over a timer.

On October 5, 2026 the owner decided to remove the daemon in `v0.2.13`, to
publish a new public contract generation without backward compatibility, to
delete daemon-era runtime state by reset without migration, and to make removed
commands fail with guidance. The evidence and the review of the earlier
proposal are recorded in the adopted dossier.

## Decision

1. **No resident process.** Every command runs to completion in the invoking
   process. `podwayd`, the LaunchAgent, the socket protocol, durable jobs,
   runtime modes, account runtime reset, daemon logs, and readiness are removed.
   A process that must stay alive for a future feature runs only while in use,
   executes through the same engine, and needs its own accepted decision. A
   resident or login-start process requires a new ADR.
2. **Direct writes.** Each mutation is evaluated and committed in one SQLite
   IMMEDIATE transaction on the owning worktree's Store, together with its
   fences and its success outcome. Preparation runs before the transaction and
   is revalidated inside it.
3. **Write-free reads.** A read observes one committed snapshot and never
   writes. Integrity scans move to `doctor` and maintenance.
4. **Process-independent maintenance.** An advisory lock file under
   `.podway/runtime/` is taken shared by ordinary opens and exclusive by Store
   creation, reset-all, workspace removal, and predecessor migration. Reset-all
   and removal keep marker-based recovery, resumed by an exact retry.
5. **Writer contract identity.** The Store records the contract-manifest digest
   of the binary that created or last migrated it. A binary with another digest
   refuses the Store unless it declares that digest as a forward-migratable
   predecessor. This replaces runtime modes for development isolation.
6. **Success-only idempotency.** Only successful mutations record outcomes. A
   rejection leaves no record, and fences prevent a retried request from
   applying a second state effect. A read-only outcome lookup by idempotency key
   replaces job lookup.
7. **No global state.** Podway keeps no per-user state. Workspace identity
   remains worktree-local.
8. **Legacy state fails closed.** Store generations 1 to 13 and daemon-era
   markers are classified without writing and rejected with a reset recipe.
   Nothing is converted.
9. **New contract generation.** Result families that carry `queue`,
   `admission`, or `job` move to one new generation, daemon-only families and
   error codes are retired, and retired commands and flags fail with
   `COMMAND_RETIRED`.
10. **Bounds and platform.** The 1 MiB request and response budgets remain as
    named input and output bounds. Releases remain native Apple Silicon macOS
    only.

## Rejected alternatives

- **Keep the daemon and trim operational features.** The job model, the
  per-command admission and execution cost, and startup recovery would remain.
- **An on-demand daemon that exits when idle.** It removes only the LaunchAgent
  installation step and keeps the rest of the daemon's complexity.
- **A dual path in which the CLI writes some commands and the daemon others.**
  Two writers on one Store would need a coexistence protocol that the product
  does not need.
- **Keep the existing result shapes with constant queue and admission values.**
  The owner chose a clean generation, and constant fields would misdescribe the
  semantics.
- **Migrate daemon-era Stores.** Runtime records are rarely revisited after a
  task ends, so a reset is acceptable and removes the migration chain.
- **Persist rejected outcomes for replay.** Fences already prevent a second
  state effect; retained failures would add key-reuse friction and retention
  without improving state safety.
- **Keep a global workspace catalog.** No remaining consumer needs one; a
  future console may introduce one under its own decision.

## Consequences

Positive:

- One binary with no installation, lifecycle, readiness, or startup recovery.
- Mutations cross one write transaction instead of a queue, a claim, and a
  terminal commit.
- The daemon crate, the service crate, the IPC protocol, runtime modes, and
  account runtime reset disappear, and a new command no longer needs admission
  and claimed-execution code.
- Development isolation follows the writer identity instead of named runtimes.

Negative:

- Detached execution, FIFO ordering among concurrent writers, and queue
  introspection are retired.
- Every existing client of the previous contract, including Aquarium and
  Dolgorae, must update before installation.
- Daemon-era sessions and history are deleted at cutover.
- Process-independent maintenance needs a lock file, an intent marker against
  writer starvation, and local filesystems with working advisory locks.
- Each release must declare its predecessor digest for Store migration.
