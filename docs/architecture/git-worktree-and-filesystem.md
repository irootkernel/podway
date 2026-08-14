# Git Worktree and Filesystem Model

## Workspace boundary

Every workspace-scoped operation MUST resolve to a valid, non-bare Git worktree. Podway fails closed when discovery or identity validation is uncertain.

Static commands that do not use workspace state are exempt, as listed in the CLI specification.

## Worktree discovery

The resolver accepts:

- the process current directory; or
- an explicit `--worktree <path>`.

The built-in read-only resolver walks the filesystem metadata to find the containing worktree. It MUST support:

- a main worktree with a `.git` directory;
- a linked worktree with a `.git` file;
- invocation from nested directories.

It MUST reject:

- no repository found;
- bare repositories;
- malformed `.git` directories or files;
- inaccessible worktree administrative metadata;
- ambiguous or unsupported repository layouts.

The resolver MUST NOT depend on invoking the `git` executable for discovery.

## Worktree layout

```text
<worktree>/
  .git/ or .git file
  .podway/
    config.yaml
    procedures/
      custom.yaml
    .gitignore
    runtime/
      state.sqlite3
      state.sqlite3-wal
      state.sqlite3-shm
      reset.marker          # only during destructive recovery
      development-v2.marker # only in the helper-managed disposable development runtime
```

`.podway/.gitignore` MUST include:

```gitignore
runtime/
```

`podway init` appends this entry if needed without overwriting unrelated ignore rules.

The development runtime helper has one narrow, development-only exception to the daemon's normal
write authority: after the isolated `podway --dev init` succeeds, it atomically publishes
`development-v2.marker` through the private runtime directory. The marker contains no task state
and grants no transition authority; the daemon treats it only as disposable-runtime provenance and
revalidates it before admitting requests through the contributor-only development
admission path. Release runtimes use the public Procedure v2 admission path and do
not consult this marker. Normal session reset and workspace
reset-all retain it so the helper-managed sandbox remains disposable after state replacement. The
helper's `clean` operation removes the complete managed root, including the marker.

## Path containment

Podway canonicalizes paths before use.

Requirements:

- `.podway` MUST resolve within the worktree;
- `.podway/runtime` MUST be a real directory, not a symlink;
- configuration procedure paths MUST resolve within the worktree;
- local procedure files MUST NOT traverse symlinks, including links whose targets remain inside the worktree;
- local procedure admission MUST reject worktree-root or source-path replacement detected during the bounded read;
- local artifact paths MUST canonicalize inside the worktree;
- external artifact locations use the explicit `reference` form and are never treated as filesystem paths;
- `..` components and symlinks MUST NOT permit escape.

Violations fail with `PATH_OUTSIDE_WORKTREE` or `WORKSPACE_PATH_UNSAFE`.

## Permissions

On macOS and Unix-like systems:

- `.podway/runtime/` SHOULD be mode `0700`;
- `state.sqlite3` and its side files SHOULD be mode `0600`;
- daemon runtime and registry directories MUST be user-private;
- existing stricter permissions are preserved;
- Podway does not loosen permissions on the worktree or tracked config.

## Workspace identity

The database stores:

- a generated workspace UUID;
- a fingerprint of the Git common directory identity;
- a fingerprint of the specific worktree administrative directory;
- the most recently validated canonical root.

The fingerprints MUST be derived from stable Git metadata and canonical filesystem identity, not only from path strings. Exact platform details are internal and versioned with the database schema.

The workspace UUID is non-secret. It detects copied runtime state and path reuse.

## Root move

When a worktree moves:

1. the CLI discovers the new canonical root;
2. the daemon opens the local database at that root;
3. the Git identity and workspace UUID are compared;
4. if both match the registered workspace, the daemon updates `last_known_root`;
5. queued jobs resume.

`podway workspace repair` performs the same validation explicitly and reports every change. It MUST NOT adopt a database whose Git identity conflicts.

## Copied runtime state

Copying `.podway/runtime/` to another live worktree can duplicate the workspace UUID.

If the daemon observes the same UUID at two live roots, both mutation paths fail with `WORKSPACE_ID_CONFLICT` until the copy is removed or one workspace is destructively reinitialized:

```bash
podway reset --all --force --yes
```

Podway does not merge copied sessions.

## Worktree deletion

Deleting the worktree deletes:

- the current session;
- procedure snapshot;
- attempts and item values;
- blockers and artifact metadata;
- queued and terminal jobs stored in that database;
- command receipts and operational journal.

Before each queued mutation, the daemon revalidates the root. If it is gone:

- the scheduler stops;
- the global registry entry is removed;
- open clients receive `WORKTREE_GONE` where possible;
- no task state is reconstructed globally.

Database connections SHOULD be closed after a short idle period so deleted files are not held indefinitely.

## Tracked and untracked content

Tracked, reviewable project content:

- `.podway/config.yaml`;
- `.podway/.gitignore`;
- `.podway/procedures/*.yaml`.

Untracked disposable runtime content:

- all files under `.podway/runtime/`.

`podway doctor` MUST detect when runtime files are tracked or not ignored and report remediation.

## Workspace configuration

Reference configuration:

```yaml
schema: podway.workspace/v1
procedure_paths:
  - .podway/procedures
default_preset: sw-dev-v2
job_queue:
  max_pending: 256
ui:
  show_stage_in_prompt: false
```

Rules:

- paths are worktree-relative;
- no remote paths, URLs, plugins, commands, or secrets;
- unknown fields are rejected in v1;
- a running session uses its stored procedure snapshot even if config later becomes invalid;
- authoring operations such as `start` and `procedure validate` require valid current config.

## Initialization behavior

`podway init` is idempotent.

It MUST:

1. validate the worktree;
2. create `.podway/` and `procedures/` if absent;
3. create or validate `config.yaml`;
4. create or update `.gitignore`;
5. create the runtime directory safely;
6. ask the daemon to initialize the database and workspace identity;
7. update the minimal daemon registry.

Incompatible existing content fails with `WORKSPACE_INIT_CONFLICT`. `podway init --repair` may fix missing directories, ignore rules, and registry paths, but MUST NOT overwrite a valid custom config or adopt conflicting state.
