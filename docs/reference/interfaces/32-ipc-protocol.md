# IPC Protocol

## Scope

IPC v1 is a private local protocol between compatible `podway` and `podwayd` binaries. It is versioned and documented because reliable automation and upgrades require predictable behavior. It is not a remote API.

Protocol identifier:

```text
podway.ipc/v1
```

## Transport

- Unix-domain stream socket;
- user-private socket path defined by the macOS service specification;
- no TCP, UDP, HTTP, WebSocket, or network fallback;
- one request and one response per connection;
- no compression;
- UTF-8 JSON payloads.

## Framing

Each message is:

```text
4-byte unsigned big-endian payload length
N bytes of UTF-8 JSON
```

Limits:

- maximum payload length: 1,048,576 bytes;
- zero-length frames are invalid;
- extra bytes after the declared single frame are rejected;
- incomplete frames time out;
- invalid UTF-8 or JSON returns `REQUEST_INVALID` when a response can be produced.

The daemon reads the length before allocating the payload buffer and rejects oversized frames with `REQUEST_TOO_LARGE`.

## Request envelope

```json
{
  "protocol": "podway.ipc/v1",
  "request_id": "2037d76d-6ea8-42c2-a11f-883248bb8774",
  "client": {
    "name": "podway",
    "version": "0.1.0",
    "pid": 12345,
    "product": "podway",
    "contract_manifest_digest": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
  },
  "operation": "mutate",
  "command": "session.complete",
  "workspace": {
    "root": "/Users/example/src/project-wt",
    "expected_uuid": "83f9fbaa-3df8-4c23-8253-f94098b0af63"
  },
  "idempotency_key": "task-42-verify-complete",
  "preconditions": {
    "session_id": "9f8ad796-e4de-4f9d-b114-b68cf3de7561",
    "session_revision": 12,
    "attempt_id": "6f8e7429-3703-47cf-81dc-ae4048352f1f"
  },
  "options": {
    "detach": false,
    "wait_timeout_ms": 30000
  },
  "payload": {}
}
```

The normative structural schema is [`../../schemas/ipc-request-v1.schema.json`](../../schemas/ipc-request-v1.schema.json).

## Operations

| Operation | Purpose | Worktree required | Idempotency key |
|---|---|---:|---:|
| `query` | status, next, job reads, doctor reads | usually | no |
| `mutate` | session and item mutations | yes | yes |
| `control` | daemon lifecycle health or queued-job cancellation | depends | cancellation yes internally |
| `bootstrap` | workspace initialization and destructive recreation | yes | yes when state is readable |

The command catalog provides the definitive mapping.

### Live daemon status probe

The CLI may send `daemon.status` as an exact `control` request with no workspace,
idempotency key, preconditions, detach, wait timeout, or payload. After the contract
handshake, the transport answers this probe before worktree parsing, durable admission,
or command dispatch. The response identifies the current daemon process and conforms
to [`../../schemas/daemon-status-result-v1.schema.json`](../../schemas/daemon-status-result-v1.schema.json)
after the CLI merges it with local service state.

## Workspace context

Workspace requests include the canonical root discovered by the CLI. `expected_uuid` is omitted before initialization and included afterward when known.

When both the envelope workspace context and worktree selector carry `expected_uuid`, the values
must agree. The daemon rejects disagreement during request decoding, before dispatch.

The daemon independently re-discovers and validates the worktree. It never trusts the path or UUID solely because the CLI supplied them.

## Preconditions

The `preconditions` object may contain:

```text
session_id
session_revision
attempt_id
item_revision
blocker_id
job_state
```

The command specification determines required fields. Session-bearing reads accept an optional
`session_id`; stage, item, reopen, replacement, and session-reset mutations require it. Unknown
precondition fields are rejected in v1.

The daemon compares these identities with the same authoritative Store view used by the operation.
Waiting reads recheck the session identity on every Store observation. New mutations check identity
inside the admission transaction before creating durable rows and again after claim before applying
a domain transition. An admission-time mismatch creates no job; a post-claim mismatch terminates
the already admitted job as a typed failure. Neither path changes session state. An exact
idempotency replay is returned before evaluating a now-stale identity fence.

## Mutation waiting

For `detach=false`:

