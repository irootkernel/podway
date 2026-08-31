# Podway Named Runtime Modes and Aquarium Development Service

## Status and authority

- Document state: `Adopted`
- Owning roadmap epic: `V2DVC`
- Target product release: v0.2.8
- Repository scope: Podway only
- External integration: Aquarium development channel contract v2
- Planning baseline: August 31, 2026

This dossier is the authoritative, decision-complete implementation plan for the
unfinished `V2DVC` epic. The active roadmap owns task order and status. Accepted
ADRs, specifications, canonical machine assets, and executable contracts retain
their normal precedence for implemented behavior. Until an owning task promotes
and implements a decision, this dossier describes intended behavior rather than
current product capability.

The external Aquarium manager interface is fixed input to this plan. Podway will
implement that interface without requiring Aquarium to add Podway-specific mode,
daemon, path, or activation behavior.

## 1. Verified context and evidence

### 1.1 Current Podway topology

The installed service has one per-user runtime containing its singleton lock,
socket, registry, logs, bootstrap diagnostics, and service metadata. Worktree
state remains under `.podway/runtime/state.sqlite3`, where the daemon is the sole
normal writer.

Current raw `--dev` derives separate development socket, registry, and log paths
but retains the production account lock. Managed contributor and release
qualification roots avoid production contention by supplying a private account
root, development root, sandbox, and exact binary snapshot through
`podway.managed-dev-runtime/v2`. They are disposable and cannot admit arbitrary
real consumer worktrees.

The CLI chooses production or development paths before contacting the daemon.
Workspace config has only `podway.workspace/v1`; it carries no daemon ownership
identity. A worktree-local Store likewise has no explicit runtime-mode binding.
Changing only the global lock path would therefore leave endpoint, registry,
observability, recovery, and Store ownership ambiguous.

### 1.2 Aquarium managed-service boundary

Aquarium contract v2 requires Podway to build one immutable managed-service
bundle from an exact clean local `main` commit. The bundle exposes a safe command,
matching `podway` and `podwayd` binaries, and a producer-owned controller.
Aquarium owns generic publication, leases, active and pending generation
selection, and mutation approval. Podway owns its service internals.

The fixed producer and controller interfaces are:

```text
make aquarium-dev-describe
make aquarium-dev-build AQUARIUM_DEV_OUTPUT=<absolute-empty-directory>

libexec/aquarium-dev-service status --json \
  --runtime-root <absolute-root>
libexec/aquarium-dev-service plan --json \
  --runtime-root <absolute-root> \
  --generation-root <absolute-generation>
libexec/aquarium-dev-service apply --json \
  --runtime-root <absolute-root> \
  --generation-root <absolute-generation> \
  --plan-token <exact-token>
```

Aquarium publishes a managed-service generation to `pending` and does not select
it as `current` until an independently approved `service-apply` succeeds. The
Podway command entrypoint must continue to invoke the bundled CLI as
`podway --dev`; Aquarium will not synthesize a generic mode argument.

### 1.3 Release-cycle boundary

The immutable `v0.2.7` tag points to current `main`, but source version metadata
and the changelog still identify v0.2.7 as the open development version. This
documentation task does not change executable version identity or the changelog.
`V2DVC-002` owns opening v0.2.8 together with the first versioned machine
contracts so the repository is never left with a documentation-only version
claim that its release verifier rejects.

## 2. Goals, non-goals, and scope

### 2.1 Goals

- Generalize daemon isolation around one bounded runtime mode key.
- Preserve `prod` as the universal default when mode is omitted.
- Permit production, Aquarium development, demo, contributor, and isolated
  qualification daemons to coexist without sharing runtime-global state.
- Keep one worktree bound to exactly one effective mode and one normal writer.
- Provide a recoverable, token-bound way to move a worktree between modes.
- Deliver the exact Aquarium managed-service bundle and persistent `dev` service
  without changing Aquarium's external interface.
- Keep publication separate from activation so routine Podway commits do not
  restart the development service used by other Aquarium work.
- Reuse the keyed runtime model for release qualification while retaining exact
  binary identity, sandbox admission, and owner-private cleanup.

### 2.2 Non-goals

- Do not allow a workspace to select several modes or several daemons at once.
- Do not allow two modes to read or write one worktree Store concurrently.
- Do not make a mode key an environment profile, feature set, plugin name,
  command, secret, or arbitrary path.
- Do not start daemons or install LaunchAgents merely because tracked config names
  a mode.
