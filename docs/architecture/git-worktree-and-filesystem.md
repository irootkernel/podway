# Git Worktree and Filesystem Model

## Workspace boundary

Each workspace operation MUST resolve a valid non-bare Git worktree. Discovery,
containment, Store binding or native replacement uncertainty fails closed.
Static commands are exempt as listed in the
[CLI specification](../specs/interfaces/cli-specification.md).

## Worktree discovery

The native read-only resolver accepts the current directory or explicit
`--worktree <path>`. It supports a main `.git` directory, a linked `.git` file and
nested invocation without executing `git`. It rejects missing/bare repositories,
malformed or inaccessible administrative metadata, unsupported layouts and
ambiguous identities. It exposes no Git mutation API.
The executable [resolver contract](../../contracts/interfaces/git-resolver-v1.json)
retains `ARC-005`, `SEC-002` and workspace invariants `INV-W01` through `INV-W05`.

## Worktree layout

```text
<worktree>/
  .git/ or .git file
  .podway/
    config.yaml
    procedures/
    .gitignore
    runtime/
      state.sqlite3
      state.sqlite3-wal       # when present
      state.sqlite3-shm       # when present
      maintenance.lock
      maintenance-intent.lock
      maintenance-intent.json # while a maintenance intent is recoverable
      reset-all.marker        # during reset-all recovery
      removal.marker          # during removal recovery
```

`.podway/.gitignore` includes `runtime/`. Initialization appends that rule without
overwriting unrelated ignore entries. Temporary private bootstrap files and
marker-publication files are internal recoverable maintenance state, not tracked
Procedure assets. All authoritative runtime state remains in this worktree.

Legacy `reset.marker`, `workspace-removal.marker`, `development-v2.marker` and
`.podway/mode-switch.json` are refused without writing. There is no current
contributor-runtime provenance marker, managed account root, socket or registry.
Confirmed reset-all removes legacy runtime state and creates a new Store; it
performs no daemon-era migration or automatic backup.

## Path containment

Runtime and local source access must remain contained in the owning worktree.
`.podway` and `runtime` are validated real directories. Local Procedure sources
reject symlink traversal, including links whose targets remain in the worktree.
Bounded source reads detect native root/source replacement. Runtime files and
lock files use nofollow inspection/opening, regular-file checks and descriptor
revalidation so renamed ancestors or swapped files cannot grant stale authority.

Local artifact paths resolve within the worktree, are read-only and stream-hashed.
External `reference` locations remain caller assertions and are never opened as
local filesystem paths. Canonical paths do not replace descriptor checks across
an operation. Internal representations preserve non-UTF-8 path bytes.

## Permissions

Native runtime directories are private (`0700`) and created state/lock files use
private permissions (`0600`) where supported. Podway does not relax worktree or
tracked configuration permissions. These controls prevent accidental exposure;
they do not protect against root or malicious same-user modification.

## Workspace identity

Generation-14 Store binding contains the generated workspace UUID, Git common
and worktree-administration fingerprints, and last validated canonical root.
The fingerprints retain ADR-0036's role-tagged hashes of lossless canonical path
bytes. Device/inode/type/birth-time evidence detects replacement during a request;
it is not persistent identity. Replacing a Git directory object at the same
canonical paths does not itself change durable workspace identity.

Resolver and execution retain native evidence long enough to validate the
current operation. There is no scheduler context or global UUID catalog. The UUID
is non-secret identification; it does not authenticate a caller.

The Store also records the binary's contract-manifest digest in `writer_contract`.
A different undeclared writer fails before writable access, including reads and
old-key replay. The current production predecessor declaration is empty.
Only an explicitly declared forward predecessor may migrate under exclusive
exclusion in a separately adopted release/open-development change.

## Root move

A linked root may move while its common/admin paths retain identity. Reads report
`root_rebind_required` and the stored root without writing. A successful mutation
updates the locator atomically with its state/outcome; explicit `workspace repair`
uses that same fenced transaction. There is no registry repair or queued work to
resume. Rejections and replay do not opportunistically rebind the root.

