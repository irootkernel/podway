# Automation Client Contract

## Authority and scope

This document defines the implemented local automation boundary accepted by
[ADR-0038](../../architecture-decision-records/0038-remove-the-daemon.md).
The [command catalog](../../../assets/specifications/command-catalog.yaml),
[JSON contract](json-contract.md), and [error catalog](errors-and-exit-codes.md)
own the exact current command, schema, and failure families. The active
[roadmap](../../roadmap/README.md) owns work status; historical task and evidence
IDs below retain provenance and do not attest a newly run gate or an installed
consumer's compatibility.

Automation invokes the single `podway` CLI through a controlled PATH, selects an
explicit Git worktree, consumes JSON, retains keys and resolved fences, and
reconciles committed success outcomes. Each process calls the typed Engine;
there is no product daemon, socket, service, queue, runtime mode, or global state.
A read is one coherent committed snapshot, not a completion barrier. A mutation
returns after its transaction commits or fails. Caller termination establishes
no continued execution guarantee.

Podway guards one task's Procedure progression. It does not interpret Dolgorae
Workflows/Roles, authorize ToolRuns, execute external work, authenticate users,
provide remote collaboration, archive artifact bytes, or judge recorded claims.
Optimistic fences and local paths are identity and containment controls, not a
security boundary against same-user processes.

Stable requirement IDs retain their original responsibility. The explicit
retirement table records guarantees removed by the direct generation; those IDs
are never reused for different guarantees. Active portions of mixed requirements
remain below with their retired clauses identified.

## Direct execution requirements

