# System Architecture

## Context

Podway separates user interaction from state mutation:

```text
Human, script, or AI agent
          |
          | podway CLI
          | local, versioned IPC
          v
+--------------------------------------------------+
|                    podwayd                       |
|                                                  |
| request validation     read/query service        |
| worktree resolver      durable job admission     |
| procedure loader       per-worktree schedulers   |
| SQLite transaction executor                      |
+--------------------------------------------------+
          |                              |
          | queue and DB A               | queue and DB B
          v                              v
  Git worktree A                  Git worktree B
  .podway/runtime/                .podway/runtime/
  state.sqlite3                   state.sqlite3
```

The daemon is user-scoped. State is workspace-scoped. The daemon has no central task database.

## Component responsibilities

### `podway` CLI

The CLI MUST:

- parse command-line arguments;
- discover or accept the target worktree;
- connect to the user-scoped daemon;
- submit mutation requests with idempotency and concurrency preconditions;
- wait for terminal job state by default;
- support detached admission;
- send read queries to the daemon;
- render human-readable text or versioned JSON;
- provide help and shell completion;
- never open a live workspace database in write mode;
- never execute procedure-defined work.

Static commands MAY run without a worktree:

- `podway help` and command help;
- `podway version`;
- `podway daemon ...`;
- `podway preset list/show/explain`;
- `podway procedure validate/show/format/lint <file>`.

All other commands require a valid Git worktree and fail closed otherwise.

### `podwayd` daemon

The daemon MUST:

- run as one process per OS user;
- accept only local Unix-domain socket requests;
- verify peer user identity when the platform API supports it;
- resolve and validate the Git worktree for every workspace request;
- locate and validate the worktree-local database;
- admit mutation jobs durably;
- execute one mutation at a time per worktree;
- allow mutations in independent worktrees to execute concurrently;
- run all state transitions inside explicit SQLite transactions;
- serve reads from the latest committed state;
- recover queued jobs after restart;
- maintain a minimal path registry for recovery;
- prune bounded operational data;
- reject arbitrary command execution, network access, remote includes, and Git mutation.

### Pure domain engine

The pure domain engine MUST be independent of:

- async runtimes;
- SQLite;
- Git libraries;
- filesystem APIs;
- LaunchAgent APIs;
- IPC framing;
- clocks and random generators.

It receives validated state and a command, then returns either:

```text
new domain state + domain effects
```

or a structured domain error. Time, IDs, and external artifact metadata are supplied as explicit command inputs.

### Procedure and configuration loader

The loader:

- parses YAML or JSON;
- validates against versioned schemas;
- resolves only worktree-local files and embedded presets;
- applies explicit defaults;
- canonicalizes deterministically;
- computes SHA-256;
- produces immutable procedure snapshots.

It never loads remote data or executable plugins.

### Store

The store owns:

- database creation and migration;
- workspace metadata;
- procedure snapshots;
- session state;
- attempts, item values, blockers, and artifact references;
- durable job admission and claiming;
- idempotency records;
- bounded operational journal;
- transaction and pruning policy.

SQLite relational state is authoritative. There is no event-sourced rebuild path.

### Git worktree resolver

The resolver:

- discovers the worktree root from nested paths;
- rejects bare repositories;
- resolves Git common-directory and worktree-admin identity;
- enforces path containment;
- detects copied workspace UUIDs;
- supports safe root-path updates after a worktree move.

It does not stage, commit, branch, reset, push, or otherwise mutate Git.

### Service manager

The service manager owns the supported native Apple Silicon macOS installation and
lifecycle through a user LaunchAgent. Its trait boundary keeps platform mechanics
out of the domain; it is not a commitment to release another service backend.

## Data ownership

| Data | Owner | Location |
|---|---|---|
| Tracked workspace config | User/project | `.podway/config.yaml` |
| Tracked custom procedures | User/project | `.podway/procedures/` |
| Active session and attempts | Worktree | `.podway/runtime/state.sqlite3` |
| Worktree mutation queue | Worktree | same SQLite database |
| Procedure snapshot | Worktree session | same SQLite database |
| Artifact bytes | External owner | original path or external system |
| Minimal worktree registry | Daemon | user application-support directory |
| Daemon logs | Daemon | user logs directory |
| Socket and lock | Daemon | user-private runtime directory |
| LaunchAgent plist | User service manager | `~/Library/LaunchAgents/` |

## Trust boundaries

```text
same OS user boundary
  + podway CLI
  + podwayd
  + worktree files
  + local scripts and agents

outside Podway trust
  + other OS users
  + remote systems
  + external artifact stores
  + root or malicious same-user software
```

Podway uses filesystem permissions and peer UID checks to reject other users. It does not claim to protect against malicious processes running as the same user.

## Mutation path

```text
CLI resolves worktree
  -> sends request with command, preconditions, and idempotency key
  -> daemon validates peer, protocol, worktree, and request size
  -> daemon durably inserts a queued job with workspace sequence
  -> scheduler claims the lowest queued sequence
  -> transaction loads state and validates preconditions
  -> pure domain engine computes transition
  -> store persists state, receipt, journal, and terminal job result atomically
  -> daemon returns terminal response or detached job ID
```

## Read path

```text
CLI resolves worktree
  -> daemon validates request and worktree
  -> daemon reads latest committed state
  -> response includes workspace sequence, session revision, and pending jobs
```

Reads do not enter the write queue. Callers that require a quiescent view use `--wait-for-idle` or `--after-job`.

## Dependency direction

```text
podway-core
  ^-- podway-config <-- podway-presets
  ^-- podway-protocol
  ^-- podway-store
  ^-- podway-git
  ^-- podway-service

podway-config + podway-protocol + podway-presets
  + podway-store + podway-git + podway-service
  -> podway-daemon
  -> podway-cli
```

`podway-core` depends on no infrastructure crate. Infrastructure crates may depend on core types, but core MUST NOT depend on them.

## Performance and scale envelope

Podway is optimized for local task state, not bulk workflow data.

Design limits:

- at most one session per workspace;
- at most 64 stages per procedure;
- at most 128 items per stage;
- at most 256 queued jobs by default per workspace;
- IPC frame size at most 1 MiB;
- text item size at most 8 KiB by default and 64 KiB hard maximum;
- list item at most 1,000 entries hard maximum;
- artifact hashing is streaming and does not load the entire file into memory.

Normal status and non-artifact mutations SHOULD complete fast enough for interactive use. Artifact digest time is proportional to file size and is reported separately in diagnostics.
