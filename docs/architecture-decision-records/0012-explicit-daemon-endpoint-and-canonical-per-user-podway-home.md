# ADR-0012: Explicit Daemon Endpoint and Canonical Per-User Podway Home

- Status: Accepted
- Date: 2026-07-23

The statement below that every daemon instance contends on the production lock is
superseded for validated managed `--dev` runtimes by
[ADR-0020](0020-managed-dev-runtime-isolation.md).

## Context

Podway currently derives service paths from ambient `HOME` and `TMPDIR` values,
stores service state under `~/Library/Application Support/Podway`, and stages a
copy of `podwayd` during installation. Those choices are convenient for an
interactive shell but do not give a local automation client a deterministic
endpoint or prove that the CLI and installed daemon belong to one release.

Podway still needs one user-scoped daemon while keeping Procedure and SQLite state
inside the Git worktree that owns it.

## Decision

The logical user-global root is `PODWAY_HOME`, resolved by looking up the effective
operating-system user account and appending `.podway` to that account's home
directory. `PODWAY_HOME` names an internal path abstraction, not an environment
variable. Its default resolution does not depend on `HOME`, `TMPDIR`, or `XDG_*`.

The v0.1.0 layout is:

```text
<effective-user-home>/.podway/
  run/
    podwayd.sock
    podwayd.lock
  state/
    service.json
    workspaces.json
  logs/
    podwayd.log
```

The root and its three directories use mode `0700`; regular state and log files
and the socket use mode `0600`. The LaunchAgent plist remains at
`<effective-user-home>/Library/LaunchAgents/dev.podway.podwayd.plist`.

The public CLI accepts an absolute `--socket` path for daemon-backed commands.
When supplied, that endpoint is the only endpoint attempted. Podway does not
expand `~` and does not fall back to service metadata, a default socket, `$TMPDIR`,
or `/tmp`. Without `--socket`, an interactive client may read
`PODWAY_HOME/state/service.json` and then use the installation default at
`PODWAY_HOME/run/podwayd.sock`.

Socket selection does not create daemon namespaces. Every daemon instance
contends on the fixed `PODWAY_HOME/run/podwayd.lock`, so a second daemon is
rejected even when it requests a different socket path.

Users invoke `podway` through `PATH`. `podway daemon install` resolves `podwayd`
in this order:

1. an explicit `--daemon-path`;
2. a sibling of the resolved current `podway` executable;
3. the current controlled `PATH`.

The resolved binary is canonicalized and its release identity is verified. The
LaunchAgent and service metadata record that actual canonical absolute path.
Installation does not stage or copy the daemon binary. LaunchAgent startup never
depends on the interactive shell's `PATH`.

Worktree-owned state remains under `<worktree>/.podway`, including
`config.yaml`, `runtime/state.sqlite3`, Procedure snapshots, and mutation state.

## Rejected alternatives

- `XDG_HOME` is not a standard XDG variable, and an XDG override is not needed by
  the v0.1.0 integration.
- Ambient `$TMPDIR` and `/tmp` fallback do not provide a stable automation
  endpoint.
- Multiple socket-selected daemon namespaces violate the one-daemon-per-user
  model.
- Moving SQLite state into the user-global root breaks worktree ownership and
  deletion semantics.
- Staging a private daemon copy obscures the selected release binary and conflicts
  with deterministic CLI/daemon identity checks.

## Consequences

- Local automation can operate with a sanitized environment and an explicit
  endpoint.
- Service installation and upgrade behavior are tied to a visible release binary.
- Package managers or users must leave the recorded daemon path valid until the
  service is reinstalled or upgraded.
- Runtime-path migration, stale socket recovery, and uninstall require explicit
  compatibility handling during the `RPATH` epic.
