# ADR-0031: Keyed Runtime Modes and Managed Development Service

- Status: Accepted
- Date: 2026-08-31
- Extends: [ADR-0003](0003-daemon-single-writer.md)
- Extends: [ADR-0004](0004-worktree-local-state.md)
- Partially supersedes: [ADR-0020](0020-managed-dev-runtime-isolation.md)

## Context

Podway currently has one installed production daemon and one `--dev` foreground
mode. Raw development mode keeps separate socket, registry, and log paths but
contends on the production singleton lock. Validated contributor and release
qualification runtimes avoid that contention by constructing private account and
development roots, but those runtimes are disposable and admit only worktrees
inside their managed sandbox.

Aquarium's managed-service producer contract requires a different lifecycle. One
immutable Podway development generation must provide a persistent, launchd-managed
daemon for real consumer worktrees while the installed production service remains
available. Publication and activation are separate: a newly published generation
must remain pending until an explicitly approved controller operation proves that
the active service can be replaced safely.

The product also needs a general answer to runtime isolation. Treating `prod` and
`dev` as a closed boolean distinction would duplicate the same special case for a
demo environment, an isolated qualification run, or another local integration
channel. Changing only the singleton lock name is insufficient because two daemon
instances would still collide through their socket, registry, logs, service
metadata, and worktree-local Store ownership.

## Decision

### Runtime mode identity

Podway adopts one bounded runtime mode key. A workspace configuration contains
either one scalar mode or no mode. Absence means `prod`; multiple simultaneous
modes for one worktree are not supported.

`prod` is reserved for the installed production topology. Every non-production
mode is a lowercase identifier matching
`[a-z][a-z0-9]*(?:-[a-z0-9]+)*` with a maximum length of 64 bytes. Values are
compared as exact bytes after validation and are never interpreted as paths,
commands, environment names, or extension points.

A daemon runtime is identified by the pair of its validated runtime root and mode
key. The key does not merely select a lock. It selects one complete namespace for
the lock, socket, registry, logs, bootstrap diagnostics, service metadata, and
recovery state. The runtime root remains part of the identity so two owner-private
qualification roots may safely use the same mode key.

The production mode retains the released per-user paths and service lifecycle.
An unmanaged named foreground mode uses a private namespace below
`~/.podway/modes/<mode>/`. A managed caller may supply another absolute root only
through validated metadata that binds the root, mode, purpose, owner, admitted
worktree boundary, and exact CLI and daemon executables. Malformed or mismatched
metadata fails closed without falling back to production or another named mode.

### Process and client selection

The CLI and daemon add an explicit `--mode <key>` selector. Omitting it selects
`prod`. Existing `podway --dev` and `podwayd --dev` invocations remain supported as
exact aliases for `--mode dev`; Aquarium's public entrypoint therefore does not
change.

A workspace mode never starts a process automatically. Production service
management, the Aquarium controller, contributor tooling, and release
qualification remain the explicit lifecycle owners for the daemons they create.
An arbitrary named mode is foreground-only unless a separately adopted service
owner manages it. Podway does not turn tracked configuration into a command or
LaunchAgent execution mechanism.

### Single-mode workspace ownership

Workspace configuration v1 remains valid and means `prod`. Workspace
configuration v2 adds one optional scalar `mode`; omission and explicit `prod`
have the same effective meaning. A canonical switch back to production writes a
v1-compatible document with no mode field while v0.2.7 remains the installed
stable consumer.

Before Store inspection or mutation, the CLI and daemon must agree with the
workspace's effective mode. The Store binds the initialized runtime state to that
same mode. A daemon, configuration, endpoint, or Store-mode mismatch fails closed
before normal Store admission. The minimal registry is already private to the
daemon namespace and cannot establish ownership for a differently bound mode.

One worktree may move between modes only through a native preview-and-apply
operation. The preview binds the exact worktree identity, source and target modes,
configuration digest, source runtime generation, complete source idleness, target
readiness, and a bounded confirmation token. Apply revalidates those facts,
closes source admission, removes only disposable runtime state and the exact
source registration, updates the configuration atomically, and initializes the
target binding. Procedures, supported configuration content and comments,
`.gitignore`, and the Git worktree are preserved. A partial destructive transition
records bounded recovery state and converges by retry; it never claims rollback
of state that was deleted.

### Aquarium development service

Aquarium owns immutable publication, its generic runtime root and locks, active
and pending generation selection, strict controller-result validation, and the
approval boundary. Podway owns the bundle contents, development LaunchAgent,
mode-`dev` paths, daemon lifecycle, identity checks, quiescence, rollback, and
recovery.

Publication of a new Podway managed-service generation changes only Aquarium's
pending selection. The active `dev` daemon continues serving its exact current
generation. An explicitly approved controller apply may activate the target only
after preventing new admission and draining in-flight clients, queued or running
jobs, maintenance, and recovery. A durable Podway session without executing work
does not by itself prevent a restart. The target must report matching CLI, daemon,
manifest, and service identities and become ready before Aquarium advances its
current selection.

Activation failure restores the prior exact service generation when the on-disk
transition is reversible. A candidate requiring an irreversible runtime migration
is not eligible for ordinary generation activation. If complete restoration
cannot be proven, the controller records bounded recovery debt and does not report
a successful rollback or activation.

### Qualification runtimes

Contributor and release qualification use the same keyed namespace model but
retain their stricter purpose-specific admission. Each release-qualification run
uses an owner-private root directly below `/private/tmp`, an exact binary snapshot,
and a sandbox-bound worktree. It never uses or changes the installed production
runtime or Aquarium's persistent `dev` runtime. Parallel qualification roots may
use the same mode key because the validated root is part of runtime identity.

## Rejected alternatives

- A closed `prod|dev` enum repeats the same isolation problem for every additional
  local channel and leaves qualification as a separate topology.
- Keying only the singleton lock permits collisions through the socket, registry,
  logs, metadata, and worktree Store.
- Allowing several mode values in one workspace configuration violates sole-writer
  ownership and makes one worktree-local database ambiguous.
- Automatically starting a daemon named by tracked configuration turns Podway
  into an execution mechanism and creates unbounded service lifecycle behavior.
- Using one shared global `release-qa` namespace makes concurrent qualification
  runs interfere and weakens cleanup ownership.
- Activating every published Aquarium generation restarts a shared development
  service before the exact candidate has an explicit approval boundary.

## Consequences

- Production, persistent Aquarium development, foreground named modes, and
  owner-private qualification daemons can coexist when they own different runtime
  identities and worktrees.
- A worktree still has one authoritative daemon and one worktree-local Store at a
  time; keyed daemons do not introduce parallel writers for the same worktree.
- Workspace config, service paths, managed-runtime metadata, daemon status,
  errors, Store binding, registry admission, CLI grammar, and mode switching need
  coordinated versioned contracts before runtime admission.
- ADR-0020's validated topology, exact-snapshot, sandbox, and fail-closed isolation
  requirements remain in force. Its `--dev`-only foreground model, raw production
  lock contention, and closed two-purpose topology are superseded by this decision.
- Aquarium's external managed-service protocol and `podway --dev` entrypoint remain
  unchanged.