- Do not add parallel Procedure execution, remote coordination, network access,
  Git mutation, configured commands, or artifact storage.
- Do not make Aquarium interpret Podway sockets, registries, logs, databases,
  recovery markers, or daemon-specific status.
- Do not let release qualification reuse the installed production or shared
  Aquarium development service.
- Do not enroll Podway into a live Aquarium installation, activate a development
  service, publish a release, or modify a user's workspace as part of this epic.

### 2.3 Owned change surface

The epic owns versioned workspace configuration, runtime-mode values, service
paths, managed-runtime metadata, CLI and daemon selectors, daemon status and
errors, Store and registry admission, workspace mode switching, Aquarium producer
artifacts and controller behavior, qualification helpers, conformance, and the
durable documentation that describes those implemented surfaces.

## 3. Accepted design decisions and interfaces

### 3.1 Mode value and workspace configuration

`RuntimeModeV1` is one validated identifier. Its serialized representation is a
lowercase ASCII string matching `[a-z][a-z0-9]*(?:-[a-z0-9]+)*`, between 1 and 64
bytes inclusive. `/`, `.`, `_`, whitespace, uppercase characters, empty segments,
leading or trailing hyphens, and longer values are rejected. `prod` is reserved
but valid; `dev` has no privileged domain semantics beyond its compatibility
aliases and Aquarium ownership.

Workspace config v1 remains byte-for-byte compatible and has effective mode
`prod`. Workspace config v2 adds one optional scalar field:

```yaml
schema: podway.workspace/v2
mode: demo
procedure_paths:
  - .podway/procedures
default_preset: sw-dev-v2
job_queue:
  max_pending: 256
ui:
  show_stage_in_prompt: false
```

Omitting `mode` in v2 also means `prod`; explicit `mode: prod` is accepted. A
sequence, mapping, null, boolean, number, duplicate field, or invalid identifier
is rejected by bounded config parsing.

While v0.2.7 is the installed stable consumer, the canonical result of switching
to production is schema v1 with no mode field. Switching from production to a
named mode upgrades the document to v2 and inserts the mode without rewriting
unrelated supported values or comments. Switching back removes the mode and
returns the document to the v1 schema. If a future config contains a v2-only field
other than mode, a lossy downgrade must fail instead of discarding it.

### 3.2 Runtime namespace and paths

Runtime identity is:

```text
(canonical runtime root, validated mode key)
```

Every identity owns a complete namespace, not only a lock:

```text
run/podwayd.lock
run/podwayd.sock
state/service.json
state/workspaces.json
state/recovery.json
logs/podwayd.log
logs/podwayd-bootstrap.log
```

The path policy is:

| purpose | runtime root | mode | lifecycle owner |
|---|---|---|---|
| installed production | existing per-user Podway home | `prod` | Podway service manager |
| unmanaged local mode | `~/.podway/modes/<mode>/` | selected key | explicit foreground caller |
| Aquarium development | `~/.aquarium-dev/runtime/podway/` | `dev` | Podway Aquarium controller |
| contributor | owner-private managed root | `contributor` | contributor helper |
| release qualification | unique owner-private root below `/private/tmp` | `release-qa` | release qualifier |

The last two keys are conventional; their unique managed root and purpose are
also identity inputs. A helper may use another valid bounded key when a run needs
an additional diagnostic distinction.

Named path construction must reject non-normalized roots, symlinked or
wrong-owner components, unsafe permissions, overlong Unix socket paths, and a
namespace that aliases production or another admitted identity.

### 3.3 Managed runtime metadata

`podway.managed-runtime/v3` replaces the closed topology of
`podway.managed-dev-runtime/v2`. The canonical metadata binds at least:

- schema and purpose;
- effective UID;
- canonical root and mode key;
- all derived runtime paths;
- optional sandbox admission boundary;
- exact CLI, daemon, and controller paths and SHA-256 identities where present;
- immutable generation identity for managed-service use; and
- a bounded metadata version and size.

Metadata is authoritative when present. An absent unmanaged manifest uses only
the canonical production or `~/.podway/modes/<mode>/` path derivation. Invalid,
unsafe, mismatched, or stale metadata prevents startup and client connection; it
never falls back to production or another mode.

Existing v2 contributor and qualification roots are disposable. Tooling must
stop and remove or recreate an exact owned v2 root rather than silently adopting
it as persistent v3 state.

### 3.4 CLI and daemon grammar

The public compatibility grammar is:

