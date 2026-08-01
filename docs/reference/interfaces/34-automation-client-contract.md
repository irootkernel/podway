# Automation Client Contract

## 1. Status and target release

This document defines the implemented local automation boundary targeted by Podway
v0.1.0. Runtime, identity, conformance, and packaged-fixture work through `DOLGI`
is complete; final release readiness remains tracked by the incomplete `REL10`
epic in the [roadmap](../../roadmap.md).

The requirement IDs in this document are stable. A requirement becomes satisfied
only when its roadmap task is completed and its planned executable evidence is in
the repository-local `make test` gate.

## 2. Scope

The contract covers a local client invoking `podway`, connecting to one
same-user `podwayd`, selecting a worktree, observing Procedure state, submitting
mutations, and reconciling durable outcomes.

## 3. Non-goals

Podway does not interpret Dolgorae Workflows or Roles, authorize ToolRuns, execute
external work, expose a network API, authenticate multiple users, maintain a
project-management history, or create multiple daemon namespaces. A client
disconnect does not automatically cancel an admitted mutation.

## 4. Terminology

- **PODWAY_HOME**: the internal abstraction for
  `<effective-user-home>/.podway`; it is not an environment variable.
- **explicit endpoint**: the absolute Unix socket supplied through `--socket`.
- **admission**: the durable transaction that assigns a job ID and workspace
  sequence before a mutation may execute.
- **contract manifest**: the deterministic set of integration-critical contract
  assets identified by `podway.contract-manifest/v1` and a SHA-256 digest.
- **outcome unknown**: the caller cannot prove whether a request was admitted
  because the transport ended before a trustworthy response arrived.
- **compact status**: a bounded, closed, quiescent projection for automation.

## 5. Automation client assumptions

Automation invokes `podway` by command name, supplies an explicit worktree and
socket for daemon-backed operations, uses JSON output, preserves idempotency keys,
and treats stable IDs and structured fields rather than text as the interface.

## 6. PATH-based CLI invocation (AUT-PATH-001–003)

| ID | Normative requirement |
|---|---|
| `AUT-PATH-001` | Automation clients MUST invoke `podway` through a controlled `PATH`; they MUST NOT need a hard-coded CLI executable path. |
| `AUT-PATH-002` | `podway daemon install` MUST resolve `podwayd` in this order: explicit `--daemon-path`, sibling of the resolved current CLI, then the controlled `PATH`. |
| `AUT-PATH-003` | Every resolved daemon MUST be canonicalized and verified against the CLI product and contract identity before installation. |

## 7. Daemon discovery and LaunchAgent execution (AUT-DAEMON-001–003)

| ID | Normative requirement |
|---|---|
| `AUT-DAEMON-001` | Service metadata and the LaunchAgent plist MUST record the verified daemon's canonical absolute path. |
| `AUT-DAEMON-002` | LaunchAgent startup MUST NOT depend on an interactive shell `PATH`. |
| `AUT-DAEMON-003` | Installation MUST NOT stage or copy `podwayd`; upgrade MUST re-resolve and re-verify the selected release binary. |

## 8. Podway user-global home and layout (AUT-HOME-001–003)

| ID | Normative requirement |
|---|---|
| `AUT-HOME-001` | PODWAY_HOME MUST resolve from the effective operating-system account home and MUST NOT require `HOME`, `TMPDIR`, or `XDG_*`. |
| `AUT-HOME-002` | The user-global layout MUST be `run/{podwayd.sock,podwayd.lock}`, `state/{service.json,workspaces.json}`, and `logs/podwayd.log` below PODWAY_HOME. |
| `AUT-HOME-003` | The LaunchAgent plist MUST remain under the effective user's `Library/LaunchAgents/dev.podway.podwayd.plist`. |

## 9. Worktree-local state boundary (AUT-HOME-004)

| ID | Normative requirement |
|---|---|
| `AUT-HOME-004` | Workspace configuration, `runtime/state.sqlite3`, Procedure snapshots, jobs, receipts, and other task state MUST remain in the owning worktree and MUST NOT be copied into PODWAY_HOME. |

## 10. Explicit socket resolution (AUT-SOCK-001–004)

