# Non-Disruptive Supervised Delegation Development

## Applicability

For a runnable learning path, use the
[supervised review walkthrough](../examples/supervised-delegation.md). Its
canonical YAML is exercised through the isolated native qualification helper;
its synthetic executor is not a live adapter.

This guidance applies to the source implementation of the
[host-neutral delegation foundation](../specs/interfaces/supervised-delegation.md)
under [ADR-0035](../architecture-decision-records/0035-host-neutral-supervised-delegation.md).
It does not authorize any service operation or claim an installed release.
The [roadmap](../roadmap/README.md#v2sdg-host-neutral-supervised-delegation-foundation)
owns delivery status.

## Protect the owner's working system

The owner uses Podway for most active work. Treat the installed production
runtime, Aquarium's persistent development runtime, active worktree Stores,
service definitions, sockets, registries, and runtime-mode bindings as protected
resources. Never use them as migration, crash, reset, or delegated-work fixtures.

Do not install or activate a candidate, restart a daemon, change a workspace
mode, replace a managed generation, rewrite a default Procedure, or test against
real agent credentials under an implementation-task authorization. A clean
commit, automatic development publication, or passing gate is not runtime
activation authority.

At task start inspect Git state and the affected authorities. Preserve unrelated
work. Record the exact baseline for executable comparison, including the real
current Store generation. The historical V2SDG planning baseline was daemon generation 11; delegation
receipts and canonical Git identity arrived in generations 12 and 13. The current
product uses direct generation 15 and declares no predecessor. The
[SQLite model](../specs/storage/sqlite-model.md) owns current compatibility and
unchanged legacy refusal; the historical generations are rejected test inputs. Do not
resurrect the obsolete 32-session inactive-retention cap.

## Use established isolated runtimes

Use the [isolated direct CLI checks](testing.md#isolated-direct-cli-checks).
Resolve the exact binary, manifest, disposable Git worktree, and workspace UUID
before a mutating command. Abort a fixture when those identities cannot be
established. The former named-runtime helper is retired; tests must never
select the user's active worktree or installed service as a fallback. Release
QA separately follows [release upgrade QA](release-upgrade-qa.md) under its
own authorization and selected contract generation.

Do not run a real Dolgorae, Codex, or other provider during this epic. A controlled
fixture may submit bounded Podway requests and emulate result provenance; it
must not become a shipped success-producing adapter or target a live session.
A fixture receipt proves a protocol case, not that real delegated work happened.

## Layer changes without opening an incomplete path

Contract reservation, domain types, storage, and projections may land in
separate task commits. Delegated runtime admission stays closed at intermediate
commits until the complete path is natively verified; V2SDG-007 opens the
explicit custom-Procedure path in source. Existing Direct behavior must
remain usable at every step. Do not advertise a route merely because its schema
or help text has been generated.

Keep the omitted-declaration path structurally simple. Avoid adapter lookups,
new global configuration, provider-dependent health checks, package construction,
or delegation-record scans for an ordinary Direct attempt unless needed by an
existing invariant. New feature failure cannot be repaired by weakening old
manifest, writer identity, Store integrity, fencing or success-only outcome
guarantees. The current direct product has no runtime mode or FIFO admission
contract.

Expose only explicitly authored delegated Procedures. Built-in presets and
tracked Aquarium workflows are compatibility fixtures, not convenient places to
turn the feature on. Source-distributed skill changes must distinguish
implemented local capability from absent real adapters and require explicit
feature use. Verify installed copies against the current direct contract; source
changes do not install or update them in the owner's environment.

## Preserve the reviewed adapter semantics

Use the [adapter contract review](delegation-adapter-contract-review.md), which
closed the Podway field meanings before schema reservation. Preserve the external
and managed field mappings, producing authority, permitted retrieval path, exact digest
scope, and response-loss recovery. Never map fields solely by spelling or treat
an internal database column as a supported adapter interface.

Keep the two outcomes separate: an ambiguous Podway field meaning blocks its
schema reservation; a clear local contract with an unavailable real executor
field leaves that adapter path unsupported pending its named prerequisite.
Neither outcome requires a running Dolgorae, a live model, credentials, private
Store inspection, or a change to Direct operations. No test may depend on a
neighboring checkout or fetch it dynamically.

Do not mark adapter derivation proven until its canonical bytes, authoritative
acceptance linkage, and recovery ownership are specified. Pin the source review
and mapping version and distinguish its known-answer checks from native Podway
conformance and later real-adapter qualification. The worksheet's Role-name
hash and installed-Role hash example is a semantic mismatch, not a prefix issue.

## Finish recording before acceptance

The first version intentionally allows only one immutable acceptance per
attempt. There is no same-attempt reacceptance or special exception for a typo.
Teach the Primary the normal sequence before it uses the feature:

1. Review the worker's result and finish all required and optional action-item
   recording, including summaries, evidence metadata, and spelling corrections.
2. Resolve ordinary blockers and verify the declared idle/released worker
   disposition through the adapter. Do not infer settlement from a request.
3. Submit acceptance for the final item state and reconcile its durable outcome.
4. Explicitly complete with current ordinary fences. Do not make routine item
   edits between acceptance and completion.

The [post-acceptance matrix](../specs/interfaces/supervised-delegation.md#post-acceptance-behavior-matrix)
is the authority for exact cases. Reads and true no-ops do not invalidate a
receipt. A successful changed item does, including an optional summary. Reverting
the text does not revert its item revision. A failed/rolled-back mutation does
not invalidate unchanged item state. Block/unblock leaves the receipt intact
but still affects the independent blocker gate. Do not use session revision
alone as a freshness test.

For example, correcting an item from `All test pass` to `All tests pass` after
acceptance is still a recorded-value/revision change. The edit is allowed and
its success remains success, but completion must now reject stale acceptance.
Showing an old acceptance success from `outcome lookup` does not restore readiness.
Use a current observation before suggesting the next transition.

Help and delegated guidance must explain this cost before acceptance and again
when staleness is observed. A suitable human explanation is:

> The item edit succeeded, but it changed the accepted item state. This attempt
> cannot be accepted again. Inspect the current state and explicitly choose retry
> or rework; no external task has been restarted by this Podway operation.

Human text is explanatory, not a new stable machine field. Use only declared
warning/diagnostic carriers and the delegation-aware result families. Do not
append prose to JSON, turn the successful edit into a command failure, introduce
new confirmations for ordinary item edits, or alter the legacy Direct result
shape to display the message. Guidance is advisory; the transaction-time
completion check remains authoritative under concurrent edits.

## Recover without repeating external effects automatically

For a lost acceptance response, first look up the original idempotency key and
inspect the current attempt and receipt applicability. Exact replay returns the
old stored outcome even if a later edit made it stale. Never retry with a new
acceptance key to overwrite the old receipt.

For actual stale acceptance, the Primary/operator chooses ordinary retry or
rework. These create new local state, not an automatic executor dispatch.
Inspect the existing artifact and external execution/writer state first. The
integration must decide the minimum justified new work or explicit artifact
revalidation against the new package; it cannot copy the old receipt, invent a
new task ID, silently rerun writes, or assume cancellation stopped a worker.
When a qualified revalidation path is unavailable, expose that integration
limitation and keep local recovery available. Never reset the owner's workspace
to bypass it. Real adapter design and proof remain outside this epic.

In deterministic fixtures, count accepted assignments/effects and assert that
Podway reads, edits, response-loss recovery, and local retry do not trigger
redispatch. This tests the controlled integration pattern, not Podway's ability
to observe or prohibit arbitrary external execution. Repeat the scenario while
another isolated worktree continues its ordinary Direct workflow.

## Verification ladder

1. Freeze baseline known answers and supported-path scenarios before product
   changes. Verify negative tests fail for the intended reason.
2. Close and record the offline adapter field/digest mapping before schema
   reservation. Then run focused schema, domain, package/digest, storage, and
   projection checks using the existing Make targets and exact aggregated Rust
   test conventions.
3. Run native single-CLI/Direct Engine/Store scenarios in standalone scratch
   Git worktrees, including
   the complete post-acceptance matrix, acceptance/edit/completion races, lost
   response and replay after a later edit, restart, retry/rework/goals, retention,
   and reset/removal. Preserve ordinary fence-error precedence.
4. Run a mixed-worktree scenario: leave one explicit delegated action without
   an executor/receipt while an unrelated Direct worktree starts, records,
   progresses, retries, and completes. Verify no provider polling or global
   blocked state has been introduced.
5. Exercise write-free legacy refusal, confirmed destructive cutover,
   transaction/crash failure and strict newer-state rejection on frozen copies
   with exact binaries. A future declared direct predecessor needs its own
   migration and restoration proof. Reconcile
   fixture cleanup without broad deletion or installed service changes.
6. Pass the full `make test` development gate through the repository-prescribed
   Gaori entry point and independently review the complete epic against its
   baseline. A focused test or mock cannot substitute for these claims.

Use only narrowly scoped, read-only document checks for a planning-only edit.
Do not run formatting targets that may rewrite unrelated
source merely to validate Markdown. After all checks inspect Git status and the
complete intended diff, including new files.

## Required regression families

| Family | Required unchanged behavior |
|---|---|
| Authoring | Existing YAML/JSON canonical bytes, digest identities, presets, validation, graph and preview output |
| Lifecycle | Prepared/start/begin, action completion, decisions, retry, rework, goal revision/assessment, cancellation and disposition |
| Evidence | Existing item/check-result recording, paging, guards, blockers, byte limits and error precedence |
| Persistence | Admitted snapshots, attempts, current/inactive sessions, canonical values, old terminal envelopes and replay |
| Runtime | Direct process crash/retry, unrelated worktree progress, writer-manifest and newer-generation rejection, write-free reads and no fallback |
| Maintenance | Existing archive, reset, removal, retention and confirmation boundaries |
| Delegation only | Mandatory gate, no skip bypass, exact package, round/disposition checks, complete post-acceptance matrix, immutable receipt, warnings and explicit stale recovery |
| Adapter mapping | Distinct Role/task/result digest domains, permitted value sources, explicit unsupported paths, no fabricated provenance or automatic redispatch |

A reproduced supported-path regression is a blocking finding. Do not relabel it
as deferred, discard the test, change the default workflow to evade it, or use
the owner's session reset as a workaround. Keep the live deployment unchanged
while fixing and requalifying the isolated candidate.

## Storage and rollback claims

Adding a table and increasing the schema version still changes compatibility.
Selecting an old binary after an upgrade is not a rollback procedure. Preserve
old values and terminal receipts, but require explicit evidence for transaction
recovery and version rejection. Never downgrade `user_version` or edit runtime
rows by hand.

A future rollout must have a compatible, quiescent backup or the documented
SQLite-consistent backup procedure and a tested restoration path. Copying only a
live WAL database's main file is not adequate evidence of a restorable backup.
Do not overwrite post-upgrade user work with an older copy under a generic
rollback instruction; stop and obtain an explicit recovery decision.

[ADR-0038](../architecture-decision-records/0038-remove-the-daemon.md) supersedes
the runtime-mode and managed-service architecture. Current Podway cannot control
an old installed writer; retirement belongs to its original owner under separate
authorization. The cutover has no daemon-era migration or restoration promise.
A future direct predecessor, consumer adoption and installation each require
separate qualification and authority; delegation implementation grants none.

## Handoff, not deployment

A completed foundation handoff records exact tested source, single CLI,
manifest and Store identities; the scenario inventory; failures resolved;
remaining external integration exclusions; and fixture cleanup. Include the
reviewed field/digest matrix, unavailable real-path prerequisites, and the
post-acceptance correction/revalidation cost. Distinguish source comparison,
local deterministic and native conformance, and live agent behavior.

Leave release selection to the owner. A feature that changes behavior and
storage requires the full distribution and upgrade qualification path, not a
metadata-only reduced patch gate. Read the existing repository release policy
and obtain the required gate/operation authorization when release work is
actually requested. Do not change CHANGELOG release selection, versions, tags,
installed binaries, or active managed generations during this epic.