| ID | Normative requirement |
|---|---|
| `AUT-DIRECT-001` | Every runtime command MUST execute through the in-process typed Engine. Product execution MUST require no resident process, IPC, global product registry, runtime mode, or service installation. |
| `AUT-DIRECT-002` | Queries MUST observe one coherent committed snapshot without writes, outcomes, migration, pruning, checkpointing, repair, or maintenance continuation. They MUST NOT promise queue barriers or global quiescence. |
| `AUT-DIRECT-003` | A normal mutation MUST use one SQLite IMMEDIATE transaction for fences, domain validation, state effects, journal updates, validated output, successful outcome insertion, and applicable pruning. A rejection MUST roll back the complete transaction. |
| `AUT-DIRECT-004` | Bounded Procedure/artifact preparation, including typed criterion citations, MUST run outside the write transaction. Local-artifact digests and stable descriptor stamps MUST be prepared before writer admission. Execution MUST revalidate workspace generation, current state, recorded artifact metadata and prepared identities/stamps inside the transaction before effects, without reading artifact bytes. |
| `AUT-DIRECT-005` | One absolute invocation deadline MUST bound maintenance, process-local connection and SQLite admission waits; preflight, preparation and human confirmation consume the remaining budget without renewing it. Every boundary MUST attempt acquisition once even with an exhausted or `0ms` budget; only retries and sleeps are bounded, and acquired locks permit work to finish past the deadline. It is not a hard whole-execution/input/confirmation timer. The default is 5,000 ms and maximum 3,600,000 ms. Maintenance exclusion expiry MUST return `WORKSPACE_MAINTENANCE`; other contention MUST return `WORKSPACE_BUSY` with `connection_gate`, `sqlite_read_admission` or `sqlite_write_lock` for the actual boundary. Reads MUST NOT report `sqlite_write_lock`. Neither error may retain a rejection or partial effect. No FIFO ordering is promised. |
| `AUT-DIRECT-006` | Automation MUST retain its caller-owned key and complete canonical request, including original resolved workspace/session/revision/attempt/item/goal fences. The CLI may fill omitted fences from fresh status; clients SHOULD supply observed fences to pin their intended target. |
| `AUT-DIRECT-007` | For retained Store successes, after workspace UUID and writer-contract classification, exact canonical request/key replay MUST return the complete original validated success envelope, including original request ID, timestamp, and fences, without repeating effects or refreshing fences. Changed input with that key MUST fail `IDEMPOTENCY_KEY_REUSED`. |
| `AUT-DIRECT-008` | Only committed successes MUST be retained as outcomes. Domain, validation, lock, and optimistic-fence rejections MUST retain no outcome. `MUTATION_OUTCOME_UNKNOWN` MUST be emitted only when COMMIT I/O leaves the commit result uncertain; response loss or SIGKILL is caller-side uncertainty, never a job state. |
| `AUT-DIRECT-009` | `outcome lookup --idempotency-key <key>` MUST be read-only and workspace-scoped, returning either `found=false` or the complete immutable retained success. It MUST NOT replay or submit a mutation, expose a non-terminal job, or retain a failed request. A miss proves absence only within retention. |
| `AUT-DIRECT-010` | Session outcomes MUST remain until session reset or runtime reset, including after archival and archive purge. Workspace removal destroys all retention. Workspace-scoped outcomes MUST be bounded to 100 entries or 30 days, pruned only on successful writes. Clients MUST account for these boundaries when interpreting a miss. |
| `AUT-DIRECT-011` | Canonically retired command families and flags MUST fail `COMMAND_RETIRED` before workspace resolution, stdin consumption, or Store access. `job.lookup` identifies `outcome.lookup`; a null replacement means none exists. Unknown syntax retains usage behavior. |
| `AUT-DIRECT-012` | Normal reads, writes, and replay MUST refuse Store generations 1–13 and daemon-era markers without conversion or deletion. Unsupported legacy workspace configuration MUST fail before Store access. Confirmed reset-all is the explicit destructive recovery for those legacy generations; reads MUST NOT resume it. Every Store admission, including reset and removal, MUST refuse a newer generation without effects and direct the caller to a supporting Podway. |
| `AUT-DIRECT-013` | The Store MUST bind its writer identity to the manifest digest and refuse unsupported identity before read, mutation, or key replay. Only an explicitly declared direct predecessor may migrate under exclusive maintenance; the initial cutover declares none. Development MUST use matching scratch worktrees. |
| `AUT-DIRECT-014` | Reset/removal MUST preserve bounded marker recovery, workspace generation fencing, explicit destructive authorization, and original-key exact retry. Reset-all preserves Procedure sources and configuration unrelated to retired legacy mode; it intentionally retires legacy-mode configuration/markers. Removal deletes only `.podway` and preserves Git/user files. A recipe grants no authority. |
| `AUT-DIRECT-015` | Manifest v2 MUST bind product/version and complete canonical contract/schema/catalog/fixture assets without IPC. Release provenance MUST separately bind the source lockfile bytes. The single CLI's complete output-v4/version-result-v2 identity MUST be validated by the authoritative offline Rust verifier against manifest, native target, source commit, and recomputed build identity. |
| `AUT-DIRECT-016` | Current JSON MUST use output v4/error v2 and each canonical current closed result/detail discriminator. Success/error envelopes MUST contain no job/admission metadata. Exit classes are 0, 1, 2, 4, 5, and 6; class 3 is retired. |
| `AUT-DIRECT-017` | Both stdin and typed `item.record_many` input MUST fit 1,048,576 bytes for the complete canonical input, including identities, key, and 1..128 unique operations. Per-item bounds MUST NOT substitute for this aggregate. Oversize MUST fail before preparation, canonicalization, or mutation with `REQUEST_TOO_LARGE`. |
| `AUT-DIRECT-018` | Successful start MUST atomically commit its validated canonical immutable Procedure snapshot with state and outcome. Running commands MUST use that snapshot instead of rereading Procedure source; exact replay MUST not depend on changed Procedure/artifact sources. Workspace configuration validation still applies on resolution. |
| `AUT-DIRECT-019` | RecoveryRecipe v2 MUST be closed, bounded, and code/boundary-specific. Ordinary recovery MUST remain read-only; legacy reset and exact initialization/reset/removal continuation MUST explicitly require authorization. No recipe may weaken a fence, fabricate a new replay key, or authorize external work. |
| `AUT-DIRECT-020` | Native qualification MUST bind one packaged release-profile CLI, immutable candidate bytes, independent extraction, exact manifest/catalog/lock/source identity, disabled debug hooks, and bounded process/fixture cleanup. A dirty-source development dry run MUST remain explicitly nonpublishable `development-qualification` with gate `development`; it MUST NOT imply clean distribution or a full/patch release gate. |