```text
podway [--mode <key>] [--worktree <path>] <command> ...
podway --dev [--worktree <path>] <command> ...

podwayd --service
podwayd --mode <key>
podwayd --dev
```

No CLI mode and production service mode select `prod`. `--dev` is an exact alias
for `--mode dev`. Supplying both selectors, repeating a selector, using `--dev`
with another mode, or combining a named foreground mode with production-only
service lifecycle arguments is invalid.

Shell completion, help, JSON identity, daemon status, diagnostics, and logs expose
the effective mode where it is required to prevent endpoint ambiguity. Automation
depends on stable machine fields, never human text.

### 3.5 Admission and Store binding

Before Store inspection or mutation, a request must establish all of the
following:

1. the CLI's selected mode and endpoint;
2. the connected daemon's runtime root and mode identity;
3. the selected worktree's current config and effective mode;
4. the worktree Store's persisted mode binding, when initialized; and
5. the daemon namespace's exact registry generation for that worktree.

Any mismatch returns a stable non-retryable `WORKSPACE_MODE_MISMATCH` before
normal Store open. The error details report bounded expected and actual mode keys
and the failing boundary without leaking raw internal paths.

Initialization stores the effective mode with the workspace identity. A v1
workspace can therefore initialize only under `prod`. A manual config edit does
not migrate ownership: the prior daemon rejects the changed config, and the
target daemon rejects the old Store binding until the supported switch lifecycle
converges it.

Registry documents remain private to a single runtime namespace. Startup and
pre-mutation recovery revalidate config and Store mode before treating a
registered worktree as owned. A registry entry in another namespace is not proof
that the current namespace may open the Store.

### 3.6 Workspace mode switching

The native command family is:

```text
podway workspace mode plan --to <key> [--worktree <path>] --json
podway workspace mode apply --to <key> \
  --plan-token <exact-token> [--worktree <path>] --json
```

Human mode may render the same preview and request confirmation, but it submits
the identical token-bound operation. Plan is read-only. Its token binds the exact
worktree and workspace UUID, source and target modes, config digest and relevant
text generation, source registry generation, Store identity and disposable state
summary, source daemon idleness, target daemon identity and readiness, and a
bounded expiry.

Plan returns `no-change` when source and target are identical. It returns a busy
or unavailable result without a mutation token when the source has an in-flight
client, queued or running job, maintenance, recovery, or an uncloseable Store; or
when the target daemon is absent, mismatched, recovering, or otherwise not ready.
A durable session with no executing work is included in the disposable-state
summary but does not make the daemon busy by itself.

Apply re-runs the plan, rejects stale input, closes source admission, drains exact
in-flight state, retires the source scheduler and registry generation, and deletes
only `.podway/runtime/` state. It then updates config atomically with comment and
mode preservation, creates a fresh runtime binding through the target daemon, and
publishes the target registry entry. Procedures, other supported config content,
`.podway/.gitignore`, the Git worktree, Git index and refs, and files outside
`.podway/runtime/` are not mutated.

A marker adjacent to disposable runtime state records the exact transition and
completed steps. Retry converges the same operation after interruption. Once
runtime deletion begins, failure does not claim that sessions, attempts, queues,
receipts, or other deleted state were rolled back. Success requires exactly one
target binding and no live source binding.

### 3.7 Aquarium producer bundle

`make aquarium-dev-describe` emits the supported producer description for one v2
`managed-service`. `make aquarium-dev-build` accepts only an absolute empty output
directory and a clean local `main`, consumes committed bytes, and emits one sealed
bundle plus the exact-SHA artifact manifest.

The bundle contains:

```text
bundle/bin/podway
bundle/libexec/podway
bundle/libexec/podwayd
bundle/libexec/aquarium-dev-service
bundle/manifest.json
```

`bin/podway` is a development-safe launcher that leases the active generation and
executes the bundled CLI as `podway --dev`. It never resolves production binaries
or falls back to the production endpoint. The internal manifest binds the full
Git SHA, `v<next>-dev.<sha12>` identity, canonical bundle digest, every executable
digest, and the expected controller and runtime protocol versions.

The controller implements Aquarium's exact `status`, `plan`, and `apply` commands.
It owns LaunchAgent label `dev.aquarium.podwayd`, the plist below its managed
runtime, and every Podway-specific service file. The plist executes the exact
generation's `libexec/podwayd --dev`, never a mutable PATH entry, and keeps all
state below Aquarium's supplied Podway runtime root.

### 3.8 Publication, activation, and rollback

