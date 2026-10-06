# ADR-0035: Prepare Host-Neutral Supervised Delegation Without Disrupting Direct Work

- Status: Accepted
- Superseded in part by: [ADR-0038](0038-remove-the-daemon.md)
- Date: 2026-09-16
- Extends: [ADR-0010](0010-generic-cli-json-integration.md)
- Preserves: [ADR-0003](0003-daemon-single-writer.md), [ADR-0006](0006-same-user-local-trust.md), [ADR-0017](0017-single-cursor-convergence.md), [ADR-0019](0019-procedure-v2-only-product.md), [ADR-0031](0031-keyed-runtime-modes-and-managed-development-service.md)
- Delivery owner: [V2SDG](../roadmap/README.md#v2sdg-host-neutral-supervised-delegation-foundation)
- Implementation availability: the source foundation is implemented; the linked roadmap owns release and live-integration status.

## Context

The owner has requested a Podway-only supervised-delegation foundation while
Dolgorae integration is still being completed. Existing work depends heavily on
Podway. The foundation must not make an ordinary Procedure, admitted session,
daemon startup, or recovery depend on an external agent service.

The earlier research candidate combined Podway's structural gate with live
Dolgorae qualification. The September 16 decision deliberately adopts only the
bounded local contract, storage, projection, and deterministic conformance work.
It does not claim that the candidate's live comparative experiment ran or that
native delegation has demonstrated superiority over prose plus an executor.
Live qualification and that comparison remain external-integration prerequisites.
This explicit scope decision replaces the earlier promotion precondition only
for the Podway-side foundation; it does not promote graph engineering.

A Primary may run outside Dolgorae or eventually inside a Dolgorae Primary Run.
The external engagement facade and internal Broker have different operational
and authorization contracts. Hard-coding the external facade into a Procedure
would make the procedure depend on where its Primary happens to run.

## Decision

### Separate procedure semantics from the execution host

Keep `Action` and `Decision` as the node kinds. An action placement may declare
one closed optional `delegation` object. Absence is Direct execution; presence
requires supervised Specialist execution. Do not store a second independently
editable Direct/Delegated flag. Decision placements, including session-goal
assessment decisions, remain Primary-owned and reject delegation in this foundation.

Primary and Specialist are execution responsibilities, not new graph-token
kinds. Supervisor is the Primary's responsibility during a delegated action,
not an additional mandatory Run. A Primary may execute many placements; a
compatible Specialist may execute many tasks. Placement, attempt, Run, task,
Podway session, and executor aggregate identities remain distinct.

The declaration names a logical executor binding and immutable Role requirement,
not a Dolgorae facade, executable, Profile, credential, or mutable provider
configuration. The external integration resolves the binding. Podway performs
no provider discovery, process launch, network request, credential lookup,
worker shutdown, or adapter installation.

### Preserve explicit progression and the trust boundary

The Primary or its integration prepares work, supervises feedback, accepts a
result, and submits a bounded structural acceptance record. Podway binds that
record to the current session, action attempt, exact work package, declaration,
and external task/result correlation. Accepting the record never advances the
cursor. Explicit action completion also requires the ordinary item and blocker
gates. Required delegation cannot be bypassed by skip or direct fallback.

All executor and supervisor claims remain caller supplied. Role labels, opaque
IDs, and digests are not credentials or proof of honest external execution.
Podway's fences prevent stale cooperative updates; they do not authenticate a
Primary against another same-user process. Trusted adapters retain their own
Controller authorization and keep credentials outside Podway payloads.

The executor owns feedback rounds, in-flight tasks, worker reuse, delivery, and
operational recovery. Podway checks the submitted round count against the
bound; it does not authorize every external round or promise exactly-once
external work. No Podway claim, lease, driver election, or worker scheduler is
introduced.

### Keep the foundation usable without Dolgorae

Use one host-neutral work package and acceptance contract for external and
managed Primary test scenarios. Store executor scope and supervisor attribution
as bounded provenance without equating them to Podway identity. Do not require a
concrete Primary Run at ordinary session admission.

Real external-facade and internal-Broker adapters remain outside this epic. A
missing adapter may prevent the owner from using an explicitly delegated custom
Procedure, but may not block unrelated worktrees, daemon readiness, Direct
sessions, or ordinary lifecycle and recovery commands.

Mid-session Primary replacement or external-to-managed host switching is not
supported by this foundation. Same-identity process recovery remains the
executor's responsibility. Future handoff needs an explicit authority-generation
contract; it must not be simulated by editing attribution or moving a Run
between aggregates.

### Make operational continuity a release gate

Procedures without delegation preserve canonical bytes, digests, default
selection, existing result payloads, and domain/error semantics. Existing
admitted snapshots are never rewritten. Do not change built-in or Aquarium
Procedure files to exercise the feature, require a new global setup command, or
make a working Direct path perform executor discovery.

New public behavior must be coherent before delegated runtime admission is
activated. No partial parser/runtime combination may accept a document whose
completion gate is not enforced. Use explicit contract capabilities, not a
product-version guess or a process environment variable, for discovery.

Existing strict CLI/daemon manifest agreement remains mandatory. Preserving
Procedure semantics does not make mixed-generation binaries compatible. Test
new binaries only in isolated named runtimes. An incompatible pair must reject
before mutation rather than fall back to a different runtime.

Any storage generation change is a separate deployment risk even when DDL is
additive. Preserve all old values and receipts, test migrations and failure
recovery on copies, and reject unsupported downgrade. Never advertise rollback
by merely selecting an older binary after migration. Managed-generation
activation continues to obey ADR-0031's irreversible-migration restriction.

Development completion authorizes neither commit nor distribution, publication,
installation, service restart, workspace mode switching, or runtime activation.
The owner separately selects the release and gate. A feature with new runtime
and storage behavior must qualify through the full release path; the reduced
metadata-only patch gate is not a shortcut for this feature.

## Rejected alternatives

- Add Primary/Specialist as mutually exclusive alternatives to Action/Decision:
  mixes execution responsibility with graph semantics.
- Name the external Dolgorae facade in every declaration: prevents clean reuse
  under a managed Primary and confuses two different authorization boundaries.
- Duplicate a Direct/Delegated flag alongside the optional declaration: creates
  contradictory representations.
- Let the worker or its result automatically complete the placement: delegates
  outer progression authority and skips the Primary's acceptance boundary.
- Add leases, fan-out, joins, Primary handoff, or a global adapter registry now:
  exceeds the local sequential contract and increases continuity risk.
- Treat receipt acceptance as proof of external work or round enforcement:
  contradicts the same-user, caller-supplied evidence boundary.
- Modify installed runtimes or default procedures during implementation:
  makes development depend on disrupting the owner's active work.

## Consequences

The foundation can be implemented and tested without a working Dolgorae
installation. A delegated custom Procedure remains unusable end to end until a
compatible real integration is separately qualified; this is not a limitation
of Direct work. Future parallel units may reuse work-description and result
identity concepts, but require separately adopted readiness, ownership, effects,
and join contracts.

The target behavior is specified in the
[supervised-delegation contract](../specs/interfaces/supervised-delegation.md).
The [requirement traceability matrix](../specs/quality/requirements-traceability.md#supervised-delegation-ownership-and-proof)
maps the implementation and its proof; the roadmap alone owns task status and
ordering. Neither this ADR nor a test adapter is a claim that live integration
or rollout has occurred.
