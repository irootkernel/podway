# Supervised Delegation Adapter Contract Review

## Purpose and authority

This is the durable review worksheet for the
[host-neutral execution identity contract](../specs/interfaces/supervised-delegation.md#sdg-003-host-neutral-execution-identity).
It supports the pre-reservation check owned by `V2SDG-002`; delivery order and
status remain in the [roadmap](../roadmap/README.md#v2sdg-host-neutral-supervised-delegation-foundation).
It adds no adapter, provider dependency, capability, or authorization to run an
agent. The [real integration candidate](../todo/TODO-podway-supervised-delegation-integration.md)
owns later implementation and live qualification.

Field names alone are not a compatibility proof. For every Podway receipt
field, distinguish its meaning, producing authority, accessible interface,
identity scope, normalization, and recovery behavior. A value stored inside an
executor is not necessarily available to a permitted adapter. Source inspection
may establish a gap; it does not authorize reading another product's live
Store, credentials, home, or private runtime files.

## Reviewed source baseline

This worksheet was reviewed on September 17, 2026 against Dolgorae commit
`5d93f43023ebf15e26b9c33b1ad032a7eeff6bbe`, with a clean Dolgorae worktree.
The corresponding Podway checkout was `bebc145`, with the uncommitted V2SDG
planning documents and unrelated existing changes. These are source-review
baselines, not installed-build, test, release, or live-compatibility evidence.

Dolgorae source locators below are relative to that repository, deliberately not
links to a machine-specific neighboring checkout:

| Locator | Inspected contract or implementation |
|---|---|
| `docs/protocol/dolgorae-external-specialist-facade-v1.schema.json` | `hire_result`, `assign_result`, `task_summary`, member and release projections |
| `docs/protocol/dolgorae-external-specialist-facade-v2.schema.json` | Current hire input and reuse of earlier result contracts |
| `docs/protocol/dolgorae-external-specialist-facade-v3.schema.json` | Accepted structured task and inline context; normalized structured result variants |
| `docs/protocol/dolgorae-orchestration-tool-v1.schema.json` | Internal `assign_task_request`, assignment/result collection, trusted transport bindings |
| `src/external_engagement.rs` | External hire aggregate binding and result construction |
| `src/engagement.rs` | `assign_task_v3`, persisted accepted request, task identity and redelivery |
| `src/task_request.rs` | `SpecialistTaskRequest` and exact task/context validation |
| `src/orchestration.rs` | Broker Role/configuration identities, task-request digest and safe result projection |
| `docs/specs/README.md` | External Specialist Engagement and Dolgorae-Orchestrated Session authority and recovery |
| `docs/roadmap/README.md` | Completed EPIC-007/014 and separately planned EPIC-008 live Primary bridge at this baseline |

Refresh affected rows when an external contract changes. A future contributor
can use these pinned source observations to plan the Podway foundation without
an installed Dolgorae or a neighboring checkout. Revalidation of the selected
real build remains mandatory before claiming that integration works.

## Compatibility verdicts

These labels classify field mappings, not roadmap status:

| Verdict | Meaning |
|---|---|
| `Contract-mapped` | A checked source/result field or trusted binding supplies the stated identity; actual adapter and live behavior remain unqualified |
| `Derivation-candidate` | The adapter could derive the value from permitted exact bytes, but byte scope, acceptance evidence, and replay rules must be fixed before using it |
| `Needs-verification` | Storage or conceptual support exists, but a permitted getter, exact semantics, or response-loss recovery path has not been established |
| `Needs-support` | The selected path needs an additional executor/host contract for the required meaning |
| `Incompatible-mapping` | A particular proposed source has different semantics and must not be assigned to the Podway field |

A mapping may have a source-level positive observation and an unresolved adapter
obligation. Do not collapse those into a claim of supported integration. An
unavailable required field makes that real adapter path unavailable, not the
Podway daemon or unrelated Direct work. Do not make a required field optional,
fill it with a placeholder, or weaken its meaning merely to obtain a passing
schema instance.

## Field and authority matrix

| Podway field | External Primary path | Managed Primary path | Review obligation |
|---|---|---|---|
| `execution.binding_ref` | `Contract-mapped` to the externally selected logical integration binding, not a Dolgorae wire field | `Contract-mapped` to the same logical binding; host selects the internal Broker adapter | Bind the exact Procedure value outside task text; neither Podway nor the model discovers a provider |
| `execution.adapter_id`, `adapter_version` | `Needs-verification`: adapter-owned versioned mapping definition; no actual adapter is qualified here | `Needs-verification`: separate internal-Broker adapter definition, not external-facade impersonation | Pin the transformation and digest rules to this identity; unannounced semantic changes are incompatible |
| `execution.primary_host` | `Contract-mapped` to `external`, selected by the trusted host | `Contract-mapped` to `managed` in trusted context; the live bridge remains `Needs-support` | Provenance only, never a privilege switch or a model assertion of authority |
| `execution.scope_kind`, `scope_ref` | `Contract-mapped` to the engagement kind and acknowledged `engagement_id` | `Contract-mapped` to the Orchestrated Session kind and trusted session ID | Preserve distinct namespaces; reconcile exact bootstrap identity after response loss rather than reopening an aggregate |
| `execution.supervisor_ref` | `Needs-verification`: choose a non-secret stable host/controller attribution permitted by the external owner interface | `Contract-mapped` conceptually to trusted Primary Run context; live bridge is `Needs-support` at the reviewed baseline | Prove permitted access and same-identity recovery; do not serialize credentials or accept a model-supplied source Run |
| `execution.worker_ref` | `Contract-mapped`: acknowledged `hire_result.specialist_run_id` | `Contract-mapped`: provisioned Specialist ID and `assign_task_result.target_run_id` | Bind actual membership/configuration, not a Role selector or an allocated-but-unpublished worker; retain the mapping through redelivery |
| `execution.task_ref` | `Contract-mapped`: `assign_result.task_id`, with matching await/collect task ID | `Contract-mapped`: `assign_task_result.task_id`, with matching result task ID | The selected task must be the actual final accepted assignment whose result the Primary accepted; collection order is not identity |
| `execution.role_ref` | `Needs-verification`: may echo the declared Role only after executor resolution is established | `Contract-mapped` to the installed policy Role selected by the Broker, subject to separate digest verification | A matching label alone is not an immutable Role match; resolve the digest row below independently |
| `execution.role_snapshot_digest` | `Incompatible-mapping` for the inspected aggregate-binding field: it hashes `role_ref` bytes, not resolved Role content; a correct permitted source is `Needs-verification` | `Needs-verification`: Broker hashes an installed Role object with `digest_value(&role)`; this is not the same byte domain as a Role-name hash | Define the immutable Role object and versioned digest domain used by declaration authoring and acceptance; prove a permitted read/resolution path |
| `execution.agent_configuration_digest` | `Needs-verification`: external hire stores `agent_configuration_digest` in the aggregate binding, but the inspected hire response does not expose it | `Needs-verification`: Broker stores immutable configuration and digest; a permitted adapter projection still needs to be identified | Distinguish requested configuration from admitted configuration, and explicit complete-input derivation from private-Store scraping |
| `execution.accepted_task_digest` | `Derivation-candidate`: `assign_task_v3` canonicalizes and hashes the complete accepted request, not merely its nested task; ordinary assign/collect results do not provide that hash | `Needs-verification`: Broker hashes its `AssignSpecialistTask` request, whose shape differs from external v3; safe summaries do not establish digest retrieval | Pin exact included/excluded fields, canonicalization, encoding and durable task-ID binding. A locally prepared request is not yet an accepted task |
| `result_digest` | `Derivation-candidate`: selected immutable collected result, including the isolated change when that is part of the accepted result | `Derivation-candidate`: complete selected task result obtained through the Broker's permitted result path | Fix whether the bytes are a canonical result object or a documented artifact representation; never hash a whole page, mutable delivery state, an artifact ID, or an arbitrary summary instead |
| `rounds_used` | `Needs-verification`: host/adapter accounting of durably accepted assignments for the Podway attempt | `Needs-verification`: host/bridge accounting under the same definition | Count accepted tasks, not polls, collection pages, transport retries, or model turns; an accepted assignment with unknown outcome still consumes a round |
| `worker_disposition` | `Needs-verification`: derive from actual idle/quiescent membership or completed release, not from the release request | `Needs-verification`: use completed graceful release or verified idle membership after task/delivery reconciliation | A requested release, one completed task, or an expired wait is not proof of worker settlement. Podway only checks the reported structural value |
| `summary`, `accepted` | `Contract-mapped` to the Primary's bounded explanation and explicit decision, not an executor result field | `Contract-mapped` to the same semantic owner, hosted by Dolgorae | `accepted: true` must not be manufactured from an empty findings list, a delivered result, or a test receipt |

This matrix is deliberately not an end-to-end pass verdict for either path.
Its `Needs-verification` entries name remaining work, not permission to invent
values. Contract reservation may proceed only after each Podway field's meaning
is closed and each unresolved external row is explicitly kept unavailable or
assigned a concrete integration prerequisite.

## Digest-domain review

Before schema reservation, record for each digest the object schema/version,
exact included and excluded fields, algorithm, canonicalization, string/byte
encoding, textual prefix, absence handling, producing owner, and task/Role/result
identity to which it binds. Register a complete mapping revision through the
adapter identity; do not vary these rules per request under one adapter version.

The following distinctions are mandatory:

- `package_digest` is Podway's attempt-bound input identity, not an executor task
  hash. It remains defined by SDG-004.
- `accepted_items_digest` is daemon-computed final item state, including item
  revisions, not either external digest. It remains defined by SDG-005.
- A Role reference hash cannot substitute for an immutable Role-content digest.
  Changing content while retaining the same Role label must not preserve its
  content identity under a conformant mapping. A Broker Role object may include
  additional policy/configuration fields; equality with a separately hashed
  Role-source document must not be assumed. To use one unchanged Procedure
  under both hosts, both mappings must satisfy its same reviewed Role-content
  identity. Otherwise record cross-host Role compatibility as unsupported;
  do not reinterpret the declared digest when changing adapters.
- The external v3 accepted-request hash and the internal task-request hash have
  different object scopes. Calling both `request_sha256` does not make them
  interchangeable. A reviewed adapter may identify its own precisely specified
  accepted-task representation, but must bind exact permitted bytes to the
  executor-acknowledged task and retain that proof through response loss.
- A result hash includes all accepted result components in its declared scope.
  A structured review plus an isolated patch is not the review alone. Report
  normalization, binary/base64 representation, and JSON canonicalization cannot
  be changed silently during redelivery.
- Adding the `sha256:` textual prefix to a verified lowercase hash is only an
  encoding conversion. It cannot repair a wrong object, unverified input, or
  unavailable producing authority.

The current Podway contract validates structure and declared equality, not
these external bytes. It cannot distinguish a forged but well-formed matching
Role-name hash from a real content hash. Adapter conformance must supply that
assurance; tests must not claim the Podway decoder proves it.

## V2SDG-002 semantic closure and availability

The Podway-side meaning is closed independently of a real adapter. An adapter
identity is the pair (`adapter_id`, `adapter_version`); that pair fixes one
mapping revision, including every digest's object schema, byte domain, and
recovery rule. Podway accepts only lowercase `sha256:` plus 64 lowercase hex
digits and compares declared values exactly. It neither rehashes executor
objects nor equates digests produced by different mapping revisions. There is
no host-independent canonical byte representation for an executor-owned Role,
configuration, accepted task, or result. Assigning one here would falsely
claim that the inspected external and Broker contracts expose the same object.

| Digest | Required immutable object and binding | Exclusions and absence rule |
|---|---|---|
| `package_digest` | Podway's `podway.delegation-work-package/v1` canonical JSON with the `podway.delegation-work-package/v1\n` domain prefix, bound to the current attempt; V2SDG-003 implements production derivation | Excludes mutable output items, read timing, and external task identity; V2SDG-002 reserves the field without authenticating fixture placeholder digests |
| `role_snapshot_digest` | Complete resolved Role content under the selected adapter revision, linked to `role_ref` and the worker's admitted Role | Role label alone, aggregate Role-reference hash, mutable policy lookup, or a missing Role is invalid |
| `agent_configuration_digest` | Complete admitted worker configuration under that revision, linked to `worker_ref` and the accepted task | Requested but unadmitted configuration, a Profile selector, or missing configuration is invalid |
| `accepted_task_digest` | Complete final task accepted by the executor under that revision, linked to acknowledged `task_ref` | Locally prepared bytes, a task summary, a transport envelope with mutable delivery fields, or an unacknowledged task is invalid |
| `result_digest` | Complete immutable selected result accepted by the Primary under that revision, linked to the same task and including every accepted component such as an isolated change | Artifact ID, page summary, mutable collection envelope, omitted patch, or unknown outcome is invalid |

Every mapping revision must state included and excluded fields, schema and
version, ordering, string/binary encoding, canonicalization, the exact SHA-256
input bytes, lowercase textual encoding, allowed producing interface, and
durable acknowledgement linkage. A value absent from a permitted interface is
unavailable; it is never filled from a private Store or an example digest.
An adapter may retain exact acknowledged bytes and derive a digest only when
the selected executor contract proves their identity to the admitted Role,
configuration, task, or result. Its durable recovery record belongs to the
integration, not to Podway's worktree receipt. Response loss requires lookup
by the exact external identity before replay. An uncertain external outcome
remains unavailable and does not become a new round or new task by assumption.

The inspected Dolgorae source does not provide a complete permitted mapping
revision for either Primary-host path. Both real paths are therefore
**unavailable** for V2SDG foundation activation, while the local Podway
contract and deterministic structural tests may proceed. The integration
candidate owns these named prerequisites:

1. Choose a versioned external and a separate managed adapter mapping, a
   trusted binding/configuration owner, and a non-secret stable Primary
   attribution and recovery path for each.
2. Expose or prove permitted immutable Role and admitted configuration
   resolution. Reject the inspected external Role-reference hash as a content
   digest; verify whether the Broker's installed Role object has the required
   complete domain and an allowed projection.
3. Prove final accepted task identity and its complete bytes for each distinct
   request shape, including lost acknowledgement recovery and durable task-ID
   linkage. A prepared request hash alone is insufficient.
4. Prove complete selected result bytes, acceptance by the Primary, durable
   round accounting, and verified `idle` or completed `released` worker state.
   The managed path additionally requires its planned trusted Primary bridge.
5. Qualify each mapping revision with same-label changed-Role, changed
   configuration, task-versus-envelope, complete-result-versus-summary, and
   lost-acknowledgement known answers before enabling that real adapter.

These are integration prerequisites, not optional Podway receipt fields. The
foundation's known-answer fixtures exercise the closed local structure and
identity distinctions only; they do not certify either real path.

## Pre-reservation review checklist

- [x] Record the Podway baseline, selected external commit/schema versions, and
  source locators, with no live runtime or provider requirement.
- [x] Map every required execution field, result digest, round count and
  disposition separately for external and managed Primary paths.
- [x] Identify the producing authority, allowed read/result/host-binding path,
  and response-loss recovery for each value. A DB column alone is insufficient.
- [x] Close each digest domain and distinguish an exact acknowledgement from a
  request prepared locally or an opaque selector.
- [x] For adapter derivation, require canonical bytes, acceptance/task-ID proof,
  recovery storage ownership and retention outside Podway, and a known-answer
  test; classify a real path without those exact facts as unavailable. Do not
  invent new Podway round or worker tables.
- [x] Classify every unresolved real-path requirement as unsupported pending a
  named integration prerequisite. Do not mark it implicitly compatible.
- [x] Resolve ambiguity in the generic Podway field meaning before reserving
  that field. A missing external getter need not block a clear local contract;
  an unclear digest meaning does.
- [x] Cover valid and malformed local receipt shapes and add mapping-level
  examples for same Role name with changed content, configuration drift,
  task-versus-envelope scope, complete result versus summary, and lost
  acknowledgement. Distinguish simulated mapping tests from real adapter tests.
- [x] Preserve fields and Direct behavior already promised by ADR-0035. Any
  broader behavior or authority change returns to explicit design review.
- [x] Carry remaining real-adapter prerequisites into the integration candidate
  and record the review outcome before `V2SDG-002` schema reservation completes.

The remaining known-answer row closes with the V2SDG-002 local fixture and
decoder checks. Completing this worksheet neither requires nor authorizes a
real model, runtime call,
provider scan, adapter installation, production mutation, or a Dolgorae change.
The worksheet and source comparison can be reviewed offline. Future tests must
not fetch external repositories or depend on a developer's neighboring checkout.

## Integration handoff

The foundation handoff includes the reviewed mapping, explicit unavailable
paths, exact local tests and their limitations. The integration owner rechecks
all permitted projections against the then-selected Dolgorae build, proves
result/identity redelivery and quiescence, and separately qualifies live
supervision. It must not widen a frozen Podway schema implicitly to hide an
adapter gap or use the external facade as a fallback for a managed Primary.
