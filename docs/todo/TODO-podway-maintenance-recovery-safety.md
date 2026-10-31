# Podway Maintenance and Recovery Safety

## Status and authority

- Document state: `Adopted`
- Owning roadmap epic: [MAINT](../roadmap/README.md#maint-maintenance-and-recovery-safety)
- Target product release: not selected
- Repository scope: Podway only
- Detailed SOT for: MAINT

This dossier owns the decision-complete plan for unfinished MAINT work. The
[roadmap](../roadmap/README.md#maint-maintenance-and-recovery-safety) alone owns
identity, order, dependencies and status. Accepted ADRs, canonical machine assets
and specifications continue to own implemented behavior. Intended changes below
become implemented authority only with their owning tasks. Registration implements
no behavior and grants no commit, release, installation or consumer-change authority.

## Verified context and evidence

ADR-0038 establishes direct, worktree-local maintenance with no daemon or global
state. The current maintenance specification already refuses newer and
indeterminate committed WAL generations. A marker diagnoses an interrupted
operation; it cannot authorize deletion of a generation that cannot be established.

Current source contains complete but repeated expected-UUID/durable-selector
fence checks in Engine maintenance and resolver admission; CLI confirmation
separately tests its representable fence. Store owns persisted-data error mapping,
while Engine lists recovery-eligible Direct Store errors. Both initialization and
existing-workspace resolution return Maintenance for `Ok(Maintenance)` when
configuration is invalid. Their difference concerns classification failure:
initialization classifies before checking configuration and propagates `Err(e)`;
existing-workspace resolution probes after a configuration error and retains that
original error unless classification returns `Ok(Newer)` or `Ok(Maintenance)`.
These are ownership improvements, not evidence of a current bypass.

Removal already holds descriptor and runtime-inode witnesses, validates exact
markers and protects a fresh successor. The runtime-absent empty-residual path can
fail on concurrent disappearance. Lone-removal-marker restoration validates the
marker, changes its schema, removes `phase`, validates the intent schema and
canonicalizes the result; it does not reconstruct every identity field. Existing
migration checkpoint code guards the database and lease before and after native
TRUNCATE, then syncs the database and directory; its private fault intervals need
deterministic coverage.

Verification owners are the registered Engine maintenance integration suite,
Store maintenance and predecessor checkpoint tests, Store classification tests,
and native direct-CLI maintenance tests. They establish current behavior alongside
the canonical specifications, rather than runtime paths or provider reports.

### Source feedback

These entries preserve the original source, deferral rationale and revisit
condition. All twelve are adopted by MAINT; promotion closes their deferral,
not implementation or acceptance. Only their remaining work is adopted.
Task mappings below and the roadmap identify the implementation owners.

DF-033 and DF-042 retain their original historical wording. The current-source
qualifications above correct the Maintenance/error distinction and describe the
validated-marker conversion without rewriting either original rationale.

| ID | Source | Feedback | Deferral reason | Revisit condition |
|---|---|---|---|---|
| DF-029 | V2DFC-015 maintainability review | Workspace UUID and durable-selector fences are enumerated in both the general guard and invalid-Store recovery admission. | Both current predicates include the same two fences, and native copied-Store reset/removal regressions refuse durable selectors without effects. Consolidation is future drift prevention, not current recovery correctness. | Before adding or changing an identity fence, give the complete fence set one Engine owner and verify both valid-Store guards and invalid-Store recovery admission. |
| DF-030 | V2DFC-015 security review | The existing interactive removal prompt renders control characters in a worktree path without escaping them. | Exact-byte path confirmation binds deletion to the descriptor-validated target and cannot redirect it; clearer display is independent prompt hardening. | Before changing removal confirmation rendering, escape control characters visibly while preserving exact target identity and test unusual native path bytes. |
| DF-031 | V2DFC-015 security review | An unmarked checksum-valid WAL with a committed page-one image lacking a SQLite header is refused as unreadable, without destructive recovery advice. | That image does not provide a trustworthy effective generation. Markers remain diagnostically visible but do not authorize deletion of an indeterminate generation; destructive admission refuses marked and unmarked unknown committed page-one images. Defining recovery for those images requires a separate policy, rather than treating a possibly unsupported Store as current. | Before broadening unreadable-State recovery, define a generation-safe policy for unmarked corrupt WAL images and prove that newer Stores cannot be deleted or opened. |
| DF-032 | V2DFC-015 testing review | Checkpoint tests do not inject a database or lease replacement during the native checkpoint, or an fsync failure immediately after TRUNCATE. | Native external-reader Busy, unchanged main bytes, committed retry, no duplicate journal or clock, and exclusive failed-read witness tests cover current acceptance. Pre/post descriptor guards and zero-WAL retry sync preserve the suffix; precise mid-call fault injection is independent regression hardening. | Before changing checkpoint or durability suffix logic, add deterministic private fault injection at these boundaries and verify changed witnesses fail closed and post-TRUNCATE retry completes sync without duplicate effects. |
| DF-033 | V2DFC-015 ordinal-two maintainability review | Classification precedence after invalid configuration is invoked in both workspace init and existing-workspace resolution. | Both refuse a known newer Store; their Maintenance-arm difference preserves each command's existing admission policy. There is no current divergent newer-generation behavior. | Before adding another precedence variant or caller, consolidate the resolver-owned probe while preserving caller-specific Maintenance policy and test init/status/reset/removal on invalid configuration. |
| DF-034 | V2DFC-015 ordinal-two maintainability review | Invalid-current read errors are classified by Store while the Engine separately enumerates errors eligible for unfenced recovery. | Narrow corruption mapping, generation admission, identity fences and native invalid-data regressions satisfy current behavior; a missed future error category would fail closed. | Before adding another recovery-eligible error kind, expose the Store-owned corruption classification explicitly and verify that IO, Busy, changed witnesses and unknown generations remain excluded. |
| DF-036 | V2DFC-015 ordinal-three maintainability review | CLI confirmation and Engine recovery separately identify an unfenced request. | The CLI supplies no durable selector identity, so both predicates agree today and the native prompt tests enforce current behavior. | Before adding a CLI identity fence or a durable selector, give the complete fence predicate one Engine owner and verify both confirmation and invalid-Store recovery. |
| DF-039 | V2DFC-015 ordinal-three product review | Interactive removal requests exact-path confirmation before reporting that a newer Store cannot be removed. | Newer state is refused before effects with compatible-version guidance; avoiding the unnecessary prompt is independent workflow refinement. | Before changing destructive confirmation order, consider an Engine-owned read-only compatibility precheck while retaining authoritative post-confirmation admission and unchanged-file refusal. |
| DF-040 | V2DFC-015 ordinal-three testing review | Deterministic replacement injection does not cover every interval between destructive-generation guards. | Held file witnesses, checks before and under the lease and before destruction, and native newer-WAL refusal cover current acceptance; no missed guard or current race failure is established. | Before changing guard placement, inject database and WAL replacement at the existing maintenance boundaries and verify unsupported-generation or changed-witness refusal without deletion. |
| DF-041 | V2DFC-016 ordinal-one logic review | Concurrent empty-residual removals after the runtime child is already absent can report a disappearing-path failure before an exact retry converges. | Marked-runtime cleanup holds the runtime inode anchor through final unlink; missing-parent races fail closed and do not delete a fresh successor. Root-only residual convergence is an independent refinement, outside the reproduced named-lock race. | Before changing runtime-absent residual admission, deterministically contend two residual removals and fresh init, retaining descriptor identity checks and successor protection while defining absence convergence. |
| DF-042 | V2DFC-016 ordinal-two maintainability M1 | Lone-removal-marker restoration repeats closed intent fields rather than reusing the canonical intent constructor. | Current exact-retry conversion validates both schemas and preserves the original response identity and time; native interruption/resume gates pass. Consolidation is independent future schema-maintenance work. | Before changing intent fields or schema identity, consolidate marker construction while retaining exact response/time, request digest and writer/root fences, and run lone-marker retry regressions. |
| DF-043 | V2DFC-016 ordinal-two product P1 | Human unforced removal of a prepared/running session reports the safe `REQUEST_INVALID` refusal without a specific force hint. | Refusal occurs before intent or filesystem effects; session-less admitted unforced removal and recovery preserve their original authority. The closed empty error details remain compatible. | Before refining human destructive-refusal presentation, provide an explicit authorized-force hint without widening JSON authority or bypassing confirmation, and test prepared/running no-effect refusal. |


## Goals, non-goals and scope

Improve ownership of maintenance admission, safe human operation and deterministic
verification of destructive and durability boundaries. Adopt all twelve source
entries, including an explicit decision to retain indeterminate-WAL refusal.

Do not add automatic repair, conversion, backup/export, a production predecessor,
new recovery authority, runtime services, performance benchmarks or installation.
Do not expand CLI grammar, public JSON families, error codes/details, Store layout,
marker schemas or the lock protocol. Preserve success-only atomic outcomes,
original deadlines, bounded traversal, non-UTF-8-safe internal paths and exact replay.

## Accepted design decisions and interfaces

### Shared admission decisions

Engine owns the complete set of request identity fences, including explicit
workspace UUID and selector-derived durable identity. Both valid-Store comparisons
and invalid-Store recovery use that owner. CLI asks Engine whether its translated
maintenance request is fenced instead of maintaining an independent policy.
Store exposes its existing invalid-current classification as a typed query;
Engine still combines it with request authority and generation admission.
Writer/schema/binding/unreadable current-state cases retain existing eligibility.
Busy, operating-system I/O, changed witnesses, foreign markers and unsupported or
unknown generations remain excluded. This is internal Rust reuse, not a public
wire contract or a broader error taxonomy.

Resolver owns one invalid-configuration classification probe. Share observations
while preserving each caller's existing error selection. With invalid
configuration present, retain this matrix:

| Classification result | Initialization | Existing-workspace resolution |
|---|---|---|
| `Ok(Current)` | Configuration error | Configuration error |
| `Ok(Newer)` | Unsupported-generation error | Unsupported-generation error |
| `Ok(Maintenance)` | Maintenance error | Maintenance error |
| `Err(e)` | Propagate classification error `e` | Retain original configuration error |

`Ok(Maintenance)` and `Err(DirectStoreErrorV1::Maintenance)` are distinct.
Do not impose unconditional propagation or suppression on both callers, or extend
one caller's policy to reset/removal. The probe creates no SQLite connection,
SHM, lease or marker. No new policy framework or behavior change is selected.

### Generation-safe recovery policy

A checksum-valid committed WAL page-one image without a trustworthy SQLite header,
or an unresolved valid stream beyond the scan budget, cannot establish a supported
generation. Reset and removal refuse it with or without a marker. They do not
infer current generation from the main file or from confirmation/force. Guidance
preserves bytes and points to Store-independent writer diagnosis; it offers no
procedure for deleting indeterminate state. Existing broader invalid-current
recovery advice is explicitly conditional on successful generation admission.
No new error code, JSON detail shape, manual marker edit or recursive deletion
recipe is introduced. Supporting such recovery later requires a separate policy.

### Confirmation and guidance

Add an Engine-owned read-only compatibility precheck before interactive removal
confirmation. It uses existing bounded native generation inspection, opens no
SQLite connection, creates no lease/lock/marker/SHM and consumes the invocation's
existing deadline. Reuse the resolver's established precedence. A positive precheck
is advisory, conveys no destructive authority and does not cache away subsequent
checks: execution revalidates after confirmation and under its exclusive lease.
Keep noninteractive flag validation and existing JSON refusal semantics.

Render path control characters visibly without changing the resolved target or
comparing the answer against an escaped alias. Preserve exact original-path input
comparison only for paths that round-trip through the existing single-line input.
Refuse non-round-trippable paths clearly and without destructive effects,
including embedded LF and trailing CR/LF; escaping the display does not make those
paths confirmable. Preserve the current non-UTF-8 interactive refusal. Do not add
normalization, lossy matching, escape aliases, shell evaluation or a new
confirmation syntax.

Guidance may name the existing fenced noninteractive alternative, subject to all
UUID, generation, writer and recovery admission rules. It is not a guaranteed
workaround: invalid state may supply no trustworthy UUID, and a fenced request
may still be refused. Never suggest inventing a fence or bypassing admission.

For unforced removal refused because a prepared/running session requires force,
show a human-only hint naming an explicit new removal request with `--force` and
normal confirmation. Select that hint from the typed ForceRequired cause observed
in the same execution; do not infer it from generic `REQUEST_INVALID`, match human
messages or make a second independent state query. Prefer the existing typed-error
observer with minimal invocation-context wiring; a new error taxonomy or generic
metadata framework is unnecessary. Unrelated `REQUEST_INVALID` failures receive
no force hint. Preserve the public envelope, codes, details, exits and retryability.
Do not imply permission, retry automatically or add force to an exact retry of a
published operation.

### Fault and concurrency boundaries

Fault hooks are explicit private/test instrumentation using existing test-support
patterns. They must not become environment-driven production behavior or wire
fields. Coordinate faults with barriers/handshakes, not sleep-based race guesses.
Checkpoint tests must replace DB/lease during the native checkpoint window, not
merely on entry/exit, and inject sync failure after a successful TRUNCATE.
Use isolated declared-predecessor fixtures without adding a production predecessor.
At the start of MAINT-004, identify a suitable existing seam or the smallest
test-only extension and explain how its evidence proves the injection occurred
inside the native call. This is bounded implementation work within MAINT-004,
not a separate research task or a mandate for a new VFS, wrapper architecture or
general fault framework. Entry/exit-only injection does not satisfy the boundary.

Retain all existing destructive generation checks and descriptor guards. Introduce
no new lock-file protocol. Residual disappearance may converge only after native
revalidation proves absence under the same canonical worktree and Git identity.
A changed/repopulated target, unknown residual contents or a fresh runtime causes
no further deletion. Reinspection must not turn a new successor into an empty
residual target, silently rebind an old request or renew its deadline.

Keep lone-removal intent restoration based on the current validated-marker
conversion. Share construction or validation only where it simplifies ownership;
avoid a generalized builder or normalization of validated information. Preserve
the original marker identity, response ID and recorded time. Restoration must not
consult the clock or generate a new response, and both closed schemas remain
validated.

## Failure handling and compatibility boundaries

- Newer/unknown generation refusals preserve bytes and file identities and create
  no SHM, lease or operation marker at initial admission. Later injected changes
  preserve the replacement and cause no destructive suffix or successful outcome;
  any already published owned intent remains governed by existing exact recovery.
- A failed sync does not report success. After committed migration, exact retry
  verifies the original committed identity and completes required sync even with
  zero WAL; it adds no journal/state effect or clock read.
- An unchanged eligible invalid current Store remains recoverable only through
  existing confirmed, unfenced authority. Fenced requests keep their original
  rejection. Root/writer/request/UUID differences never adopt an old marker.
- Complete removal destroys outcome retention. A later absence result is
  idempotent convergence, not a claim to replay a destroyed original response.
- Preserve existing result fields, error exit/retryability, canonical bytes and
  digests, UUID interpretation and caller-specific precedence. Human prompt and
  hint changes are the intended output deltas.
- If delivery would require a maintenance protocol change, stop and obtain a new
  adopted design and the required Store generation change. Do not quietly expand
  this Epic or weaken guards to satisfy a concurrency test.

## Roadmap ownership, dependencies and traceability

The following contracts map to roadmap tasks without duplicating lifecycle status.
Each task consumes the predecessor's accepted result and updates affected durable
owners before review. Every task retains original source-feedback obligations.

### Admission ownership

`MAINT-001` owns DF-029, DF-034 and DF-036. Deliver the shared complete Engine
fence predicate, Store-owned classification and CLI use. Verify explicit-only,
selector-only, absent, matching and conflicting fences on valid and invalid state;
IO/Busy and changed-witness cases must remain ineligible. Update System Architecture
and maintenance admission guidance with the actual resulting ownership.

### Compatibility diagnosis

`MAINT-002` owns DF-031 and DF-033. Deliver the resolver probe and the retained
WAL-refusal policy with conditional safe guidance. Exercise init/status/reset/remove
against invalid configuration with current/newer/maintenance cases and at least
one representative classification failure. Preserve the matrix above: init
propagates classification failure, existing resolution retains the configuration
error, and `Err(Maintenance)` must not be collapsed into `Ok(Maintenance)`.
Reset/removal retain their command-specific admission and destructive authority;
the shared probe creates no SQLite connection, SHM, lease or marker. Cover marked
and unmarked indeterminate committed page-one images and the scan-budget boundary.
Update maintenance and SQLite specifications without advertising a new recovery
route.

### Confirmation and guidance

`MAINT-003` owns DF-030, DF-039 and DF-043. Deliver the advisory precheck and
human-only display/hints. Native TTY tests prove that known newer state refuses
before prompting, a replacement after confirmation still refuses, controls are
visible while only supported exact original input confirms, and display escape
aliases do not confirm. Embedded-LF and trailing-CR/LF paths fail closed without
destructive effects; non-UTF-8 stays rejected. Prepared/running unforced removal
has no intent/effects and names explicit force from its same-execution typed cause;
unrelated `REQUEST_INVALID` failures receive no such hint. Preserve public envelope,
exit and retryability semantics without another state query. Update CLI and
maintenance guidance.

### Checkpoint durability

`MAINT-004` owns DF-032. First identify the smallest suitable private seam and
record how the test proves replacement during the native checkpoint call rather
than before entry or after return. Add only the test instrumentation needed for
that proof and post-TRUNCATE sync faults in real isolated declared-predecessor
fixtures. DB/lease replacement during the call fails witness validation;
post-TRUNCATE fsync failure is reported and retry completes required sync even
with zero WAL, preserving recorded identity/time without duplicate effects.
Retain native external-reader Busy and unchanged-main refusal regressions.
No production predecessor, environment switch, public field, general fault
framework or timing-based race guess is introduced. Document actual regression
owners in testing guidance; no separate mandatory spike is added.

### Destructive generation fences

`MAINT-005` owns DF-040. Inject database/WAL replacement before lease admission,
under exclusive admission and before runtime destruction. Cover newer generation,
changed inode and indeterminate committed page-one images, with markers where
applicable. Assert replacement survival, no successful rejection outcome and no
unauthorized deletion. Retain lease and marker correctness and name the registered
regressions in testing guidance.

### Residual removal convergence

`MAINT-006` owns DF-041. Reproduce two concurrent runtime-absent residual removals
and fresh init using explicit barriers. Make disappearance of the same owned empty
target converge after descriptor-relative revalidation; preserve original output
field meanings. Both removals can converge if no successor appears. With fresh
init or changed ancestry, preserve the successor and fail closed rather than
traversing it; an old UUID never authorizes its deletion. Preserve nonempty/symlink
refusal and the original wait budget. Update maintenance behavior and test owners.

### Exact marker construction

`MAINT-007` owns DF-042. Keep consolidation proportional to the existing validated
marker-to-intent conversion; share construction or validation only where ownership
becomes simpler. Retain request fields, digest, root/writer/UUID fences, response ID
and recorded time byte-for-byte where contractually identical, without a generalized
builder or normalization. Run interruption, changed-force/fence and lone-marker
exact retry regressions. Update marker ownership and recovery guidance without
changing its schema or protocol; preserve the historical DF-042 source entry.

### Integrated qualification

`MAINT-008` integrates all task contracts. Run the combined native CLI and Store/
Engine matrix and required development gate on the assembled candidate. Update
[testing guidance](../implementation-tips/testing.md) with lasting regression owners
and all original feedback details/revisit conditions before dossier retirement.
Complete affected maintenance, SQLite, CLI and architecture documentation. Inventory
all consumers of this dossier and prepare exact replacement links and deletion
scope; leave retirement and Epic completion to later whole-Epic assessment and
explicit owner acceptance. No packaged/release or installed-runtime claim is due.

## Verification and release acceptance

Use registered crate `int_suite` exact-test routes and repository Make targets.
Route dynamic exact Rust tests and long/noisy checks through the configured Gaori
commands; use focused checks first and `full` for the required `make test` gate.
A behavior-changing task must reproduce its failure when practical and pass its
focused regression plus the required development gate before sharing a revision.

Native CLI acceptance uses owned isolated Git fixtures and bounded child cleanup.
Assert result fields/error semantics, file bytes and inode witnesses, marker and
outcome absence/presence, exact original replay and successor survival as applicable.
Mocked results and source/prose matching do not establish the native fault cases.
Use deterministic coordination rather than performance measurements.

Design registration itself changes only documentation: read back all five files,
check links/anchors, task membership and all twelve feedback mappings, run
`make architecture-static` and `git diff --check`, then inspect status and the full
diff. Ouroboros QA and independent Design assessment precede exact-diff approval;
recheck approved target bytes before application. Registration is not implementation
acceptance, and requires no `make test`, `make dist` or installation.

No release version, distribution gate or upgrade/install operation is selected.
Future release work retains its separately authorized full/patch gate selection.

## References

- [Documentation authority](../README.md#precedence)
- [TODO lifecycle](README.md#lifecycle)
- [Direct execution authority](../architecture-decision-records/0038-remove-the-daemon.md)
- [Recovery, retention and maintenance](../specs/storage/recovery-retention-and-maintenance.md)
- [Transaction contract](../specs/storage/transactions-concurrency-and-idempotency.md)
- [SQLite model](../specs/storage/sqlite-model.md)
- [CLI specification](../specs/interfaces/cli-specification.md)
- [System Architecture](../architecture/system.md)
- [Testing guidance](../implementation-tips/testing.md)
- [Deferred feedback register](../deferred-feedback/README.md)
