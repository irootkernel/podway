# User Runtime Factory Reset

## Authority and availability

[ADR-0032](../../architecture-decision-records/0032-retire-ordinary-user-runtimes.md)
owns the account-runtime retirement decision. The command catalog admits read-only
`runtime.reset.plan`. `runtime.reset.apply` and `daemon.runtime_reset` remain
reserved and cannot execute. Until the control route is admitted, a live daemon
is reported as unsupported and cannot receive a planning token. The active
roadmap owns remaining implementation work.

## Selection and preservation

The supported command forms are:

```text
podway --mode <key> runtime reset plan
podway runtime reset plan --all-modes
podway --mode <key> runtime reset apply --plan-token <token> --yes
podway runtime reset apply --all-modes --plan-token <token> --yes
```

One explicit `--mode`, its existing `--dev` alias, or `--all-modes` is mandatory.
Conflicting or repeated selectors, `--worktree`, `--socket`, and custom/managed
runtime-root overrides are rejected. These commands need neither a Git worktree
nor a healthy registry. Account discovery uses the effective OS user, not `HOME`,
`TMPDIR`, or `XDG_*`. The existing debug-only account injection remains limited to
isolated tests; no new production root override is added.

Production is `<account-home>/.podway`; named modes are its ordinary
`modes/<key>` namespaces. All-mode discovery scans only immediate mode entries,
orders prod first and other keys by exact bytes, and never follows an external
root. Every plan discloses external managed-runtime exclusion. Encountered
managed namespaces are explicitly excluded rather than traversed. Invalid or
ambiguous ordinary entries block the operation.

Approved deletion classes are registry (`state/workspaces.json`), runtime recovery
(`state/recovery.json`), service metadata (`state/service.json`), the socket
(`run/podwayd.sock` after proven exit), daemon and bootstrap log streams and their
documented rotations, and empty state/log directories. Only documented
producer-owned temporary files within those classes may join the inventory.
Production service retirement additionally removes the exact
`Library/LaunchAgents/dev.podway.podwayd.plist` through its native service owner.
An unexpected entry within a deletion tree is a blocking unsafe layout.

Every Git repository, worktree-local `.podway` tree, current or archived session,
job, configuration, Procedure, installed executable, unrelated file and
unselected namespace is preserved. Aquarium, contributor and qualification roots,
backups, and exports are outside selection. No registry entry is followed into a
worktree. Existing `reset`, `reset --all`, workspace removal, uninstall,
`daemon.terminate`, and Aquarium controller contracts keep their meanings.

The operation preserves each selected namespace's root, `run` directory and
`run/podwayd.lock` anchor. An existing `state/workspaces.json.lock` registry
anchor and its containing state directory are also preserved; anchors alone do
not make a deletion target. It also preserves owner-private account control paths
`maintenance/runtime-reset.lock`, `maintenance/runtime-start.lock`,
`maintenance/runtime-reset-commit.lock`, and `maintenance/runtime-reset.json`.
Their validated ancestry and regular non-symlink identities remain stable across
reset. This bounded coordination residue is intentional; reset never recursively
removes the account root.

## Read-only plan and confirmation token

Plan creates no directory, file, lock anchor, database, or service. It reports
target paths and approved resource classes, preserved residue, exclusions,
live-idle/offline/absent/busy/unsupported/unsafe states, and closed blocking
reasons. An absent or already retired selection is `no_change` with no token.
Only lock anchors and completion records do not make a deletion target.

An eligible plan returns `ready`, a token and an expiry. A blocked plan has no
token. An unsafe selected namespace appears in a blocked plan. Account-wide
discovery failures, including invalid entries directly under `modes/`, return a
catalogued error because the complete selection cannot be established.
A fresh plan encountering an in-progress record reports
`recovery_required` and its operation ID and token digest; it cannot supersede
that operation. The operator retains the original token for explicit retry.

The token is unpadded base64url of the canonical UTF-8 JSON
`podway.runtime-reset-token/v1` document, followed by `.` and the lowercase
SHA-256 hex digest of those bytes. Canonical JSON uses lexicographically ordered
object keys, no insignificant whitespace, integers only, and unescaped Unicode.
Account records and control requests identify the entire encoded token with
`token_sha256`, the `sha256:`-prefixed digest of its ASCII bytes.
This checksum detects invalid encoding; it is neither a secret nor a same-user
security boundary. Noncanonical encodings and unknown fields are rejected.