| ID | Normative requirement |
|---|---|
| `AUT-SOCK-001` | Every daemon-backed command MUST accept global `--socket <absolute-unix-socket-path>`; static version and offline Procedure validation MAY omit it. |
| `AUT-SOCK-002` | `--socket` MUST reject relative paths and MUST NOT expand `~`. |
| `AUT-SOCK-003` | When supplied, the explicit endpoint MUST be the only endpoint attempted; failure MUST NOT fall back to metadata, a default socket, `$TMPDIR`, or `/tmp`. |
| `AUT-SOCK-004` | With explicit socket and worktree paths, daemon-backed commands MUST operate without reading `HOME`, `TMPDIR`, or `XDG_*`. |

When no explicit endpoint is supplied, an interactive client may read
`PODWAY_HOME/state/service.json` and then use
`PODWAY_HOME/run/podwayd.sock` as the installation default.

## 11. Socket and directory security (AUT-SEC-001–004)

| ID | Normative requirement |
|---|---|
| `AUT-SEC-001` | PODWAY_HOME and its `run`, `state`, and `logs` directories MUST use mode `0700`; service files, registry, log, and socket MUST use mode `0600`. |
| `AUT-SEC-002` | Podway MUST validate socket type, owner, parent permissions, peer effective UID, and platform path-length limits before use. |
| `AUT-SEC-003` | Podway MUST NOT replace a regular file, directory, or symlink found at a socket path. |
| `AUT-SEC-004` | Stale-socket recovery MUST fail closed unless ownership, type, singleton state, and path containment prove safe removal. |

## 12. One-daemon-per-user invariant (AUT-SOCK-005)

| ID | Normative requirement |
|---|---|
| `AUT-SOCK-005` | Every daemon MUST contend on `PODWAY_HOME/run/podwayd.lock`; a second daemon MUST be rejected even when configured with a different socket. |

## 13. CLI and daemon contract identity (AUT-CONTRACT-001–005)

| ID | Normative requirement |
|---|---|
| `AUT-CONTRACT-001` | `podway.contract-manifest/v1` MUST deterministically cover product version, supported IPC IDs, Procedure and IPC schemas, all integration result and error schemas, catalogs, transition matrix, canonicalization rules, and known-answer fixtures. |
| `AUT-CONTRACT-002` | `podway --json version` MUST expose product, version, target, build identity, source commit when available, manifest schema and digest, and supported IPC IDs. |
| `AUT-CONTRACT-003` | `podway` and `podwayd` from one release MUST embed the same manifest digest. |
| `AUT-CONTRACT-004` | Installation and every daemon connection MUST reject a product or manifest mismatch before command execution or durable admission; IPC compatibility alone MUST NOT authorize the connection. |
| `AUT-CONTRACT-005` | `daemon status --json` MUST expose daemon version, executable path, process-instance identity, configured and effective socket, and manifest digest. |

The client version field is diagnostic and MUST NOT independently authorize or
reject a connection. Admission is determined by product and manifest identity.
Because the manifest covers product version, peers from different releases have
different manifest digests even when they support the same IPC ID. Changing only
the declared client version while retaining a matching product and manifest does
not create a contract mismatch.

## 14. Workspace and session identity preconditions (AUT-ID-001–007)

| ID | Normative requirement |
|---|---|
| `AUT-ID-001` | The public CLI MUST expose `--if-workspace-uuid` and `--if-session-id` alongside revision, attempt, and item-revision preconditions. |
| `AUT-ID-002` | Session-bearing reads MUST enforce workspace UUID and, when supplied by the client, session ID. |
| `AUT-ID-003` | `start` MUST enforce workspace UUID and the normal no-existing-session rule. |
| `AUT-ID-004` | Stage transitions MUST enforce workspace UUID, session ID, session revision, and attempt ID for automation requests. |
| `AUT-ID-005` | Item mutations MUST enforce workspace UUID, session ID, attempt ID, and item revision for automation requests. |
| `AUT-ID-006` | `reopen`, replacement, and reset MUST enforce the currently observed workspace, session, and applicable session revision before mutation. |
| `AUT-ID-007` | Identity mismatches MUST return stable typed errors with closed details containing expected and actual identities and no mutation. |

