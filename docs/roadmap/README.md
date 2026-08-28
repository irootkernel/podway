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

The next product line is implemented through eight sequential epics:

```text
V2LIF -> V2SCL -> V2AST -> V2WAL -> V2ARC -> V2GRD -> V2REF -> V2SKL
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
| `V2LIF-006` | Version lifecycle-aware durable job wrappers | Completed | Preserve the released v3 job-wrapper command set and emit new closed v4 wrappers for prepared-lifecycle commands. | [JSON contract](../specs/interfaces/json-contract.md) |
| `V2LIF-007` | Restore complete durable job read-back | Completed | Keep workspace initialization and lifecycle job receipts readable individually and as one ordered bounded job list. | [automation client contract](../specs/interfaces/automation-client-contract.md) |
| `V2LIF-008` | Enforce prepared item mutation failures | Completed | Reject every prepared item mutation with `SESSION_NOT_RUNNING` before attempt or item fences and without state change. | [automation client contract](../specs/interfaces/automation-client-contract.md) |

## V2SCL — Bounded Evidence Scale and Read-back

Dependencies: completed `V2LIF` epic.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2SCL-001` | Adopt the bounded evidence scale authority | Completed | Adopt the ADR and normative scale, paging, compatibility, and diagnostic contracts. | [ADR-0024](../architecture-decision-records/0024-bounded-evidence-scale-and-paged-read-back.md), [item bounds](../specs/domain/procedure-and-item-specification.md#item-bounds) |
| `V2SCL-002` | Reserve pageable evidence read contracts | Completed | Register `evidence.read`, `next-result/v3`, `observation-result/v3`, the closed `output-v3` branch, shared and record-many schema bounds, public errors, command routes, manifest digests, and compatibility fixtures without premature runtime admission. | [IPC evidence paging](../specs/interfaces/ipc-protocol.md#evidence-paging), [evidence scale failures](../specs/interfaces/errors-and-exit-codes.md#bounded-evidence-scale-failures) |
| `V2SCL-003` | Align item limits and structured diagnostics | Completed | Unify authoring and runtime limits, protocol slices, total-list and attempt bounds, re-pin the rotated preset digests, and report exact exceeded fields and maxima. | [item bounds](../specs/domain/procedure-and-item-specification.md#item-bounds), [ADR-0024](../architecture-decision-records/0024-bounded-evidence-scale-and-paged-read-back.md) |
| `V2SCL-004` | Implement snapshot-bound evidence paging | Completed | Serve deterministic bounded text and list pages under current evidence identity, freshness, page-token, metadata, and IPC constraints. | [evidence paging](../specs/interfaces/ipc-protocol.md#evidence-paging), [evidence reads](../specs/interfaces/cli-specification.md#evidence-reads) |
| `V2SCL-005` | Integrate observation budgets and close conformance | Completed | Derive the six `session.next` allocations and the five observation windows from the published composition, bound every window by bytes with an exact total and a truthful truncation flag, close the compact-status envelope guard and the stale read-back projection, promote durable documentation, remove the completed dossier, and pass the development gate. | [response budgets](../specs/interfaces/ipc-protocol.md#response-budgets), [ADR-0024](../architecture-decision-records/0024-bounded-evidence-scale-and-paged-read-back.md) |

## V2AST — External Check Result Typing

Dependencies: completed `V2SCL` epic.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2AST-001` | Adopt the external check result authority | Completed | Adopt the extending ADR and normative item, trust, storage, protocol, and compatibility contracts. | [ADR-0025](../architecture-decision-records/0025-structurally-bound-external-check-results.md), [item specification](../specs/domain/procedure-and-item-specification.md#external-check-results) |
| `V2AST-002` | Reserve check-result contracts and storage | Completed | Reserve the closed schemas, observation windows, canonical SQLite v6 DDL and migration identity, manifest changes, and compatibility fixtures without runtime admission. | [automation contract](../specs/interfaces/automation-client-contract.md#structurally-bound-external-check-results-aut-chk-001008), [SQLite model](../specs/storage/sqlite-model.md) |
| `V2AST-003` | Add the check-result domain and authoring model | Completed | Add bounded declarations, complete values, satisfaction, parsing, diagnostics, and canonicalization while preserving existing Procedure digests. | [item specification](../specs/domain/procedure-and-item-specification.md#external-check-results) |
| `V2AST-004` | Migrate, decode, and record check results | Completed | Rebuild the constrained item table, add bounded protocol decoding and atomic frame-sized record-many support, and prove replay, restart, idempotency, and downgrade protection. | [automation contract](../specs/interfaces/automation-client-contract.md#structurally-bound-external-check-results-aut-chk-001008), [SQLite model](../specs/storage/sqlite-model.md) |
| `V2AST-005` | Expose check-result guidance and read-back | Completed | Add allowed actions, suggestions, the bounded structured projection and stdin template, single-page evidence read-back, CLI guidance, and honest rendering. | [automation contract](../specs/interfaces/automation-client-contract.md#structurally-bound-external-check-results-aut-chk-001008), [IPC protocol](../specs/interfaces/ipc-protocol.md#evidence-paging) |
| `V2AST-006` | Close check-result compatibility and conformance | Completed | Prove compatibility, promote durable documentation, remove the completed dossier, and close trust, migration, recovery, frame, projection, and maximum-size evidence. | [ADR-0025](../architecture-decision-records/0025-structurally-bound-external-check-results.md), [security and trust](../specs/operations/security-and-trust.md), [automation contract](../specs/interfaces/automation-client-contract.md#structurally-bound-external-check-results-aut-chk-001008) |

## V2WAL — Runtime State Snapshot Coherence

Dependencies: completed `V2AST` epic.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2WAL-001` | Restore active Store snapshot coherence | Completed | Keep previews on the active Store, detect divergence from disposable snapshots, preserve existing migrations, and prove graceful WAL recovery without discarding current state. | [read consistency](../specs/storage/transactions-concurrency-and-idempotency.md#read-consistency), [doctor checks](../specs/storage/recovery-retention-and-maintenance.md#doctor-checks) |

## V2ARC — Retained Inactive Sessions

Dependencies: completed `V2WAL` epic.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2ARC-001` | Retain disposed terminal sessions | Completed | Add bounded immutable inactive-session retention, atomic terminal replacement, explicit archive reads and purge, SQLite v7 migration, public contracts, operator guidance, and development-gate coverage. | [ADR-0026](../architecture-decision-records/0026-retain-inactive-terminal-sessions.md), [lifecycle specification](../specs/domain/rework-and-lifecycle.md) |
| `V2ARC-002` | Make session start state-aware | Completed | Replace public replacement flags with lifecycle-specific human choices and explicit automation policies; preserve superseded work atomically and expose its successor link. | [ADR-0027](../architecture-decision-records/0027-state-aware-session-start.md), [CLI specification](../specs/interfaces/cli-specification.md) |

## V2GRD — Typed Guards and Authoring Diagnostics

Dependencies: completed `V2ARC` epic.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2GRD-001` | Adopt the typed predicate authority | Completed | Adopt the ADR and normative predicate, authoring, runtime, error, and compatibility contracts. | [ADR-0028](../architecture-decision-records/0028-bounded-typed-procedure-guards.md), [Procedure specification](../specs/domain/procedure-and-item-specification.md#typed-predicates-and-conditional-requirements) |
| `V2GRD-002` | Reserve typed-condition contracts | Completed | Reserve authoring and result schemas, frozen-catalog replacements, the closed error branch, protocol tables, response budgets, and compatibility fixtures without runtime admission. | [Automation contract](../specs/interfaces/automation-client-contract.md#typed-conditions-and-option-guards-aut-grd-001010), [JSON contract](../specs/interfaces/json-contract.md) |
| `V2GRD-003` | Add conditional required items | Completed | Implement bounded same-attempt required conditions with static validation, canonical and preview projection, atomic derived satisfaction, and observation status. | [Procedure specification](../specs/domain/procedure-and-item-specification.md#typed-predicates-and-conditional-requirements), [State transitions](../specs/domain/state-transitions.md) |
| `V2GRD-004` | Add decision option guards | Completed | Implement typed guards over required selected fresh evidence with complete options, authoritative allowed IDs, and bounded three-valued statuses. | [Procedure specification](../specs/domain/procedure-and-item-specification.md#typed-predicates-and-conditional-requirements), [Automation contract](../specs/interfaces/automation-client-contract.md#typed-conditions-and-option-guards-aut-grd-001010) |
| `V2GRD-005` | Enforce runtime condition gates | Completed | Enforce identity, freshness, and guards in order; expose structured statuses; and register non-retryable `OPTION_GUARD_UNSATISFIED`. | [State transitions](../specs/domain/state-transitions.md), [Errors and exit codes](../specs/interfaces/errors-and-exit-codes.md#typed-condition-failures) |
| `V2GRD-006` | Repair phase-owner lint and close conformance | Completed | Remove the two count constants and legacy codes, add distinct-label and weak-criteria diagnostics, promote durable documentation, remove the completed dossier, and pass the development gate. | [ADR-0028](../architecture-decision-records/0028-bounded-typed-procedure-guards.md), [Errors and exit codes](../specs/interfaces/errors-and-exit-codes.md#typed-condition-failures), [workflow example](../examples/v2-workflow.md#author-typed-conditions) |

### V2GRD cold-validation record

An independent cold validation on August 22, 2026 audited the complete V2GRD
implementation and its hardening target from base
`f8ceefc069793e781ce0fc558a4e515eb9126640` through committed snapshot
`5fd0206f85339c2afbe2987275a8d24ba932f832` plus the goal-owned working-tree
changes. It corrected the workflow example's evidence selector, promoted the
typed-condition compatibility fixture to production automation, and added
daemon/SQLite cold-restart and runtime failure-precedence coverage. The two
focused production tests passed, and `gaori --json run full` completed the
`make test` development gate with exit `0`, artifact status `passed`, and zero
failures.

Mulgae run `r_01a02855-2db5-7e3b-966f-c976512fbdb7` reviewed target
`sha256:4272c04e9f17c01ffcd89194f9774965b32e52c13193827698168f32c9f38de0`
with complete six-role coverage, CI decision `pass`, committed publication, a
successful low-severity findings query, and zero findings. This validation does
not claim distribution readiness, upstream publication, installation, or
runtime activation, and `make dist` was not run.

## V2REF — Reference Procedures and Authoring Guidance

Dependencies: completed `V2GRD` epic.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2REF-001` | Adopt the reference procedure quality contract | Completed | Promoted the requirement matrix, exact graphs, selection boundaries, compatibility and trust boundaries, and executable path inventory into durable authorities. | [Built-in presets](../specs/domain/built-in-presets.md), [testing and conformance](../specs/quality/testing-and-conformance.md) |
| `V2REF-002` | Rebuild sw-dev-v2 as the full reference | Completed | Added bounded planning, check results, conditions, guards, paging, artifacts, goals, and phase-owner rework; re-pinned the digest and updated its budget known answers. | [Built-in presets](../specs/domain/built-in-presets.md#sw-dev-v2) |
| `V2REF-003` | Harden bug-fix-v2 and small-change-v2 | Completed | Added guarded fresh bug-fix evidence, preserved the assertion-only small-change boundary, re-pinned both digests, and updated their budget known answers. | [Built-in presets](../specs/domain/built-in-presets.md) |
| `V2REF-004` | Add English recording and authoring examples | Completed | Documented English narrative recording, the new-session default impact, and copyable page-token, check-result, condition, guard, selector, and rework patterns. | [Procedure v2 workflow](../examples/v2-workflow.md), [user workflows](../specs/product/user-workflows.md) |
| `V2REF-005` | Dogfood supported paths and pass the development gate | Completed | Bound all 31 accepted paths to runtime proofs; added shipped bug-fix and isolated software-reference dogfood for guarded, conditional, phase-owner, paging, goal, restart, snapshot, and manual-rework paths; closed the complete development gate. | [Testing and conformance](../specs/quality/testing-and-conformance.md#shipped-reference-path-evidence) |

### V2REF cold-validation record

An independent cold validation on August 23, 2026 audited the complete V2REF
implementation from base `caf621190876e43693fdfd6c64f2d14d6cac8923` through
committed snapshot `6956aa05a045e9e828379cf6ba68ee44c2d4c64a` plus this
goal-owned roadmap record. All five task rows are completed, the adopted dossier
and its stale references are absent, and the 31-case path inventory is bound to
checked runtime proof symbols. Static architecture and contract checks passed at
the committed snapshot. Gaori full run `20260823T005743` completed the `make test`
development gate with exit `0`, artifact status `passed`, and zero failures.

Mulgae run `r_01a02c41-fd35-7200-b5ef-0bea2d7433db` reviewed the complete epic
target `sha256:4aa9479557813d7166daeaa40f9827ee4f2e12e428c509181447c80a7166efd8`
with all six roles, complete coverage, CI decision `pass`, committed publication,
a successful low-severity findings query, and zero findings. This validation does
not claim distribution readiness, upstream publication, installation, or runtime
activation, and `make dist` was not run.

### V2REF revalidation record — August 24, 2026

A current cold revalidation audited the complete V2REF implementation from base
`caf621190876e43693fdfd6c64f2d14d6cac8923` through committed snapshot
`0f253435ef40296054cf07eddfc079fdf4c7f091` plus this goal-owned roadmap
record. The shipped inventory is four presets and 45 accepted paths: 14
`analysis-v2`, 10 `bug-fix-v2`, 5 `small-change-v2`, and 16 `sw-dev-v2`
cases. Every locator is bound to a checked runtime proof symbol, and the
historical 31-case record above remains the accurate evidence for its August 23
snapshot.

Documentation, contract, layout, formatting, and focused runtime checks passed.
Gaori full run `20260824T101739` completed the development gate with status
`passed`, exit `0`, and zero failures. Subsequent focused CLI run
`20260824T110023` and prepare run `20260824T110538` passed after the
documentation and completion-regression corrections; product and runtime code
were unchanged. Mulgae whole-epic run
`r_01a03378-8d0c-7240-bfea-0e442c2f9d94` completed all six roles with complete
coverage, CI decision `pass`, and committed publication. Its only confirmed live
gap was the absence of this current 45-case roadmap supplement; this record
resolves that gap, and no other valid in-scope finding remained after prose
adjudication.

V2REF therefore passes current development and contract validation, but the
repository is not distribution-release-ready. The immutable `v0.2.5` tag points
to `bbc597a66bd276009b10af9b44fbd967da1c095c`, while current source and release
metadata still identify an unpublished `0.2.5` candidate. Version and release
metadata reconciliation, `make dist`, tagging, pushing, publication,
installation, and runtime activation remain release-owner work and were not
performed by this validation.

## V2SKL — Procedure Authoring Skill Boundary

Dependencies: completed `V2REF` epic.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2SKL-001` | Add a dedicated Procedure authoring skill | Completed | Provide focused guidance for creating, revising, reviewing, statically qualifying, and handing off repository-owned custom Procedure v2 source without runtime mutation. | [Authoring skill](../../skills/create-podway-procedure/SKILL.md) |
| `V2SKL-002` | Separate runtime and authoring ownership | Completed | Keep `use-podway` responsible for explicit runtime and lifecycle work while routing Procedure source changes to the authoring skill. | [Runtime skill](../../skills/use-podway/SKILL.md), [repository structure](../architecture/repository-structure.md) |
| `V2SKL-003` | Verify the source-distributed skill package | Completed | Bind the exact skill file sets, metadata, links, command workflow, and documentation to repository checks. | [Documentation verifier](../../tools/verify_docs.py) |

## V2RDY — Phase-Aware Daemon Readiness

Dependencies: completed `V2SKL` epic.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2RDY-001` | Adopt and reserve daemon readiness contracts | Completed | Adopt the early-control-plane decision and reserve status v2, wait-ready, phase-aware errors, recovery recipes, routes, and manifest pins without runtime admission. | [ADR-0029](../architecture-decision-records/0029-serve-phase-aware-daemon-readiness.md), [IPC contract](../specs/interfaces/ipc-protocol.md) |
| `V2RDY-002` | Serve verified status during recovery | Completed | Start the authenticated control plane before registry/worktree/job recovery, expose monotonic readiness progress, and reject normal routes before ready. | [Observability contract](../specs/operations/observability.md) |
| `V2RDY-003` | Add bounded readiness waiting and close conformance | Completed | Add daemon wait-ready, reuse it for install, render non-ready status honestly, return actionable timeouts, prove retry preservation, promote durable guidance, and pass the development gate. | [CLI contract](../specs/interfaces/cli-specification.md), [automation contract](../specs/interfaces/automation-client-contract.md) |

## V2RET — Workspace Retirement and Registry Hygiene

Dependencies: completed `V2RDY` epic.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2RET-001` | Adopt workspace retirement authority | Completed | Adopt exact missing-root pruning, explicitly confirmed full `.podway` removal, recovery, compatibility, and safety boundaries. | [ADR-0030](../architecture-decision-records/0030-retire-workspaces-and-prune-stale-registry-entries.md), [V2RET dossier](../todo/TODO-podway-workspace-retirement-and-registry-hygiene.md) |
| `V2RET-002` | Reserve workspace removal contracts | Completed | Register the command, result, errors, removal marker, replay semantics, observability, manifest identity, and compatibility fixtures without premature runtime admission. | [V2RET dossier](../todo/TODO-podway-workspace-retirement-and-registry-hygiene.md#3-accepted-interfaces) |
| `V2RET-003` | Prune stale registry entries | Completed | Remove only conclusively missing exact UUID/root generations during startup and pre-mutation revalidation, then integrate successful cleanup with readiness and bounded observability. | [Recovery specification](../specs/storage/recovery-retention-and-maintenance.md#moved-worktrees); [worktree deletion](../architecture/git-worktree-and-filesystem.md#worktree-deletion) |
| `V2RET-004` | Remove a selected Podway workspace | Completed | Add exact-path confirmation, maintenance fencing, marker-backed recovery, empty-residual replay convergence, bounded observability, and complete `.podway` removal while preserving the Git worktree. | [V2RET dossier](../todo/TODO-podway-workspace-retirement-and-registry-hygiene.md#32-explicit-workspace-removal) |
| `V2RET-005` | Close retirement and cleanup conformance | Planned | Prove races, crashes, replay, compatibility, path safety, reinitialization, and E2E behavior; promote durable guidance; remove the dossier; and pass the development gate. | [Testing and conformance](../specs/quality/testing-and-conformance.md) |
