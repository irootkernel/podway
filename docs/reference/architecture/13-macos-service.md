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
| LaunchAgent plist | `<effective-user-home>/Library/LaunchAgents/dev.podway.podwayd.plist` |
| Podway user-global root | `<effective-user-home>/.podway/` |
| Minimal registry | `<effective-user-home>/.podway/state/workspaces.json` |
| Service install metadata | `<effective-user-home>/.podway/state/service.json` |
| Logs | `<effective-user-home>/.podway/logs/` |
| Socket | `<effective-user-home>/.podway/run/podwayd.sock` |
| Singleton lock | `<effective-user-home>/.podway/run/podwayd.lock` |

These are the implemented v0.1.0 paths. Service installation and explicit
endpoint resolution use the effective OS account and do not require `HOME`,
`TMPDIR`, or `XDG_*`. Over-long socket paths fail with a stable typed error.
PODWAY_HOME plus `run`, `state`, and `logs` use mode `0700`; service metadata,
the registry, logs, the singleton lock, and the socket use mode `0600`.

The production CLI and daemon resolve this layout from the effective OS account
even when launched with a sanitized environment from an arbitrary working
directory. Test-only account-root injection is compiled only in debug builds so
process-level integration tests can exercise the real binaries without touching
the developer's installed service state.

## Foreground dev mode

`podwayd --dev` is a contributor-only foreground execution mode compiled into the
release binaries so packaged IPC conformance can exercise the actual artifact
without installing a LaunchAgent. Its default root is
`<effective-user-home>/.podway/dev/`; an absolute `PODWAY_DEV_HOME` overrides that
root only in dev mode. The dev socket, registry, metadata, and log paths live below
that root. The singleton lock remains the production
`<effective-user-home>/.podway/run/podwayd.lock`, preventing simultaneous production
and dev daemon ownership.

`podway --dev` selects the dev socket directly and never consults installed-service
metadata. `podway --dev terminate` sends the dev-only `daemon.terminate` control
request, waits for endpoint cleanup, and safely removes a same-user stale Unix
socket when no daemon is listening. Production daemons reject that request. Normal
signal shutdown and dev IPC shutdown share the endpoint guard, which removes only
the socket identity owned by that process; dev registry and log files remain.

This mode validates Podway's daemon/CLI interface. LaunchAgent plist generation,
absolute arguments, permissions, atomic publication, and command-runner behavior
remain covered by static and mocked adapter tests rather than by recreating a macOS
GUI login domain during distribution packaging.

## LaunchAgent configuration

The installed plist MUST:

- use the exact absolute path to the installed `podwayd` binary;
- set `RunAtLoad` to true;
- use a keep-alive policy that restarts unexpected exits;
- pass fixed `--service --socket <absolute-path>` mode arguments;
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

1. resolves the canonical absolute `podwayd` path by explicit option, CLI sibling,
   then controlled `PATH`;
2. runs a time- and output-bounded version probe and verifies that the binary is
   executable and has the exact CLI product and contract-manifest identity;
3. creates PODWAY_HOME directories with user-private permissions;
4. writes the plist atomically;
5. bootstraps the LaunchAgent in the current GUI user domain;
6. waits for `daemon.status` to report the installed executable path and the
   complete expected build, protocol, and contract identity;
7. records install metadata.

The target installer does not stage or copy the daemon. It is idempotent when the
same actual binary and configuration are installed. A changed binary path or
contract identity updates the plist and restarts the service.

The current CLI executable is canonicalized before sibling lookup. Consequently,
invoking a PATH-installed CLI symlink still selects `podwayd` beside the resolved
CLI binary before consulting the controlled `PATH`. Every selected daemon path is
canonicalized and verified before the absolute path is written to the plist.

### Stop and start

`stop` boots out the LaunchAgent, which prevents the keep-alive policy from immediately restarting it. `start` bootstraps the installed plist. `restart` performs an ordered stop and start.

### Uninstall

`uninstall`:

- stops and removes the LaunchAgent;
- removes service metadata while leaving socket and lock ownership to the daemon
  endpoint guard;
- preserves all worktree-local Podway state;
- preserves daemon logs by default;
- supports `--purge-logs` as an explicit option.

## Daemon health

The daemon exposes a pre-dispatch, read-only, non-durable health request over the socket.
`podway daemon status --json` merges that live response with launchd service state and reports:

```text
status
installed
loaded
reachable
daemon_version
protocol_versions
contract_manifest_digest
pid
process_id
executable_path
started_at
uptime_ms
configured_socket_path
effective_socket_path
registered_worktree_count
active_scheduler_count
queued_job_count
running_job_count
```

`process_id` is a UUID created once per daemon process; `pid` is the operating-system
numeric process ID. `started_at` is fixed at process startup and `uptime_ms` is measured
with a monotonic clock. `reachable=false` distinguishes an installed but unhealthy
service from an uninstalled service. For a stopped or unreachable installation, static
build identity, the canonical executable, and the configured socket remain available,
while PID, process UUID, start time, uptime, and effective socket are `null`.

## Socket safety

At startup the daemon:

1. creates PODWAY_HOME and its directories with mode `0700`;
2. acquires the fixed per-user singleton lock regardless of selected socket;
3. validates the selected socket parent as a real, effective-user-owned mode
   `0700` directory;
4. rejects a regular file, directory, symlink, wrong-owner socket, or
   wrong-mode socket without unlinking it;
5. if a healthy daemon responds, exits as duplicate;
6. removes a refused stale socket only while holding the singleton lock and
   only when its type, owner, mode, device, and inode still match;
7. binds the new socket, sets mode `0600`, and begins accepting requests.

The daemon verifies the client peer UID before reading a frame. The CLI likewise
validates socket type, owner, mode, parent permissions, path length, and the
connected daemon peer UID before sending a frame.

## Logging

The daemon writes structured local logs under
`<effective-user-home>/.podway/logs/`.

Defaults:

- log file: `podwayd.log`;
- maximum file size: 10 MiB;
- retained rotated files: 5;
- exact record form: `ts=<seconds> operation=<name> outcome=<name>`;
- bounded event queues account for dropped records;
- no log levels or runtime log-level configuration in v0.1.0;
- no item values, task titles, artifact locations, or full request payloads in normal logs;
- operation and outcome names come from closed internal categories.

`podway daemon logs` prints the resolved log path and recent content. `--follow` streams appended lines.

The LaunchAgent sends both standard output and standard error to the same `podwayd.log` path used by the rotating daemon sink. There is no separate bootstrap log in v0.1.0; after sink rotation, launchd may retain an older file descriptor until the service restarts.

## Upgrade behavior

A CLI and daemon product or contract-manifest mismatch fails before command
execution or admission. During service refresh, a stale daemon that still owns
the socket is not considered ready; installation waits until the replacement
process reports the installed executable and current identity. Package upgrade
should:

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
- endpoint-owned stale socket cleanup without service-layer unlinking;
- duplicate daemon prevention;
- socket owner and mode;
- log creation and rotation;
- uninstall with state preservation;
- incompatible CLI/daemon product and contract identity reporting;
- duplicate daemon rejection with the same and a different socket;
- explicit socket no-fallback and sanitized-environment operation;
- command-name invocation through a CLI symlink from an arbitrary directory;
- explicit, resolved-CLI-sibling, and controlled-PATH daemon selection;
- absolute daemon execution in the generated LaunchAgent plist.
