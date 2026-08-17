# Podway Roadmap

This document owns adopted work, execution order, and current status. Candidate
work and adopted design dossiers live in [TODO](../todo/) under their distinct
lifecycle rules. Completed release history is preserved under [archive](archive/),
including the [v0.1.1 release roadmap](archive/v0.1.1.md).

## Status definitions

- `Planned`: adopted but not started
- `In Progress`: implementation or verification is underway
- `In Review`: implementation is complete and acceptance is being reviewed
- `Completed`: explicit acceptance has passed
- `Deferred`: intentionally removed from the current release scope
- `Blocked`: cannot progress without an external decision or prerequisite

New roadmap task IDs use the five-character epic ID, a hyphen, and a
three-digit sequence, for example `V2CTR-001`. Existing `REL12` compact task
IDs are retained as historical identifiers.

## REL12 — Podway v0.1.2 Contract Recovery and Release

| id | title | status | goal | references |
|---|---|---|---|---|
| `REL12001` | Freeze the v0.1.2 recovery design | Completed | Adopt the decision-complete design, authority boundaries, release constraints, and ordered implementation plan. | [Release completion](archive/v0.1.2-release-report.md) |
| `REL12002` | Audit the v1 compatibility boundary | Completed | Prove released-schema compatibility and record the exact pre-release consumer migration boundary. | [Compatibility evidence](archive/v0.1.2-release-report.md#qualification-and-publication) |
| `REL12003` | Repair the version identity contract | Completed | Make both binaries emit one identical schema-conformant identity and reject malformed runtime probes. | [Identity contract](../specs/interfaces/automation-client-contract.md#13-cli-and-daemon-contract-identity-aut-contract-001005) |
| `REL12004` | Enforce authoritative packaged-schema validation | Completed | Validate complete identity envelopes using only the exact manifest-bound packaged contract set. | [Release contract](../specs/interfaces/automation-client-contract.md#23-release-artifact-and-installation-aut-rel-001004) |
| `REL12005` | Harden qualification and release evidence | Completed | Add early singleton diagnostics and close provenance, handoff, digest, and conformance validation. | [Release packaging](../specs/operations/release-and-packaging.md#checksums-and-provenance) |
| `REL12006` | Build and qualify the native v0.1.2 distribution | Completed | Advance the version and pass every clean native arm64 and extracted-distribution release gate. | [Qualification evidence](archive/v0.1.2-release-report.md#qualification-and-publication) |
| `REL12007` | Publish and independently reverify v0.1.2 | Completed | Publish the annotated immutable release and reverify all downloaded bytes and closed identities. | [Final report](archive/v0.1.2-release-report.md) |

Tasks are completed in table order. At most the first incomplete task may be `In
Progress`, `In Review`, or `Blocked`; later tasks remain `Planned`.

## Release program PV2GA — Podway v0.2.0 Full-Feature GA

`PV2GA` is a completed release program, not an epic or task prefix. Its ten epics
delivered one stable release; no individual epic was a supported partial v2
release. The task goals remain below, current behavior is owned by the
[current specifications](../specs/), and the immutable
[v0.2.0 release report](archive/v0.2.0-release-report.md) records completion.

Epic dependencies are:

```text
V2CTR -> V2MOD
          |-> V2AUT -----------------------------|
          |-> V2GRF -> V2RUN -> V2DRW -> V2GOL -|-> V2DOG -> V2REL
          `-> V2PLT ----^------------------------|
```

Within each epic, tasks execute in numeric order. At most the first incomplete
task in an unblocked epic may be `In Progress`, `In Review`, or `Blocked`; later
tasks in that epic remain `Planned`. An epic with an incomplete dependency must
remain entirely `Planned`.

## V2CTR — Canonical Contract Baseline

Dependencies: none.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2CTR-001` | Promote accepted decisions into specifications | Completed | Establish the normative graph, recorded-item, compatibility, admission, and GA boundaries. | [Current contract traceability](../specs/quality/requirements-traceability.md) |
| `V2CTR-002` | Add the Procedure v2 schema | Completed | Define the closed bounded YAML and JSON authoring contract. | [Current contract traceability](../specs/quality/requirements-traceability.md) |
| `V2CTR-003` | Define v2 result and diagnostic schemas | Completed | Close every new or version-bumped public result family. | [Current contract traceability](../specs/quality/requirements-traceability.md) |
| `V2CTR-004` | Register the public contract delta | Completed | Register the exact route, error, schema, and manifest surface. | [Current contract traceability](../specs/quality/requirements-traceability.md) |
| `V2CTR-005` | Extend conformance traceability | Completed | Map every v2 requirement to a contract, test class, and task. | [Current contract traceability](../specs/quality/requirements-traceability.md) |
| `V2CTR-006` | Build the v2 fixture corpus | Completed | Provide bounded known-answer, negative, compatibility, and maximum-size evidence. | [Current contract traceability](../specs/quality/requirements-traceability.md) |

## V2MOD — Procedure Model and Configuration

Dependencies: `V2CTR`.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2MOD-001` | Add v2 domain values | Completed | Represent action, decision, route, rework, and goal values in core. | [Current Procedure contract](../specs/domain/procedure-and-item-specification.md) |
| `V2MOD-002` | Enforce graph cursor invariants | Completed | Preserve exactly one authoritative cursor and active attempt. | [Current Procedure contract](../specs/domain/procedure-and-item-specification.md) |
| `V2MOD-003` | Add workflow memory record types | Completed | Represent recorded-item references and immutable decision, rework, and goal records. | [Current Procedure contract](../specs/domain/procedure-and-item-specification.md) |
| `V2MOD-004` | Parse v2 YAML | Completed | Dispatch and parse bounded v2 YAML without changing v1. | [Current Procedure contract](../specs/domain/procedure-and-item-specification.md) |
| `V2MOD-005` | Parse v2 JSON | Completed | Produce semantics identical to equivalent YAML. | [Current Procedure contract](../specs/domain/procedure-and-item-specification.md) |
| `V2MOD-006` | Validate v2 semantics | Completed | Reject invalid identities, references, routes, selectors, goal mappings, and bounds. | [Current Procedure contract](../specs/domain/procedure-and-item-specification.md) |
| `V2MOD-007` | Canonicalize and digest v2 | Completed | Produce deterministic IR, ordering, snapshots, and digests. | [Current Procedure contract](../specs/domain/procedure-and-item-specification.md) |
| `V2MOD-008` | Lock v1 configuration compatibility | Completed | Keep released v1 parsing and canonical identities unchanged. | [Current Procedure contract](../specs/domain/procedure-and-item-specification.md) |

## V2AUT — Authoring Toolchain

Dependencies: `V2MOD`.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2AUT-001` | Format to stdout | Completed | Emit deterministic canonical authoring text without mutation. | [Current authoring contract](../specs/domain/procedure-and-item-specification.md) |
| `V2AUT-002` | Check formatting | Completed | Detect formatting drift with stable non-writing exit behavior. | [Current authoring contract](../specs/domain/procedure-and-item-specification.md) |
| `V2AUT-003` | Write formatting safely | Completed | Update only the named file while preserving supported comments. | [Current authoring contract](../specs/domain/procedure-and-item-specification.md) |
| `V2AUT-004` | Lint Procedure v2 | Completed | Emit stable advisory authoring diagnostics. | [Current authoring contract](../specs/domain/procedure-and-item-specification.md) |
| `V2AUT-005` | Check Procedure v2 | Completed | Aggregate validate, vet, lint, digest, and summary results. | [Current authoring contract](../specs/domain/procedure-and-item-specification.md) |
| `V2AUT-006` | Scaffold Procedure v2 | Completed | Generate a minimal bounded reviewable authoring starting point. | [Current authoring contract](../specs/domain/procedure-and-item-specification.md) |
| `V2AUT-007` | Convert v1 to v2 | Completed | Produce a deterministic review-required action-only v2 candidate. | [Current authoring contract](../specs/domain/procedure-and-item-specification.md) |
| `V2AUT-008` | Close authoring diagnostics | Completed | Stabilize diagnostic codes, locations, ordering, bounds, and JSON. | [Current authoring contract](../specs/domain/procedure-and-item-specification.md) |

## V2GRF — Graph Vetting and Projections

Dependencies: `V2MOD`.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2GRF-001` | Vet graph semantics | Completed | Prove topology, routing, dominance, evidence, skip, rework, and goal rules. | [Current graph contract](../specs/domain/procedure-and-item-specification.md) |
| `V2GRF-002` | Vet liveness and budgets | Completed | Enforce static and read-back budgets without limiting valid traversal. | [Current graph contract](../specs/domain/procedure-and-item-specification.md) |
| `V2GRF-003` | Project graph JSON | Completed | Emit a deterministic canonical machine projection. | [Current graph contract](../specs/domain/procedure-and-item-specification.md) |
| `V2GRF-004` | Project Mermaid | Completed | Emit the required human review projection. | [Current graph contract](../specs/domain/procedure-and-item-specification.md) |
| `V2GRF-005` | Project PlantUML | Completed | Emit deterministic PlantUML without invoking a renderer. | [Current graph contract](../specs/domain/procedure-and-item-specification.md) |
| `V2GRF-006` | Project DOT | Completed | Emit deterministic DOT without invoking Graphviz. | [Current graph contract](../specs/domain/procedure-and-item-specification.md) |
| `V2GRF-007` | Preview Procedure v2 | Completed | Present read-only checks, summary, Mermaid, digest, and confirmed start argv. | [Current graph contract](../specs/domain/procedure-and-item-specification.md) |
| `V2GRF-008` | Close projection conformance | Completed | Prove all formats agree on identities and transitions, exclude evidence references as flow edges and runtime or sensitive state, and remain stable across equivalent input forms. | [Current graph contract](../specs/domain/procedure-and-item-specification.md) |

## V2PLT — Persistence, Protocol, CLI, and Admission

Dependencies: `V2CTR` and `V2MOD`.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2PLT-001` | Add SQLite schema v3 | Completed | Add parallel v2 tables through an atomic v1-preserving migration. | [Current storage contract](../specs/storage/sqlite-model.md) |
| `V2PLT-002` | Persist graph and action state | Completed | Persist snapshots, cursor, trace, counters, and action attempts. | [Current storage contract](../specs/storage/sqlite-model.md) |
| `V2PLT-003` | Persist workflow memory | Completed | Persist items, references, decisions, rework, validity, and history. | [Current storage contract](../specs/storage/sqlite-model.md) |
| `V2PLT-004` | Persist goal state | Completed | Persist goal revisions, criteria, results, and assessments. | [Current storage contract](../specs/storage/sqlite-model.md) |
| `V2PLT-005` | Harden store lifecycle | Completed | Close upgrade, reopen, recovery, reset, and downgrade behavior. | [Current storage contract](../specs/storage/sqlite-model.md) |
| `V2PLT-006` | Add bounded v2 protocol | Completed | Decode and serialize closed compatible bounded v2 envelopes. | [Current storage contract](../specs/storage/sqlite-model.md) |
| `V2PLT-007` | Dispatch v2 daemon routes | Completed | Preserve sole-writer mutation and read-only authoring behavior. | [Current storage contract](../specs/storage/sqlite-model.md) |
| `V2PLT-008` | Add v2 CLI surfaces | Completed | Provide grammar, JSON, human rendering, help, and completion. | [Current storage contract](../specs/storage/sqlite-model.md) |
| `V2PLT-009` | Gate development admission | Completed | Provide development-only admission eligibility for explicitly isolated disposable state. | [Current storage contract](../specs/storage/sqlite-model.md) |
| `V2PLT-010` | Close persistence and protocol failures | Completed | Prove stale, duplicate, malformed, restart, storage, peer, and downgrade behavior. | [Current storage contract](../specs/storage/sqlite-model.md) |

## V2RUN — Action Runtime

Dependencies: `V2GRF` and `V2PLT`.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2RUN-001` | Start confirmed v2 sessions | Completed | Bind custom and preset starts to reviewed canonical digests. | [Current transition contract](../specs/domain/state-transitions.md) |
| `V2RUN-002` | Serve v2 status and next | Completed | Expose deterministic bounded cursor, readiness, action, goal, and guidance views. | [Current transition contract](../specs/domain/state-transitions.md) |
| `V2RUN-003` | Complete actions and read items back | Completed | Gate completion on recorded items and present selected prior values. | [Current transition contract](../specs/domain/state-transitions.md) |
| `V2RUN-004` | Retry v2 actions | Completed | Create a fresh attempt while preserving immutable history. | [Current transition contract](../specs/domain/state-transitions.md) |
| `V2RUN-005` | Skip eligible placements | Completed | Enforce declared skip policy and terminal readiness. | [Current transition contract](../specs/domain/state-transitions.md) |
| `V2RUN-006` | Derive terminal and blocked states | Completed | Present completed, blocked, and dead-end states without ambiguity. | [Current transition contract](../specs/domain/state-transitions.md) |
| `V2RUN-007` | Enforce runtime preconditions | Completed | Reject stale mutations and replay exact idempotent receipts. | [Current transition contract](../specs/domain/state-transitions.md) |
| `V2RUN-008` | Close action runtime recovery | Completed | Prove concurrency, restart, durable-job, storage, and repeated-retry behavior. | [Current transition contract](../specs/domain/state-transitions.md) |

## V2DRW — Decisions and Rework

Dependencies: `V2RUN`.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2DRW-001` | Decide the active route | Completed | Validate and atomically record one allowed decision option. | [Current lifecycle contract](../specs/domain/rework-and-lifecycle.md) |
| `V2DRW-002` | Persist decision transitions | Completed | Preserve immutable option, route, reason, attribution, evidence snapshots, and trace records. | [Current lifecycle contract](../specs/domain/rework-and-lifecycle.md) |
| `V2DRW-003` | Rework a valid trace target | Completed | Enforce allowed targets and completed-session reactivation policy. | [Current lifecycle contract](../specs/domain/rework-and-lifecycle.md) |
| `V2DRW-004` | Invalidate and re-enter a suffix | Completed | Atomically stale the affected trace and activate one fresh target attempt. | [Current lifecycle contract](../specs/domain/rework-and-lifecycle.md) |
| `V2DRW-005` | Read decisions and rework back | Completed | Expose bounded current and stale workflow history without satisfying progression. | [Current lifecycle contract](../specs/domain/rework-and-lifecycle.md) |
| `V2DRW-006` | Close decision and rework failures | Completed | Prove invalid, stale, duplicate, crash, restart, and repeated-cycle one-writer/cursor behavior. | [Current lifecycle contract](../specs/domain/rework-and-lifecycle.md) |

## V2GOL — Goal Tracking

Dependencies: `V2DRW`.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2GOL-001` | Define and revise session goals | Completed | Enforce opt-in, revisions, criteria, rework, stale checks, and reactivation. | [Current domain contract](../specs/domain/domain-model.md) |
| `V2GOL-002` | Assess goal criteria | Completed | Record homogeneous bounded cited criterion results atomically. | [Current domain contract](../specs/domain/domain-model.md) |
| `V2GOL-003` | Derive goal outcomes | Completed | Map assessments to outcomes and gate terminal progression on fresh results. | [Current domain contract](../specs/domain/domain-model.md) |
| `V2GOL-004` | Read goal history back | Completed | Expose bounded immutable revisions and assessments with stale state distinguished. | [Current domain contract](../specs/domain/domain-model.md) |
| `V2GOL-005` | Close goal failure and recovery | Completed | Prove mode, citation, revision, target, cancellation, restart, and budget errors. | [Current domain contract](../specs/domain/domain-model.md) |

## V2DOG — Presets and Dogfood

Dependencies: `V2AUT`, `V2GRF`, `V2RUN`, `V2DRW`, and `V2GOL`.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2DOG-001` | Add the software-development preset | Completed | Ship a bounded full-feature `sw-dev-v2` Procedure. | [Current preset contract](../specs/domain/built-in-presets.md) |
| `V2DOG-002` | Add the bug-fix preset | Completed | Ship a bounded full-feature `bug-fix-v2` Procedure. | [Current preset contract](../specs/domain/built-in-presets.md) |
| `V2DOG-003` | Bind and package v2 presets | Completed | Align source, embedded bytes, digest, manifest, and archive identity. | [Current preset contract](../specs/domain/built-in-presets.md) |
| `V2DOG-004` | Complete user-facing guidance | Completed | Synchronize help, completion, examples, and operator documentation. | [Current preset contract](../specs/domain/built-in-presets.md) |
| `V2DOG-005` | Dogfood the full v2 workflow | Completed | Exercise complete paths only in isolated disposable development workspaces. | [Current preset contract](../specs/domain/built-in-presets.md) |
| `V2DOG-006` | Prepare the Dolgorae handoff | Completed | Specify adapter, schema-pin, manifest, migration, and reactivation integration. | [Current preset contract](../specs/domain/built-in-presets.md) |

## V2REL — Conformance and v0.2.0 GA

Dependencies: `V2CTR`, `V2MOD`, `V2AUT`, `V2GRF`, `V2PLT`, `V2RUN`, `V2DRW`,
`V2GOL`, and `V2DOG`.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2REL-001` | Close v1/v2 compatibility | Completed | Prove v1 stability, v2 family separation, and unsupported-peer behavior. | [Release evidence](archive/v0.2.0-release-report.md) |
| `V2REL-002` | Prove all resource bounds | Completed | Close parser, collection, payload, whole-frame, escaping, truncation, and fuzz evidence. | [Release evidence](archive/v0.2.0-release-report.md) |
| `V2REL-003` | Qualify native runtime behavior | Completed | Prove real daemon, persistence, queue, concurrency, crash, endpoint, and admission behavior. | [Release evidence](archive/v0.2.0-release-report.md) |
| `V2REL-004` | Synchronize final authority and release docs | Completed | Make specifications, assets, contracts, examples, versioning, and release copy exact. | [Release evidence](archive/v0.2.0-release-report.md) |
| `V2REL-005` | Pass the development gate | Completed | Pass `make test` from the final integrated candidate. | [Release evidence](archive/v0.2.0-release-report.md) |
| `V2REL-006` | Enable and qualify the native distribution | Completed | Implement production public v2 admission, then pass `make dist` on the exact clean unpublished commit and prove packaged identity plus development-unlock exclusion. | [Release evidence](archive/v0.2.0-release-report.md) |
| `V2REL-007` | Publish and close v0.2.0 GA | Completed | Publish the unchanged qualified bytes, independently reverify immutable assets, and record the release report and archive bookkeeping. | [Final report](archive/v0.2.0-release-report.md) |

## V2CUT — Procedure v2-Only Product

Dependencies: completed `PV2GA` release program.

This epic shipped in the immutable [v0.2.1 release](archive/v0.2.1-release-report.md)
on August 15, 2026.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2CUT-001` | Adopt the v2-only authority | Completed | Supersede v1 preservation and define the breaking contract boundary. | [V2CUT dossier](archive/podway-v2-only.md) |
| `V2CUT-002` | Remove the v1 model and presets | Completed | Delete v1 parsing, linear domain behavior, conversion, and shipped presets. | [V2CUT dossier](archive/podway-v2-only.md) |
| `V2CUT-003` | Unify v2-only success contracts | Completed | Produce only the closed `podway.output/v3` success family. | [V2CUT dossier](archive/podway-v2-only.md) |
| `V2CUT-004` | Migrate to v2-only storage | Completed | Add schema-v4 and reject nonempty legacy state without mutation. | [V2CUT dossier](archive/podway-v2-only.md) |
| `V2CUT-005` | Synchronize product surfaces | Completed | Align machine assets, tests, current specifications, examples, and guidance. | [V2CUT dossier](archive/podway-v2-only.md) |
| `V2CUT-006` | Pass the development gate | Completed | Pass focused coverage and complete `make test`. | [V2CUT dossier](archive/podway-v2-only.md) |

## V2AGT — Agent Workflow Ergonomics

Dependencies: completed `V2CUT` epic.

This completed epic shipped in the immutable
[v0.2.2 release](archive/v0.2.2-release-report.md). It did not independently
authorize distribution.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2AGT-001` | Adopt the agent-loop contract | Completed | Freeze the self-contained observation, atomic recording, recovery, lightweight-preset, delivery, and hardening boundaries. | [V2AGT dossier](archive/podway-agent-workflow-ergonomics.md#accepted-design) |
| `V2AGT-002` | Add self-contained session observation | Completed | Serve one bounded running-or-terminal observation with typed active inputs and fenced mutation templates. | [Session observation](archive/podway-agent-workflow-ergonomics.md#session-observation) |
| `V2AGT-003` | Build atomic multi-item recording core | Completed | Apply a bounded current-attempt item set atomically through existing state and durable-job machinery. | [Atomic recording](archive/podway-agent-workflow-ergonomics.md#atomic-multi-item-recording) |
| `V2AGT-004` | Expose bounded multi-item recording | Completed | Add the closed JSON-stdin CLI, route, results, receipts, and agent guidance. | [Atomic recording](archive/podway-agent-workflow-ergonomics.md#atomic-multi-item-recording) |
| `V2AGT-005` | Add structured recovery recipes | Completed | Return bounded read-only remediation commands for common automation failures. | [Recovery recipes](archive/podway-agent-workflow-ergonomics.md#recovery-recipes) |
| `V2AGT-006` | Add the small-change preset | Completed | Ship and dogfood a short verified change path without goal tracking. | [Small-change preset](archive/podway-agent-workflow-ergonomics.md#small-change-preset) |
| `V2AGT-007` | Harden integrated agent workflows | Completed | Review the complete epic, fix findings, close conformance, and pass the development gate. | [Verification](archive/podway-agent-workflow-ergonomics.md#verification-and-acceptance) |

### V2AGT cold-validation record

An independent cold validation on August 15, 2026 audited committed snapshot
`c76eb13b279c8b38008e167a5bd30a59f2b26f16` and the complete V2AGT change
target `d737cc7abe117217b3edb219150739792f69267c...404dd187f6f931624c9a1b6dbf853f5b89e02504`.
The requirement-to-implementation matrix, canonical contracts, runtime wiring,
persistence and recovery boundaries, tests, documentation, generated artifacts,
and roadmap state had no confirmed gap. `gaori --json run full` completed the
`make test` development gate with exit `0`, artifact status `passed`, and zero
failures. Mulgae run `r_01a00487-123a-78eb-b278-805fa780b5cd` reviewed target
`sha256:d4c369dd419273e3a734d5728e14c99940424d38a6d80c929ebf8616489c2f52`
with complete six-role ZCode coverage, CI decision `pass`, committed publication,
a successful low-severity findings query, and zero findings. No remediation goal
or remediation commit was required.

Podway runtime evidence was explicitly waived for this validation because the
installed v0.2.1 runtime rejected the preserved workspace state with
`LEGACY_PROCEDURE_STATE_UNSUPPORTED`; the validator did not mutate that state.
This record does not claim distribution readiness, upstream publication,
installation, or runtime activation, and `make dist` was not run.

An independent cold revalidation on August 16, 2026 audited committed snapshot
`acfcf9a63f7da4bd33da9ab03a8dd76e5a82304b` and the current path-scoped V2AGT
target `sha256:fd7b293942cb2e06120c482583ee283808ab972796ab8df613ee45afd58f8c89`.
The fresh requirement-to-implementation audit found no confirmed gap. The exact
atomic-recording regression test passed, and `gaori --json run full` completed
the `make test` development gate with exit `0`, artifact status `passed`, and
zero failures. Mulgae run `r_01a0094a-171e-7846-a1ff-af149b8b2b2e` completed
all six ZCode roles with complete coverage, CI decision `pass`, committed
publication, a successful low-severity findings query, and zero findings. No
remediation goal or remediation commit was required. Podway was explicitly
excluded from this validation. This record does not claim distribution
readiness, upstream publication, installation, or runtime activation, and
`make dist` was not run.

## V2REC — Workspace Recovery Conformance

Dependencies: completed `V2AGT` epic.

This completed recovery task shipped in the immutable
[v0.2.2 release](archive/v0.2.2-release-report.md).

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2REC-001` | Repair workspace identity recovery | Completed | Prevent duplicate-root workspace identities and make confirmed reset atomically converge a proven legacy registry generation. | [V2REC dossier](archive/podway-workspace-recovery-conformance.md) |

## Procedure Evidence and Reference Quality

The next product line is implemented through five sequential epics:

```text
V2LIF -> V2SCL -> V2AST -> V2GRD -> V2REF
```

Within each epic, tasks execute in numeric order. An epic remains entirely
`Planned` until its dependency is complete. At most the first incomplete task in
the first unblocked epic may be `In Progress`, `In Review`, or `Blocked`.

Each final epic task must first promote completed design and operating knowledge
from its adopted TODO dossier into the appropriate ADRs, machine contracts,
specifications, architecture, implementation tips, examples, and roadmap
evidence. It then repairs dossier references, removes the TODO index entry, and
deletes the completed dossier before marking the epic `Completed`. Completed TODO
dossiers are not moved to `docs/roadmap/archive/`; roadmap archival is separate
maintenance performed only when historical roadmap content needs compaction.

## V2LIF — Prepared Session Lifecycle

Dependencies: completed `V2REC` epic.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2LIF-001` | Adopt the prepared session lifecycle authority | Completed | Adopt the ADR and normative lifecycle, ownership, reset, replacement, and compatibility contracts. | [ADR-0021](../architecture-decision-records/0021-separate-session-preparation-from-execution.md), [lifecycle specification](../specs/domain/rework-and-lifecycle.md) |
| `V2LIF-002` | Reserve prepared lifecycle contracts | Completed | Register prepared-aware result schemas, begin and disposition routes, reset and replacement results, SQLite v5, public errors, manifest digests, and compatibility fixtures without premature runtime admission. | [JSON contract](../specs/interfaces/json-contract.md), [SQLite model](../specs/storage/sqlite-model.md) |
| `V2LIF-003` | Persist prepared session lifecycle | Completed | Add prepared domain invariants, SQLite v5 migration and storage, terminal disposition persistence, and exact reconstruction while preserving every existing v4 session state. | [lifecycle specification](../specs/domain/rework-and-lifecycle.md), [SQLite model](../specs/storage/sqlite-model.md) |
| `V2LIF-004` | Expose smart session reset and replacement | Completed | Make start prepare, add atomic begin and terminal disposition, delete eligible sessions by default, preserve explicit summarized force deletion, and expose eligible replacement across daemon and CLI. | [CLI specification](../specs/interfaces/cli-specification.md), [automation client contract](../specs/interfaces/automation-client-contract.md) |
| `V2LIF-005` | Close prepared lifecycle conformance | Completed | Prove migration, restart, replay, stale fencing, deletion eligibility, observation, help, completion, and E2E behavior; promote durable documentation; remove the completed dossier; and pass the development gate. | [requirements traceability](../specs/quality/requirements-traceability.md), [Procedure v2 workflow](../examples/v2-workflow.md) |

## V2SCL — Bounded Evidence Scale and Read-back

Dependencies: completed `V2LIF` epic.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2SCL-001` | Adopt the bounded evidence scale authority | Planned | Adopt the ADR and normative scale, paging, compatibility, and diagnostic contracts. | [V2SCL dossier](../todo/TODO-podway-evidence-scaling.md#3-accepted-design-and-public-interfaces) |
| `V2SCL-002` | Reserve pageable evidence read contracts | Planned | Register `evidence.read`, `next-result/v3`, `observation-result/v3`, the closed `output-v3` branch, shared and record-many schema bounds, public errors, command routes, manifest digests, and compatibility fixtures without premature runtime admission. | [V2SCL dossier](../todo/TODO-podway-evidence-scaling.md#32-pageable-evidence-reads) |
| `V2SCL-003` | Align item limits and structured diagnostics | Planned | Unify authoring and runtime limits, protocol slices, total-list and attempt bounds, re-pin the rotated preset digests, and report exact exceeded fields and maxima. | [V2SCL dossier](../todo/TODO-podway-evidence-scaling.md#31-scale-envelope) |
| `V2SCL-004` | Implement snapshot-bound evidence paging | Planned | Serve deterministic bounded text and list pages under current evidence identity, freshness, page-token, metadata, and IPC constraints. | [V2SCL dossier](../todo/TODO-podway-evidence-scaling.md#32-pageable-evidence-reads) |
| `V2SCL-005` | Integrate observation budgets and close conformance | Planned | Prove the compact-status observation composition, its five component windows and truncation semantics, promote durable documentation, remove the completed dossier, and pass the development gate. | [V2SCL dossier](../todo/TODO-podway-evidence-scaling.md#6-verification-and-acceptance) |

## V2AST — External Check Result Typing

Dependencies: completed `V2SCL` epic.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2AST-001` | Adopt the external check result authority | Planned | Adopt the extending ADR and normative item, trust, storage, protocol, and compatibility contracts. | [V2AST dossier](../todo/TODO-podway-assurance-typing.md#3-accepted-design-and-public-interfaces) |
| `V2AST-002` | Reserve check-result contracts and storage | Planned | Reserve the closed schemas, observation windows, canonical SQLite v6 DDL and migration identity, manifest changes, and compatibility fixtures without runtime admission. | [V2AST dossier](../todo/TODO-podway-assurance-typing.md#35-compatibility-sensitive-contract-inventory) |
| `V2AST-003` | Add the check-result domain and authoring model | Planned | Add bounded declarations, complete values, satisfaction, parsing, diagnostics, and canonicalization while preserving existing Procedure digests. | [V2AST dossier](../todo/TODO-podway-assurance-typing.md#32-procedure-declaration) |
| `V2AST-004` | Migrate, decode, and record check results | Planned | Rebuild the constrained item table, add bounded protocol decoding and atomic frame-sized record-many support, and prove replay, restart, idempotency, and downgrade protection. | [V2AST dossier](../todo/TODO-podway-assurance-typing.md#33-recorded-value) |
| `V2AST-005` | Expose check-result guidance and read-back | Planned | Add allowed actions, suggestions, the bounded structured projection and stdin template, single-page evidence read-back, CLI guidance, and honest rendering. | [V2AST dossier](../todo/TODO-podway-assurance-typing.md#34-recording-and-observation) |
| `V2AST-006` | Close check-result compatibility and conformance | Planned | Prove compatibility, promote durable documentation, remove the completed dossier, and close trust, migration, recovery, frame, projection, and maximum-size evidence. | [V2AST dossier](../todo/TODO-podway-assurance-typing.md#6-verification-and-acceptance) |

## V2GRD — Typed Guards and Authoring Diagnostics

Dependencies: completed `V2AST` epic.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2GRD-001` | Adopt the typed predicate authority | Planned | Adopt the ADR and normative predicate, authoring, runtime, error, and compatibility contracts. | [V2GRD dossier](../todo/TODO-podway-typed-guards.md#3-accepted-design-and-public-interfaces) |
| `V2GRD-002` | Reserve typed-condition contracts | Planned | Reserve authoring and result schemas, frozen-catalog replacements, the closed error branch, protocol tables, response budgets, and compatibility fixtures without runtime admission. | [V2GRD dossier](../todo/TODO-podway-typed-guards.md#36-compatibility-sensitive-contract-inventory) |
| `V2GRD-003` | Add conditional required items | Planned | Implement bounded same-attempt required conditions with static validation, canonical and preview projection, atomic derived satisfaction, and observation status. | [V2GRD dossier](../todo/TODO-podway-typed-guards.md#32-conditional-required-items) |
| `V2GRD-004` | Add decision option guards | Planned | Implement typed guards over required selected fresh evidence with complete options, authoritative allowed IDs, and bounded three-valued statuses. | [V2GRD dossier](../todo/TODO-podway-typed-guards.md#33-decision-option-guards) |
| `V2GRD-005` | Enforce runtime condition gates | Planned | Enforce identity, freshness, and guards in order; expose structured statuses; and register non-retryable `OPTION_GUARD_UNSATISFIED`. | [V2GRD dossier](../todo/TODO-podway-typed-guards.md#33-decision-option-guards) |
| `V2GRD-006` | Repair phase-owner lint and close conformance | Planned | Remove the two count constants and legacy codes, add distinct-label and weak-criteria diagnostics, promote durable documentation, remove the completed dossier, and pass the development gate. | [V2GRD dossier](../todo/TODO-podway-typed-guards.md#34-lint-semantics) |

## V2REF — Reference Procedures and Authoring Guidance

Dependencies: completed `V2GRD` epic.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2REF-001` | Adopt the reference procedure quality contract | Planned | Promote the adopted requirement matrix, exact graphs, selection boundaries, and executable path fixtures into durable authorities. | [V2REF dossier](../todo/TODO-podway-reference-procedures.md#3-accepted-preset-designs) |
| `V2REF-002` | Rebuild sw-dev-v2 as the full reference | Planned | Add bounded planning, check results, conditions, guards, paging, artifacts, goals, and phase-owner rework; re-pin the digest and update its budget known answers. | [V2REF dossier](../todo/TODO-podway-reference-procedures.md#33-sw-dev-v2) |
| `V2REF-003` | Harden bug-fix-v2 and small-change-v2 | Planned | Add guarded fresh bug-fix evidence, preserve the assertion-only small-change boundary, re-pin both digests, and update their budget known answers. | [V2REF dossier](../todo/TODO-podway-reference-procedures.md#3-accepted-preset-designs) |
| `V2REF-004` | Add English recording and authoring examples | Planned | Document English narrative recording, the new-session default impact, and copyable page-token, check-result, condition, guard, selector, and rework patterns. | [V2REF dossier](../todo/TODO-podway-reference-procedures.md#34-english-recording-policy) |
| `V2REF-005` | Dogfood supported paths and pass the development gate | Planned | Prove every path, budget fit, guard dominance, satisfiability, and option totality; promote durable guidance; delete the completed dossier; and close bundle and development-gate evidence. | [V2REF dossier](../todo/TODO-podway-reference-procedures.md#7-verification-and-acceptance) |