Publishing a generation changes only Aquarium's pending selector. It does not
restart or repoint the active daemon. Candidate identity is the exact Git SHA and
bundle digest, not the shared base string `v0.2.8-dev`.

Controller status reports the observed service phase, runtime mode, active CLI
and daemon identities, LaunchAgent identity, busy state, pending recovery, and
bounded recovery debt. Plan is read-only and returns one of `install`, `activate`,
`repair`, `defer`, or `no-change`, with a confirmation token bound to the observed
active generation and exact target.

Apply revalidates the token under Aquarium's service lock. For replacement it
closes new admission, drains clients, jobs, maintenance, and recovery, stops the
old exact LaunchAgent generation, installs the exact target plist, bootstraps it,
and waits for matching ready status. Only then may its result permit Aquarium to
advance `current` and clear `pending`.

The activation path admits only storage changes that remain readable by the
immediately prior generation or that have a producer-owned reversible transition.
If target startup or readiness fails, the controller stops the target and restores
the prior exact generation. It reports successful rollback only after that
generation is ready with its original identity. Otherwise it persists bounded
recovery debt, leaves the pending target recoverable, and fails closed.

An exact pending generation must pass the Podway development gate and focused
controller rollover qualification before an operator approves activation. This is
an approval policy and evidence boundary; the controller does not infer test
success from a version string or mutable filesystem marker.

### 3.9 Contributor and release qualification

Contributor tooling and packaged release qualification use managed-runtime/v3
with purpose-specific policy. Contributor mode may enable debug-only admission
only inside its exact sandbox. Release qualification never enables debug-only
behavior and uses only extracted release-profile binaries whose digests match the
packaged manifest.

Each release-qualification run creates a unique owner-private root below
`/private/tmp`, starts its own foreground `release-qa` daemon, creates worktrees
whose config selects that same mode, runs the packaged scenarios, stops the exact
daemon, removes its socket, and reconciles only its owned root. It does not inspect,
stop, acquire, modify, or activate production or Aquarium development state.

Parallel qualification runs remain independent because runtime root is part of
identity. A failure or interruption preserves the exact recoverable root and
process identity rather than performing broad temporary-directory cleanup.

## 4. Failure handling and compatibility boundaries

### 4.1 Fail-closed boundaries

The implementation must fail before Store access or service mutation for invalid
mode values, unsafe roots, endpoint mismatch, metadata identity drift, CLI/daemon
mode disagreement, config mode disagreement, Store binding disagreement, stale
registry state, missing target readiness, busy source state, stale plan tokens,
irreversible activation migrations, or unresolved recovery debt.

All input, metadata, tokens, paths, mode values, status collections, retries,
waits, logs, and recovery records remain bounded. User-controlled input cannot
panic the CLI, daemon, controller, parser, or service manager.

### 4.2 Production compatibility

- Existing workspace v1 documents continue to parse with identical defaults and
  canonical identities and always select `prod`.
- The installed production service retains its label, paths, lock, socket,
  registry, logs, CLI behavior, and operator-controlled lifecycle.
- Returning a workspace to production writes v1-compatible config while v0.2.7
  remains the stable installed consumer.
- Production commands do not fall back to a named-mode daemon, and named-mode
  commands do not fall back to production.
- `--dev` remains accepted but changes from the legacy raw shared-lock topology to
  the keyed `dev` namespace only when the owning implementation task admits the
  complete config and Store-mode safety boundary.

### 4.3 Aquarium compatibility

Podway does not change Aquarium's producer description, artifact manifest,
manager operations, controller command grammar, result schemas, current/pending
selection, generic lock ownership, or approval model. The `--dev` alias and
producer-owned controller absorb Podway's generalized mode design.

No Podway commit, build, or publish operation automatically invokes
`service-apply`. Other Aquarium development work continues using the prior current
generation until an exact pending generation is separately qualified and
approved.

## 5. Roadmap ownership, dependencies, and traceability

`V2DVC` depends on completed `V2RET`. Its tasks execute in numeric order; the
active roadmap alone owns their status.