The [CLI specification](cli-specification.md#worktree-selection-and-deadlines)
owns duration syntax and prompting. Retired syntax is listed in the
[canonical table](../../../assets/specifications/retired-commands-v1.json).
Every retry must first classify current workspace/writer identity. After lost
stdout or uncertain COMMIT I/O, look up the original key, then use exact original
request replay when appropriate. Pending removal uses its original marker request;
complete removal destroys the Store, so an absent-removal result is convergence
rather than replay of the original response. A fresh key or freshly substituted revision is
a new mutation and requires a new justified decision.

## Retained invocation and identity requirements

| ID | Normative requirement |
|---|---|
| `AUT-PATH-001` | Automation clients MUST invoke `podway` through a controlled `PATH`; they MUST NOT need a hard-coded CLI executable path. |
| `AUT-HOME-004` | Workspace configuration, `runtime/state.sqlite3`, Procedure snapshots, immutable delegation receipts, successful outcomes, and other task state MUST remain in the owning worktree. No product-owned global copy is permitted. The former jobs/global-home clause is retired. |
| `AUT-CONTRACT-002` | `podway version --json` MUST emit exactly the compact `name` and `v`-prefixed `version`; `podway version --json --identity` MUST emit a validated `podway.output/v4` envelope with closed `podway.version-result/v2`, exposing product, version, target, build identity, source commit when available, and manifest schema and digest. The former IPC-identity clause is retired. |

The full identity version is `v`-prefixed, like the compact summary. Cargo and
manifest product versions remain bare. `version` text is diagnostic; native
release verification uses the complete closed identity, manifest, source, target,
and build identity, not a version string alone. Product v0.2.12 is the current
source version; the roadmap's v0.2.13 target does not constitute a version bump,
release, installation, or external consumer adoption.

| ID | Normative requirement |
|---|---|
| `AUT-ID-001` | The public CLI MUST expose `--if-workspace-uuid` and `--if-session-id` alongside revision, attempt, and item-revision preconditions. |
| `AUT-ID-002` | Session-bearing reads MUST enforce workspace UUID and, when supplied by the client, session ID. |
| `AUT-ID-003` | `start` MUST enforce workspace UUID and the normal no-existing-session rule, subject to the explicitly supported lifecycle and existing-session policies below. |
| `AUT-ID-004` | Graph-node transitions MUST enforce workspace UUID, session ID, session revision, and attempt ID for automation requests. |
| `AUT-ID-005` | Item mutations MUST enforce workspace UUID, session ID, attempt ID, and item revision for automation requests. |
| `AUT-ID-006` | Replacement and reset MUST enforce the currently observed workspace, session, and applicable session revision before mutation. |
| `AUT-ID-007` | Identity mismatches MUST return stable typed errors with closed details containing expected and actual identities and no mutation. |
| `AUT-ID-008` | `item.record_many` MUST enforce workspace UUID, session ID, session revision, active attempt, and every selected item revision before atomically changing any selected item. |
| `AUT-ID-009` | `begin`, terminal disposition, eligible or force replacement, and eligible or force reset MUST enforce the exact observed workspace UUID, session ID, and session revision; begin and disposition MUST NOT accept an attempt fence. |

`record --stdin` uses closed `podway.item-record-many-input/v1`; each operation
contains a typed complete record or `clear: true`. Operations and outcomes are
canonicalized by item ID. It returns `podway.item-record-many-result/v2` and
never advances the cursor. Identity and idempotency flags cannot duplicate the
stdin identities. The typed boundary shares the complete-input bound.

A matching numeric revision does not identify a session. Goal revision and
criterion assessment additionally bind the exact positive current goal revision;
that fence does not replace session revision or attempt identity.

## Retained Procedure and lifecycle requirements

| ID | Normative requirement |
|---|---|
| `AUT-START-001` | `start --expect-procedure-digest sha256:<hex>` MUST compare the expected digest with the validated, defaulted, canonical Procedure before creating or replacing a session. |
| `AUT-START-004` | The canonical Procedure digest and relevant start preconditions MUST participate in idempotency identity and MUST be returned by start and later session observations. |

| ID | Normative requirement |
|---|---|
| `AUT-LIF-001` | Every successful new-session `start` outcome MUST create a `prepared` session at revision 0 with no attempt, cursor, goal revision, item value, or blocker. |
| `AUT-LIF-002` | `begin` MUST atomically create the entry-node attempt, optionally create and bind initial goal revision 1, change lifecycle to `running`, and advance session revision exactly once. |
| `AUT-LIF-003` | Prepared sessions MUST reject item, goal, blocker, cursor, completion, cancellation, retry, skip, decision, and rework mutations with `SESSION_NOT_RUNNING` and no state change. |
| `AUT-LIF-004` | Public terminal disposition MUST accept exactly one closed `handed_off` summary/reference or `not_required` reason shape, bind it to the current completed or cancelled session revision, and treat every assertion as caller-supplied. State-aware running preservation additionally records the closed internal `superseded` reason, optional actor, and successor session identity. |
| `AUT-LIF-005` | Reactivating a completed session MUST make every earlier terminal disposition non-current; a later terminal revision MUST require a new disposition for default deletion. |
| `AUT-LIF-006` | Default reset MUST delete only prepared sessions or terminal sessions with a disposition for the current revision. Start MUST archive disposed terminal sessions automatically; prepared deletion MUST be an explicit interactive choice or `--on-existing delete`. |
| `AUT-LIF-007` | Force reset and start-time deletion of a running session MUST require destructive confirmation and a non-blank progress summary bounded to 4,000 Unicode scalars. |
| `AUT-LIF-008` | Reset eligibility MUST NOT depend on Git cleanliness, roadmap status, process state, external reference reachability, or any network request. |
| `AUT-LIF-009` | Prepared `status`, compact status, next, and observation MUST expose the lifecycle without inventing cursor, attempt, goal, item, blocker, or history values; observation MUST provide only fenced begin, eligible-reset, and eligible-replacement templates. |
| `AUT-LIF-010` | Exact idempotent replay and uncertain-outcome reconciliation MUST cover begin, disposition, eligible and force reset, and every state-aware start resolution without weakening identity or revision fences. |
| `AUT-LIF-011` | Archival MUST retain the complete disposed terminal session as immutable inactive state, MUST NOT evict inactive sessions automatically, and MUST NOT fail archival or start because of inactive count. |
| `AUT-LIF-012` | Start MUST atomically archive a disposed terminal current session before creating the new prepared session. Preserving running work MUST atomically cancel it, record a successor-linked `superseded` disposition, archive it, and create the successor; terminal disposition choices MUST likewise disposition, archive, and start atomically. |
| `AUT-LIF-013` | Inactive sessions MUST support a bounded newest-first list that reports the exact total, truncation, and oldest summary, and full read-only show by session ID including terminal disposition, MUST NOT restore or reactivate, and purge MUST require exact session revision plus destructive confirmation. |
| `AUT-LIF-014` | Human TTY start MUST offer continue or delete for prepared state; continue, preserve, or delete for running state; and continue, handed-off, or not-required for undisposed terminal state. Enter or EOF MUST continue without creating a session. JSON, quiet, non-TTY, and policy execution MUST never prompt and MUST return `SESSION_START_DECISION_REQUIRED` when no sufficient policy exists. |
| `AUT-LIF-015` | When an existing-session policy is required, automation MUST use `--on-existing preserve\|delete`. Preserve applies only to running state and requires a bounded reason; deleting running state additionally requires a bounded progress summary and confirmation. Plain local start dry-run MUST remain Procedure-only, while a policy dry-run MUST report the observed existing-session lifecycle and proposed action without mutation. |

Session mutations bind their exact applicable fences in the Direct request.
Archive purge records a workspace-scoped success; original-key lookup is the
primary response-loss reconciliation. A fresh archive list can confirm absence,
but cannot identify the caller that deleted it. Stored session outcomes remain
available under `AUT-DIRECT-010`.

## Retained reconciliation, observation, and evidence requirements

| ID | Normative requirement |
|---|---|
| `AUT-RECON-004` | Reusing an idempotency key for a different canonical request MUST continue to fail with the stable reuse error. |
| `AUT-OBS-003` | Compact status MUST omit instructions, prompts, task titles, previous-attempt narratives, and item values unless a value is strictly required to form a valid mutation. |
| `AUT-OBS-004` | Compact status MUST use a closed schema and the complete serialized JSON envelope of a standalone `status --compact` response MUST NOT exceed 262,144 UTF-8 bytes. The compact projection nested inside an observation is budgeted separately by `AUT-OBS-009`. |

| ID | Normative requirement |
|---|---|
| `AUT-OBS-006` | Prepared and running observations MUST use closed `podway.observation-result/v5` for both Direct and delegated Procedures. Running contains a bounded value-free compact status projection, current next guidance, active item declarations with type-specific constraints and bounded typed value projections, and applicable mutation templates. Prepared contains that compact projection, cursor-free prepared guidance, no active items, and only begin, eligible-reset, and eligible-replacement templates. |
| `AUT-OBS-007` | Every mutation template MUST carry exact current workspace, session, and applicable revision/attempt/item/goal fences, mark the idempotency-key requirement, and classify whether an explicit user request is required. Templates MUST NOT invent semantic values or an idempotency key, and fences MUST NOT be represented as authentication or authorization. |
| `AUT-OBS-008` | A completed or cancelled observation MUST succeed with prepared-aware terminal status, null guidance, no active items, and only a terminal-disposition template when the current terminal revision has no disposition; a disposed terminal state MUST instead offer only eligible reset and replacement templates. Existing `next` terminal behavior remains unchanged. |
| `AUT-OBS-009` | Observation MUST omit history and compose four bounded allocations within a 1,048,576-byte complete JSON envelope: complete compact status plus running `podway.next-result/v5` or cursor-free prepared `podway.prepared-next-result/v3` guidance share 786,432 bytes. Active items have 131,072 bytes, mutation templates have 65,536 bytes, and 65,536 bytes remain for serialization and the outer envelope. Status and guidance retain the complete graph-wide counters and MUST be measured together. Item and template windows MUST report their exact total and truncation; complete-member or envelope overflow MUST fail closed. |
| `AUT-OBS-010` | Observation guidance MUST emit evidence metadata only, with no preview data and no page token. Active-item choice constraints MUST expose at most eight choices with the exact choice count and a truncation flag, and template argv elements MUST NOT exceed 4,096 Unicode scalars. |
| `AUT-OBS-011` | Reducing the observation status member from the standard tier to the compact projection removes `purpose`, `item_values`, and the other standard-only fields from observation. Current progression guidance and active-item mutation information MUST remain self-contained in observation; a consumer that needs the removed fields, or trace history, MUST use `session.status`. |
| `AUT-OBS-012` | A truncated mutation-template window MUST still carry the first template of every command it would otherwise have offered, so no action named by `allowed_actions` is left without a template. |

Both Direct and delegated snapshots use status/compact/observation v5, running
next v5, and prepared next v3. Delegation data remains conditional and truthful;
there is no invented worker on a Direct placement or cursor on prepared state.
The active-item allocation covers the validated maximum; template truncation must
retain one usable template for every offered command.

### Bounded evidence scale and paging (AUT-EVR-001–008)

| ID | Normative requirement |
|---|---|
| `AUT-EVR-001` | Authoring validation, runtime recording, persistence reconstruction, typed input and public request slices, and documentation MUST enforce one scale envelope in Unicode scalar values: text 65,536, list 1,000 entries, 8,192 per entry, 1,000,000 total list content, 16,777,216 recorded text and list content per attempt, and 128 items per node definition and attempt. |
| `AUT-EVR-002` | A public value bound MUST NOT be narrower than its domain hard maximum. The named 1,048,576-byte complete `item.record_many` input bound remains independent of per-item and per-attempt limits and applies to stdin and typed Direct callers. |
| `AUT-EVR-003` | `evidence.read` MUST read only an item selected by a currently resolved, declared evidence reference of the current consumer attempt, MUST be fenced by workspace UUID, session ID, and consumer attempt ID, and MUST create no session revision, outcome, or state change. |
| `AUT-EVR-004` | An `evidence.read` page MUST end only at a Unicode scalar boundary for text and at an entry boundary for lists, MUST carry at most 262,144 bytes of encoded page data, and its complete encoded response including one token MUST NOT exceed 327,680 bytes. |
| `AUT-EVR-005` | The page token MUST be opaque, at most 256 base64url characters, and validated field by field against current authoritative state on every read. It MUST NOT be represented as an authentication or authorization credential. |
| `AUT-EVR-006` | Malformed, oversized, cross-session, and wrong-binding tokens MUST fail as `REQUEST_INVALID` before state comparison; an unresolved or skipped reference read without a token MUST fail with `EVIDENCE_NOT_AVAILABLE`; a changed bound snapshot MUST fail retryably with `EVIDENCE_PAGE_TOKEN_STALE`; and an offset at or past the logical end MUST fail with `EVIDENCE_PAGE_TOKEN_EXHAUSTED`, which the Engine MUST never issue a token for. |
| `AUT-EVR-007` | `session.next` MUST enforce five independent placement-local byte ceilings, add their actual charges to the exact complete graph-counter charge, and accept only an aggregate of at most 999,424 bytes so 49,152 bytes remain for serialization and the envelope. It MUST supply complete evidence metadata for every item in an accepted projection and MUST NOT mark its evidence metadata window truncated. Explicit selectors receive previews in declaration order for at most 32 items, and at most 128 item metadata records may appear in one response. |
| `AUT-EVR-008` | Every parsed bound failure MUST report `field`, `actual`, `maximum`, and `unit` and name the violated constraint. `REQUEST_TOO_LARGE` is the exception for bounded stdin acceptance or complete typed-input encoding before a parsed field path is available. |

### Structurally bound external check results (AUT-CHK-001–009)

| ID | Normative requirement |
|---|---|
| `AUT-CHK-001` | Procedure v2 MUST admit a closed `check_result` declaration with one operation ID, one lowercase SHA-256 operation digest, and one to three unique accepted outcomes from `pass`, `fail`, and `inconclusive`. |
| `AUT-CHK-002` | `item.record_many` MUST admit a closed complete check-result value with required operation, input-basis, executor, outcome, summary, and output-digest fields and MUST reject malformed, unknown, or over-bound content before partial mutation. |
| `AUT-CHK-003` | A check-result item MUST be satisfied only when operation ID and digest equal the declaration and the recorded outcome is accepted; a valid mismatch MAY be stored but MUST remain unsatisfied. |
| `AUT-CHK-004` | Every complete check-result value MUST fit within 32 KiB canonical encoded JSON; 128 such values derive a 4 MiB per-attempt product without adding an aggregate error or a second storage lifecycle. This stored per-attempt product does not permit a 4 MiB atomic request: `AUT-DIRECT-017` still bounds every complete record-many input. |
| `AUT-CHK-005` | Value-bearing next, observation, and preview projections MUST expose exactly operation ID, outcome, operation digest, input-basis digest, and output digest and MUST omit free-text descriptor, executor, and summary fields. Compact-item projections MUST identify the item as `check_result` and MUST expose no recorded value fields. |
| `AUT-CHK-006` | Observation MAY expose one `stdin_template` of at most 16 KiB only on the `item.record_many` template for the first unsatisfied check result, prioritizing required declarations; no template may execute the operation or invent caller-supplied result values. |
| `AUT-CHK-007` | `evidence.read` MUST return a complete check-result value in one terminal page under the existing current-reference and freshness rules. |
| `AUT-CHK-008` | Podway MUST treat every check-result field as caller supplied and MUST NOT claim factual verification, authenticate the executor, inspect external bytes, compare the input basis with the worktree, execute a command, or store logs. |
| `AUT-CHK-009` | Observation MUST expose one closed `unsatisfied_reason` (`operation_id_mismatch`, `operation_digest_mismatch`, or `outcome_not_accepted`) only on a stored, unsatisfied check result, naming the first failed declaration comparison in that order. The reason MUST derive from declaration/value alone, MUST be absent from compact-item projections, and MUST NOT claim that an operation ran. |

### Typed conditions and option guards (AUT-GRD-001–010)

| ID | Normative requirement |
|---|---|
| `AUT-GRD-001` | Procedure v2 MUST admit only the closed typed predicate vocabulary, one to four AND-combined predicates per condition, and the operator/item-type pairings defined by ADR-0028. |
| `AUT-GRD-002` | `required_when` MUST reference only earlier unconditionally required items in the same definition and MUST reject self-reference, chains, cycles, later controllers, and condition-bearing items whose authored `required` is true. |
| `AUT-GRD-003` | Completion MUST derive `required_now` and satisfaction atomically from one authoritative active-attempt snapshot; a missing required controller MUST make the condition unevaluable and leave `required_now` false. |
| `AUT-GRD-004` | Option guards MUST read only explicitly selected items from required, dominating, fresh `evidence_from` references and MUST reject optional, unselected, current-decision, cross-attempt, or cross-session sources. |
| `AUT-GRD-005` | Running next MUST preserve every authored option, publish authoritative `allowed_option_ids`, and, when any guard exists, publish one complete bounded three-valued status for every option. |
| `AUT-GRD-006` | Observation active items MUST expose authored `required`, derived `required_now`, and bounded predicate status without exposing text or list content. |
| `AUT-GRD-007` | Decision execution MUST validate identity and revision fences before evidence freshness and evidence freshness before guards; each rejected check MUST write no state. |
| `AUT-GRD-008` | An unmet or unevaluable selected guard MUST return non-retryable exit-1 `OPTION_GUARD_UNSATISFIED` with closed bounded details, while stale evidence MUST retain retryable `EVIDENCE_REFERENCE_STALE`. |
| `AUT-GRD-009` | Omitted condition fields MUST preserve existing canonical Procedure bytes, preset digests, stored snapshot reconstruction, and unguarded runtime behavior without a SQLite migration. |
| `AUT-GRD-010` | Authoring lint MUST replace count-based option and cycle warnings with distinct-label, concrete-criteria, and operationally distinguishable rework-route diagnostics. |

## Retained JSON, error, and release requirements

| ID | Normative requirement |
|---|---|
| `AUT-JSON-001` | The retained version identity, Procedure validation, start/begin/disposition/reset/archive/list/show/purge, status/next/observation, item mutation, and graph transition families MUST have their canonical closed result schemas. Other retained generic results keep their expressly documented JSON-object boundary. The former daemon-status, mode-plan/apply, detached-admission, and job clauses are retired. |
| `AUT-JSON-002` | Identity, revision, attempt, digest, idempotency, direct lock deadline, and adopted recovery failures MUST use their canonical closed detail schemas. The former daemon and socket clauses are retired. Generic detail objects remain only where the outer error schema expressly permits them. |
| `AUT-JSON-003` | Named result and error-detail families MUST carry an unambiguous schema identifier or discriminator; explicitly retained generic JSON-object families keep their compatibility boundary. |
| `AUT-JSON-004` | A closed v1 object MUST reject unknown fields; adding fields requires a new schema identifier or discriminator version rather than an undocumented additive-field exception. |
| `AUT-ERR-005` | Recovery recipes MUST derive only from existing public details and preserve code, retryability, and exit class. They MUST NOT copy item values, full requests, environment variables, file contents, credentials, or artifact bytes. Former admission facts are retired. |
| `AUT-REL-001` | Podway MUST publish and support only native thin-arm64 `aarch64-apple-darwin` artifacts built and verified on an untranslated arm64 macOS host. |

The [error specification](errors-and-exit-codes.md#structured-recovery-recipes)
owns RecoveryRecipe v2 commands, authorized destructive cases, and detail IDs.
The [release specification](../operations/release-and-packaging.md) owns normal
clean full distribution and separately confirmed patch guards. The nonpublishable
development chain is independent of those publication claims. No historical
service retirement or installed runtime mutation is authorized by qualification.

## Retired requirement IDs and clauses

These guarantees applied to the previous daemon generation. They are historical
requirements, not current acceptance criteria or aliases for new direct IDs.

| IDs or clauses | Retired guarantee |
|---|---|
| `AUT-PATH-002`–`003` | Daemon path discovery and installation verification |
| `AUT-DAEMON-001`–`009` | LaunchAgent metadata, daemon startup/readiness/control, wait and installation |
| `AUT-HOME-001`–`003` | Global home resolution, account runtime layout and LaunchAgent path |
| `AUT-HOME-004` jobs/global-home clauses | Durable jobs and copying constraints for the removed product-global home |
| `AUT-SOCK-001`–`005`, `AUT-SEC-001`–`004` | Endpoint selection, socket security/recovery and per-user daemon singleton |
| `AUT-CONTRACT-001`, `003`–`005`; `002` IPC clause | IPC-bearing manifest, CLI/daemon equality, connection/install rejection and daemon status identity |
| `AUT-ID-010`–`011` | Named-mode ownership agreement and mode plan/apply |
| `AUT-START-002`–`003` | Durable admitted-start snapshot and post-admission daemon execution guarantee |
| `AUT-ADMIT-001`–`003` | Job/sequence admission envelopes, continuing execution after timeout/disconnect, transport-unknown classification |
| `AUT-RECON-001`–`003` | Job lookup, non-terminal state, failed/cancelled terminal receipt retention |
| `AUT-OBS-001`–`002`, `005` | Queue barriers, quiescence, queue identities, `--wait-for-idle` and `--after-job` |
| `AUT-LIF-014` detached clause | Detached interactive policy behavior |
| `AUT-JSON-001` daemon/mode/detached/job clauses; `002` daemon/socket clauses | Closed daemon-generation result and error families |
| `AUT-ERR-001`–`004`; `005` admission clause | Daemon/mode/socket mapping, admission-false reporting and RecoveryRecipe v1's old read-only command set |
| `AUT-REL-002`–`003` | Two-binary archive, paired CLI/daemon identity and offline pair verification |
| `AUT-REL-004` | Historical v0.1.0 tagging prerequisite; not a current version or release instruction |

Retirement does not authorize deletion, installation, stopping, or conversion of
existing historical services or user runtime state. Unsupported syntax fails
`COMMAND_RETIRED`; unsupported legacy state requires its explicit supported
recovery decision.

## Acceptance evidence and traceability

The executable checks are repository-owned; reading this table does not claim
that a fresh gate ran. Current development uses `make test`; full distribution
uses `make dist` only under its release authority. The current direct source and
packaged scenarios are mapped in the
[requirements traceability](../quality/requirements-traceability.md).

| Evidence ID | Current required scope | Historical provenance and retired portions |
|---|---|---|
| `AUT-T-PATH` | Controlled PATH, sanitized environment, arbitrary directory, single CLI identity | `RPATH006`, `DOLGI001`; daemon discovery/plist probes retired |
| `AUT-T-SOCK` | Retired | `RPATH003`–`RPATH006` socket and singleton probes |
| `AUT-T-CONTRACT` | Complete source/package manifest/schema/identity validation, source and candidate binding, malformed identity/drift rejection | `CONID003`–`006`, `REL12003`–`004`; peer equality/IPC/restart probes retired |
| `AUT-T-ID` | Replaced workspace/session, same revision on another session, stale replacement, attempt/item and guarded read fences | `CASID003`–`005` |
| `AUT-T-START` | Matching/mismatching digest, prepared creation, immutable snapshot, changed source, exact replay/key reuse | `PSTRT001`–`005`, `V2LIF-004`–`005`; admitted daemon execution retired |
| `AUT-T-LIF` | Prepared/begin/goals, forbidden prepared mutations, disposition currentness, eligible/force reset/replacement, cold-process reconstruction and stale replay fences | `V2LIF-002`–`005` |
| `AUT-T-RECON` | Commit/response-loss lookup, exact success replay, rejected absence, key reuse, retention and UUID fail-closed | `RECON001`–`005`; job states, cancelled/failure receipts and wait guarantees retired |
| `AUT-T-OBS` | Coherent write-free reads, closed compact/next/observation budgets and applicable templates | `MCONT004`, `MCONT006`, `DOLGI002`; idle barriers retired |
| `AUT-T-EVR` | All scale bounds, worst-case encoding, typed and stdin aggregate rejection, reconstruction, paging/token distinctions, graph-counter budgets and complete template coverage | `V2SCL-002`–`005`, `V2GSC-004`; IPC framing retired |
| `AUT-T-CHK` | Declaration/value bounds and digests, all outcomes, atomic recording, stale retry/rework, projections/templates/read-back, unsatisfied reasons and caller-trust boundary | `V2AST-002`–`006`, ADR-0033; daemon migration/restart retired |
| `AUT-T-GRD` | Typed predicate pairs, controller/selector/freshness rules, guard status, fence precedence, projections and unchanged canonical bytes | `V2GRD-002`–`006` |
| `AUT-T-JSON` | Current result/detail discriminators, malformed/unknown field refusal and no retired metadata | `MCONT001`–`006` |
| `AUT-T-DIST` | Same archived CLI bytes through independent extraction, identity, disabled hooks, lifecycle/fences/contention, output loss, exact reconciliation and bounded process/fixture cleanup | `DOLGI005`, `REL10001`–`004`; foreground daemon modes/sockets retired |

| Active requirements | Origin and direct continuity | Evidence |
|---|---|---|
| `AUT-PATH-001`, `AUT-HOME-004`, `AUT-CONTRACT-002` | RPATH/CONID; `V2DFC-007`–`009` | `AUT-T-PATH`, `AUT-T-CONTRACT` |
| `AUT-ID-001`–`009` | CASID/V2LIF; `V2DFC-006`–`007` | `AUT-T-ID`, `AUT-T-LIF` |
| `AUT-START-001`, `004`, `AUT-LIF-001`–`015` | PSTRT/V2LIF; `V2DFC-006`–`007` | `AUT-T-START`, `AUT-T-LIF` |
| `AUT-RECON-004`, `AUT-OBS-003`–`004`, `006`–`012` | RECON/MCONT/V2SCL; `V2DFC-006`–`007` | `AUT-T-RECON`, `AUT-T-OBS`, `AUT-T-EVR` |
| `AUT-EVR-001`–`008`, `AUT-CHK-001`–`009`, `AUT-GRD-001`–`010` | V2SCL/V2AST/V2GRD, ADR-0033; `V2DFC-006`–`007` | `AUT-T-EVR`, `AUT-T-CHK`, `AUT-T-GRD` |
| Active `AUT-JSON-001`–`004`, `AUT-ERR-005`, `AUT-REL-001` | MCONT/V2AGT/REL; `V2DFC-007`–`009` | `AUT-T-JSON`, `AUT-T-DIST` |
| `AUT-DIRECT-001`–`020` | ADR-0038, `V2DFC-001`–`010` | Direct native/Store/protocol and packaged evidence mapped by current traceability |

## Example command sequence

Examples are local invocations; no external adapter capability is implied.
Variables stand for values retained from validated current responses. Each
mutation has its own caller-owned key; replay preserves its complete original
argv/input, not newly observed fences.

```bash
env -i PATH="<release-bin>:/usr/bin:/bin" podway version --json --identity

podway --json --worktree "$WORKTREE" --timeout 25s \
  --if-workspace-uuid "$WORKSPACE_UUID" observe

podway --json --worktree "$WORKTREE" --timeout 25s \
  --idempotency-key "$START_KEY" --if-workspace-uuid "$WORKSPACE_UUID" \
  start --procedure ".podway/procedures/task.yaml" \
  --expect-procedure-digest "$PROCEDURE_DIGEST" --task "Implement the requested task"

podway --json --worktree "$WORKTREE" --timeout 25s \
  --idempotency-key "$BEGIN_KEY" --if-workspace-uuid "$WORKSPACE_UUID" \
  --if-session-id "$SESSION_ID" --if-session-revision 0 begin

podway --json --worktree "$WORKTREE" --timeout 25s \
  --idempotency-key "$ITEM_KEY" --if-workspace-uuid "$WORKSPACE_UUID" \
  --if-session-id "$SESSION_ID" --if-attempt "$ATTEMPT_ID" \
  --if-item-revision "$ITEM_REVISION" set "$ITEM_ID" "$VALUE"

podway --json --worktree "$WORKTREE" --timeout 25s \
  --if-workspace-uuid "$WORKSPACE_UUID" outcome lookup --idempotency-key "$ITEM_KEY"

podway --json --worktree "$WORKTREE" --timeout 25s \
  --idempotency-key "$COMPLETE_KEY" --if-workspace-uuid "$WORKSPACE_UUID" \
  --if-session-id "$SESSION_ID" --if-session-revision "$SESSION_REVISION" \
  --if-attempt "$ATTEMPT_ID" complete
```

Remote protocols, multi-user security, executable Procedures, parallel cursors, fan-out, global task copies,
Dolgorae Workflow/Role/ToolRun execution, and other release platforms require
separately adopted authority.
