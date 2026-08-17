# Automation Client Contract

## 1. Status and target release

This document defines Podway's implemented local automation boundary. Current
work and release ownership are tracked by the active [roadmap](../../roadmap/);
historical task identifiers remain in the traceability tables as provenance.

The requirement IDs in this document are stable. A requirement becomes satisfied
only when its roadmap task is completed and its planned executable check is in
`make test` or the distribution qualification in `make dist`.

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
| `AUT-CONTRACT-002` | `podway version --json` MUST emit exactly the compact `name` and `v`-prefixed `version`; `podway version --json --identity` MUST emit a validated `podway.output/v3` envelope with a closed `podway.version-result/v1` result exposing product, version, target, build identity, source commit when available, manifest schema and digest, and supported IPC IDs. |
| `AUT-CONTRACT-003` | `podway` and `podwayd` from one release MUST emit exactly equal closed identity results and embed the same manifest digest. |
| `AUT-CONTRACT-004` | Installation and every daemon connection MUST reject a malformed complete identity envelope or a product or manifest mismatch before command execution or durable admission; IPC compatibility alone MUST NOT authorize the connection. |
| `AUT-CONTRACT-005` | `daemon status --json` MUST expose daemon version, executable path, process-instance identity, configured and effective socket, and manifest digest. |

The client version field is diagnostic and MUST NOT independently authorize or
reject a connection. Admission is determined by product and manifest identity.

`podwayd version --json` emits the compact daemon name and `v`-prefixed version.
`podwayd version --json --identity` emits the full versioned identity envelope used
by installation and qualification probes. Runtime probes reject bare results and
validate the outer discriminator, command, and closed result before extracting
identity fields.

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
| `AUT-ID-004` | Graph-node transitions MUST enforce workspace UUID, session ID, session revision, and attempt ID for automation requests. |
| `AUT-ID-005` | Item mutations MUST enforce workspace UUID, session ID, attempt ID, and item revision for automation requests. |
| `AUT-ID-006` | Replacement and reset MUST enforce the currently observed workspace, session, and applicable session revision before mutation. |
| `AUT-ID-007` | Identity mismatches MUST return stable typed errors with closed details containing expected and actual identities and no mutation. |
| `AUT-ID-008` | `item.record_many` MUST enforce workspace UUID, session ID, session revision, active attempt, and every selected item revision before atomically changing any selected item. |
| `AUT-ID-009` | `begin`, terminal disposition, eligible or force replacement, and eligible or force reset MUST enforce the exact observed workspace UUID, session ID, and session revision; begin and disposition MUST NOT accept an attempt fence. |

`podway record --stdin` consumes only closed
`podway.item-record-many-input/v1` JSON bounded to 1 MiB and 1..64 unique item
operations. Each operation contains exactly one typed complete record value or
`clear: true`. Operations and per-item outcomes are canonicalized by item ID.
The command uses the normal durable admission, detached execution, idempotent
replay, and `job lookup` reconciliation contracts. It returns
`podway.item-record-many-result/v1` and never advances the graph cursor.

A matching numeric revision is not sufficient evidence that an operation targets
the same session.

Procedure v2 goal revision and criterion-assessment mutations additionally bind
the exact positive current goal revision. That fence is distinct from, and does
not replace, the session revision or active-attempt fence.

## 15. Procedure start integrity (AUT-START-001–004)

| ID | Normative requirement |
|---|---|
| `AUT-START-001` | `start --expect-procedure-digest sha256:<hex>` MUST compare the expected digest with the validated, defaulted, canonical Procedure before creating or replacing a session. |
| `AUT-START-002` | The canonical Procedure snapshot used by an admitted start MUST be durable before successful admission is reported. |
| `AUT-START-003` | An admitted start MUST NOT depend on later source-file reads; deletion, replacement, truncation, symlink change, or daemon restart MUST NOT alter the admitted Procedure. |
| `AUT-START-004` | The canonical Procedure digest and relevant start preconditions MUST participate in idempotency identity and MUST be returned by start and later session observations. |

### Prepared session lifecycle (AUT-LIF-001–010)

