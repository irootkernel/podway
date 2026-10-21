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

New epic IDs use exactly five uppercase ASCII letters (`[A-Z]{5}`), without
a version prefix. Task IDs use that epic ID, a hyphen, and a three-digit
sequence, for example `OPTIM-001`. Existing alphanumeric epic IDs, including
`V2CTR` and `REL12`, and the compact `REL12` task IDs remain unchanged historical
identifiers. Do not rename completed work to apply the new convention.

## Structural simplification program

On 2026-10-19 the owner adopted four sequential design dossiers following the
accepted OPTIM, BOUND and READS work:

```text
VIEWS -> PARSE -> STATE -> ADMIT
```

The goal is to remove structural, logical and conceptual duplication. The
2026-10-19 registration was design-only; execution began with `VIEWS-001` on
2026-10-20, and the task tables below alone own current status. Only the first
incomplete task of the first eligible epic may start; accept each epic before
starting its successor. There is no cross-epic implementation interleaving.

Elapsed-time tests, latency percentiles including p95/p99, throughput benchmarks,
performance baselines and performance thresholds are excluded from this program.
Acceptance requires preserved behavior and evidence that the targeted duplicate
work has been removed. Historical qualification is neither a new requirement nor
a baseline for this program. No release, installation, commit, publication or
external-consumer change is selected by this design registration.

### Shared preservation boundaries

- Preserve CLI grammar, JSON fields and schemas, error codes/details/exit and
  retryability, canonical bytes/digests, Store layout and writer identity. No
  migration, reset, table, index, persistent cache or new runtime is included.
- Keep eager persisted-value validation, including hidden historical values;
  archive-list remains summary-only for archived rows, with its existing current
  session admission unchanged. Share admitted values within one coherent read
  snapshot, or through ADMIT's identity-checked invocation context, without
  replacing independent trust-boundary checks.
- Preserve error precedence, fences, clocks/IDs, exact stored response replay,
  atomic success-only outcomes, bounded output and read-only file/SHM/WAL checks.
  Retain full Standard preflight projection/schema/envelope-equivalent failures.
- Reuse facts only while their Store, root, writer, session and snapshot identity
  remains valid. Release read guards before mutation admission; keep the real
  competing-writer gap and mutation artifact preparation outside writer
  transactions and read leases. Preserve deep doctor's existing exclusive
  maintenance scope, including its artifact rehash; do not extend that exception
  to mutations. Preserve deadlines as behavior without measuring performance.
- Keep separate start/reset observations and existing user interaction semantics.
  [CALLS](../todo/TODO-podway-cli-interaction-simplification.md) is an unadopted
  candidate, with no roadmap tasks or implementation commitment.