The document binds effective UID, account-root identity, explicit selection,
the complete observed ordinary mode inventory for all-mode selection, each
namespace and its run/state/log directory identities, stable lock and endpoint
identities, service definition/metadata digests, exact live process generation,
and approved resource classes. Internal paths retain raw bytes; public path
objects pair unpadded base64url bytes with a display-only string. Display strings
are never deletion authority. Registry and log bytes are preview data, not token
identity; normal append/rotation before shutdown is permitted.

Expiry is exactly ten minutes after creation and is enforced before durable
commit. Apply requires the same selection, the token, and `--yes`, even on a TTY.
It revalidates all bound facts and rejects a newly appeared ordinary mode before
the first destructive step. An in-progress record keeps its exact original token
valid after expiry. A completed receipt matches the token digest before reading
any newly created namespace; replay returns `already_applied` without deleting it.

## Reservation and locking

For live daemons, queued or executing jobs, other admitted clients, maintenance,
ongoing recovery, missing activity counts, and unresolved recovery preventing
proof of idleness block reset. A durable session with no executing work does not.
Reset never waits for work completion and has no force option.

Offline cleanup requires proof that the service is unloaded, the reset operation
exclusively owns the namespace singleton, and socket/process ownership is
unambiguous. An unreachable endpoint alone does not prove absence. Corrupt bytes
in an otherwise known owned registry file do not block this path. Preserved
offline queued or stale-running Store rows are not inspected; normal recovery
may resume them after an explicitly started daemon re-registers the worktree.
An older or unknown live daemon is unsupported, not an offline candidate.

Lock order is account reset mutex, existing production service lifecycle lock
when selected, account topology gate, commit interlock, then singleton locks in
target order. Ordinary namespace creation and daemon startup take the topology
gate before directory creation, singleton acquisition and reset-record checking.
The topology gate spans reset's preflight through completion or failure; a
committed record continues fencing selected namespaces after a coordinator exits.
External managed runtimes do not join these gates.

Reserve every live participant before destructive work. The daemon closes normal
admission atomically with a complete idle snapshot; the coordinator's reset
connection is excluded from client activity. Failure releases all prior
uncommitted reservations without stopping services or removing data. Never hold
a live participant's singleton while requesting its shutdown.

The coordinator holds the commit interlock while revalidating reservation
generations through read-only snapshots and fsyncing the account record. A
disconnect/release handler acquires that same interlock before the admission
mutex; it reopens admission only without a matching committed record. Snapshot
revalidation does not acquire the interlock. A lost connection or generation
before revalidation refuses commit. After commit, reservation release retains
the fence and explicit recovery may reattach only to the recorded participant.
A conflicting process generation fails closed.

## Bounded control exchange

`daemon.runtime_reset` is an internal synchronous `control` route, not an
additional human CLI command or a durable job. Its ordinary IPC envelope has no
workspace, idempotency key, preconditions, detach or wait options. The payload is
the closed `podway.runtime-reset-control-input/v1` object. Actions are `inspect`,
`reserve`, `snapshot`, `release`, and `shutdown`.

Before `inspect`, the caller verifies the ordinary peer's handshake and obtains
its process UUID from `daemon.status`. `inspect` binds that UUID as
`expected_process_id`, returns one idle/busy/unsupported snapshot and closes.
A successful
`reserve` binds the caller's operation ID and token digest to the expected
namespace and process UUID and returns a fresh reservation UUID. The connection
then accepts only matching snapshot, release, or shutdown actions. Each uses the
same operation, token, namespace and process identities and the returned
reservation UUID. Other commands or pipelined frames are rejected. The exchange
is limited to eight request/response pairs, 1 MiB per frame and a 30-second
per-frame inactivity timeout; shutdown uses the existing bounded service-exit
timeout. Normal IPC retains one request/response per connection.

Snapshot returns the reservation and current committed state without relaxing
admission. Release before commit restores admission; after commit it reports
`committed` and leaves admission fenced. Shutdown is admitted only for a matching
durable stop intent, responds `shutting_down`, closes the exchange, and requests
orderly exit. Recovery may reserve the same recorded participant again while
admission remains closed; it cannot adopt another process generation.

## Durable retirement and recovery

One owner-only, no-follow account record serializes single and all-mode reset.
Publish its `in_progress` state and fsync it before service shutdown or deletion.
The record binds operation ID, original token digest and decoded plan, participant
reservation identities, and ordered per-mode progress. A later operation may
replace a completed receipt only when its own commit is durable.