| requirement group | owning task | durable authority promoted by completion |
|---|---|---|
| Accepted architecture, dossier, compatibility and execution order | `V2DVC-001` | ADR-0031, TODO index, active roadmap |
| v0.2.8 identity and versioned config, mode, status, error and managed-runtime contracts | `V2DVC-002` | changelog, schemas, executable contracts, interface specifications |
| Runtime mode domain, keyed paths, metadata validation, CLI and daemon selectors | `V2DVC-003` | architecture, service and CLI specifications, implementation guidance |
| Pre-Store mode binding, registry ownership and recoverable mode switch | `V2DVC-004` | storage, recovery, IPC, CLI and automation specifications |
| Aquarium producer bundle, controller, LaunchAgent and safe generation activation | `V2DVC-005` | producer assets, service contracts, operations specifications |
| Contributor and release qualification migration to private named runtimes | `V2DVC-006` | release specification, qualification tools, cleanup guidance |
| Integrated conformance, documentation promotion and dossier retirement | `V2DVC-007` | testing traceability, durable docs, roadmap completion evidence |

One task and its direct verification belong in one commit. A task does not gain
runtime status merely because earlier tasks reserve schemas or document planned
behavior. The final task must replace every surviving reference to this dossier
with durable implemented authority, remove it from the TODO index, and delete it
before completing the epic.

## 6. Verification and release acceptance

### 6.1 Contract and parser coverage

- v1 and v2-without-mode select `prod`;
- explicit `prod` and valid named modes parse deterministically;
- arrays, mappings, nulls, duplicates, unsafe YAML, invalid identifiers, and all
  first-over-limit values fail;
- canonicalization and schema manifests bind exact bytes;
- `--dev` and `--mode dev` select the same runtime identity;
- conflicting CLI and daemon selectors fail with stable errors.

### 6.2 Runtime and ownership coverage

- production, dev, demo, contributor, and parallel release-qa paths share no
  lock, socket, registry, log, metadata, or recovery file;
- production and multiple named daemon processes coexist;
- different worktrees operate under different modes concurrently;
- the same worktree cannot be registered, opened, or mutated by two modes;
- config, endpoint, daemon, registry, and Store mismatch fail before Store open;
- startup, restart, registry recovery, and stale-root pruning retain mode binding;
- non-UTF-8 worktree paths and bounded Unix socket paths retain their current
  safety guarantees.

### 6.3 Mode-switch coverage

- every source-to-target combination, including prod-to-named, named-to-prod, and
  named-to-named;
- no-change preview, stale token, target unavailable, source busy, and identity
  drift;
- preservation of Procedures, comments, supported config values, `.gitignore`,
  and Git state;
- deletion of only runtime sessions, attempts, queues, receipts, and registration;
- interruption at every destructive boundary and convergent retry;
- v0.2.7 opening the canonical config after return to production.

### 6.4 Aquarium coverage

- exact clean-main describe and build results;
- immutable bundle layout, containment, digest and matching executable identity;
- initial install, login-persistent restart and production coexistence;
- publication changing pending without restarting current;
- busy deferral and stale controller token rejection;
- idle activation to the exact pending generation;
- failed target start, proven rollback, incomplete rollback and recovery repair;
- launcher generation and service-generation leases;
- no production fallback from the Aquarium entrypoint.

### 6.5 Qualification coverage

- contributor and release purposes enforce their distinct admission policy;
- extracted CLI and daemon identities match the packaged manifest;
- parallel private qualification daemons coexist with production and Aquarium
  development;
- success leaves no qualification process or socket;
- failure preserves only exact helper-owned recoverable state;
- shared Aquarium current, pending, LaunchAgent, registry and logs remain
  unchanged.

Each executable or contract-changing task runs focused checks and the complete
`make test` development gate. `make dist` is required only when distribution
readiness is explicitly in scope under the repository release workflow. Epic
completion does not itself authorize push, tag, publication, installation,
enrollment, workspace switching, or runtime activation.

## 7. References

- [ADR-0003: Daemon Single Writer](../architecture-decision-records/0003-daemon-single-writer.md)
- [ADR-0004: Worktree-Local State](../architecture-decision-records/0004-worktree-local-state.md)
- [ADR-0012: Explicit Daemon Endpoint and Canonical Per-User Podway Home](../architecture-decision-records/0012-explicit-daemon-endpoint-and-canonical-per-user-podway-home.md)
- [ADR-0020: Managed Dev Runtime Isolation](../architecture-decision-records/0020-managed-dev-runtime-isolation.md)
- [ADR-0031: Keyed Runtime Modes and Managed Development Service](../architecture-decision-records/0031-keyed-runtime-modes-and-managed-development-service.md)
- [System architecture](../architecture/system.md)
- [Git worktree and filesystem](../architecture/git-worktree-and-filesystem.md)
- [macOS service architecture](../architecture/macos-service.md)
- [Release workflow](../implementation-tips/release.md)
- [Active roadmap](../roadmap/README.md)