| ID | Normative requirement |
|---|---|
| `AUT-LIF-001` | `start` and both replacement modes MUST create a `prepared` session at revision 0 with no attempt, cursor, goal revision, item value, or blocker. |
| `AUT-LIF-002` | `begin` MUST atomically create the entry-node attempt, optionally create and bind initial goal revision 1, change lifecycle to `running`, and advance session revision exactly once. |
| `AUT-LIF-003` | Prepared sessions MUST reject item, goal, blocker, cursor, completion, cancellation, retry, skip, decision, and rework mutations with `SESSION_NOT_RUNNING` and no state change. |
| `AUT-LIF-004` | Terminal disposition MUST accept exactly one closed `handed_off` summary/reference or `not_required` reason shape, bind it to the current completed or cancelled session revision, and treat every assertion as caller-supplied. |
| `AUT-LIF-005` | Reactivating a completed session MUST make every earlier terminal disposition non-current; a later terminal revision MUST require a new disposition for default deletion. |
| `AUT-LIF-006` | Default reset and `--replace-eligible` MUST delete only prepared sessions or terminal sessions with a disposition for the current revision and MUST evaluate eligibility atomically with deletion. |
| `AUT-LIF-007` | Force reset and force replacement of a running or undisposed terminal session MUST require destructive confirmation and a non-blank progress summary bounded to 4,000 Unicode scalars. |
| `AUT-LIF-008` | Reset eligibility MUST NOT depend on Git cleanliness, roadmap status, process state, external reference reachability, or any network request. |
| `AUT-LIF-009` | Prepared `status`, compact status, next, and observation MUST expose the lifecycle without inventing cursor, attempt, goal, item, blocker, or history values; observation MUST provide only fenced begin, eligible-reset, and eligible-replacement templates. |
| `AUT-LIF-010` | Exact idempotent replay and uncertain-outcome reconciliation MUST cover begin, disposition, eligible and force reset, and eligible and force replacement without weakening identity or revision fences. |

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
| `AUT-RECON-003` | Lookup MUST return the complete immutable original `podway.output/v3` success envelope or `podway.error/v1` terminal envelope, including request correlation, command, completion timestamp, workspace, job, session/result or public error details, from the retained receipt after the terminal job row is pruned. It MUST NOT retain or reveal the full original request. Cancelled jobs retain the closed cancellation summary. |
| `AUT-RECON-004` | Reusing an idempotency key for a different canonical request MUST continue to fail with the stable reuse error. |

## 19. Quiescent observation (AUT-OBS-001)

| ID | Normative requirement |
|---|---|
| `AUT-OBS-001` | A successful `status --wait-for-idle` MUST read after the queue barrier and report `pending_mutations=false`, `queued_count=0`, `running_job_id=null`, and a present latest workspace sequence. |

## 20. Compact status contract (AUT-OBS-002–004)

| ID | Normative requirement |
|---|---|
| `AUT-OBS-002` | `status --wait-for-idle --compact` MUST return workspace and queue identity, Procedure identity, session lifecycle and revision, and lifecycle-valid cursor, readiness, item, and blocker projections; every cursor-bearing projection MUST be null or empty for prepared state. |
| `AUT-OBS-003` | Compact status MUST omit instructions, prompts, task titles, previous-attempt narratives, and item values unless a value is strictly required to form a valid mutation. |
| `AUT-OBS-004` | Compact status MUST use a closed schema and the complete serialized JSON envelope MUST NOT exceed 262,144 UTF-8 bytes. |

### Self-contained observation (AUT-OBS-005–009)

| ID | Normative requirement |
|---|---|
| `AUT-OBS-005` | `observe`, `observe --wait-for-idle`, and `observe --after-job <job-id>` MUST use the same immediate and queue-barrier semantics as the existing read routes and MUST return one coherent Store observation. |
| `AUT-OBS-006` | A prepared or running observation MUST return closed `podway.observation-result/v2`. Running contains prepared-aware status, current next guidance, active item declarations with type-specific constraints and bounded typed value projections, and applicable mutation templates. Prepared contains prepared-aware status, cursor-free prepared guidance, no active items, and only begin, eligible-reset, and eligible-replacement templates. |
| `AUT-OBS-007` | Every mutation template MUST carry exact current workspace, session, and applicable revision/attempt/item/goal fences, mark the idempotency-key requirement, and classify whether an explicit user request is required. Templates MUST NOT invent semantic values or an idempotency key, and fences MUST NOT be represented as authentication or authorization. |
| `AUT-OBS-008` | A completed or cancelled observation MUST succeed with prepared-aware terminal status, null guidance, no active items, and only a terminal-disposition template when the current terminal revision has no disposition; a disposed terminal state MUST instead offer only eligible reset and replacement templates. Existing `next` terminal behavior remains unchanged. |
| `AUT-OBS-009` | Observation MUST omit history, bound every projected item value and lifecycle template, and leave the existing 65,536-byte envelope reserve within the 1,048,576-byte frame for the admitted maximum Procedure fixture. |