Each task includes focused correctness verification. Future executable changes
require `make test` before sharing a development revision. Documentation-only
changes use documentation checks. See
[structural simplification verification](../implementation-tips/testing.md#structural-simplification-verification).
Each epic's first task updates its adopted dossier with the remove/retain ledger,
eligible and deliberately split paths, and mapped correctness checks defined
there. This is the first task's checkable deliverable, not another status source.
Final tasks integrate correctness evidence, review and durable documentation;
they do not postpone all verification until closeout. Retire each adopted dossier
only through the [TODO lifecycle](../todo/README.md#lifecycle).

## VIEWS — Projection Simplification

Dependencies: accepted `READS` completion. Implemented composition:
[Request-scoped read composition](../architecture/system.md#request-scoped-read-composition).

| id | title | status | goal | references |
|---|---|---|---|---|
| `VIEWS-001` | Pin projection behavior and validation boundaries | Completed | Map complete outputs, byte/scalar limits, hidden-value validation and failure order before changing projection work. | [Projection ownership](../architecture/system.md#request-scoped-read-composition) |
| `VIEWS-002` | Account for serialized prefixes incrementally | Completed | Replace growing-prefix reserialization with exact incremental byte accounting, retaining wrapper, trace and truncation semantics. | [Incremental accounting](../architecture/system.md#request-scoped-read-composition) |
| `VIEWS-003` | Construct only admitted display content | Completed | Avoid full scalar buffers and undisplayed formatting while retaining logical totals, first-three valued-slot semantics and eager validation. | [Bounded construction](../architecture/system.md#request-scoped-read-composition) |
| `VIEWS-004` | Share evidence selection and observation projections | Completed | Reuse evidence selection across readback and delegation packages; omit observation previews while preserving their failure checks. | [Projection reuse](../architecture/system.md#request-scoped-read-composition) |
| `VIEWS-005` | Qualify projection equivalence and close documentation | Completed | Verify bounds, errors, output equivalence and corruption refusal, complete the development gate and review, then promote lasting documentation. | [Regression owners](../implementation-tips/testing.md#functional-regression-owners) |

### VIEWS delivery record

VIEWS-001 through VIEWS-005 are Completed at `3d32cad`, `6dcefee`, `d06021b`,
`3cb19dc` and `3f4319f` under delegated Task acceptance: pinned projection
boundaries, exact incremental serialized-prefix accounting, bounded display
construction, shared evidence selection with preview-free observation, and
lasting ownership and verification guidance.
Behavioral ownership is registered under
[functional regression owners](../implementation-tips/testing.md#functional-regression-owners)
and projection composition under
[request-scoped read composition](../architecture/system.md#request-scoped-read-composition).
Fresh documentation checks cover VIEWS-005; the full `make test` result is
reused for unchanged executable, test and contract inputs.
Master accepted the assembled Epic after final integration review. Its dossier
is retired under the [TODO lifecycle](../todo/README.md#lifecycle).

## PARSE — Document Admission Reuse

Dependencies: accepted `VIEWS` completion. Implemented ownership:
[Document admission reuse](../architecture/system.md#document-admission-reuse).

| id | title | status | goal | references |
|---|---|---|---|---|
| `PARSE-001` | Pin parsing and validation responsibilities | Completed | Map source, Store and Protocol trust boundaries, canonical-byte rules and failure precedence. | [Admission ownership](../architecture/system.md#document-admission-reuse) |
| `PARSE-002` | Reuse one parsed Store outcome document | Completed | Share one parsed value through outcome admission without dropping canonical, depth, schema or relational checks or rebuilding replay bytes. | [Store outcome admission](../architecture/system.md#document-admission-reuse) |
| `PARSE-003` | Remove unnecessary response ownership conversions | Completed | Borrow or move validated response values through construction and validation while retaining independent checks and output bounds. | [Response ownership](../architecture/system.md#document-admission-reuse) |
| `PARSE-004` | Carry first preview parse facts | Completed | Select delegated preview generation from first-parse facts, preserving malformed and parsed-but-invalid behavior; adopt DF-006. | [Preview facts](../architecture/system.md#document-admission-reuse) |
| `PARSE-005` | Qualify admission and exact replay | Completed | Verify error order, canonical bytes, preview variants and original-response replay, complete the development gate and review, then promote documentation. | [Regression owners](../implementation-tips/testing.md#functional-regression-owners) |

PARSE is owner-accepted. Store outcome admission reuses one parsed document
without rebuilding replay bytes; Protocol moves or borrows response values, and
Config carries first-parse authoring facts. Independent validation and public
contracts remain preserved. Its dossier is retired under the
[TODO lifecycle](../todo/README.md#lifecycle).

## STATE — Snapshot State Reuse

Dependencies: accepted `PARSE` completion. Implemented ownership:
[Staged snapshot state reuse](../architecture/system.md#staged-snapshot-state-reuse).

| id | title | status | goal | references |
|---|---|---|---|---|
| `STATE-001` | Pin reconstruction and validation ownership | Completed | Map same-snapshot graph, disposition, attempt and receipt stages with their existing failure order. | [Ownership and failure order](../architecture/system.md#staged-snapshot-state-reuse) |
| `STATE-002` | Reuse same-snapshot graph and dispositions | Completed | Carry admitted state through view, reset and doctor paths without advancing receipt/disposition checks across existing fences or clocks. | [Staged snapshot reuse](../architecture/system.md#staged-snapshot-state-reuse) |
| `STATE-003` | Index borrowed cached attempts once | Completed | Share one reconstruction-local attempt index across owner discovery and reconstruction while preserving validation order and changed-row bounds. | [Borrowed attempt index](../architecture/system.md#staged-snapshot-state-reuse) |
| `STATE-004` | Reuse admitted active receipt for readiness | Completed | Derive readiness from an already admitted matching receipt, retaining history validation and fallback acquisition when coverage is unproven. | [Receipt readiness](../architecture/system.md#staged-snapshot-state-reuse) |
| `STATE-005` | Qualify snapshot continuity and corruption refusal | Completed | Verify corruption priority, supported Store continuity and replay, complete the development gate and review, then promote documentation. | [Regression owners](../implementation-tips/testing.md#functional-regression-owners) |

STATE is owner-accepted. Same-snapshot reads stage one admitted graph with its
validated dispositions; reconstruction shares one borrowed id-keyed attempt
index, and views derive receipt readiness from the admitted window while the
write path keeps point-query acquisition. Validation, failure order, public
contracts and Store continuity remain preserved. Its dossier is retired under
the [TODO lifecycle](../todo/README.md#lifecycle).

## ADMIT — Request Admission Reuse

Dependencies: accepted `STATE` completion. Design:
[Request admission reuse](../todo/TODO-podway-request-admission-reuse.md).

| id | title | status | goal | references |
|---|---|---|---|---|
| `ADMIT-001` | Pin admission order and lease boundaries | Completed | Map binding, native identity, snapshot, preflight, artifact and writer stages per command before combining any work. | [Design and interfaces](../todo/TODO-podway-request-admission-reuse.md#accepted-design-decisions-and-interfaces) |
| `ADMIT-002` | Combine binding admission and first snapshot | Planned | Share eligible first-operation acquisition while retaining native identity/error order and releasing leases before artifact work. | [Binding and first snapshot](../todo/TODO-podway-request-admission-reuse.md#binding-and-first-snapshot) |
| `ADMIT-003` | Reuse preflight-admitted graph in preparation | Planned | Carry Store-prepared state across distinct Engine requests in one opaque invocation context; check it at preparation and preserve existing reload/refusal windows, preflight failures and guard release. | [Preflight and mutation preparation](../todo/TODO-podway-request-admission-reuse.md#preflight-and-mutation-preparation) |
| `ADMIT-004` | Qualify admission races and exact replay | Planned | Verify competing writers, file replacement, unsupported-state refusal and replay, complete the development gate and review, then promote documentation and retire program-planning text. | [Verification](../todo/TODO-podway-request-admission-reuse.md#verification-and-release-acceptance) |

## Current internal improvements

On 2026-10-14 the owner adopted three internal-improvement epics, based on the
v0.2.14 review at `fa29dba929777b020377bc9803cb0a3954d219b1`:

```text
OPTIM -> BOUND -> READS
```

These are three separately accepted development epics with one shared goal and
a fixed order, not a release program and not parallel work. OPTIM is Completed.
BOUND and READS are Completed and owner-accepted. Master separately authorized local
BOUND Task commits and delegated Task acceptance to Main for Epic Harmony.
No release version, installation, remote push or external-consumer change is authorized here.
V2DFC remains completed; its historical qualification is not a fresh measurement
for these improvements.

Only the first incomplete task of the first eligible epic may be In Progress,
In Review or Blocked. All later tasks remain Planned. Finish and accept each
epic before starting its successor; there is no cross-epic implementation
interleaving. Execution started at `OPTIM-001`, and that epic is accepted. The
2026-10-14 plan review is incorporated into the target inventories. On
2026-10-16 the owner withdrew OPTIM performance measurement; that withdrawal
revises OPTIM-001. Measurement requirements were withdrawn; completion is
recorded below.

Each task includes its own focused verification. An executable change must pass
`make test` under [AGENTS.md](../../AGENTS.md); a documentation-only task uses the
applicable documentation checks. Final tasks integrate the epic-wide checks,
independent review and acceptance, not defer all testing until the end. The
roadmap alone owns status and order. Completed OPTIM behavior is recorded in
architecture, testing guidance, and this roadmap, as is completed BOUND behavior.
READS behavior and correctness boundaries are recorded in the
[system architecture](../architecture/system.md#request-scoped-read-composition)
and [functional closeout guidance](../implementation-tips/direct-cli-performance-qualification.md#reads-005-functional-closeout).
The three adopted dossiers are retired. Performance measurement is excluded
from OPTIM and READS acceptance; BOUND qualification remains historical evidence.
A named test file or ordinary test gate does not prove an ignored scenario ran.

### Preservation rules

- Keep current CLI grammar, JSON schemas, error codes/details/retryability/exit
  semantics, canonical Procedure bytes/digests, stored request/outcome identity,
  and supported current-Store continuity. No reset, migration, new index/table,
  public canonical-asset or writer-manifest-input change is part of this scope.
  Verification-only fixtures and test registration may change without rotating
  public/storage contracts. Preserve legacy/newer/foreign-writer refusal.
- Preserve normal cold-read decoding for the existing graph-reading paths,
  including values outside status/history/archive-show windows. Keep archive-list
  summary-only and ordinary-open checks unchanged; do not move doctor scans onto
  those paths. Purpose-specific views may reduce queries/copies/rendering but
  cannot weaken required validation. Typed fence preflight must also preserve
  failures from its former status projection/schema/envelope validation. Reduced
  loading is optional where an equivalent guarantee can be proven.
- Preserve command-specific fences and validation order, atomic success-only
  outcomes, exact replay, clock/ID ordering, original wait deadlines, coherent
  write-free snapshots and maintenance/file-identity checks. Artifact bytes stay
  outside writer transactions. Single-item and batch fence policies remain
  distinct. No performance result can compensate for a correctness regression.
- Stay within the existing eight-crate dependency direction and daemon-free,
  single-cursor product. No generic framework, persistent cache or new runtime
  service is implied. Use request-scoped sharing and existing test infrastructure.
- OPTIM acceptance is behavioral and structural. Established release
  qualification remains the historical G7/G8 protocol in the
  [performance qualification guide](../implementation-tips/direct-cli-performance-qualification.md);
  it is not an OPTIM latency, count, or memory baseline. Do not add OPTIM
  projection probes, batch-copy probes, or a current-Direct comparator. A
  missing OPTIM performance baseline is not a prerequisite for OPTIM-002 or
  later OPTIM tasks, and later epics must not treat one as inherited evidence.
- Graph/continuation research may proceed independently, but implementation that
  restructures the same domain model should follow BOUND. Any new history/read
  features must reconcile with READS first. The existing feature candidates
  remain unadopted; these improvements do not approve them.

An implementation that requires a preserved contract or validation policy to
change is out of scope and returns for separate authorization. Technical naming,
module layout and test implementation within these boundaries need no new
product decision. At closeout, promote durable requirements and evidence,
replace dossier links, and retire each dossier under the
[TODO lifecycle](../todo/README.md#lifecycle).

## OPTIM — Execution Path Optimization

Dependencies: none. Status: Completed. The owner accepted the current working
tree on 2026-10-17. All four tasks are Completed.

| id | title | status | goal | references |
|---|---|---|---|---|
| `OPTIM-001` | Retain functional characterization | Completed | Remove OPTIM measurement scaffolding and baseline requirements. Keep identity, overflow, and atomicity behavior. | [Procedure identity boundaries](../architecture/system.md#procedure-identity-boundaries); [functional regression owners](../implementation-tips/testing.md#functional-regression-owners); [Internal-improvement comparisons](../implementation-tips/direct-cli-performance-qualification.md#internal-improvement-comparisons) |
| `OPTIM-002` | Share validated read projections | Completed | Share one validated request-scoped projection across the seven Direct read entries, including package validation, while preserving behavior and identity; absorb DF-007. | [Request-scoped read composition](../architecture/system.md#request-scoped-read-composition); [functional regression owners](../implementation-tips/testing.md#functional-regression-owners) |
| `OPTIM-003` | Apply atomic batches through one draft | Completed | Collapse per-item WorkflowMemoryStateV2 reconstruction; prove sorted intermediate limits on preloaded attempts while preserving no-op revisions and atomic outcomes. | [Unpublished batch draft](../architecture/system.md#unpublished-batch-draft); [functional regression owners](../implementation-tips/testing.md#functional-regression-owners) |
| `OPTIM-004` | Qualify and close execution-path optimization | Completed | Prove behavioral equivalence, complete the development gate and independent review, and promote durable documentation. | [OPTIM completion](#optim-completion); [functional regression owners](../implementation-tips/testing.md#functional-regression-owners); [Internal-improvement comparisons](../implementation-tips/direct-cli-performance-qualification.md#internal-improvement-comparisons) |

### OPTIM completion

The owner accepted the implementation and test changes on 2026-10-17. Those
changes passed the default full `make test` with the inherited `/var` `TMPDIR`
and four workers. Independent Codex, Grok, and agy integration reviews were
complete, including the agy repair review. Acceptance records no latency, count,
or memory metric. Main verified this documentation closeout separately.

## BOUND — Domain and Application Boundaries

Dependencies: accepted `OPTIM` completion. Status: Completed. Implementation:
[System Architecture](../architecture/system.md#component-responsibilities) and
[boundary traceability](../specs/quality/requirements-traceability.md#boundary-refactoring-ownership-and-proof).

| id | title | status | goal | references |
|---|---|---|---|---|
| `BOUND-001` | Freeze responsibility and invariant ownership | Completed | Inventory type/error seams, all nine callback transports, three helper groups, preflight ownership and verification against accepted OPTIM. | [Ownership and proof](../specs/quality/requirements-traceability.md#boundary-refactoring-ownership-and-proof) |
| `BOUND-002` | Separate domain state and transitions from persistence | Completed | Move pure graph/workflow/goal rules into Core while retaining SQL reconstruction, relational checks and changed-row writes in Store. | [Pure state and persisted admission](../architecture/system.md#pure-graph-state-and-persisted-admission) |
| `BOUND-003` | Clarify Procedure runtime representation and admission | Completed | Share pure runtime declarations without merging authoring and persistence validation or adding reverse dependencies. | [Runtime and admission](../architecture/system.md#pure-graph-state-and-persisted-admission) |
| `BOUND-004` | Unify typed mutation failure transport and shared helpers | Completed | Replace all inventoried callback error transports with per-site failure/replay proof, preserving distinct fences/envelopes and ordering; absorb DF-023. | [Typed callback failures](../architecture/system.md#typed-mutation-callback-failures) |
| `BOUND-005` | Decompose CLI responsibilities without changing behavior | Completed | Decompose CLI concerns and move fence-preflight fetching/parsing together under one owner for READS, leaving completion and separate start observations unchanged. | [CLI responsibilities](../architecture/system.md#podway-cli) |
| `BOUND-006` | Qualify and close boundary refactoring | Completed | Prove dependency direction, current-Store continuity, error/behavior equivalence and no performance regression, then complete review and promotion. | [Continuity](../implementation-tips/testing.md#supported-store-continuity), [native qualification](../implementation-tips/direct-cli-performance-qualification.md#native-scale-execution) |

### BOUND delivery record

On 2026-10-17 Main accepted BOUND-001 under Master's Epic Harmony delegation,
after independent Judge and two Worker reviews, focused deferred-admission and
nine-callback failure/replay checks, the full `make test` development gate and
post-correction documentation checks. This is delegated Task acceptance;
the owner's subsequent whole-Epic acceptance is recorded below.

Main accepted BOUND-002 on the same delegation after independent Judge and two
Worker PASS verdicts, pure-test migration and persisted-admission regression
checks, static checks and the full `make test` development gate. Core owns the
pure graph/workflow/goal implementation; Store retains independent persistence
admission and SQL.

Main accepted BOUND-003 under the same delegation after Judge and two Worker
PASS verdicts, source/persisted adapter and historical identity checks, pure
placement/goal independence checks, static checks and the full `make test`
development gate. Source and Store share Core runtime declarations while
retaining their independent admission boundaries.

Main accepted BOUND-004 under the same delegation after Judge and two Worker
PASS verdicts, the Leader's consolidated PASS, typed callback rollback/retry and
identity checks, all-nine per-site error/replay checks, separate COMMIT I/O
classification, static checks and the full `make test` development gate. One
Store runner now transports typed failures; DF-023 is implemented.

Main accepted BOUND-005 under the same delegation after Judge, two Worker and
Main-native CLI PASS verdicts, the Leader's consolidated PASS, actual file-safety,
preview, completion-budget, stdin-priority and archive-replay regressions, static
checks and the full `make test` development gate. Private CLI modules now own
grammar, authoring, file access, start, rendering and fence preflight.

Main accepted BOUND-006 under the same delegation after independent Main and
AGY integrated PASS verdicts, final Judge and two Worker confirmation, the final
`make test` development gate, pinned release Clippy and the complete nine-case
native qualification. Supported generation-15 Stores continue without reset or
migration, preserving writer/snapshot identity and exact original response bytes
before and after progression. The final CLI also passes fresh-process observation,
old-attempt replay and read-only lookup without logical state changes.

The final compiler-only CLI import correction retains the native qualification
of the byte-identical Store/Engine release test executables; the original native
source snapshot remains distinct from the final overall source snapshot. Existing
300-node blocking and maximum-graph informational policies remain unchanged; no
OPTIM baseline, comparator or count ceiling was introduced. Architecture,
traceability and testing guidance record the implemented ownership. All six member
Tasks are complete under Main delegation.

Master explicitly accepted the complete BOUND epic on 2026-10-17 at
`036e86ed922a4e153acce02ae3e267b357a217a1`, after receiving the integrated
implementation, verification and six local Task commits. This is user Epic
acceptance, distinct from Main's delegated Task acceptance. Durable requirements,
ownership and verification guidance replace the retired BOUND dossier under the
[TODO lifecycle](../todo/README.md#lifecycle). READS delivery follows in the
[current section](#reads-read-models-and-data-access). No release, installation
or remote publication is included.

## READS — Read Models and Data Access

Dependencies: accepted `BOUND` completion. Status: Completed. Implementation: [validated reads and typed facts](../implementation-tips/direct-cli-performance-qualification.md#reads-005-functional-closeout).

Master authorized direct Epic Harmony delivery of READS on 2026-10-17, including
Main's delegated Task acceptance, lifecycle decisions and local Task commits.
Master accepted the assembled Epic and exact documentation closeout on
2026-10-19, separately from Main's delegated Task acceptance. The adopted
dossier is retired under the [TODO lifecycle](../todo/README.md#lifecycle);
lasting implementation and correctness boundaries remain in the linked
architecture, specifications and guidance. No Aquarium handler or Podway
session was required. This is local development completion, with no release,
installation or remote publication.

Master reaffirmed on 2026-10-19 that performance measurement is excluded from
READS acceptance. No latency, memory, native-scale or matched G7/G8 benchmark
is required or authorized for this closeout. Existing deterministic query,
copy, bounds and behavior assertions remain correctness evidence; historical
comparison registrations create no instruction to execute measurements.

| id | title | status | goal | references |
|---|---|---|---|---|
| `READS-001` | Freeze data requirements and validation equivalence | Completed | Map queries, decode/validation scope, error ordering and deterministic count assertions before reducing data-access work. | [READS query and validation registration](../implementation-tips/direct-cli-performance-qualification.md#reads-registered-unchanged-query-baseline) |
| `READS-002` | Bulk-load attempt data without reducing validation | Completed | Bulk-load slots/blockers/evidence, goal criteria and citations; batch changed-slot reloads while preserving header-only reuse, validation order and bounded memory. | [Cold and prepared acquisition](../implementation-tips/direct-cli-performance-qualification.md#reads-cold-family-acquisition) |
| `READS-003` | Build purpose-specific views over validated state | Completed | Build typed views over shared validation, prove hidden historical-value refusal and keep archive-list summary-only without reintroducing OPTIM's eliminated copies. | [Projection ownership](../implementation-tips/direct-cli-performance-qualification.md#reads-projection-copy-elimination) |
| `READS-004` | Replace CLI status-JSON preflight with typed facts | Completed | Replace the named fence-preflight module with typed Engine facts, preserving status-only failures; prove zero stdin preflight and the post-facts competing-writer gap. | [Typed preflight facts](../implementation-tips/direct-cli-performance-qualification.md#reads-typed-fence-preflight-facts) |
| `READS-005` | Qualify and close validated data-access improvements | Completed | Confirm behavioral, storage, validation and typed-facts preservation with existing correctness evidence, then complete independent review and durable documentation without performance measurement. | [Functional closeout](../implementation-tips/direct-cli-performance-qualification.md#reads-005-functional-closeout) |

## REL12 — Podway v0.1.2 Contract Recovery and Release

| id | title | status | goal | references |
|---|---|---|---|---|
| `REL12001` | Freeze the v0.1.2 recovery design | Completed | Adopt the decision-complete design, authority boundaries, release constraints, and ordered implementation plan. | [Release completion](archive/v0.1.2-release-report.md) |
| `REL12002` | Audit the v1 compatibility boundary | Completed | Prove released-schema compatibility and record the exact pre-release consumer migration boundary. | [Compatibility evidence](archive/v0.1.2-release-report.md#qualification-and-publication) |
| `REL12003` | Repair the version identity contract | Completed | Make both binaries emit one identical schema-conformant identity and reject malformed runtime probes. | [Identity contract](../specs/interfaces/automation-client-contract.md#retired-requirement-ids-and-clauses) |
| `REL12004` | Enforce authoritative packaged-schema validation | Completed | Validate complete identity envelopes using only the exact manifest-bound packaged contract set. | [Release contract](../specs/interfaces/automation-client-contract.md#retired-requirement-ids-and-clauses) |
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
| `V2SCL-002` | Reserve pageable evidence read contracts | Completed | Register `evidence.read`, `next-result/v3`, `observation-result/v3`, the closed `output-v3` branch, shared and record-many schema bounds, public errors, command routes, manifest digests, and compatibility fixtures without premature runtime admission. | [IPC evidence paging](../specs/interfaces/json-contract.md#evidence-paging), [evidence scale failures](../specs/interfaces/errors-and-exit-codes.md#bounded-input-and-evidence-failures) |
| `V2SCL-003` | Align item limits and structured diagnostics | Completed | Unify authoring and runtime limits, protocol slices, total-list and attempt bounds, re-pin the rotated preset digests, and report exact exceeded fields and maxima. | [item bounds](../specs/domain/procedure-and-item-specification.md#item-bounds), [ADR-0024](../architecture-decision-records/0024-bounded-evidence-scale-and-paged-read-back.md) |
| `V2SCL-004` | Implement snapshot-bound evidence paging | Completed | Serve deterministic bounded text and list pages under current evidence identity, freshness, page-token, metadata, and IPC constraints. | [evidence paging](../specs/interfaces/json-contract.md#evidence-paging), [evidence reads](../specs/interfaces/cli-specification.md#evidence-reads) |
| `V2SCL-005` | Integrate observation budgets and close conformance | Completed | Derive the original response-budget composition, bound every window by bytes with an exact total and a truthful truncation flag, close the compact-status envelope guard and the stale read-back projection, promote durable documentation, remove the completed dossier, and pass the development gate. `V2GSC-004` later corrected the composition for 1,024 complete graph counters. | [response budgets](../specs/interfaces/json-contract.md#response-budgets), [ADR-0024](../architecture-decision-records/0024-bounded-evidence-scale-and-paged-read-back.md) |

## V2AST — External Check Result Typing

Dependencies: completed `V2SCL` epic.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2AST-001` | Adopt the external check result authority | Completed | Adopt the extending ADR and normative item, trust, storage, protocol, and compatibility contracts. | [ADR-0025](../architecture-decision-records/0025-structurally-bound-external-check-results.md), [item specification](../specs/domain/procedure-and-item-specification.md#external-check-results) |
| `V2AST-002` | Reserve check-result contracts and storage | Completed | Reserve the closed schemas, observation windows, canonical SQLite v6 DDL and migration identity, manifest changes, and compatibility fixtures without runtime admission. | [automation contract](../specs/interfaces/automation-client-contract.md#structurally-bound-external-check-results-aut-chk-001009), [SQLite model](../specs/storage/sqlite-model.md) |
| `V2AST-003` | Add the check-result domain and authoring model | Completed | Add bounded declarations, complete values, satisfaction, parsing, diagnostics, and canonicalization while preserving existing Procedure digests. | [item specification](../specs/domain/procedure-and-item-specification.md#external-check-results) |
| `V2AST-004` | Migrate, decode, and record check results | Completed | Rebuild the constrained item table, add bounded protocol decoding and atomic frame-sized record-many support, and prove replay, restart, idempotency, and downgrade protection. | [automation contract](../specs/interfaces/automation-client-contract.md#structurally-bound-external-check-results-aut-chk-001009), [SQLite model](../specs/storage/sqlite-model.md) |
| `V2AST-005` | Expose check-result guidance and read-back | Completed | Add allowed actions, suggestions, the bounded structured projection and stdin template, single-page evidence read-back, CLI guidance, and honest rendering. | [automation contract](../specs/interfaces/automation-client-contract.md#structurally-bound-external-check-results-aut-chk-001009), [IPC protocol](../specs/interfaces/json-contract.md#evidence-paging) |
| `V2AST-006` | Close check-result compatibility and conformance | Completed | Prove compatibility, promote durable documentation, remove the completed dossier, and close trust, migration, recovery, frame, projection, and maximum-size evidence. | [ADR-0025](../architecture-decision-records/0025-structurally-bound-external-check-results.md), [security and trust](../specs/operations/security-and-trust.md), [automation contract](../specs/interfaces/automation-client-contract.md#structurally-bound-external-check-results-aut-chk-001009) |

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
| `V2GRD-005` | Enforce runtime condition gates | Completed | Enforce identity, freshness, and guards in order; expose structured statuses; and register non-retryable `OPTION_GUARD_UNSATISFIED`. | [State transitions](../specs/domain/state-transitions.md), [Errors and exit codes](../specs/interfaces/errors-and-exit-codes.md#typed-conditions-and-authoring-diagnostics) |
| `V2GRD-006` | Repair phase-owner lint and close conformance | Completed | Remove the two count constants and legacy codes, add distinct-label and weak-criteria diagnostics, promote durable documentation, remove the completed dossier, and pass the development gate. | [ADR-0028](../architecture-decision-records/0028-bounded-typed-procedure-guards.md), [Errors and exit codes](../specs/interfaces/errors-and-exit-codes.md#typed-conditions-and-authoring-diagnostics), [workflow example](../examples/v2-workflow.md#author-typed-conditions) |

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
| `V2RDY-001` | Adopt and reserve daemon readiness contracts | Completed | Adopt the early-control-plane decision and reserve status v2, wait-ready, phase-aware errors, recovery recipes, routes, and manifest pins without runtime admission. | [ADR-0029](../architecture-decision-records/0029-serve-phase-aware-daemon-readiness.md), [IPC contract](../specs/interfaces/json-contract.md#direct-cli-generation) |
| `V2RDY-002` | Serve verified status during recovery | Completed | Start the authenticated control plane before registry/worktree/job recovery, expose monotonic readiness progress, and reject normal routes before ready. | [Observability contract](../specs/operations/observability.md) |
| `V2RDY-003` | Add bounded readiness waiting and close conformance | Completed | Add daemon wait-ready, reuse it for install, render non-ready status honestly, return actionable timeouts, prove retry preservation, promote durable guidance, and pass the development gate. | [CLI contract](../specs/interfaces/cli-specification.md), [automation contract](../specs/interfaces/automation-client-contract.md) |

## V2RET — Workspace Retirement and Registry Hygiene

Status: Completed.

Canonical Outcomes: [ADR-0030](../architecture-decision-records/0030-retire-workspaces-and-prune-stale-registry-entries.md), [IPC protocol](../specs/interfaces/json-contract.md#direct-cli-generation), [JSON contract](../specs/interfaces/json-contract.md), [CLI specification](../specs/interfaces/cli-specification.md), [recovery specification](../specs/storage/recovery-retention-and-maintenance.md), [testing and conformance](../specs/quality/testing-and-conformance.md), and [crash-boundary registry](../../quality/crash-boundaries-v1.json).

Dependencies: completed `V2RDY` epic.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2RET-001` | Adopt workspace retirement authority | Completed | Adopt exact missing-root pruning, explicitly confirmed full `.podway` removal, recovery, compatibility, and safety boundaries. | [ADR-0030](../architecture-decision-records/0030-retire-workspaces-and-prune-stale-registry-entries.md), [recovery specification](../specs/storage/recovery-retention-and-maintenance.md) |
| `V2RET-002` | Reserve workspace removal contracts | Completed | Register the command, result, errors, removal marker, replay semantics, observability, manifest identity, and compatibility fixtures without premature runtime admission. | [IPC contract](../specs/interfaces/json-contract.md#direct-cli-generation), [public schemas](../specs/interfaces/json-contract.md) |
| `V2RET-003` | Prune stale registry entries | Completed | Remove only conclusively missing exact UUID/root generations during startup and pre-mutation revalidation, then integrate successful cleanup with readiness and bounded observability. | [Recovery specification](../specs/storage/recovery-retention-and-maintenance.md#moved-worktrees); [worktree deletion](../architecture/git-worktree-and-filesystem.md#worktree-deletion) |
| `V2RET-004` | Remove a selected Podway workspace | Completed | Add exact-path confirmation, maintenance fencing, marker-backed recovery, empty-residual replay convergence, bounded observability, and complete `.podway` removal while preserving the Git worktree. | [CLI contract](../specs/interfaces/cli-specification.md), [recovery specification](../specs/storage/recovery-retention-and-maintenance.md) |
| `V2RET-005` | Close retirement and cleanup conformance | Completed | Prove races, crashes, replay, compatibility, path safety, reinitialization, and E2E behavior; promote durable guidance; remove the dossier; and pass the development gate. | [Testing and conformance](../specs/quality/testing-and-conformance.md), [crash registry](../../quality/crash-boundaries-v1.json) |

## V2DVC — Named Runtime Modes and Aquarium Development Channel

Status: Completed.

Canonical Outcomes: [ADR-0031](../architecture-decision-records/0031-keyed-runtime-modes-and-managed-development-service.md), [CLI specification](../specs/interfaces/cli-specification.md), [IPC protocol](../specs/interfaces/json-contract.md#direct-cli-generation), [SQLite model](../specs/storage/sqlite-model.md), [recovery specification](../specs/storage/recovery-retention-and-maintenance.md), [historical Aquarium service decision](../architecture-decision-records/0031-keyed-runtime-modes-and-managed-development-service.md), [release and packaging](../specs/operations/release-and-packaging.md), [testing and conformance](../specs/quality/testing-and-conformance.md), and [requirements traceability](../specs/quality/requirements-traceability.md).

Dependencies: completed `V2RET` epic.

This epic generalized the production/development distinction into one bounded
runtime mode key while preserving exactly one effective mode and one normal
writer for each worktree. It delivered the external Aquarium v2 managed-service
interface on that model.
That historical surface is retired by `V2DFC-009`;
[ADR-0038](../architecture-decision-records/0038-remove-the-daemon.md) owns the
current direct target. Completed entries below preserve the original work history.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2DVC-001` | Adopt the keyed runtime authority | Completed | Adopt the architecture, compatibility boundaries, external Aquarium interface, and ordered implementation plan. | [ADR-0031](../architecture-decision-records/0031-keyed-runtime-modes-and-managed-development-service.md) |
| `V2DVC-002` | Reserve v0.2.8 runtime-mode contracts | Completed | Reconcile the v0.2.8 cycle and reserve bounded workspace mode, managed-runtime, status, error, protocol, storage, and compatibility contracts without premature runtime admission. | [CLI specification](../specs/interfaces/cli-specification.md), [SQLite model](../specs/storage/sqlite-model.md), [changelog](../../CHANGELOG.md) |
| `V2DVC-003` | Add keyed daemon namespaces | Completed | Implement runtime-mode values, complete mode-keyed paths, managed metadata validation, CLI and daemon selectors, and the `--dev` compatibility aliases. | [ADR-0031](../architecture-decision-records/0031-keyed-runtime-modes-and-managed-development-service.md), [macOS service architecture](../architecture/macos-service.md) |
| `V2DVC-004` | Bind and switch workspace modes | Completed | Enforce config, endpoint, daemon, registry, and Store mode agreement before Store access, then add token-bound runtime-reset switching with crash recovery. | [recovery specification](../specs/storage/recovery-retention-and-maintenance.md), [IPC protocol](../specs/interfaces/json-contract.md#direct-cli-generation) |
| `V2DVC-005` | Add the persistent Aquarium development service | Completed | Build the immutable managed-service bundle and producer controller, keep publication pending-only, and activate exact idle generations with proven rollback or bounded recovery debt. | [historical Aquarium service decision](../architecture-decision-records/0031-keyed-runtime-modes-and-managed-development-service.md), [macOS service architecture](../architecture/macos-service.md) |
| `V2DVC-006` | Generalize isolated qualification runtimes | Completed | Move contributor and packaged release qualification to private named runtime identities while preserving exact snapshots, sandbox admission, cleanup, and production and Aquarium isolation. | [current contributor isolation](../implementation-tips/testing.md#isolated-direct-cli-checks), [release workflow](../implementation-tips/release.md) |
| `V2DVC-007` | Close named-runtime conformance | Completed | Prove compatibility, coexistence, switching, recovery, Aquarium rollover, and parallel qualification; promote durable documentation; retire the dossier; and pass the development gate. | [testing and conformance](../specs/quality/testing-and-conformance.md), [requirements traceability](../specs/quality/requirements-traceability.md) |

## V2FRT — User Runtime Factory Reset

Status: Completed.

Canonical Outcomes: [ADR-0032](../architecture-decision-records/0032-retire-ordinary-user-runtimes.md), [CLI specification](../specs/interfaces/cli-specification.md), [IPC protocol](../specs/interfaces/json-contract.md#direct-cli-generation), [JSON contract](../specs/interfaces/json-contract.md), [runtime reset](../specs/operations/runtime-reset.md), [recovery specification](../specs/storage/recovery-retention-and-maintenance.md), [macOS service architecture](../architecture/macos-service.md), [testing and conformance](../specs/quality/testing-and-conformance.md), [requirements traceability](../specs/quality/requirements-traceability.md), [crash-boundary registry](../../quality/crash-boundaries-v1.json), and [operator guide](../specs/operations/runtime-reset.md#historical-retired-contract).

Dependencies: completed `V2DVC` epic.

Provide explicitly confirmed single-mode and all-mode retirement of ordinary
user-global runtime data and the selected production service. Preserve
worktrees, their Podway state, installed binaries, and external managed runtimes.
Reject busy runtimes; support bounded offline cleanup and crash-safe exact retry.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2FRT-001` | Adopt reset authority and reserve contracts | Completed | Promote the reset ownership, compatibility, public contracts, and bounded recovery model before runtime admission. | [ADR-0032](../architecture-decision-records/0032-retire-ordinary-user-runtimes.md), [IPC protocol](../specs/interfaces/json-contract.md#direct-cli-generation), [JSON contract](../specs/interfaces/json-contract.md) |
| `V2FRT-002` | Add bounded read-only reset planning | Completed | Add explicit selection, no-write inventory, eligibility, exclusions, and state-bound confirmation tokens. | [runtime reset](../specs/operations/runtime-reset.md), [CLI specification](../specs/interfaces/cli-specification.md) |
| `V2FRT-003` | Add atomic idle reservation and startup fencing | Completed | Close admission and commit races while preserving stable lock identity and existing lifecycle contracts. | [runtime reset](../specs/operations/runtime-reset.md), [macOS service architecture](../architecture/macos-service.md) |
| `V2FRT-004` | Retire one ordinary runtime with durable recovery | Completed | Add live and offline cleanup, exact service shutdown, bounded resource deletion, and interruption-safe replay. | [runtime reset](../specs/operations/runtime-reset.md), [recovery specification](../specs/storage/recovery-retention-and-maintenance.md), [crash-boundary registry](../../quality/crash-boundaries-v1.json) |
| `V2FRT-005` | Compose all-mode reset and exact retry | Completed | Preflight and reserve the frozen selection, report partial failure honestly, and preserve recreated runtimes on replay. | [runtime reset](../specs/operations/runtime-reset.md), [testing and conformance](../specs/quality/testing-and-conformance.md) |
| `V2FRT-006` | Close conformance and operator guidance | Completed | Prove isolated runtime, crash, race, compatibility and re-registration behavior; promote durable guidance, retire the dossier, and pass the development gate. | [testing and conformance](../specs/quality/testing-and-conformance.md), [requirements traceability](../specs/quality/requirements-traceability.md), [operator guide](../specs/operations/runtime-reset.md#historical-retired-contract) |

Tasks execute in numeric order. At most the first incomplete task may be In
Progress, In Review, or Blocked; later tasks remain Planned. Completing the tasks
does not replace independent epic acceptance. Release selection, distribution,
installation, and runtime activation remain outside this epic.

<a id="v2pid-canonical-path-workspace-identity"></a>

## V2PID — Canonical-Path Workspace Identity

Status: Completed.

This corrective epic replaces reboot-unstable filesystem-object fingerprints
with canonical Git paths while preserving request-local descriptor race checks.
It is independent of delegated Procedure behavior and does not activate or
install a runtime.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2PID-001` | Replace durable filesystem identity with canonical paths | Completed | Add path-only Git identity, exact-root schema-v13 migration, missing-registry reset recovery, regression coverage, and durable architecture guidance. | [ADR-0036](../architecture-decision-records/0036-use-canonical-git-paths-for-workspace-identity.md), [SQLite model](../specs/storage/sqlite-model.md), [Git model](../architecture/git-worktree-and-filesystem.md) |

<a id="v2gsc-procedure-graph-scale-correction"></a>

## V2GSC — Procedure Graph Scale Correction

Status: Completed.

Dependencies: completed `V2PID` epic. `V2SDG-006` and later tasks depend on this
correction because their complete projections must preserve the graph-wide
counter set.

This corrective epic raises the accidental 64-placement ceiling while preserving
Podway's bounded-input and single-cursor contracts. It does not change storage
schema, install or activate a runtime, or alter unrelated 64-entry collections.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2GSC-001` | Adopt the 1,024-node graph authority | Completed | Establish one portable graph ceiling and preserve all independent collection and frame bounds. | [ADR-0037](../architecture-decision-records/0037-bound-procedure-graphs-at-1024-nodes.md) |
| `V2GSC-002` | Scale graph analysis, persistence, and contracts | Completed | Use bounded non-recursive analysis, complete 1,024-counter result contracts, exact response admission, and changed-row-only counter persistence without a Store migration. | [Procedure specification](../specs/domain/procedure-and-item-specification.md), [system architecture](../architecture/system.md) |
| `V2GSC-003` | Close graph-scale conformance | Completed | Prove 1,024 acceptance, 1,025 rejection, contract consistency, and regression safety through the development gate. | [Testing and conformance](../specs/quality/testing-and-conformance.md) |
| `V2GSC-004` | Close response-admission and boundary-proof gaps | Completed | Charge both observation counter copies, prove exact canonical and multiword graph boundaries, keep preview fail-closed, and align public schemas, fixtures, and documentation. | [IPC protocol](../specs/interfaces/json-contract.md#direct-cli-generation), [ADR-0037](../architecture-decision-records/0037-bound-procedure-graphs-at-1024-nodes.md) |

<a id="v2sdg-host-neutral-supervised-delegation-foundation"></a>

## V2SDG — Host-Neutral Supervised Delegation Foundation

Status: Completed.

Release target: `v0.2.12`. The source foundation and development acceptance
are complete. The `v0.2.11` stable release contains the earlier internal
foundations. The source targeting `v0.2.12` implements native routes and
opt-in admission, with native lifecycle and migration qualification through
`V2SDG-008`. Public support follows release qualification and publication.

Dependencies: completed `V2FRT`, completed `V2GSC` before `V2SDG-006`, and the
completed evidence, guard, reference, readiness, retention, and named-runtime foundations. There is no live Dolgorae,
Primary bridge, graph-engineering, continuation-generation, or Web-console
dependency for this Podway-only epic.

Authority: [ADR-0035](../architecture-decision-records/0035-host-neutral-supervised-delegation.md).
Target behavior: [supervised-delegation specification](../specs/interfaces/supervised-delegation.md).
Operational constraints: [non-disruptive development guidance](../implementation-tips/supervised-delegation.md).

Goal: add an explicitly opt-in, host-neutral action-delegation declaration,
canonical work package, structural acceptance receipt, and completion gate while
preserving existing Direct work. A Primary may be externally hosted or eventually
Dolgorae-managed without changing the procedure's execution responsibility.
Existing snapshots, preset digests, Direct result shapes, and supported lifecycle
and recovery behavior are compatibility gates, not migration conveniences.

Implemented behavior and proof are mapped in the
[requirement traceability matrix](../specs/quality/requirements-traceability.md#supervised-delegation-ownership-and-proof).
Source capability does not grant publication, installation, mode switching,
daemon restart, or production and Aquarium runtime activation. Real executor
integration remains a
[separate candidate](../todo/TODO-podway-supervised-delegation-integration.md).

The offline [adapter field/authority/digest review](../implementation-tips/delegation-adapter-contract-review.md)
closed the Podway meanings before schema reservation. Both real integration
paths remain unavailable pending their named prerequisites. The local native
fixture covers both host labels, single acceptance, the post-acceptance matrix,
and recovery without automatic redispatch; it does not qualify a real adapter.
Development acceptance passed the full `make test` gate and the independent
committed-epic audit. All nine members are complete; the adopted dossier was
retired after its lasting content moved to the authorities linked above.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2SDG-001` | Freeze continuity baseline and qualification matrix | Completed | Capture exact existing contracts, supported paths, offline adapter-review inputs and post-acceptance cases; add isolated compatibility fixtures before product behavior changes. | [continuity](../specs/interfaces/supervised-delegation.md#sdg-010-operational-continuity-and-acceptance) |
| `V2SDG-002` | Reserve closed delegation contracts | Completed | Close the offline field/authority/digest mapping and classify unavailable real paths before reserving schemas, projections, errors/jobs, bounds and traceability; keep runtime admission closed. | [public versions](../specs/interfaces/supervised-delegation.md#sdg-008-public-contract-generations) |
| `V2SDG-003` | Add placement semantics and canonical work packages | Completed | Implement optional host-neutral declarations and pure package identity while preserving old canonical bytes, node kinds, and single-cursor behavior. | [package](../specs/interfaces/supervised-delegation.md#sdg-004-canonical-work-package) |
| `V2SDG-004` | Persist immutable attempt receipts safely | Completed | Add receipt storage and byte-preserving migration, atomic job/state results, reconstruction, crash recovery, and isolated downgrade rejection. | [persistence](../specs/interfaces/supervised-delegation.md#sdg-009-persistence-and-isolation) |
| `V2SDG-005` | Enforce acceptance and completion semantics | Completed | Validate exact identities and final item state; enforce the full post-acceptance matrix, no-op/rollback preservation, replay and transaction-time gates without changing Direct paths. | [acceptance](../specs/interfaces/supervised-delegation.md#sdg-005-structural-acceptance) |
| `V2SDG-006` | Add truthful projections and bounded receipt history | Completed | Expose bounded work/receipt views and truthful missing/current/stale states; explain final recording before acceptance and the fresh-attempt cost of later edits without changing Direct output. | [readiness](../specs/interfaces/supervised-delegation.md#sdg-006-readiness-and-subsequent-changes) |
| `V2SDG-007` | Integrate native routes and coherent opt-in admission | Completed | Wire CLI, daemon, job recovery and opt-in admission only when coherent; verify correction/replay guidance in permitted carriers and preserve no-executor Direct operation. | [public versions](../specs/interfaces/supervised-delegation.md#sdg-008-public-contract-generations) |
| `V2SDG-008` | Qualify lifecycle migration and host-neutral recovery | Completed | Prove native crash/replay and the post-acceptance matrix, no automatic external redispatch, both simulated hosts, Store upgrade and independent Direct progress without live providers. | [recovery](../specs/interfaces/supervised-delegation.md#sdg-007-runtime-lifecycle-and-external-recovery) |
| `V2SDG-009` | Close full gate independent review and durable guidance | Completed | Promote durable authorities and reviewed mappings, pass make test and independent review, resolve regressions, document correction/integration limits, and retire the dossier without deployment. | [operational guidance](../implementation-tips/supervised-delegation.md) |

Tasks execute in numeric order with one active task across this epic. At most
the first incomplete task may be In Progress, In Review, or Blocked; every later
task remains Planned. Each task is one logical implementation-and-verification
commit, subject to separate commit authorization. A supported-path regression
blocks acceptance and must not be deferred or hidden by changing default
procedures. Partial delegated runtime admission is forbidden before V2SDG-007.

The conditional stable-release target is `v0.2.12`. Completing this epic proves
development acceptance but does not by itself authorize or prove release,
installation, or runtime activation; the stable candidate must still pass its
separately authorized release and upgrade qualification. Real Dolgorae or managed
Primary integration remains outside this epic. A schema change must not bypass
ADR-0031's managed-generation migration restrictions.

## V2DHP — Fast Daemon Status and Readiness

Status: Completed.

Dependencies: completed `V2RDY` and `V2DVC` epics. `daemon status` acquired
aggregate activity counts in `V2DVC-005` (first released in v0.2.8); its
disposable inspection copied and checked every active workspace database and
WAL, causing a live daemon to exceed the CLI's five-second read timeout on a
host with 22 active schedulers and about 133 MB of main databases. This
corrective epic uses indexed counts from short read transactions for status,
while retaining stronger disposable inspection for runtime reset and workspace
maintenance. It also bounds named-runtime readiness probes by the caller's
deadline and starts Podway sessions with worktree observation.

Development acceptance: the complete `make test` gate passed. An isolated
debug daemon with 22 active schedulers and 22 worktrees returned `daemon
status` in 12.71 ms median and 14.32 ms maximum; `daemon wait-ready` took
11.73 ms. Each worktree received six
supported `record --stdin` batches of 16 optional text items containing
55,000 characters each. The live SQLite main databases totaled 138,493,952
bytes, with another 115,307,144 bytes in WAL files. This measures the actual
CLI-to-daemon path at the reported scale. After an orderly stop, the initial
cold-recovery implementation remained in `recovering` beyond 180 seconds.
`V2DHP-003` removed redundant database copies and integrity scans from startup
activation while retaining the complete guarded Store preflight. An optimization
trial on the isolated 22-worktree fixture reached `ready` in 47.31 seconds.
After read transactions were limited to startup to preserve ordinary resolution
safety, the final route reached `ready` in 7.76 seconds on two worktrees with
14,925,824 bytes of checkpointed main databases. The 22-worktree fixture was
not rerun after that routing correction. The later
[direct execution decision](../architecture-decision-records/0038-remove-the-daemon.md)
retires the daemon startup-recovery candidate; current Store checks are owned by
the [SQLite model](../specs/storage/sqlite-model.md).
No release, installation, or active-daemon replacement was performed.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2DHP-001` | Read live activity without database copies | Completed | Count queued and running jobs through bounded Store reads; leave unproven counts null and preserve deeper maintenance checks. | [Daemon status](../specs/operations/observability.md#daemon-status) |
| `V2DHP-002` | Bound readiness waiting and align session entry | Completed | Clamp named-runtime probes to the readiness deadline, use worktree observation for session entry, and prove status and wait-ready behavior through the development gate. | [CLI specification](../specs/interfaces/cli-specification.md#command-groups), [runtime skill](../../skills/use-podway/SKILL.md) |
| `V2DHP-003` | Remove redundant cold recovery inspections | Completed | Use read transactions for startup binding checks, retain the guarded Store integrity preflight, and skip proven empty recovered queues. | [Recovery specification](../specs/storage/recovery-retention-and-maintenance.md#daemon-restart) |

Tasks execute in numeric order. Development acceptance does not authorize a
release, installation, or replacement of an active daemon.

<a id="v2dfc-daemon-free-core"></a>

## V2DFC — Daemon-Free Core

Status: Completed.

Reopened on October 9, 2026. An independent implementation review of the
completed Epic found acceptance gaps. `V2DFC-011` through `V2DFC-018`
remediated them under the
[direct execution contract](../specs/storage/transactions-concurrency-and-idempotency.md),
[filesystem architecture](../architecture/git-worktree-and-filesystem.md), and
[acceptance checks](../specs/quality/testing-and-conformance.md). The earlier
closeout did not satisfy the final closeout of the reopened Epic. Reopening
registered work only; it did not authorize release or installation.

Conditional release target: `v0.2.13`. Inclusion requires completion of
`V2DFC-001` through `V2DFC-018`, the required development acceptance, and a
separately authorized stable-release qualification. Installation additionally
requires Aquarium and Dolgorae releases that support the new contract
generation.

Dependencies: completed `V2DHP` and the completed foundations it builds on.
There is no graph-engineering, continuation-generation, Web-console, or live
Dolgorae dependency.

Authority: [ADR-0038](../architecture-decision-records/0038-remove-the-daemon.md),
accepted by `V2DFC-002` after the feasibility gate.
Implemented requirements: [direct execution traceability](../specs/quality/requirements-traceability.md#direct-execution-requirements).

Goal: remove `podwayd` so that every command runs to completion in the invoking
process and commits each mutation in one SQLite transaction on the owning
worktree's Store. The release publishes a new public contract generation
without backward compatibility. Daemon-era runtime state fails closed and is
reset without migration.

This adoption registers work only. It does not change shipped behavior,
supersede an accepted ADR, start a task, or authorize commit, release,
installation, or removal of an installed daemon.

| id | title | status | goal | references |
|---|---|---|---|---|
| `V2DFC-001` | Prove the direct-write model against pre-registered thresholds | Completed | Measure contention, crash, maintenance, preparation-race, latency, open-cost, WAL, and legacy-classification checks with a test-only prototype; any failure blocks the epic. | [feasibility gate and historical baselines](../implementation-tips/direct-cli-performance-qualification.md) |
| `V2DFC-002` | Accept the daemon-free architecture authority | Completed | Accept ADR-0038 with the gate evidence, link supersessions in both directions, and add AGENTS.md transition wording without changing contracts or code. | [ADR-0038](../architecture-decision-records/0038-remove-the-daemon.md) |
| `V2DFC-003` | Reserve the daemon-free contract generation | Completed | Reserve the new schemas, error codes, outcome lookup route, retired-command table, generation-14 Store DDL, and verifier counts without changing runtime output. | [JSON contract](../specs/interfaces/json-contract.md), [SQLite model](../specs/storage/sqlite-model.md) |
| `V2DFC-004` | Extract host-independent domain services into podway-engine | Completed | Move read projections, preparation, request digests, workspace resolution, and marker codecs into a new engine crate without behavior change. | [repository structure](../architecture/repository-structure.md), [Rust codebase](../architecture/rust-codebase.md) |
| `V2DFC-005` | Build the direct Store and maintenance primitives beside the legacy Store | Completed | Add the generation-14 Store with write-free classification and reads, the maintenance lock and markers, writer identity, and single-transaction success outcomes. | [SQLite model](../specs/storage/sqlite-model.md), [recovery](../specs/storage/recovery-retention-and-maintenance.md) |
| `V2DFC-006` | Execute every retained command through the direct engine | Completed | Run every retained read, mutation, and maintenance command in-process with revalidated preparation and validated new-generation envelopes. | [transactions](../specs/storage/transactions-concurrency-and-idempotency.md), [JSON contract](../specs/interfaces/json-contract.md) |
| `V2DFC-007` | Switch the CLI to the direct engine and delete the daemon host | Completed | Route the CLI through the engine, emit the new contract, return retired-command guidance, and delete the daemon crate and binary. | [CLI](../specs/interfaces/cli-specification.md), [performance qualification](../implementation-tips/direct-cli-performance-qualification.md) |
| `V2DFC-008` | Retire residual daemon-era infrastructure and contracts | Completed | Delete the service crate, IPC protocol, legacy Store and migrations, runtime modes, and daemon-only schemas and errors, and retarget fuzzing. | [repository structure](../architecture/repository-structure.md), [testing and conformance](../specs/quality/testing-and-conformance.md) |
| `V2DFC-009` | Rebuild release and development tooling around one binary | Completed | Package and qualify one binary, regenerate identity and handoff contracts, retire the Aquarium development service, and rewrite release QA. | [release workflow](../implementation-tips/release.md), [upgrade QA](../implementation-tips/release-upgrade-qa.md) |
| `V2DFC-010` | Promote durable documentation | Completed | Promote lasting decisions and verification baselines to specifications, architecture, README, skills, and AGENTS.md, and update related candidates before the final Epic audit. | [documentation authorities](../README.md), [requirements traceability](../specs/quality/requirements-traceability.md) |
| `V2DFC-011` | Repair CLI-local failure envelopes and retired-surface guidance | Completed | Emit a validated error envelope for every JSON-mode failure without aborting, honor a late `--json`, and complete retired-command help and completion. | [errors and exit codes](../specs/interfaces/errors-and-exit-codes.md), [CLI](../specs/interfaces/cli-specification.md) |
| `V2DFC-012` | Bound only lock waits by the invocation deadline | Completed | Attempt every lock once before reporting expiry, remove post-acquisition deadline checks, and report the boundary that actually expired. | [CLI](../specs/interfaces/cli-specification.md), [automation client](../specs/interfaces/automation-client-contract.md) |
| `V2DFC-013` | Revalidate prepared state against current fences | Completed | Re-evaluate fences and start policy inside the transaction instead of failing on any workspace sequence change, and never report contention as an internal error. | [transactions](../specs/storage/transactions-concurrency-and-idempotency.md), [errors and exit codes](../specs/interfaces/errors-and-exit-codes.md) |
| `V2DFC-014` | Prepare artifact digests outside the write transaction | Completed | Hash local artifacts during preparation, then require each file's stamp, recorded metadata and revisions to match inside the transaction without reading file bytes. | [automation client](../specs/interfaces/automation-client-contract.md), [transactions](../specs/storage/transactions-concurrency-and-idempotency.md) |
| `V2DFC-015` | Provide confirmed destructive recovery and the newer-generation policy | Completed | Let explicitly confirmed reset and removal recover invalid current-generation Stores, refuse newer generations with actionable guidance, and record the generation-bump invariant. | [recovery](../specs/storage/recovery-retention-and-maintenance.md), [release and packaging](../specs/operations/release-and-packaging.md) |
| `V2DFC-016` | Report maintenance markers truthfully and close removal finalization | Completed | Emit distinct maintenance boundaries with recipes that can make progress, and reproduce the removal lock-unlink race before resolving it. | [errors and exit codes](../specs/interfaces/errors-and-exit-codes.md), [recovery](../specs/storage/recovery-retention-and-maintenance.md) |
| `V2DFC-017` | Restore qualification execution paths and re-measure the candidate | Completed | Give every required qualification a documented execution path, close test and fuzz gaps, and re-measure G1/G2, G7/G8, and G9 on the remediated candidate. | [testing and conformance](../specs/quality/testing-and-conformance.md), [performance qualification](../implementation-tips/direct-cli-performance-qualification.md) |
| `V2DFC-018` | Promote remediated documentation and contract corrections | Completed | Remove daemon-era normative claims and stale examples, complete the changelog, correct contract drift, and settle stale records before the final Epic audit. | [documentation authorities](../README.md), [requirements traceability](../specs/quality/requirements-traceability.md) |

### V2DFC-017 remediation qualification (October 11, 2026)

The first completion review identified a Store stale-fence workload omission,
an output-line boundary defect, missing tool failure-path sentinels, and a G9
drain clock starting after report collection. All four corrections passed
focused regression checks. Corrected Store qualification and the complete
source-bound development chain passed. The completion assessment confirmed the
corrected candidate, and the Task is accepted under the approved Epic execution
envelope. Task018 and the final Epic audit remain pending.
No numerical threshold failure or threshold change was recorded.

Native qualification used Apple M5 Pro (18 logical CPUs, 64 GiB), macOS 27.0.1
build 26A434, pinned Rust 1.97.1, release profile and observed AC power with
AC low-power mode off. Source was dirty on base
`4101a014a187d175e6c7cd53df9b671d613ba625`; no diagnostic override was used.
Measurements have separate frozen bases:

- Retained seven Engine cases: source
  `002f0a0bdbf33bea2f68efd70be087ca7d53a5f40ff2361c3f007054c64403b1`,
  suite SHA-256 `929b65933fdd378e1bea3b8da6a87f5681b9367ba479ed41f6664bba811c6429`.
- Corrected CLI G7/G8 and L lifecycle: source
  `d53789b648d9abb992da27e1439616207f33026a17d8c5d2bfcb98f8ab06b036`,
  CLI SHA-256 `a44fd4b3128420c192e9acc25812ccb69250e2cb457a4936c737bec225e83157`.
  The intervening qualifier repair removed unsupported session-reset `--force`.
- Corrected Store G1/G2 and G9, and fresh complete development chain: source
  `f1f9604ddf43a6f5b9055e56fb6c448acf8f7df8d1fcb04f6cd8a71000111081`,
  Store suite SHA-256 `1d7e7bbf085afb59c92b82187e4e4338c5de75ecfdded0a053f381181f41f96a`.
  The freshly checked/package-qualified CLI retained the exact SHA-256 above.

Source reconstruction proves the later six-path correction left production,
Engine qualification inputs, the CLI qualifier and schemas unchanged. Retained
Engine results belong only to their original frozen suite; an unselected newly
built Engine suite with a different hash is not covered by those results.
Earlier Store G1/G2 and G9 measurements are superseded. The development chain
began after required Store/300 measurements passed, while Store/1024 and G9
ran in separate fixtures; host load was not exclusive. Measured-record and
lifecycle metadata follow the frozen checks without changing executable inputs.

G1 used eight writers with 1,000 successes each, exact replays, two observers
and 20 warm-up successes per writer. Every grown main database exceeded
43,384,832 bytes; Engine used the same API-grown active session. Both sizes
passed serial replay and correctness. Stale preparations reject with no outcome
and retry under a fresh key. Engine loss, duplication, stale-fence commits,
Busy reaching the caller and observer violations were zero. G2 thresholds
apply at 300 nodes; 1,024-node latency is informational. Timing below is
p50/p95/p99/maximum in milliseconds.

- Store / 300: 40,765 measured samples (7,840 commits, 7,840 replays, 25,085 stale-fence rejections);
  wait 0.002/8.381/16.871/107.072 ms;
  hold 5.363/9.272/11.573/23.902 ms; writer fairness 1.070.
  Rejection outcomes and stale-fence commits: zero; G1/G2 pass.
- Store / 1024: 39,646 measured samples (7,840 commits, 7,840 replays, 23,966 stale-fence rejections);
  wait 0.002/15.363/30.545/492.070 ms;
  hold 7.469/13.604/16.789/507.636 ms; writer fairness 1.067.
  Rejection outcomes and stale-fence commits: zero; G1 pass; G2 informational.
- Engine / 300: 39,879 measured samples; wait 0.003/8.417/21.658/396.506 ms;
  hold 6.297/16.049/24.342/406.843 ms; writer fairness 1.063; G1/G2 pass.
- Engine / 1024: 40,521 measured samples; wait 0.003/11.646/28.456/514.327 ms;
  hold 9.331/20.018/29.547/522.954 ms; writer fairness 1.024; G1 pass;
  G2 informational.

Commit hold ends at `AfterCommitBeforeOutput`; replay/rejection hold uses typed
API return as a conservative upper bound. Passing that bound establishes the
hold limit without claiming exact rollback instrumentation. Retained Engine
cases passed G3, all five crash windows (20 SIGKILLs each), 200 random SIGKILLs,
G5 and G6, in addition to both G1/G2 sizes. G5 completed 100 cycles with four
writers/readers, 1,500 marker-step SIGKILLs and 596 acknowledged writes: zero
loss, marked/unexplained missing or unexpected outcomes. Its 1,500 acquisition
samples had p99 97.637 ms against the unchanged 2 s limit. G6 rejected all
500 state and 500 artifact preparation races.

G9 used eight writers/four readers for 600,004 ms: 13,560 successes, including
69 completes, and 10,574 WAL observations. Maximum observed active WAL was
21,147,992 bytes (limit 64 MiB). At 331 ms from stop-marker publication it was
4,041,752 bytes, satisfying 4 MiB within 5 s. This conservative endpoint includes
in-flight work and child/report cleanup; it is never restarted after cleanup.
The idle WAL connection stayed open and no post-load nudge writes ran. A real
one-second negative fixture with six-second observer cleanup verifies that a
late observation cannot pass by resetting the clock; it is not the required soak.

G7 matched S/M/L history with 20 warm-ups and 220 measured calls per cell.
Every p50/p99 met the unchanged recorded daemon comparator. Full-invocation
p50/p95/p99/maximum in milliseconds:

- S observe: 40.329/48.291/50.858/51.597 ms; Pass.
- S set: 71.498/86.680/90.536/102.939 ms; Pass.
- S record: 48.437/63.412/68.272/86.662 ms; Pass.
- S complete: 48.677/63.707/71.999/78.094 ms; Pass.
- M observe: 40.555/54.789/66.648/78.423 ms; Pass.
- M set: 72.241/88.859/113.815/154.427 ms; Pass.
- M record: 49.585/67.100/82.538/113.122 ms; Pass.
- M complete: 50.331/68.200/113.163/142.034 ms; Pass.
- L observe: 76.645/87.436/103.469/130.165 ms; Pass.
- L set: 109.952/123.423/142.542/147.245 ms; Pass.
- L record: 87.938/102.766/125.268/133.238 ms; Pass.
- L complete: 87.445/102.983/119.576/127.411 ms; Pass.

G8 restored genuine mode tuples and matched L/E1 prehistory, with 20 warm-ups
and 220 measured calls per cell. Full invocation is a conservative open-cost
upper bound, rather than isolated-open instrumentation. Required L results,
p50/p95/p99/maximum in milliseconds:

- L ro-warm: 24.606/25.856/28.083/70.060 ms; p99 limit 100 ms; Pass.
- L ro-cold: 26.120/27.634/29.366/39.748 ms; p99 limit 1000 ms; Pass.
- L rw-warm: 22.360/25.118/27.225/63.844 ms; p99 limit 100 ms; Pass.
- L rw-cold: 23.475/24.444/25.480/71.926 ms; p99 limit 1000 ms; Pass.

L lifecycle restored the original grown image per tuple; archive followed
cancellation and explicit not-required disposition, and start followed archive.
Each command had 20 warm-ups and 220 measured calls. These full-invocation
costs are informational; no lifecycle comparator or total-call limit was invented.

- reset: 131.702/140.116/252.917/749.823 ms; informational.
- archive: 121.599/131.331/168.170/192.605 ms; informational.
- start: 50.842/58.439/75.226/222.545 ms; informational.

The corrected complete CLI run passed canonical shipped validation for every
successful response, retained frozen executable hashes and left zero owned
children. Its first attempt stopped at invalid lifecycle argv before final
validation; it is not an overall pass or a threshold failure. The freshly frozen
corrected source passed `make test`, `make release-prepare`, all seven fuzz
targets (100,000 runs each), the locked release CLI build and the source-bound
development package, qualification, local handoff and bundle verification.
All 16 packaged-conformance scenarios passed. Its artifact class is
`development-qualification`, nonpublishable. Private receipts/logs remain local
runtime evidence. Task018 and final Epic acceptance are separate. These checks
establish no release, installation, activation or consumer acceptance.

### V2DFC final audit maintenance correction (October 12, 2026)

The first full direct audit found a maintenance recovery diagnostic race owned
by `V2DFC-016`. A read had already refused maintenance, but secondary recovery
diagnostics replaced that refusal with `WORKSPACE_STATE_UNREADABLE` when native
Git metadata changed during final removal. The bounded correction is active
within the continuing Epic audit. Completed member states retain their accepted
history; the roadmap has no reopen state for an earlier completed member while
later members remain Completed.

The failed native run used clean source
`ba98bd9784824b8218a67c0d0a38d719484d9c7e`, snapshot
`799c94e60d166eacd6d708f6062fe49ed0575a3a5abc95973500311329219eb3`,
Apple M5 Pro (18 logical CPUs, 64 GiB), macOS 27.0.1 build 26A434, AC power,
release profile and pinned Rust 1.97.1. Host load was not exclusive. Store suite
SHA-256 was `dd85f253fd7c3cbc27a29c24471478d712bc91c8ff4e65915c4c69a7ceb8cd3d`;
Engine suite SHA-256 was `2acc83279cfdb4de14e40966bf9c68e0fb0bff1bd223772919af6c4e65f1636b`.

Store and Engine G1/G2 at both sizes, the deadline check, all five crash
boundaries and random SIGKILL recovery passed. G2 at 1,024 nodes remains
informational. G5 completed 100 cycles and 1,500 killed attempts with four
writers and four readers: 755 acknowledgments, zero lost outcomes, zero marked
or unexplained missing responses, and acquisition p99 79.156 ms over 1,500
samples. Two unexpected state-unreadable responses failed its correctness gate.
The run stopped after 97 minutes 18.738 seconds; G6 and G9 were not executed.
Both frozen binary hashes and the source snapshot remained unchanged. All owned
children were reaped and the exact disposable G5 worktree was removed.

The correction preserves the original maintenance refusal and uses read-only
writer diagnosis when the recovery snapshot changes. A permanent NativeGit and
Engine regression first failed with the same public error, then passed unchanged
after the fix. It replaces real Git roots at two diagnostic intervals, validates
both recovery boundaries and proves no Store effect or new-root initialization.
The correction requires `make test` before its completion commit. Changed-candidate
native qualification, current packaging, matched G7/G8, whole-Epic review and
actual owner acceptance remain separate obligations; no threshold was relaxed.

### V2DFC corrected-candidate qualification (October 12, 2026)

The Task016 maintenance diagnostic correction passed its permanent NativeGit
regression, the complete development gate and its owned correction commit.
Fresh full native qualification then passed all nine registered cases on clean
source `1f78827394b1337f31869aa4d0be2d52ae9cb26d`, snapshot
`9cb39b845c3bdf77a00aa95ec786e5e817a36633c0913bd455469d7a8267e180`.
The earlier failed run remains historical failure evidence; it is not relabelled
as a pass. No threshold or workload was relaxed.

The host was Apple M5 Pro (18 logical CPUs, 64 GiB), macOS 27.0.1 build 26A434,
with observed AC power, pinned Rust 1.97.1 and release profile. Initial load
averages were 3.998/3.696/6.966; host load was not exclusive. Store suite SHA-256
was `dd85f253fd7c3cbc27a29c24471478d712bc91c8ff4e65915c4c69a7ceb8cd3d`;
Engine suite SHA-256 was
`83c93c97b76de7caa00b9fdb49afd3e7c421fbdde414ffa4e198097ec23c8e22`.
Source and both frozen suites remained unchanged. All required cases completed
in 99 minutes 14.497 seconds; bounded owned children were reaped and the
maintenance harness's disposable worktree was removed.

G1 used eight writers with 1,000 successes each, two observers, exact replays
and stale-fence rejections on grown L-scale Stores at both sizes. Each writer
had 20 warm-up successes. Engine retained the same API-grown active session.
Every initial main database exceeded 43,384,832 bytes. G1 and serial replay
passed at both sizes, with zero rejection outcomes, stale-fence commits,
Engine loss, duplication, Busy reaching the caller or observer violations.
G2 gates apply at 300 nodes; 1,024-node latency is informational. Percentiles
below are p50/p95/p99/maximum in milliseconds.

- Store / 300: 38,954 measured samples; wait 0.003/12.066/24.908/171.076 ms; hold 5.963/12.326/18.472/471.025 ms; writer fairness 1.216; G1/G2 pass.
- Store / 1024: 38,948 measured samples; wait 0.004/21.527/44.224/404.019 ms; hold 9.476/18.717/29.213/475.814 ms; writer fairness 1.134; G1 pass; G2 informational.
- Engine / 300: 38,362 measured samples; wait 0.003/7.492/19.117/466.577 ms; hold 6.217/13.784/19.347/467.802 ms; writer fairness 1.098; G1/G2 pass.
- Engine / 1024: 38,992 measured samples; wait 0.002/9.013/20.975/80.746 ms; hold 8.088/15.615/20.876/220.787 ms; writer fairness 1.061; G1 pass; G2 informational.

Store measured 7,840 commits and 7,840 replays per size, plus 23,274 stale-fence
rejections at 300 nodes and 23,268 at 1,024. Engine includes 8,000 commits and
8,000 replays in its transaction counters, including warm-ups; rejected counts
were 22,683 and 23,313. Hold ends at `AfterCommitBeforeOutput` for commits;
replay and rejection use typed API return as a conservative upper bound.

The deadline check, all five crash windows (20 SIGKILLs each), 200 random
SIGKILLs, G5 and G6 passed. G5 completed 100 reset/remove cycles, 1,500 killed
attempts and 1,500 acquisition samples with four writers and four readers:
1,110 acknowledgments, zero lost outcomes, zero marked or unexplained missing
responses and zero unexpected responses. Acquisition p99 was 32.591 ms against
the unchanged 2 s limit. G6 rejected all 500 state and 500 artifact races.

G9 ran eight writers and four readers for 600,001 ms, recording 13,687
successes, including 66 completes, and 10,735 WAL samples. Maximum active WAL
was 21,840,152 bytes against the 64 MiB limit. At 237 ms from stop-marker
publication it was 103,032 bytes, satisfying 4 MiB within 5 s. The idle WAL
connection remained open. The drain endpoint conservatively includes in-flight
work and child/report cleanup; it is never restarted after cleanup.

The same unchanged clean source passed the complete frozen development chain:
`make test`, `make release-prepare`, all seven fuzz targets (100,000 runs each),
the pinned locked release CLI build, development packaging, all 16 packaged
conformance scenarios, local handoff creation and final bundle verification.
The chain completed in 19 minutes 55.371 seconds. Its artifact class is
`development-qualification`, nonpublishable. Packaged qualification disabled
hooks and removed all exact owned children and worktrees. Manifest digest was
`sha256:c7302b444f34e8f38e940a5290d40b16afc0b8837309e3ea831f6a6a30a18edc`.
The extracted packaged CLI and checked build both had SHA-256
`c4d0d23bb692993033e555dde2d913e10ed5d55b687a999a14f984ea2fe0ed81`.
The local handoff is contract-delivery evidence only; it does not establish
consumer adoption. No release, installation or runtime activation occurred.

Matched G7/G8 used that extracted packaged CLI and the qualified Engine suite
above on the same clean source, with 20 warm-ups and 220 measured calls per
cell. S/M/L block history, genuine original mode tuples and L/E1 prehistory
followed the unchanged performance guide. No daemon was executed for the
comparison. Every required G7 p50/p99 met its recorded daemon comparator.
Full-invocation p50/p95/p99/maximum in milliseconds:

- S / observe: 41.757/52.087/54.000/55.918 ms; 220 measured samples; pass.
- S / set: 47.909/63.504/76.041/91.197 ms; 220 measured samples; pass.
- S / record: 49.527/66.918/72.902/75.661 ms; 220 measured samples; pass.
- S / complete: 48.199/63.230/69.328/102.593 ms; 220 measured samples; pass.
- M / observe: 41.875/51.039/52.734/57.455 ms; 220 measured samples; pass.
- M / set: 46.760/61.858/65.503/67.354 ms; 220 measured samples; pass.
- M / record: 49.614/64.080/67.457/70.175 ms; 220 measured samples; pass.
- M / complete: 48.452/62.825/65.834/66.529 ms; 220 measured samples; pass.
- L / observe: 76.090/86.179/88.985/92.213 ms; 220 measured samples; pass.
- L / set: 83.623/97.389/101.677/109.622 ms; 220 measured samples; pass.
- L / record: 86.101/100.845/104.829/110.643 ms; 220 measured samples; pass.
- L / complete: 84.715/98.579/103.100/108.826 ms; 220 measured samples; pass.

G8 full invocation is a conservative upper bound on open cost. All four
required L cells passed the unchanged p99 limits: 100 ms warm and 1 s cold.
S/M cells are informational, including the M read-write warm tail; no limit
is assigned to them by the registered method.

- S / ro-warm: 24.581/26.155/27.358/28.800 ms; 220 measured samples; informational.
- S / ro-cold: 26.333/27.933/28.934/29.896 ms; 220 measured samples; informational.
- S / rw-warm: 23.471/25.951/32.884/33.450 ms; 220 measured samples; informational.
- S / rw-cold: 23.737/25.112/25.656/34.403 ms; 220 measured samples; informational.
- M / ro-warm: 24.687/26.362/27.923/31.197 ms; 220 measured samples; informational.
- M / ro-cold: 26.433/28.051/29.562/30.942 ms; 220 measured samples; informational.
- M / rw-warm: 25.439/31.291/249.319/256.556 ms; 220 measured samples; informational.
- M / rw-cold: 24.229/25.641/26.878/26.878 ms; 220 measured samples; informational.
- L / ro-warm: 25.629/29.202/30.804/34.852 ms; 220 measured samples; pass.
- L / ro-cold: 26.706/28.452/30.085/33.586 ms; 220 measured samples; pass.
- L / rw-warm: 25.429/37.549/85.251/160.218 ms; 220 measured samples; pass.
- L / rw-cold: 23.897/30.261/34.203/36.772 ms; 220 measured samples; pass.

The L lifecycle restored the original grown image before each tuple; archive
followed cancellation and explicit not-required disposition, and start followed
archive. Each command had 20 warm-ups and 220 measured calls. Full-invocation
costs are informational, with no lifecycle comparator or total-call limit:

- reset: 154.230/194.573/571.079/608.766 ms; 220 measured samples; informational.
- archive: 141.772/168.331/275.405/737.209 ms; 220 measured samples; informational.
- start: 57.256/71.621/109.817/166.146 ms; 220 measured samples; informational.

The matched CLI run completed in 14 minutes 25.863 seconds. Every successful
response passed the canonical shipped validator; frozen CLI/helper hashes
remained unchanged and zero owned children remained. Isolated measurement
fixtures were retained as local evidence. Earlier Task017 measurements retain
their original source identity; this record supplies the corrected final-audit
basis. At this measured-record commit, all member Tasks were Completed and
the Epic remained In Progress. Whole-Epic Independent Review and actual Master
final acceptance were pending; the final closeout below records their settlement.

### Reopened Epic closeout

The owner accepted the completed reopened Epic and this exact documentation
closeout. All eighteen member Tasks are Completed. The
accepted technical baseline is `2a381487305eccb9d5c93e0a4f4bf7e9425b1e59`,
including the maintenance race correction in `1f78827394b1337f31869aa4d0be2d52ae9cb26d`
and the current measurement record above. The full Epic audit found no current
correctness, contract, completion, or required-evidence gap.

Three fresh Independent Review reports assessed the complete original member
requirements, nine cross-task seams, direct execution traceability, and retained
domain and delegation behavior. Every applicable criterion was met; no actionable
finding, unmet criterion, or unverified criterion remained. Reviewers inspected
source and supplied execution receipts; they did not execute the checks themselves.
The first whole-Epic review completed cleanly, so no additional review was required.

The native nine-case matrix, complete development gate, bounded fuzzing, sixteen
packaged scenarios, local handoff and bundle verification, and matched packaged
CLI G7/G8 checks passed on the unchanged clean implementation baseline `1f78827`.
Commit `2a38148` changes only the required roadmap measurement record; the accepted
baseline retains identical executable, contract, test, fixture, and version bytes.
The final closeout changes only roadmap and TODO lifecycle documentation and passed
focused documentation preparation checks. Earlier failed qualification and its
corrected source identity remain recorded; no threshold or acceptance scope changed.

Last-consumer inspection found only this Epic's roadmap link and the TODO index
entry. Lasting requirements already belong to ADR-0038, the linked specifications,
architecture and quality contracts, and the
[performance qualification](../implementation-tips/direct-cli-performance-qualification.md)
and [release guide](../implementation-tips/release.md). The adopted implementation
dossier and its index entry are retired in this closeout. Independent future work
remains in the [deferred-feedback register](../deferred-feedback/README.md).

This is development completion. Packaged qualification used the nonpublishable
`development-qualification` class; no stable distribution gate, publication,
installation, runtime activation, external consumer modification, or fresh UTM
qualification is claimed. The conditional `v0.2.13` release and its Aquarium and
Dolgorae prerequisites remain separately authorized. No additional release-note
entry is required for this documentation-only closeout.

Tasks execute in numeric order with one active task across this epic. At most
the first incomplete task may be In Progress, In Review, or Blocked; every later
task remains Planned. Each task is one logical implementation-and-verification
commit that passes `make test`, subject to separate commit authorization. If
any feasibility check in `V2DFC-001` fails, that task becomes Blocked and the
epic returns to the owner; there is no automatic fallback. No patch release is
cut from the main branch between `V2DFC-007` and `V2DFC-009`. Development
acceptance does not authorize a release, installation, or removal of an
installed daemon.

Epic completion includes the full Epic audit, independent completion review,
and separately accepted Epic-owned closeout. The last-consumer implementation
dossier is retired and its links point to durable authorities. Development
completion does not authorize release or installation.
