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
| `REL12001` | Freeze the v0.1.2 recovery design | Completed | Adopt the decision-complete design, authority boundaries, release constraints, and ordered implementation plan. | [Design authority](../todo/TODO-podway-v0.1.2-contract-recovery.md#status-and-authority) |
| `REL12002` | Audit the v1 compatibility boundary | Completed | Prove released-schema compatibility and record the exact pre-release consumer migration boundary. | [V1 compatibility boundary](../todo/TODO-podway-v0.1.2-contract-recovery.md#v1-compatibility-boundary) |
| `REL12003` | Repair the version identity contract | Completed | Make both binaries emit one identical schema-conformant identity and reject malformed runtime probes. | [Version identity result](../todo/TODO-podway-v0.1.2-contract-recovery.md#version-identity-result) |
| `REL12004` | Enforce authoritative packaged-schema validation | Completed | Validate complete identity envelopes using only the exact manifest-bound packaged contract set. | [Authoritative packaged schema registry](../todo/TODO-podway-v0.1.2-contract-recovery.md#authoritative-packaged-schema-registry) |
| `REL12005` | Harden qualification and release evidence | Completed | Add early singleton diagnostics and close provenance, handoff, digest, and conformance validation. | [Qualification and release evidence](../todo/TODO-podway-v0.1.2-contract-recovery.md#qualification-and-release-evidence) |
| `REL12006` | Build and qualify the native v0.1.2 distribution | Completed | Advance the version and pass every clean native arm64 and extracted-distribution release gate. | [Local gate](../todo/TODO-podway-v0.1.2-contract-recovery.md#local-gate) |
| `REL12007` | Publish and independently reverify v0.1.2 | Completed | Publish the annotated immutable release and reverify all downloaded bytes and closed identities. | [Final report](archive/v0.1.2-release-report.md) |

Tasks are completed in table order. At most the first incomplete task may be `In
Progress`, `In Review`, or `Blocked`; later tasks remain `Planned`.

## Release program PV2GA — Podway v0.2.0 Full-Feature GA

`PV2GA` is a completed release program, not an epic or task prefix. Its ten epics
delivered one stable release; no individual epic was a supported partial v2
release. The path-retained
[historical release dossier](../todo/TODO-podway-v2-full-feature-ga.md#status-and-authority)
preserves the detailed design, and the
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
| `V2CTR-001` | Promote accepted decisions into specifications | Completed | Establish the normative graph, recorded-item, compatibility, admission, and GA boundaries. | [V2CTR plan](../todo/TODO-podway-v2-full-feature-ga.md#191-epic-v2ctr-canonical-contract-baseline) |
| `V2CTR-002` | Add the Procedure v2 schema | Completed | Define the closed bounded YAML and JSON authoring contract. | [V2CTR plan](../todo/TODO-podway-v2-full-feature-ga.md#191-epic-v2ctr-canonical-contract-baseline) |
| `V2CTR-003` | Define v2 result and diagnostic schemas | Completed | Close every new or version-bumped public result family. | [V2CTR plan](../todo/TODO-podway-v2-full-feature-ga.md#191-epic-v2ctr-canonical-contract-baseline) |
| `V2CTR-004` | Register the public contract delta | Completed | Register the exact route, error, schema, and manifest surface. | [V2CTR plan](../todo/TODO-podway-v2-full-feature-ga.md#191-epic-v2ctr-canonical-contract-baseline) |
| `V2CTR-005` | Extend conformance traceability | Completed | Map every v2 requirement to a contract, test class, and task. | [V2CTR plan](../todo/TODO-podway-v2-full-feature-ga.md#191-epic-v2ctr-canonical-contract-baseline) |
| `V2CTR-006` | Build the v2 fixture corpus | Completed | Provide bounded known-answer, negative, compatibility, and maximum-size evidence. | [V2CTR plan](../todo/TODO-podway-v2-full-feature-ga.md#191-epic-v2ctr-canonical-contract-baseline) |

## V2MOD — Procedure Model and Configuration

Dependencies: `V2CTR`.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2MOD-001` | Add v2 domain values | Completed | Represent action, decision, route, rework, and goal values in core. | [V2MOD plan](../todo/TODO-podway-v2-full-feature-ga.md#192-epic-v2mod-procedure-model-and-configuration) |
| `V2MOD-002` | Enforce graph cursor invariants | Completed | Preserve exactly one authoritative cursor and active attempt. | [V2MOD plan](../todo/TODO-podway-v2-full-feature-ga.md#192-epic-v2mod-procedure-model-and-configuration) |
| `V2MOD-003` | Add workflow memory record types | Completed | Represent recorded-item references and immutable decision, rework, and goal records. | [V2MOD plan](../todo/TODO-podway-v2-full-feature-ga.md#192-epic-v2mod-procedure-model-and-configuration) |
| `V2MOD-004` | Parse v2 YAML | Completed | Dispatch and parse bounded v2 YAML without changing v1. | [V2MOD plan](../todo/TODO-podway-v2-full-feature-ga.md#192-epic-v2mod-procedure-model-and-configuration) |
| `V2MOD-005` | Parse v2 JSON | Completed | Produce semantics identical to equivalent YAML. | [V2MOD plan](../todo/TODO-podway-v2-full-feature-ga.md#192-epic-v2mod-procedure-model-and-configuration) |
| `V2MOD-006` | Validate v2 semantics | Completed | Reject invalid identities, references, routes, selectors, goal mappings, and bounds. | [V2MOD plan](../todo/TODO-podway-v2-full-feature-ga.md#192-epic-v2mod-procedure-model-and-configuration) |
| `V2MOD-007` | Canonicalize and digest v2 | Completed | Produce deterministic IR, ordering, snapshots, and digests. | [V2MOD plan](../todo/TODO-podway-v2-full-feature-ga.md#192-epic-v2mod-procedure-model-and-configuration) |
| `V2MOD-008` | Lock v1 configuration compatibility | Completed | Keep released v1 parsing and canonical identities unchanged. | [V2MOD plan](../todo/TODO-podway-v2-full-feature-ga.md#192-epic-v2mod-procedure-model-and-configuration) |

## V2AUT — Authoring Toolchain

Dependencies: `V2MOD`.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2AUT-001` | Format to stdout | Completed | Emit deterministic canonical authoring text without mutation. | [V2AUT plan](../todo/TODO-podway-v2-full-feature-ga.md#193-epic-v2aut-authoring-toolchain) |
| `V2AUT-002` | Check formatting | Completed | Detect formatting drift with stable non-writing exit behavior. | [V2AUT plan](../todo/TODO-podway-v2-full-feature-ga.md#193-epic-v2aut-authoring-toolchain) |
| `V2AUT-003` | Write formatting safely | Completed | Update only the named file while preserving supported comments. | [V2AUT plan](../todo/TODO-podway-v2-full-feature-ga.md#193-epic-v2aut-authoring-toolchain) |
| `V2AUT-004` | Lint Procedure v2 | Completed | Emit stable advisory authoring diagnostics. | [V2AUT plan](../todo/TODO-podway-v2-full-feature-ga.md#193-epic-v2aut-authoring-toolchain) |
| `V2AUT-005` | Check Procedure v2 | Completed | Aggregate validate, vet, lint, digest, and summary results. | [V2AUT plan](../todo/TODO-podway-v2-full-feature-ga.md#193-epic-v2aut-authoring-toolchain) |
| `V2AUT-006` | Scaffold Procedure v2 | Completed | Generate a minimal bounded reviewable authoring starting point. | [V2AUT plan](../todo/TODO-podway-v2-full-feature-ga.md#193-epic-v2aut-authoring-toolchain) |
| `V2AUT-007` | Convert v1 to v2 | Completed | Produce a deterministic review-required action-only v2 candidate. | [V2AUT plan](../todo/TODO-podway-v2-full-feature-ga.md#193-epic-v2aut-authoring-toolchain) |
| `V2AUT-008` | Close authoring diagnostics | Completed | Stabilize diagnostic codes, locations, ordering, bounds, and JSON. | [V2AUT plan](../todo/TODO-podway-v2-full-feature-ga.md#193-epic-v2aut-authoring-toolchain) |

## V2GRF — Graph Vetting and Projections

Dependencies: `V2MOD`.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2GRF-001` | Vet graph semantics | Completed | Prove topology, routing, dominance, evidence, skip, rework, and goal rules. | [V2GRF plan](../todo/TODO-podway-v2-full-feature-ga.md#194-epic-v2grf-graph-vetting-and-projections) |
| `V2GRF-002` | Vet liveness and budgets | Completed | Enforce static and read-back budgets without limiting valid traversal. | [V2GRF plan](../todo/TODO-podway-v2-full-feature-ga.md#194-epic-v2grf-graph-vetting-and-projections) |
| `V2GRF-003` | Project graph JSON | Completed | Emit a deterministic canonical machine projection. | [V2GRF plan](../todo/TODO-podway-v2-full-feature-ga.md#194-epic-v2grf-graph-vetting-and-projections) |
| `V2GRF-004` | Project Mermaid | Completed | Emit the required human review projection. | [V2GRF plan](../todo/TODO-podway-v2-full-feature-ga.md#194-epic-v2grf-graph-vetting-and-projections) |
| `V2GRF-005` | Project PlantUML | Completed | Emit deterministic PlantUML without invoking a renderer. | [V2GRF plan](../todo/TODO-podway-v2-full-feature-ga.md#194-epic-v2grf-graph-vetting-and-projections) |
| `V2GRF-006` | Project DOT | Completed | Emit deterministic DOT without invoking Graphviz. | [V2GRF plan](../todo/TODO-podway-v2-full-feature-ga.md#194-epic-v2grf-graph-vetting-and-projections) |
| `V2GRF-007` | Preview Procedure v2 | Completed | Present read-only checks, summary, Mermaid, digest, and confirmed start argv. | [V2GRF plan](../todo/TODO-podway-v2-full-feature-ga.md#194-epic-v2grf-graph-vetting-and-projections) |
| `V2GRF-008` | Close projection conformance | Completed | Prove all formats agree on identities and transitions, exclude evidence references as flow edges and runtime or sensitive state, and remain stable across equivalent input forms. | [V2GRF plan](../todo/TODO-podway-v2-full-feature-ga.md#194-epic-v2grf-graph-vetting-and-projections) |

## V2PLT — Persistence, Protocol, CLI, and Admission

Dependencies: `V2CTR` and `V2MOD`.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2PLT-001` | Add SQLite schema v3 | Completed | Add parallel v2 tables through an atomic v1-preserving migration. | [V2PLT plan](../todo/TODO-podway-v2-full-feature-ga.md#195-epic-v2plt-persistence-protocol-cli-and-admission) |
| `V2PLT-002` | Persist graph and action state | Completed | Persist snapshots, cursor, trace, counters, and action attempts. | [V2PLT plan](../todo/TODO-podway-v2-full-feature-ga.md#195-epic-v2plt-persistence-protocol-cli-and-admission) |
| `V2PLT-003` | Persist workflow memory | Completed | Persist items, references, decisions, rework, validity, and history. | [V2PLT plan](../todo/TODO-podway-v2-full-feature-ga.md#195-epic-v2plt-persistence-protocol-cli-and-admission) |
| `V2PLT-004` | Persist goal state | Completed | Persist goal revisions, criteria, results, and assessments. | [V2PLT plan](../todo/TODO-podway-v2-full-feature-ga.md#195-epic-v2plt-persistence-protocol-cli-and-admission) |
| `V2PLT-005` | Harden store lifecycle | Completed | Close upgrade, reopen, recovery, reset, and downgrade behavior. | [V2PLT plan](../todo/TODO-podway-v2-full-feature-ga.md#195-epic-v2plt-persistence-protocol-cli-and-admission) |
| `V2PLT-006` | Add bounded v2 protocol | Completed | Decode and serialize closed compatible bounded v2 envelopes. | [V2PLT plan](../todo/TODO-podway-v2-full-feature-ga.md#195-epic-v2plt-persistence-protocol-cli-and-admission) |
| `V2PLT-007` | Dispatch v2 daemon routes | Completed | Preserve sole-writer mutation and read-only authoring behavior. | [V2PLT plan](../todo/TODO-podway-v2-full-feature-ga.md#195-epic-v2plt-persistence-protocol-cli-and-admission) |
| `V2PLT-008` | Add v2 CLI surfaces | Completed | Provide grammar, JSON, human rendering, help, and completion. | [V2PLT plan](../todo/TODO-podway-v2-full-feature-ga.md#195-epic-v2plt-persistence-protocol-cli-and-admission) |
| `V2PLT-009` | Gate development admission | Completed | Provide development-only admission eligibility for explicitly isolated disposable state. | [V2PLT plan](../todo/TODO-podway-v2-full-feature-ga.md#195-epic-v2plt-persistence-protocol-cli-and-admission) |
| `V2PLT-010` | Close persistence and protocol failures | Completed | Prove stale, duplicate, malformed, restart, storage, peer, and downgrade behavior. | [V2PLT plan](../todo/TODO-podway-v2-full-feature-ga.md#195-epic-v2plt-persistence-protocol-cli-and-admission) |

## V2RUN — Action Runtime

Dependencies: `V2GRF` and `V2PLT`.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2RUN-001` | Start confirmed v2 sessions | Completed | Bind custom and preset starts to reviewed canonical digests. | [V2RUN plan](../todo/TODO-podway-v2-full-feature-ga.md#196-epic-v2run-action-runtime) |
| `V2RUN-002` | Serve v2 status and next | Completed | Expose deterministic bounded cursor, readiness, action, goal, and guidance views. | [V2RUN plan](../todo/TODO-podway-v2-full-feature-ga.md#196-epic-v2run-action-runtime) |
| `V2RUN-003` | Complete actions and read items back | Completed | Gate completion on recorded items and present selected prior values. | [V2RUN plan](../todo/TODO-podway-v2-full-feature-ga.md#196-epic-v2run-action-runtime) |
| `V2RUN-004` | Retry v2 actions | Completed | Create a fresh attempt while preserving immutable history. | [V2RUN plan](../todo/TODO-podway-v2-full-feature-ga.md#196-epic-v2run-action-runtime) |
| `V2RUN-005` | Skip eligible placements | Completed | Enforce declared skip policy and terminal readiness. | [V2RUN plan](../todo/TODO-podway-v2-full-feature-ga.md#196-epic-v2run-action-runtime) |
| `V2RUN-006` | Derive terminal and blocked states | Completed | Present completed, blocked, and dead-end states without ambiguity. | [V2RUN plan](../todo/TODO-podway-v2-full-feature-ga.md#196-epic-v2run-action-runtime) |
| `V2RUN-007` | Enforce runtime preconditions | Completed | Reject stale mutations and replay exact idempotent receipts. | [V2RUN plan](../todo/TODO-podway-v2-full-feature-ga.md#196-epic-v2run-action-runtime) |
| `V2RUN-008` | Close action runtime recovery | Completed | Prove concurrency, restart, durable-job, storage, and repeated-retry behavior. | [V2RUN plan](../todo/TODO-podway-v2-full-feature-ga.md#196-epic-v2run-action-runtime) |

## V2DRW — Decisions and Rework

Dependencies: `V2RUN`.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2DRW-001` | Decide the active route | Completed | Validate and atomically record one allowed decision option. | [V2DRW plan](../todo/TODO-podway-v2-full-feature-ga.md#197-epic-v2drw-decisions-and-rework) |
| `V2DRW-002` | Persist decision transitions | Completed | Preserve immutable option, route, reason, attribution, evidence snapshots, and trace records. | [V2DRW plan](../todo/TODO-podway-v2-full-feature-ga.md#197-epic-v2drw-decisions-and-rework) |
| `V2DRW-003` | Rework a valid trace target | Completed | Enforce allowed targets and completed-session reactivation policy. | [V2DRW plan](../todo/TODO-podway-v2-full-feature-ga.md#197-epic-v2drw-decisions-and-rework) |
| `V2DRW-004` | Invalidate and re-enter a suffix | Completed | Atomically stale the affected trace and activate one fresh target attempt. | [V2DRW plan](../todo/TODO-podway-v2-full-feature-ga.md#197-epic-v2drw-decisions-and-rework) |
| `V2DRW-005` | Read decisions and rework back | Completed | Expose bounded current and stale workflow history without satisfying progression. | [V2DRW plan](../todo/TODO-podway-v2-full-feature-ga.md#197-epic-v2drw-decisions-and-rework) |
| `V2DRW-006` | Close decision and rework failures | Completed | Prove invalid, stale, duplicate, crash, restart, and repeated-cycle one-writer/cursor behavior. | [V2DRW plan](../todo/TODO-podway-v2-full-feature-ga.md#197-epic-v2drw-decisions-and-rework) |

## V2GOL — Goal Tracking

Dependencies: `V2DRW`.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2GOL-001` | Define and revise session goals | Completed | Enforce opt-in, revisions, criteria, rework, stale checks, and reactivation. | [V2GOL plan](../todo/TODO-podway-v2-full-feature-ga.md#198-epic-v2gol-goal-tracking) |
| `V2GOL-002` | Assess goal criteria | Completed | Record homogeneous bounded cited criterion results atomically. | [V2GOL plan](../todo/TODO-podway-v2-full-feature-ga.md#198-epic-v2gol-goal-tracking) |
| `V2GOL-003` | Derive goal outcomes | Completed | Map assessments to outcomes and gate terminal progression on fresh results. | [V2GOL plan](../todo/TODO-podway-v2-full-feature-ga.md#198-epic-v2gol-goal-tracking) |
| `V2GOL-004` | Read goal history back | Completed | Expose bounded immutable revisions and assessments with stale state distinguished. | [V2GOL plan](../todo/TODO-podway-v2-full-feature-ga.md#198-epic-v2gol-goal-tracking) |
| `V2GOL-005` | Close goal failure and recovery | Completed | Prove mode, citation, revision, target, cancellation, restart, and budget errors. | [V2GOL plan](../todo/TODO-podway-v2-full-feature-ga.md#198-epic-v2gol-goal-tracking) |

## V2DOG — Presets and Dogfood

Dependencies: `V2AUT`, `V2GRF`, `V2RUN`, `V2DRW`, and `V2GOL`.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2DOG-001` | Add the software-development preset | Completed | Ship a bounded full-feature `sw-dev-v2` Procedure. | [V2DOG plan](../todo/TODO-podway-v2-full-feature-ga.md#199-epic-v2dog-presets-and-dogfood) |
| `V2DOG-002` | Add the bug-fix preset | Completed | Ship a bounded full-feature `bug-fix-v2` Procedure. | [V2DOG plan](../todo/TODO-podway-v2-full-feature-ga.md#199-epic-v2dog-presets-and-dogfood) |
| `V2DOG-003` | Bind and package v2 presets | Completed | Align source, embedded bytes, digest, manifest, and archive identity. | [V2DOG plan](../todo/TODO-podway-v2-full-feature-ga.md#199-epic-v2dog-presets-and-dogfood) |
| `V2DOG-004` | Complete user-facing guidance | Completed | Synchronize help, completion, examples, and operator documentation. | [V2DOG plan](../todo/TODO-podway-v2-full-feature-ga.md#199-epic-v2dog-presets-and-dogfood) |
| `V2DOG-005` | Dogfood the full v2 workflow | Completed | Exercise complete paths only in isolated disposable development workspaces. | [V2DOG plan](../todo/TODO-podway-v2-full-feature-ga.md#199-epic-v2dog-presets-and-dogfood) |
| `V2DOG-006` | Prepare the Dolgorae handoff | Completed | Specify adapter, schema-pin, manifest, migration, and reactivation integration. | [V2DOG plan](../todo/TODO-podway-v2-full-feature-ga.md#199-epic-v2dog-presets-and-dogfood) |

## V2REL — Conformance and v0.2.0 GA

Dependencies: `V2CTR`, `V2MOD`, `V2AUT`, `V2GRF`, `V2PLT`, `V2RUN`, `V2DRW`,
`V2GOL`, and `V2DOG`.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2REL-001` | Close v1/v2 compatibility | Completed | Prove v1 stability, v2 family separation, and unsupported-peer behavior. | [V2REL plan](../todo/TODO-podway-v2-full-feature-ga.md#1910-epic-v2rel-conformance-and-ga) |
| `V2REL-002` | Prove all resource bounds | Completed | Close parser, collection, payload, whole-frame, escaping, truncation, and fuzz evidence. | [V2REL plan](../todo/TODO-podway-v2-full-feature-ga.md#1910-epic-v2rel-conformance-and-ga) |
| `V2REL-003` | Qualify native runtime behavior | Completed | Prove real daemon, persistence, queue, concurrency, crash, endpoint, and admission behavior. | [V2REL plan](../todo/TODO-podway-v2-full-feature-ga.md#1910-epic-v2rel-conformance-and-ga) |
| `V2REL-004` | Synchronize final authority and release docs | Completed | Make specifications, assets, contracts, examples, versioning, and release copy exact. | [V2REL plan](../todo/TODO-podway-v2-full-feature-ga.md#1910-epic-v2rel-conformance-and-ga) |
| `V2REL-005` | Pass the development gate | Completed | Pass `make test` from the final integrated candidate. | [V2REL plan](../todo/TODO-podway-v2-full-feature-ga.md#1910-epic-v2rel-conformance-and-ga) |
| `V2REL-006` | Enable and qualify the native distribution | Completed | Implement production public v2 admission, then pass `make dist` on the exact clean unpublished commit and prove packaged identity plus development-unlock exclusion. | [V2REL plan](../todo/TODO-podway-v2-full-feature-ga.md#1910-epic-v2rel-conformance-and-ga) |
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

This epic is post-v0.2.1 unreleased work. It does not advance the product
version or authorize distribution.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2AGT-001` | Adopt the agent-loop contract | Completed | Freeze the self-contained observation, atomic recording, recovery, lightweight-preset, delivery, and hardening boundaries. | [V2AGT dossier](../todo/TODO-podway-agent-workflow-ergonomics.md#accepted-design) |
| `V2AGT-002` | Add self-contained session observation | Completed | Serve one bounded running-or-terminal observation with typed active inputs and fenced mutation templates. | [Session observation](../todo/TODO-podway-agent-workflow-ergonomics.md#session-observation) |
| `V2AGT-003` | Build atomic multi-item recording core | Planned | Apply a bounded current-attempt item set atomically through existing state and durable-job machinery. | [Atomic recording](../todo/TODO-podway-agent-workflow-ergonomics.md#atomic-multi-item-recording) |
| `V2AGT-004` | Expose bounded multi-item recording | Planned | Add the closed JSON-stdin CLI, route, results, receipts, and agent guidance. | [Atomic recording](../todo/TODO-podway-agent-workflow-ergonomics.md#atomic-multi-item-recording) |
| `V2AGT-005` | Add structured recovery recipes | Planned | Return bounded read-only remediation commands for common automation failures. | [Recovery recipes](../todo/TODO-podway-agent-workflow-ergonomics.md#recovery-recipes) |
| `V2AGT-006` | Add the small-change preset | Planned | Ship and dogfood a short verified change path without goal tracking. | [Small-change preset](../todo/TODO-podway-agent-workflow-ergonomics.md#small-change-preset) |
| `V2AGT-007` | Harden integrated agent workflows | Planned | Review the complete epic, fix findings, close conformance, and pass the development gate. | [Verification](../todo/TODO-podway-agent-workflow-ergonomics.md#verification-and-acceptance) |
