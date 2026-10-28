# Podway Maintenance and Recovery Safety

## Status and authority

- Document state: `Candidate`
- Working label: MAINT
- Owning roadmap epic: none
- Target product release: undecided
- Repository scope: Podway only

This working label groups proposed work; it reserves no roadmap ID. This
candidate adopts no implementation, public interface, release or consumer change.
The [roadmap](../roadmap/README.md) alone owns adopted work and task status.

## 1. Context

Maintenance feedback spans invalid-Store admission, generation diagnosis,
durable identity fences, destructive confirmation, descriptor races, checkpoint
durability and interrupted removal. Existing paths fail closed under current
contracts; additional recovery policy and fault coverage must preserve that
boundary rather than silently authorize deletion of indeterminate state.

## 2. Goal

Shape a bounded improvement to maintenance ownership, safe operator
guidance and deterministic recovery verification while protecting newer or
unknown generations, changed filesystem witnesses and fresh successor Stores.

## 3. Non-goals

No automatic repair or conversion, broader destructive authority, installed
runtime operation or weakening of confirmation, generation, descriptor or
writer checks. No claim that the deferred cases establish a current exploit or
acceptance failure.

## 4. Rough Scope

- Review ownership of complete fence sets, Store corruption classification
  and resolver precedence while preserving caller-specific maintenance policy.
- Define whether and how indeterminate corrupt WAL state can have safe recovery
  guidance; retain current refusal until a generation-safe policy is adopted.
- Improve unusual-path display, compatibility-before-prompt behavior and explicit
  authorized-force guidance without changing exact target identity.
- Plan deterministic checkpoint, fsync and replacement faults plus concurrent
  residual-removal/fresh-init cases, retaining no-effect refusal and successor
  protection.
- Review marker construction reuse without changing the original request,
  response identity, recorded time or exact retry behavior.

### Source feedback

These entries retain the original source, reason for deferral and revisit
condition. Candidate grouping does not resolve, adopt or implement them.
Only the remaining work described by each entry enters this candidate;
already delivered subparts are preserved as context.

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

## 5. Open Decisions

- Which recovery cases can be defined safely under existing authority,
  and which require an explicit new policy or accepted ADR?
- What read-only compatibility precheck may precede confirmation while the
  authoritative post-confirmation check remains intact?
- How should concurrent absence converge without deleting a fresh successor?
- Which private fault boundaries and owned fixtures prove refusal, retry and
  durability? Can the selected policy and mechanical cleanup form one bounded
  epic, or do they need separate adopted scopes?

## 6. Roadmap Promotion Conditions

Close the selected recovery policy, exact identity and generation rules,
operator behavior and fault matrix before roadmap adoption. Preserve every
source entry's original no-effect, newer-State, replay and cleanup conditions.
Use isolated owned fixtures and focused checks plus the required development
gate for future executable changes; select no release or installation here.

## 7. References

- [TODO authority and lifecycle](README.md)
- [Direct execution authority](../architecture-decision-records/0038-remove-the-daemon.md)
- [Recovery, retention and maintenance](../specs/storage/recovery-retention-and-maintenance.md)
- [Transaction contract](../specs/storage/transactions-concurrency-and-idempotency.md)
- [SQLite model](../specs/storage/sqlite-model.md)
- [Deferred feedback register](../deferred-feedback/README.md)
