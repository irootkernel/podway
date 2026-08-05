# Contributor Development Runtime

Use the managed development runtime when you need a disposable Podway daemon
and worktree that cannot touch the installed production service state.

The helper is `tools/dev_runtime.py`. It builds the pinned debug `podway` and
`podwayd` pair, snapshots both binaries under a private root, and drives only
that snapshot through `--dev` with debug-only account-root isolation.

## Commands

```bash
make dev-daemon
python3 tools/dev_runtime.py daemon

python3 tools/dev_runtime.py init
python3 tools/dev_runtime.py run -- --json status
python3 tools/dev_runtime.py clean --yes
make dev-runtime-test
python3 tools/dev_runtime.py self-test
```

Direct `python3 tools/dev_runtime.py ...` invocation resolves Cargo through
`rustup` for the checkout-pinned `1.97.1` toolchain from `rust-toolchain.toml`,
even when Make has not adjusted `PATH`. It fails clearly when that toolchain is
missing. Builds honor `CARGO_TARGET_DIR` using the same relative/absolute
resolution as `tools/run_g005_vertical.py`.

- `daemon` builds with `--locked`, verifies debug test-isolation capability,
  snapshots matching immutable binaries, writes private metadata atomically, and
  `exec`s the snapshot `podwayd --dev` in the foreground so signal handling stays
  with the daemon process.
- `init` creates one managed disposable Git sandbox under the runtime root and
  runs the snapshotted `podway --dev init` only against that sandbox.
- `run -- <podway args>` invokes the same snapshot CLI from the managed sandbox,
  prepends `--dev`, and rejects `--socket`, `--worktree`, `--dev`, daemon
  lifecycle commands, and `terminate`.
- `clean --yes` revalidates the exact managed root, ownership, directory modes,
  and symlink-free tree; acquires the isolated daemon lock; proves no live
  endpoint owns the socket; atomically renames the well-known root to a unique
  same-parent trash path that must not already exist; re-audits that trash tree;
  and deletes only the trash tree while the lock descriptor still refers to the
  renamed inode. If rename fails, nothing is deleted. If trash deletion fails,
  the recoverable trash path is reported and a recreated well-known root is left
  untouched. Clean fails closed when the lock cannot be acquired or a live
  endpoint is observed.
- `self-test` covers toolchain/target-dir resolution, path/permission/symlink/
  traversal and command-escape failures, snapshot containment, identity metadata
  mismatches, production-lock disjointness, rename-to-trash cleanup with
  recreated-root survival, and a real dual-daemon coexistence regression. It is
  part of `make test` through `make dev-runtime-test`.

## Managed root and sandbox

The persistent root is:

```text
/private/tmp/podway-dev-<uid>-<12-char-sha256-of-canonical-checkout>
```

Below that root the helper keeps:

| Path | Role |
|---|---|
| `account/` | `PODWAY_TEST_ACCOUNT_ROOT`; `account/.podway/run/podwayd.lock` is the isolated singleton lock |
| `dev/` | `PODWAY_DEV_HOME`; owns the dev socket, registry, and logs |
| `sandbox/` | disposable Git worktree used by `init` and `run` |
| `snapshots/<id>/` | immutable matching `podway` and `podwayd` binaries |
| `runtime.json` | bounded private metadata for the adopted snapshot |

The helper validates exact root identity, current-user ownership, `0700`
directories, no symlinks, and macOS Unix-socket path capacity before trusting or
deleting the tree. Managed roots and lock paths are checked as disjoint from the
effective account's production lock path
(`<account-home>/.podway/run/podwayd.lock`) without creating or locking that
production path.

## Snapshot and restart behavior

Each `daemon` invocation rebuilds the debug pair and prepares a content-addressed
snapshot. It adopts that snapshot only after the previous isolated lock is free
and no live endpoint owns the managed socket. Snapshot binary paths must be
absolute, free of `.` / `..` components, resolve inside the managed root, and pass
no-symlink ownership checks before execution. Runtime metadata must also match
the freshly derived checkout, uid, root, account_root, `dev_home`, and sandbox
identity. A running managed daemon keeps serving its adopted snapshot until you
stop it and start `daemon` again.

## Production coexistence

The managed runtime sets debug-only `PODWAY_TEST_ACCOUNT_ROOT` together with a
separate `PODWAY_DEV_HOME`. That makes the singleton lock disjoint from the real
account's `~/.podway/run/podwayd.lock`, so the managed daemon can coexist with an
installed production LaunchAgent.

Raw `podwayd --dev` without this helper still uses the production singleton lock
and therefore still contends with an installed production daemon and with the
release-archive production-lock preflight.

The managed runtime does not change release-archive contents or the installed
singleton/preflight behavior used by packaging qualification.

## Procedure v2 admission

This runtime does not unlock Procedure v2 admission. Development admission stays
closed until `V2PLT-009` lands. Use the helper for isolated v1 contributor work
and packaged-style `--dev` flows only.

## Why the helper stays sizable

The tool remains larger than a thin wrapper because it owns several fail-closed
surfaces in one process: pinned toolchain and `CARGO_TARGET_DIR` resolution,
immutable snapshot pairing and containment, identity metadata checks,
path/ownership/symlink/socket-capacity audit, argument escape rejection,
rename-to-trash cleanup, and a binary-backed dual-daemon regression. Shared
walk/audit helpers keep that surface from splitting into multiple unsafe scripts.