A matching numeric revision is not sufficient evidence that an operation targets
the same session.

## 15. Procedure start integrity (AUT-START-001–004)

| ID | Normative requirement |
|---|---|
| `AUT-START-001` | `start --expect-procedure-digest sha256:<hex>` MUST compare the expected digest with the validated, defaulted, canonical Procedure before creating or replacing a session. |
| `AUT-START-002` | The canonical Procedure snapshot used by an admitted start MUST be durable before successful admission is reported. |
| `AUT-START-003` | An admitted start MUST NOT depend on later source-file reads; deletion, replacement, truncation, symlink change, or daemon restart MUST NOT alter the admitted Procedure. |
| `AUT-START-004` | The canonical Procedure digest and relevant start preconditions MUST participate in idempotency identity and MUST be returned by start and later session observations. |

## 16. Durable mutation admission (AUT-ADMIT-001–002)

| ID | Normative requirement |
|---|---|
| `AUT-ADMIT-001` | Every mutation result or error MUST distinguish `admission.admitted=true` with job ID and workspace sequence from `admission.admitted=false`. |
| `AUT-ADMIT-002` | A synchronous wait timeout for an admitted job MUST return the admitted job ID; timeout or client termination MUST NOT cancel an admitted mutation. |

## 17. Timeout, disconnect, and unknown outcome (AUT-ADMIT-003)

| ID | Normative requirement |
|---|---|
| `AUT-ADMIT-003` | Automation MUST treat response loss after possible admission as `MUTATION_OUTCOME_UNKNOWN`, preserve the original idempotency key in its closed reconciliation details, and perform `job.lookup` before deciding whether to retry with a new key. |

## 18. Job lookup by idempotency key (AUT-RECON-001–004)

| ID | Normative requirement |
|---|---|
| `AUT-RECON-001` | `job lookup --idempotency-key <key>` MUST be a read-only, worktree-scoped query and MUST NOT submit or replay a mutation. |
| `AUT-RECON-002` | Lookup MUST return `found=false` for no record and MUST return job ID, sequence, command, request digest, and state for admitted non-terminal jobs. |
| `AUT-RECON-003` | Lookup MUST return the complete immutable original `podway.output/v1` or `podway.error/v1` terminal envelope, including request correlation, command, completion timestamp, workspace, job, session/result or public error details, from the retained receipt after the terminal job row is pruned. It MUST NOT retain or reveal the full original request. Cancelled jobs retain the closed cancellation summary. |
| `AUT-RECON-004` | Reusing an idempotency key for a different canonical request MUST continue to fail with the stable reuse error. |

## 19. Quiescent observation (AUT-OBS-001)

| ID | Normative requirement |
|---|---|
| `AUT-OBS-001` | A successful `status --wait-for-idle` MUST read after the queue barrier and report `pending_mutations=false`, `queued_count=0`, `running_job_id=null`, and a present latest workspace sequence. |

## 20. Compact status contract (AUT-OBS-002–004)

| ID | Normative requirement |
|---|---|
| `AUT-OBS-002` | `status --wait-for-idle --compact` MUST return workspace and queue identity, Procedure identity, session lifecycle and revision, current stage and attempt, completion readiness, item identity/state/revision, and blocker identity/state. |
| `AUT-OBS-003` | Compact status MUST omit instructions, prompts, task titles, previous-attempt narratives, and item values unless a value is strictly required to form a valid mutation. |
| `AUT-OBS-004` | Compact status MUST use a closed schema and the complete serialized JSON envelope MUST NOT exceed 262,144 UTF-8 bytes. |

## 21. Command-specific JSON schemas (AUT-JSON-001–004)

| ID | Normative requirement |
|---|---|
| `AUT-JSON-001` | Version, daemon status, Procedure validation, start, status, next, item mutation, stage transition, detached admission, job status/wait, and job lookup MUST each have a closed result schema. |
| `AUT-JSON-002` | Daemon, socket, identity, revision, attempt, digest, idempotency, and timeout failures MUST each have closed error-detail schemas. |
| `AUT-JSON-003` | Results and error details MUST carry an unambiguous schema identifier or discriminator. |
| `AUT-JSON-004` | A closed v1 object MUST reject unknown fields; adding fields requires a new schema identifier or discriminator version rather than an undocumented additive-field exception. |