## 21. Command-specific JSON schemas (AUT-JSON-001–004)

| ID | Normative requirement |
|---|---|
| `AUT-JSON-001` | Version, daemon status, Procedure validation, start, begin, terminal disposition, reset, status, running and prepared next, observation, item mutation, graph-node transition, detached admission, job status/wait, and job lookup MUST each have a closed result schema. |
| `AUT-JSON-002` | Daemon, socket, identity, revision, attempt, digest, idempotency, and timeout failures MUST each have closed error-detail schemas. |
| `AUT-JSON-003` | Results and error details MUST carry an unambiguous schema identifier or discriminator. |
| `AUT-JSON-004` | A closed v1 object MUST reject unknown fields; adding fields requires a new schema identifier or discriminator version rather than an undocumented additive-field exception. |

## 22. Error and exit-code requirements (AUT-ERR-001–005)

| ID | Normative requirement |
|---|---|
| `AUT-ERR-001` | `WORKSPACE_UUID_MISMATCH`, `SESSION_ID_MISMATCH`, `PROCEDURE_DIGEST_MISMATCH`, `DAEMON_CONTRACT_MISMATCH`, socket errors, and wait timeout MUST have stable catalog entries and exit mappings. |
| `AUT-ERR-002` | A pre-admission contract, identity, digest, endpoint, or validation failure MUST report `admission.admitted=false`; a mismatch MUST NOT admit a job. |
| `AUT-ERR-003` | Adopted recoverable errors MUST carry one closed bounded `recovery` recipe containing `action`, canonical read-only `command`, structured `argv`, bounded `reason`, and `requires_explicit_authorization=false`. |
| `AUT-ERR-004` | Recovery recipes MUST recommend only `session.observe`, `job.lookup`, `job.wait`, `daemon.status`, or `workspace.doctor`; they MUST NOT weaken a fence or recommend retry, restart, repair, reset, reinstall, or another mutation. |
| `AUT-ERR-005` | Recovery recipes MUST derive only from existing public error details, MUST preserve code, retryability, exit class, and admission facts, and MUST NOT copy item values, requests, environment variables, file contents, credentials, or artifact bytes. |

## 23. Release artifact and installation (AUT-REL-001–004)

| ID | Normative requirement |
|---|---|
| `AUT-REL-001` | Podway MUST publish and support only native thin-arm64 `aarch64-apple-darwin` artifacts built and verified on an untranslated arm64 macOS host. |
| `AUT-REL-002` | The archive MUST contain both binaries, completions, presets, schemas, specs, canonicalization fixtures, the contract manifest, README, and license, with checksum and provenance. |
| `AUT-REL-003` | One offline manifest-bound Rust verifier MUST validate the source and packaged manifest shape, canonical and member digests, complete unique schema registry, full CLI and daemon envelopes and closed results, and agreement of packaged manifest digest, binary identities, source revision, target, and toolchain identity. |
| `AUT-REL-004` | v0.1.0 MUST NOT be tagged until every preceding roadmap task is completed and the repository-local `make dist` gate passes. |

## 24. Acceptance matrix

The controlled-PATH harness includes these probes from a directory that is not a
Podway worktree:

