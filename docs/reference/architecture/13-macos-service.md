# macOS Service Specification

## Service model

The macOS release runs `podwayd` as a per-user LaunchAgent. It starts when the user logs in, not before login. Root privileges and a system-wide LaunchDaemon are not used.

Reference service label:

```text
dev.podway.podwayd
```

The label is an implementation constant for v1. Changing it requires migration of the installed service definition.

## Filesystem locations

| Purpose | Location |
|---|---|
| LaunchAgent plist | `~/Library/LaunchAgents/dev.podway.podwayd.plist` |
| Application support | `~/Library/Application Support/Podway/` |
| Minimal registry | `~/Library/Application Support/Podway/workspaces.json` |
| Service install metadata | `~/Library/Application Support/Podway/service.json` |
| Logs | `~/Library/Logs/Podway/` |
| Primary socket directory | `$TMPDIR/podway-<uid>/` |
| Socket | `$TMPDIR/podway-<uid>/podwayd.sock` |
| Singleton lock | `$TMPDIR/podway-<uid>/podwayd.lock` |

The socket path must stay below the macOS Unix-domain path limit. If the expanded `$TMPDIR` path is too long, the implementation uses `/tmp/podway-<uid>/podwayd.sock` after securely creating a `0700` directory owned by the user.

## LaunchAgent configuration

The installed plist MUST:

- use the exact absolute path to the installed `podwayd` binary;
- set `RunAtLoad` to true;
- use a keep-alive policy that restarts unexpected exits;
- pass a fixed `--service` mode argument;
- avoid network service declarations;
- set a conservative restart throttle;
- avoid embedding secrets;
- use user-owned paths only.

The reference template is [`../../spec/launchagent.plist.template`](../../spec/launchagent.plist.template).

## Commands

```bash
podway daemon install
podway daemon uninstall
podway daemon start
podway daemon stop
podway daemon restart
podway daemon status
podway daemon logs
podway daemon logs --follow
```

### Install

`podway daemon install`:

1. resolves the absolute `podwayd` binary path;
2. verifies that the binary is executable and version-compatible with the CLI;
3. creates application-support and log directories with user-private permissions;
4. writes the plist atomically;
5. bootstraps the LaunchAgent in the current GUI user domain;
6. waits for the socket health check;
7. records install metadata.

The command is idempotent when the same binary and configuration are already installed. A changed binary path updates the plist and restarts the service.

### Stop and start

`stop` boots out the LaunchAgent, which prevents the keep-alive policy from immediately restarting it. `start` bootstraps the installed plist. `restart` performs an ordered stop and start.

### Uninstall

`uninstall`:

- stops and removes the LaunchAgent;
- removes service metadata and stale socket files;
- preserves all worktree-local Podway state;
- preserves daemon logs by default;
- supports `--purge-logs` as an explicit option.

## Daemon health

The daemon exposes a local health request over the socket. `podway daemon status --json` reports:

```text
installed
loaded
reachable
daemon_version
protocol_versions
pid
started_at
uptime_ms
socket_path
registered_worktree_count
active_scheduler_count
queued_job_count
running_job_count
```

`reachable=false` distinguishes an installed but unhealthy service from an uninstalled service.

## Socket safety

At startup the daemon:

1. creates the runtime directory with mode `0700`;
2. acquires the singleton lock;
3. inspects an existing socket;
4. if a healthy daemon responds, exits as duplicate;
5. if no process owns the stale socket, removes it;
6. binds the new socket and sets mode `0600`;
7. begins accepting requests.

The daemon verifies the peer UID using the platform's local-socket credential API where available.

## Logging

The daemon writes structured local logs under `~/Library/Logs/Podway/`.

Defaults:

- log file: `podwayd.log`;
- maximum file size: 10 MiB;
- retained rotated files: 5;
- default level: `info`;
- no item values, task titles, artifact locations, or full request payloads in normal logs;
- job, workspace, command, error code, duration, and state transition identifiers may be logged.

`podway daemon logs` prints the resolved log path and recent content. `--follow` streams appended lines.

## Upgrade behavior

A CLI and daemon major protocol mismatch fails clearly. Package upgrade should:

1. install both binaries together;
2. run `podway daemon install` or equivalent service refresh;
3. restart the daemon;
4. migrate each workspace database lazily on first access;
5. fail closed on a database created by an unsupported newer version.

The LaunchAgent plist contains no version-specific workspace paths.

## Service tests

The macOS integration suite MUST cover:

- plist generation and validation;
- idempotent install;
- update after binary path change;
- start at login in an isolated test account or equivalent harness;
- explicit stop despite keep-alive;
- restart and stale socket cleanup;
- duplicate daemon prevention;
- socket owner and mode;
- log creation and rotation;
- uninstall with state preservation;
- incompatible CLI/daemon version reporting.