Moving the common/admin paths changes persistent identity and is not silently
adopted. Use the supported explicit destructive reset policy; Podway does not
merge or reinterpret state under a new Git identity.

## Copied runtime state

A runtime copied to a different Git worktree has a mismatched Store binding and
fails closed. No global scan attempts to discover every offline copy or disable
another root. Removing a copy or explicitly resetting the target is an operator
choice. Copied task histories are never merged.

## Worktree deletion

Deleting a worktree deletes its local sessions, snapshots, values, blockers,
artifact metadata, delegation receipts, outcomes and journal. Native resolution
and precommit revalidation reject absent or replaced roots. There is no daemon
connection to close, global task reconstruction or registry entry to prune.
Descriptor-safe `workspace remove` deletes only the selected `.podway` boundary,
preserves Git and unrelated worktree files, and checks its original UUID.
Exact interrupted retry follows the persisted removal marker; after complete
removal, the deleted runtime retains no response or outcome.

## Maintenance exclusion and recovery

Ordinary opens hold a shared `maintenance.lock` lease. Exclusive maintenance first
holds `maintenance-intent.lock` so fresh opens cannot continually overtake it,
then waits for existing holders within the command deadline. Marker and native
file identities are checked again after acquisition. Missing files are
reclassified so an active marker is reported as maintenance rather than accepted
as an uninitialized Store.

Cold write-free reads lacking required WAL/SHM state take exclusive exclusion and
use an immutable/heap WAL snapshot. They MUST NOT repair sidecars. Doctor and
current creation/reset/removal use exclusive exclusion. Any future activated
predecessor migration also requires exclusive exclusion and separate adoption;
Engine currently has no migration caller. SQLite IMMEDIATE transactions serialize
ordinary writers; native locks protect the filesystem generation rather than
implementing a FIFO scheduler.

Reset-all/removal persist typed intent and phase markers. Only an exact retry with
the original key, request identity and required fences resumes them. Removal
deletes the intent and named locks first, retaining the validated removal marker
as the exact-retry identity until that final marker is deleted. Reset keeps its
intent until reset-marker cleanup completes. The held runtime inode exclusion
survives marker and lock-name deletion. Durable marker writes and directory
changes are synced through the existing descriptor-safe primitives. Never
manually edit these files or a database to simulate recovery. The
[recovery specification](../specs/storage/recovery-retention-and-maintenance.md)
owns retention and public retry recipes.

## Tracked and untracked content

Tracked project content may include config, `.podway/.gitignore` and custom
Procedures. Runtime state MUST remain untracked and ignored. Ignore/layout policy
is established by initialization and native layout checks; Doctor's current
SQLite/domain checks must not be described as a Git-index audit it does not perform.

## Workspace configuration

```yaml
schema: podway.workspace/v1
procedure_paths:
  - .podway/procedures
default_preset: sw-dev-v2
ui:
  show_stage_in_prompt: false
```

Procedure paths are local and relative; configuration contains no remote includes,
plugins, executable commands or credential feature. The legacy v1 `job_queue`
member is accepted and ignored, and new configuration omits it. Config v2 accepts
only absent or `prod` mode; named modes fail with guidance to remove mode and reset.
These formats do not select an account namespace.

Existing-workspace resolution validates current workspace config before Store
access. A session retains its immutable Procedure snapshot even if the original
Procedure source changes; that does not exempt invalid workspace configuration
from current admission checks.

## Initialization behavior

`init` validates Git identity and publishes a safe local layout, custom-config
preservation and ignore rule before initializing the current Store under
exclusive exclusion. Private Store construction is checkpointed/synced and
published without clobbering authoritative state. Compatible existing state
returns `WORKSPACE_ALREADY_INITIALIZED`, except exact-key replay of the original
success. Incompatible content fails closed. Initialization classifies persisted maintenance and
newer-generation compatibility before validating configuration, so invalid
configuration cannot hide either refusal.

`init --repair` may repair supported missing layout/ignore details; it must not
overwrite valid custom config or adopt conflicting identity. `workspace repair`
repairs the current Store's root locator, not a removed global registry.
Initialization, reset-all and removal tests use owned fixtures, never an active
user worktree or installed old service.