```bash
env -i PATH="<release-bin>:/usr/bin:/bin" podway version --json
env -i PATH="<release-bin>:/usr/bin:/bin" podway version --json --identity

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
| `AUT-T-CONTRACT` | matching peers, complete source/package schema validation, malformed v0.1.1 and generated identity/manifest/schema/reference drift, same version/different manifest, different version/same IPC, replaced executable, restart after upgrade | `CONID003`–`CONID006`, `REL12003`–`REL12004` |
| `AUT-T-ID` | replaced workspace/session, same numeric revision on another session, stale replacement, wrong attempt/item, guarded reads | `CASID003`–`CASID005` |
| `AUT-T-START` | matching/mismatching digest, prepared creation, source deletion/replacement/race, restart, exact replay, key reuse with another digest | `PSTRT001`–`PSTRT005`, `V2LIF-004`–`005` |
| `AUT-T-LIF` | prepared reconstruction, begin with and without a goal, forbidden prepared mutations, terminal disposition currentness, eligible and force reset/replacement, restart, replay, and stale fences | `V2LIF-002`–`005` |
| `AUT-T-RECON` | disconnect before/after admission, wait timeout, lookup in every state, domain failure, pruning, missing key, key reuse | `RECON001`–`RECON005` |
| `AUT-T-OBS` | idle barrier invariants, closed compact schema, maximum envelope size | `MCONT004`, `MCONT006`, `DOLGI002` |
| `AUT-T-JSON` | every result/detail fixture validates its discriminator and rejects unknown or malformed fields | `MCONT001`–`MCONT006` |
| `AUT-T-DIST` | `make dist` packages the native release-profile archive once, verifies its identity and layout, then runs packaged lifecycle, conflict, timeout, response-loss, reconciliation, identity, termination, and socket-cleanup checks through isolated foreground dev mode | `DOLGI005`, `REL10001`–`REL10004` |

The distribution qualification is the executable proof for the packaged client
contract and runs against the same archive selected for handoff.

## Procedure v2 automation boundary

Automation discovers Procedure v2 capability from the manifest-bound command
route and result-schema registries before dispatch. Registered but unserved v2
routes fail with `UNSUPPORTED_V2_CAPABILITY`; absent routes retain the ordinary
unknown-command or usage behavior. Automation never infers support from product
version text or human-readable output.

Every v2 mutation uses the existing workspace, session, attempt, revision, and
idempotency fences applicable to its command. A successful mutation result and
an admitted terminal error carry the same bounded job admission identity used by
lookup and replay. A retained terminal envelope always names a v2 mutation, and
job lookup requires its nested command to equal the immutable job command.
Preview and other authoring reads remain side-effect free.

## 25. Requirements-to-roadmap traceability

| Requirements | Implemented by | Planned evidence |
|---|---|---|
| `AUT-PATH-001`–`003`, `AUT-DAEMON-001`–`003` | `RPATH004`, `RPATH006`, `CONID006` | `AUT-T-PATH`, `AUT-T-CONTRACT` |
| `AUT-HOME-001`–`004` | `RPATH001`, `RPATH002`, `RPATH004` | `AUT-T-PATH`, `AUT-T-SOCK` |
| `AUT-SOCK-001`–`005`, `AUT-SEC-001`–`004` | `RPATH003`–`RPATH006` | `AUT-T-SOCK` |
| `AUT-CONTRACT-001`–`005` | `CONID001`–`CONID006` | `AUT-T-CONTRACT`, `AUT-T-DIST` |
| `AUT-ID-001`–`009` | `CASID001`–`CASID005`, `V2LIF-004` | `AUT-T-ID`, `AUT-T-LIF`, `AUT-T-JSON` |
| `AUT-START-001`–`004` | `PSTRT001`–`PSTRT005` | `AUT-T-START` |
| `AUT-LIF-001`–`010` | `V2LIF-002`–`005` | `AUT-T-LIF`, `AUT-T-OBS`, `AUT-T-JSON` |
| `AUT-ADMIT-001`–`003`, `AUT-RECON-001`–`004` | `RECON001`–`RECON005` | `AUT-T-RECON` |
| `AUT-OBS-001`–`009` | `MCONT004`, `MCONT006`, `DOLGI002`, `V2LIF-004`–`005` | `AUT-T-OBS`, `AUT-T-LIF` |
| `AUT-JSON-001`–`004`, `AUT-ERR-001`–`005` | `CASID004`, `MCONT001`–`MCONT006`, `V2AGT-005` | `AUT-T-JSON` |
| `AUT-REL-001`–`004` | `DOLGI005`, `REL10001`–`REL10005` | `AUT-T-DIST`, repository-local `make dist` |

## 26. Example Dolgorae command sequences

Dolgorae is the first consumer; these examples do not add Dolgorae workflow or
authorization semantics to Podway.

```bash
podway version --json --identity

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
  --task "WI-0042: Implement Podway graph dispatch"

podway --json \
  --socket "/Users/example/.podway/run/podwayd.sock" \
  --worktree "/Users/example/src/project" \
  --timeout 25s \
  --idempotency-key "$BEGIN_IDEMPOTENCY_KEY" \
  --if-workspace-uuid "$WORKSPACE_UUID" \
  --if-session-id "$SESSION_ID" \
  --if-session-revision 0 \
  begin \
  --goal "Complete WI-0042 safely" \
  --criterion verified="Required checks pass" \
  --actor "dolgorae"

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