1. daemon admits the job durably;
2. daemon waits for terminal state up to `wait_timeout_ms`;
3. terminal success returns `result.admission` with the admitted job ID and workspace sequence;
4. terminal error returns the same identity in `details.admission`;
5. if wait expires, return `JOB_WAIT_TIMEOUT` with that admitted identity; the job continues.

For `detach=true`, the response returns immediately after admission and carries
the same closed `result.admission` identity. A failure proved to occur before
durable admission carries exactly `details.admission={"admitted":false}`.

A client disconnect never cancels an admitted job.

## Response

The response frame contains either:

- `podway.output/v1`; or
- `podway.error/v1`.

Protocol-level failures use the same error envelope where possible. If the request is too malformed to recover `request_id`, the response generates a new ID and includes `details.request_id_recovered=false`.

## Peer validation

Before processing a frame, the daemon obtains local peer credentials when available and verifies that peer UID equals daemon UID. A mismatch closes the connection and may log an access denial. No workspace token or access key is used.

## Timeouts

- connection establishment timeout is controlled by the CLI;
- frame read timeout defaults to 5 seconds;
- daemon query execution timeout defaults to 30 seconds;
- synchronous job wait timeout defaults to 30 seconds and is caller-configurable;
- timeouts do not cancel admitted jobs.

## Compatibility

The current daemon compares the request protocol identifier before command
parsing. This is the implemented baseline, not the final v0.1.0 automation gate.

- exact supported v1 is accepted;
- unsupported identifiers return `PROTOCOL_VERSION_UNSUPPORTED` and a list of supported protocol IDs;
- current CLI and daemon package versions may differ when they share a compatible protocol and command schema;
- commands or fields unavailable in the older peer return a structured compatibility error rather than being ignored.

After decoding a request, the daemon compares the client product and exact embedded
`podway.contract-manifest/v1` digest before command parsing, dispatch, or durable
admission. The bundled v0.1.0 CLI fails closed against any daemon with a different
product or manifest identity even when the IPC identifier is compatible.

## Request canonicalization

For mutation idempotency, the daemon constructs canonical request identity from:

```text
protocol major version
command
workspace UUID
workspace and session identity fences
semantic preconditions
payload
canonical Procedure digest for start
```

For `session.start` and `session.start_replace`, the optional
`expected_procedure_digest` payload field is accepted only with a Procedure-file source. The
daemon compares it with the validated, defaulted canonical snapshot before durable admission.
Start idempotency identity binds both that guard, when present, and the resolved canonical
Procedure digest. Exact retries reconstruct the identity from the immutable admitted execution,
so they do not depend on a later source-file read; a changed digest or start precondition reusing
the same key returns `IDEMPOTENCY_KEY_REUSED`.

Every successful `session.start` and `session.start_replace` response returns the admitted
canonical digest as `result.procedure_digest`, including a newly queued detached admission,
synchronous terminal completion, and an idempotent terminal replay. Terminal projections returned
by `job.list`, `job.status`, and `job.wait` carry the same `procedure_digest`. Later
`session.status` observations expose that identity as `result.task.procedure.digest`; these values
come from the durable admitted snapshot and never require another source-file read.

It excludes:

```text
request_id
client pid and version
wait timeout
detach preference
transport timestamps
```

Thus a retry may change timeout or detached behavior while referring to the same logical mutation.

## Request limits

In addition to the frame limit:

- command name maximum 128 bytes;
- idempotency key maximum 256 bytes;
- task title maximum 500 Unicode scalar values;
- reason maximum 4000 characters;
- individual CLI text item constrained by procedure and hard limit;
- list mutation value maximum 4000 characters;
- external reference maximum 4000 characters;
- no recursive JSON beyond depth 64.

## Socket shutdown behavior

On graceful daemon shutdown, newly accepted requests receive `DAEMON_SHUTTING_DOWN`. Existing read requests finish. Admitted jobs remain durable; currently committing transactions finish. Clients retry against the restarted daemon.

## Protocol testing

The suite MUST include:

- fragmented length and payload reads;
- multiple payload sizes including exact limit;
- oversized and zero frames;
- invalid UTF-8 and JSON;
- unsupported protocol;
- peer UID rejection;
- connection loss before and after admission;
- synchronous wait timeout;
- duplicate idempotent requests over separate connections;
- fuzzing of frame decoder and request deserializer.