## 22. Error and exit-code requirements (AUT-ERR-001–002)

| ID | Normative requirement |
|---|---|
| `AUT-ERR-001` | `WORKSPACE_UUID_MISMATCH`, `SESSION_ID_MISMATCH`, `PROCEDURE_DIGEST_MISMATCH`, `DAEMON_CONTRACT_MISMATCH`, socket errors, and wait timeout MUST have stable catalog entries and exit mappings. |
| `AUT-ERR-002` | A pre-admission contract, identity, digest, endpoint, or validation failure MUST report `admission.admitted=false`; a mismatch MUST NOT admit a job. |

## 23. Release artifact and installation (AUT-REL-001–004)

| ID | Normative requirement |
|---|---|
| `AUT-REL-001` | Podway MUST publish and support only native thin-arm64 `aarch64-apple-darwin` artifacts built and verified on an untranslated arm64 macOS host. |
| `AUT-REL-002` | The archive MUST contain both binaries, completions, presets, schemas, specs, canonicalization fixtures, the contract manifest, README, and license, with checksum and provenance. |
| `AUT-REL-003` | The packaged manifest digest, CLI identity, daemon identity, source revision, target, and toolchain identity MUST agree. |
| `AUT-REL-004` | v0.1.0 MUST NOT be tagged until every preceding roadmap task is completed and the final packaged Dolgorae conformance run and repository-local `make test` pass. |

## 24. Acceptance matrix

The controlled-PATH harness includes these probes from a directory that is not a
Podway worktree:

```bash
env -i PATH="<release-bin>:/usr/bin:/bin" podway --json version

env -i PATH="<release-bin>:/usr/bin:/bin" \
  podway --json \
  --socket "/Users/test/.podway/run/podwayd.sock" \
  --worktree "/Users/test/src/project" \
  --timeout 25s \
  status --wait-for-idle --compact
```

| Evidence ID | Required scenarios | Roadmap |
|---|---|---|
| `AUT-T-PATH` | sanitized environment, arbitrary directory, CLI symlink, sibling/explicit/PATH daemon resolution, absolute plist path | `RPATH006`, `DOLGI001` |
| `AUT-T-SOCK` | correct, wrong, relative, over-long, regular-file, symlink, insecure-parent, wrong-owner, stale, same/different-socket duplicate daemon | `RPATH003`–`RPATH006` |
| `AUT-T-CONTRACT` | matching peers, same version/different manifest, different version/same IPC, replaced executable, restart after upgrade | `CONID003`–`CONID006` |
| `AUT-T-ID` | replaced workspace/session, same numeric revision on another session, stale reopen, wrong attempt/item, guarded reads | `CASID003`–`CASID005` |
| `AUT-T-START` | matching/mismatching digest, source deletion/replacement/race, restart, exact replay, key reuse with another digest | `PSTRT001`–`PSTRT005` |
| `AUT-T-RECON` | disconnect before/after admission, wait timeout, lookup in every state, domain failure, pruning, missing key, key reuse | `RECON001`–`RECON005` |
| `AUT-T-OBS` | idle barrier invariants, closed compact schema, maximum envelope size | `MCONT004`, `MCONT006`, `DOLGI002` |
| `AUT-T-JSON` | every result/detail fixture validates its discriminator and rejects unknown or malformed fields | `MCONT001`–`MCONT006` |
| `AUT-T-DIST` | native debug test-fixture archive on controlled PATH passes the complete Dolgorae consumer conformance suite with fail-closed isolation; `make dist` then extracts the actual release-profile archive and repeats packaged lifecycle, conflict, timeout, response-loss, reconciliation, identity, termination, and socket-cleanup checks through its isolated foreground dev mode | `DOLGI005`, `REL10001`–`REL10004` |

The test-fixture slice supersedes DOLGI's earlier release-profile wording. It is
local executable evidence for the packaged client contract, not evidence that the
release-profile archive has passed its `REL10003` qualification.

## 25. Requirements-to-roadmap traceability

