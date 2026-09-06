# Post-admission Response Reconstruction QA

Use this independent scenario when a release delta changes post-admission response
reconstruction. It complements the normal CLI admission and replay scenarios by
exercising a real committed Store receipt that production cannot render. A call
directly to the error normalizer does not establish this behavior.

## Execution boundary

Use an assigned fixture beneath the release-QA pass's physical evidence root.
Build the exact clean candidate's release libraries offline with the pinned Rust
toolchain, `--locked`, and all build outputs and caches inside that root. Build an
independently authored, non-test executable against those libraries. Do not run or
copy the repository's regression tests as release-QA evidence.

The executable composes `WorkspaceRuntimeManagerV1` and
`compose_dispatcher_v1`, using the real Store, native execution worker, registry,
and production request dispatcher. Create standalone Git fixtures beneath the
assigned root and use `ServiceRuntimePathsV1::for_dev_home` with explicit private
account and development roots. Bind the manager's allowed worktree scope to the
fixture root. Never use an installed daemon, default endpoint, or ambient runtime
configuration.

This scenario injects a renderer through an existing library dependency boundary;
it does not inject a fault into the shipped `podwayd` executable or simulate a
physical disk failure. Normal CLI/daemon scenarios separately cover the packaged
transport. No new CLI flag, environment-controlled product failpoint, or public
protocol is required.

## Independent process cases

Run each of these cases in a fresh process, with a bounded deadline and private
fixture: normal session start, malformed session-start response, and normal
confirmed workspace reset. The renderer is held in a global `OnceLock`, so
injecting it after production composition or sharing its process with another
case invalidates the setup.

For the malformed session-start case, call
`podway_store::install_terminal_envelope_sealer_v1` before constructing the
dispatcher. Supply a pure renderer that returns:

```json
{"schema":"podway.output/v3"}
```

The schema is valid for the Store's persisted envelope container, while the
missing public fields must be rejected during production response reconstruction.
Let the normal Store transaction write the receipt. Never edit a database,
registry, receipt, runtime link, or socket to manufacture the failure. The normal
cases leave the renderer registration to production composition.

Initialize through `workspace.init`, then submit `session.start` using
`small-change-v2`, or confirmed `workspace.reset_all`. Use explicit workspace
identity, a stable idempotency key, and the same original preconditions for replay.
Send requests through `dispatch_daemon` after normal request-envelope decoding.
Do not call `admitted_response_reconstruction_failure_v1` or construct the error
response yourself.

Reset reconstructs its result through `reread_reset_terminal` from receipt fields,
not the frozen public envelope. This renderer injection does not induce reset
reconstruction failure. Keep reset as a normal replay control; do not claim its
failure path is verified. If the release matrix requires that failure path, record
it as `INCOMPLETE` until an independent safe trigger is established.

An initialization worker may still be releasing its claim when reset is submitted.
A bounded retry is permitted only for `WORKSPACE_MAINTENANCE` with
`retryable=true` and `admission.admitted=false`; retain every response.

## Required observations

1. The normal cases return valid success envelopes. The malformed session-start
   case returns a protocol-decodable `INTERNAL_ERROR`, exit code `6`, and
   `retryable=false`.
2. Error details identify `podway.internal-error-details/v1` and
   `ADMITTED_RESPONSE_RECONSTRUCTION_FAILED`; `diagnostic_id` equals the current
   request ID. Admission is true, and both copies of the job ID and sequence agree.
3. Read the durable binding and reconciliation snapshot through
   `SqliteStoreV1::inspect_workspace_binding` and
   `SqliteStoreV1::inspect_reconciliation_snapshot`. These read-only inspections
   must find a real terminal receipt. In the malformed case it contains the
   injected envelope, with the exact job ID and sequence reported by the error.
   A fabricated receipt or an in-memory job ID is not sufficient evidence of
   committed admission.
4. Inspect the graph through `inspect_graph_workspace_view_v2`: session start
   creates one prepared session; reset publishes a new workspace UUID and leaves
   no current session. Verify the session-start side effect despite response failure.
5. Replay the same operation with its original workspace fence, idempotency key,
   payload and preconditions, but a new request ID. It retains the same durable
   job, receipt, graph and workspace sequence. Malformed responses remain internal
   errors and carry the new request ID as their diagnostic ID. In particular,
   reset replay must not require the old workspace UUID to become current again.
6. Drop the dispatcher and manager and inspect the Store again. The receipt must
   remain durable. This is a cold read, not a claim that a malformed
   retained response has been repaired or that every read API can render it.

Record commands, build identity, fixture paths, controlled environment, input and
output envelopes, durable comparisons, exit status, and source Git status before
and after. Reap all child processes and remove their exact runtime, worktrees and
executables; retain only bounded text and JSON evidence. An execution gap remains
`INCOMPLETE`; do not substitute helper-only normalization results for these
production and persistence observations.

## Development regression coverage

The daemon's registered `int_post_admission_reconstruction` module exercises these
cases in isolated child processes as part of `make test`. It is development
evidence, separate from an independent release-QA pass. The public error contract
is owned by [errors and exit codes](../specs/interfaces/errors-and-exit-codes.md).