For each mode, persist `stop_intended` with the exact production uninstall or
foreground shutdown participant before acting. Expected endpoint-guard socket
cleanup and production plist/metadata removal are part of that intent. Wait only
the existing bounded shutdown interval, never escalate to kill, and obtain the
singleton only after verified exit. Recognize exact intended teardown
postconditions after interruption and fail on ambiguous substitutions.

Persist `inventory_intended`, enumerate final leaves with no writers, then fsync
`inventory_ready` with their class, relative raw path, type and filesystem
identity. Shutdown-time log rotation is included within the approved classes;
new namespaces, classes and external paths cannot enter the inventory. Repeating
an interrupted inventory publication under exclusive ownership is safe because
runtime-data deletion cannot precede `inventory_ready`.

For every entry, persist `intended`, perform the descriptor-relative removal,
fsync the parent, and persist `completed`. Child entries precede their directories.
An absent entry after durable intent and proven exclusive ownership satisfies the
removal even if its completion write was lost. Absence without applicable intent
or a conflicting recreated inode is an unsafe recovery result. All ancestry,
ownership, device/mount, mode and hard-link checks are repeated at the relevant
boundary. Sensitive regular files must have one link. Symlinked ancestry and
unexpected deletion-tree entries never trigger recursive fallback.

Failure after commit preserves the record and returns non-success with bounded
per-mode/resource progress. Retry processes only unfinished original resources.
No automatic restart or restoration is attempted. After all targets complete,
replace the record with one bounded `completed` receipt; it contains no registry
contents, job payloads or log data. The next committed operation replaces it and
makes its old token stale.

## Public results, errors and bounds

The closed plan, result, control, token and record schemas live in
`assets/schemas/`. Plan and apply use the existing `podway.output/v3` envelope
without workspace, session or job projections. Apply success is `complete` or
`already_applied`. `incomplete` appears only in the structured result inside
`podway.error/v1` details, never a successful output. A reset-specific `retry`
object describes the original explicit apply selection, token and confirmation
requirement. It is not the existing read-only `recovery` recipe and grants no
authority to execute a destructive command.

| Error | Exit | Retryable | Meaning |
|---|---:|---:|---|
| `RUNTIME_RESET_BUSY` | 4 | false | Live activity prevents reservation; obtain a new plan after work ends. |
| `RUNTIME_RESET_UNSAFE` | 5 | false | Path, ownership, service or process absence cannot be proved. |
| `RUNTIME_RESET_UNSUPPORTED` | 3 | false | A live peer or selected namespace is outside the supported reset contract. |
| `RUNTIME_RESET_PLAN_STALE` | 4 | false | Token, expiry, identity or selection changed before commit. |
| `RUNTIME_RESET_LIMIT_EXCEEDED` | 2 | false | A fixed discovery, encoding or inventory bound was exceeded. |
| `RUNTIME_RESET_IN_PROGRESS` | 4 | true | Another operation owns the account; only its exact retry can proceed. |
| `RUNTIME_RESET_INCOMPLETE` | 4 | true | A committed operation needs exact explicit retry. |

Missing confirmation and invalid selectors use existing `CONFIRMATION_REQUIRED`
and `REQUEST_INVALID`. Stable reason values belong to the reset component schema.
Unknown live contracts fail closed before sending any reset action.

Fixed admission bounds are 64 ordinary namespaces including prod, 256 immediate
mode-directory entries, 4,096 total resource entries, depth eight, 512 KiB for
each record/result, and 256 KiB for an encoded token. Check entry and serialized
size bounds before destructive work and again before final inventory publication;
reserve space for bounded teardown rotations during preflight. Never truncate a
selection or deletion inventory. No caller-adjustable override exists.
The resource-entry limit is an aggregate across all selected modes. Protocol
result and error validation checks that total in addition to the per-array schema
bounds; the coordinator must enforce it for plans and durable records as well.

## Verification ownership

Contract tests bind command availability, schemas, envelope selection, errors,
manifest identity and unchanged legacy meanings. Isolated service and filesystem
tests exercise malicious layouts, empty roots, offline corrupt registries,
durable intent and exact replay. Barrier tests interleave work admission, startup,
disconnect/release and commit. Real CLI/daemon E2E covers live/offline single and
all-mode retirement, bounds, partial failure, preserved Store jobs and subsequent
explicit re-registration. macOS service adapter tests use the established
isolated native-service harness. Tests never operate on the user's actual
account-runtime data, production LaunchAgent or Aquarium development runtime.