| Requirements | Implemented by | Planned evidence |
|---|---|---|
| `AUT-PATH-001`–`003`, `AUT-DAEMON-001`–`003` | `RPATH004`, `RPATH006`, `CONID006` | `AUT-T-PATH`, `AUT-T-CONTRACT` |
| `AUT-HOME-001`–`004` | `RPATH001`, `RPATH002`, `RPATH004` | `AUT-T-PATH`, `AUT-T-SOCK` |
| `AUT-SOCK-001`–`005`, `AUT-SEC-001`–`004` | `RPATH003`–`RPATH006` | `AUT-T-SOCK` |
| `AUT-CONTRACT-001`–`005` | `CONID001`–`CONID006` | `AUT-T-CONTRACT`, `AUT-T-DIST` |
| `AUT-ID-001`–`007` | `CASID001`–`CASID005` | `AUT-T-ID`, `AUT-T-JSON` |
| `AUT-START-001`–`004` | `PSTRT001`–`PSTRT005` | `AUT-T-START` |
| `AUT-ADMIT-001`–`003`, `AUT-RECON-001`–`004` | `RECON001`–`RECON005` | `AUT-T-RECON` |
| `AUT-OBS-001`–`004` | `MCONT004`, `MCONT006`, `DOLGI002` | `AUT-T-OBS` |
| `AUT-JSON-001`–`004`, `AUT-ERR-001`–`002` | `CASID004`, `MCONT001`–`MCONT006` | `AUT-T-JSON` |
| `AUT-REL-001`–`004` | `DOLGI005`, `REL10001`–`REL10005` | `AUT-T-DIST`, repository-local `make test` |

## 26. Example Dolgorae command sequences

Dolgorae is the first consumer; these examples do not add Dolgorae workflow or
authorization semantics to Podway.

```bash
podway --json version

podway --socket "/Users/example/.podway/run/podwayd.sock" daemon install

podway --json \
  --socket "/Users/example/.podway/run/podwayd.sock" \
  --worktree "/Users/example/src/project" \
  --timeout 25s \
  --if-workspace-uuid "$WORKSPACE_UUID" \
  --if-session-id "$SESSION_ID" \
  status --wait-for-idle --compact

podway --json \
  --socket "/Users/example/.podway/run/podwayd.sock" \
  --worktree "/Users/example/src/project" \
  --timeout 25s \
  --idempotency-key "$IDEMPOTENCY_KEY" \
  --if-workspace-uuid "$WORKSPACE_UUID" \
  start \
  --procedure ".dolgorae/runtime/podway/request-42/procedure.json" \
  --expect-procedure-digest "$PROCEDURE_DIGEST" \
  --task "WI-0042: Implement Podway stage dispatch"

podway --json \
  --socket "/Users/example/.podway/run/podwayd.sock" \
  --worktree "/Users/example/src/project" \
  --timeout 25s \
  --idempotency-key "$IDEMPOTENCY_KEY" \
  --if-workspace-uuid "$WORKSPACE_UUID" \
  --if-session-id "$SESSION_ID" \
  --if-session-revision "$SESSION_REVISION" \
  --if-attempt "$ATTEMPT_ID" \
  complete

podway --json \
  --socket "/Users/example/.podway/run/podwayd.sock" \
  --worktree "/Users/example/src/project" \
  --timeout 25s \
  --idempotency-key "$IDEMPOTENCY_KEY" \
  --if-workspace-uuid "$WORKSPACE_UUID" \
  --if-session-id "$SESSION_ID" \
  --if-attempt "$ATTEMPT_ID" \
  --if-item-revision "$ITEM_REVISION" \
  set "$ITEM_ID" "$VALUE"

podway --json \
  --socket "/Users/example/.podway/run/podwayd.sock" \
  --worktree "/Users/example/src/project" \
  --timeout 25s \
  job lookup --idempotency-key "$IDEMPOTENCY_KEY"
```

## 27. Explicitly deferred features

Configurable or XDG homes, remote protocols, multi-user authorization, multiple
daemon namespaces, a global Procedure-state copy, executable Procedures,
disconnect cancellation, Dolgorae Workflow/Role/ToolRun logic, and non-Apple-
Silicon-macOS releases are outside the v0.1.0 contract.
