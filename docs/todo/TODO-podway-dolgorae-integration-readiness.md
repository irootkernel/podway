# Podway Dolgorae Integration Readiness

## 1. Status and authority

- Document state: `Adopted`
- Owning epic: [INTEG](../roadmap/README.md#integ-dolgorae-integration-readiness)
- Repository scope: Podway only
- Target release: unassigned
- Architectural decisions: [ADR-0041](../architecture-decision-records/0041-user-integration-settings.md) and [ADR-0042](../architecture-decision-records/0042-separate-user-settings-namespace.md)

This is the execution dossier for unfinished readiness work. The roadmap alone
owns task order and status. It describes intended changes, not currently
implemented commands or configuration. Design adoption authorizes no product
implementation, Dolgorae change, release, installation or runtime activation.
The [real-executor candidate](TODO-podway-executor-integration.md) retains all
unselected adapter and live-qualification obligations.

## 2. Verified context and evidence

The current Direct model keeps authority under the owning worktree and forbids
provider execution and user-global runtime state. ADR-0041 permits only the
bounded user-settings exception, with ADR-0042 separating its path from the
workspace configuration/removal namespace. Existing closed workspace v1/v2 assets and
`WorkspaceConfigV1` reject an `integrations` field; configuration tests preserve
explicit default bytes and canonical digests. The workspace resolver validates
project configuration before Store access and has no user-settings lookup.

The generic CLI/JSON boundary and host-neutral declaration/package/receipt
foundation already exist. Podway does not dispatch workers or authenticate
executor claims. An external consumer must adopt the new permission query before
it controls that consumer's dispatch. A newer executor source contract is not
proof of an installed binary's capabilities or a working live adapter.

Current source and authority locators are the [configuration implementation](../../crates/podway-config/src/lib.rs),
[workspace resolver](../../crates/podway-engine/src/workspace.rs),
[configuration regressions](../../crates/podway-config/tests/int_phase4_workspace_config.rs),
[repository structure](../architecture/repository-structure.md) and
[delegation contract](../specs/interfaces/supervised-delegation.md).

## 3. Goals, non-goals, and scope

Provide user defaults at `$HOME/.config/podway/config.yaml`, worktree overrides at
`.podway/config.yaml` and an explicit, read-only effective-permission query.
The default is disabled. Missing Dolgorae and unusable user configuration must
not make ordinary Direct operations depend on an external backend or home.

Do not add a provider adapter, command runner, network client, credential store,
worker scheduler, auto-dispatch, model invocation, global workspace registry,
resident process, group/parallel Action semantics or delegation fallback.
Do not modify Dolgorae source, configuration, installed binaries or account state.
Do not convert built-in/Aquarium Procedures to delegation or relax receipts.
No global configuration writer or CLI enable/disable command is included.

## 4. Accepted design decisions and interfaces

### Settings and resolution

Introduce closed `podway.workspace/v3` with only `schema`, `procedure_paths`,
`default_preset`, `ui` and `integrations` root fields. Preserve existing meanings
and defaults for paths, preset and UI; add optional `integrations.dolgorae.enabled`.
Reject legacy `mode` and `job_queue` in v3. Keep released v1/v2 schemas, legacy
handling and canonical serialization unchanged; their omission means inheritance.
Default initialization continues to emit current v1 bytes. No automatic config
rewrite occurs. Older binaries reject v3 instead of silently ignoring its policy.

Introduce closed `podway.user-config/v1` with required `schema` and optional
`integrations.dolgorae.enabled`; it contains no project or runtime fields.
Both new nested objects are closed. Missing objects/`enabled` inherit; explicit
`true` and `false` are strict booleans. Reject null, quoted booleans, unknown
fields and duplicate keys. An explicit empty object may omit `enabled`.

The future complete minimum documents are:

```yaml
schema: podway.user-config/v1
integrations:
  dolgorae:
    enabled: false
```

```yaml
schema: podway.workspace/v3
integrations:
  dolgorae:
    enabled: true
```

Each file uses existing configuration ceilings: 65,536 bytes, depth 16 and
1,024 nodes. Require one UTF-8 YAML document with the existing parser's safety
rules, including refusal of anchors, aliases, tags, merge keys and nulls.
Preserve lossless native path handling outside document content.

Only the explicit integration query resolves user settings. It validates route
options/generation, discovers Git, reads/validates project config, resolves HOME,
reads/validates user config, then merges **project > user > false**. A present
invalid user file is an error even when project v3 specifies a boolean. This
validation precedence is separate from value precedence. Missing config files
or missing configuration directories inherit; an empty present file is invalid.

Use lossless `HOME` only for this query. Require a nonempty absolute existing
directory; no XDG/passwd fallback or alternate-root flag. Canonicalize/open that
selected root, then descriptor-relative nofollow-open `.config`, `podway` and
`config.yaml`. This fixed HOME-derived path ignores `XDG_CONFIG_HOME`; never
look up the old `~/.podway/config.yaml` as user settings or migrate it.
Require real directories and a regular bounded file; reject symlinks below the
selected home root, nonregular files, unreadable paths and changed witnesses.
Reuse Engine descriptor/revalidation patterns; never create a directory or file.
Two bounded file observations are not an atomic transaction across both files.

A HOME-root Git worktree retains its normal `.podway/config.yaml` project schema.
Its user settings remain a distinct `.config/podway/config.yaml` document, also
when a separate worktree queries them. Neither workspace initialization nor
confirmed removal of the HOME-root worktree's `.podway` tree owns the user file.

### Public query

Add `podway integration show dolgorae`; canonical command: `integration.show`.
It is an Engine-owned Git/config-only query and works before workspace init.
It never opens SQLite, takes a maintenance lease, reads a workspace UUID/session,
executes Dolgorae or reads its private home. Its Output v4 contains neither
`workspace` nor `session`, and returns this closed result:

```json
{"schema":"podway.integration-settings-result/v1","integration":"dolgorae","enabled":false,"source":"default"}
```

`source` is exactly `project`, `user` or `default`, identifying the winning value.
Use the same new result family in accepted contract generations 1 and 2; preserve
all existing selections and defaults. Register the explicit route/result in the
catalog, Protocol validation and Output v4 command memberships. Keep Error v2.
Human output explains effective permission and source without claiming readiness.

Accept `--worktree`, `--timeout`, rendering controls and contract generation.
Reject idempotency keys, UUID/session/attempt/item/goal fences and destructive
flags as `REQUEST_INVALID`; no durable identity is available to honor them.
Reject unknown integration names before configuration I/O. Respect existing
syntax/retired-command/generation precedence. The query takes no stdin document.

`enabled: true` is permission, not capability discovery or runtime qualification.
`false` instructs a cooperating external adapter not to begin new assignments;
it neither cancels accepted work nor disables result lookup, reconciliation,
receipts, acceptance or explicit completion. An unknown prior assignment cannot
be redispatched with a new key. Podway does not enforce external dispatch, and
an adapter that has not adopted this query does not become toggle-aware.

### Ownership

Config owns new document types, bounded parsing and pure optional-boolean merge.
Engine owns the explicit native configuration query and typed failures; add a
Git-only operation rather than normal Store-backed workspace resolution.
Protocol owns result/error validation, CLI owns grammar, rendering, help and
completion. Core/Store gain no settings, home lookup or provider dependency.
Ordinary Direct paths may validate the selected project schema but must never
read HOME/user config or probe Dolgorae, including when project enables it.

### Connection verification

Add repository-only `tools/verify_dolgorae_connection.py`, not a shipped route or
adapter. Inputs are explicit `--podway <file>`, `--dolgorae <file>` and
`--output <new-json-file>`; `--timeout-seconds` accepts 1..60, default 15.
Resolve and pin regular executable bytes, record their SHA-256, and recheck them
after all probes. Never build, install, upgrade or patch the selected backend.

Run only Podway `--json version --identity`, Dolgorae `version --json` and
`runtime capabilities`, without Profile selection. Keep process status separate
from complete response validation. No gateway, account initialization, credential
or model is needed. Keep source revision distinct from selected executable
identity; no checkout or version label proves installed export support.

The supported metadata contracts are fixed as follows. Schema identifiers are
offline registry keys, not permission to fetch a URL.

| Probe | Supported response |
|---|---|
| Podway identity | Existing `podway.output/v4` with `podway.version-result/v2`, validated by the existing identity contract |
| Dolgorae version | `https://dolgorae.local/schema/version-output/v1`: closed stateless `name`/`version` object, not a Machine envelope |
| Dolgorae capabilities | `https://dolgorae.local/schema/machine-output/v4`: `schema_version: 4`, `command: "capabilities"`, with `https://dolgorae.local/schema/capabilities/v2` data |

Use these original Dolgorae source bytes at commit
`5fa36ad896a5bb417bd3ef14e1e4e96caa599d80`; paths are relative to that source
repository, not runtime dependencies on a neighboring checkout:

| Source | SHA-256 |
|---|---|
| `docs/protocol/dolgorae-version-v1.schema.json` | `f1f98c3d5bca2f57c159e88f9239aeb9d56dc9f8d94c2c04ea08f1968107b64f` |
| `docs/protocol/dolgorae-machine-v4.schema.json` | `c88500ee28e68aaeea6c12944f67d604889d53fa3544943c13b992d0a169a665` |
| `docs/protocol/dolgorae-capabilities-v2.schema.json` | `adcecc3bfb5346abe0c8217f97318bf6d7eccdf4438d0853a36e8a4154cf4828` |
| `docs/protocol/dolgorae-error-contract-v4.json` | `19eb4af3d76567be12c38ecd27e7928c0d1f576a51a5f066c0981ce63c4d02ac` |

INTEG-004 freezes the supported original definitions and all referenced definitions
needed for these success/error validations under `contracts/dolgorae-metadata-v1/`.
Its lock maps schema IDs to local files, original commit/path and byte hashes;
retain upstream IDs/bytes and applicable notices. Include the recognized error
contract and its relevant reference closure. Register an explicit supported
unsupported-operation stable-code mapping only from that frozen authority;
never invent a code or classify by human message text. Routine verification uses
this local registry only: no adjacent source, online fetch or unpinned tree.
Missing/corrupt definitions are incomplete verification prerequisites.

Validate both response contracts before comparing versions. Require exactly
`version_output.version == "v" + capabilities_envelope.data.dolgorae_version`.
Preserve complete prerelease/build suffixes and case; do not trim, drop suffixes
or generalize the prefix rule to future response families. The documented prefix
difference passes, actual full-text disagreement fails. An identifiable unknown
family/version is unsupported; an invalid instance of a supported family fails.

One monotonic deadline covers all probes without renewal. Bound each captured
stdout to 1 MiB and stderr to 64 KiB. Reap each child; on budget/output failure
terminate only the verifier-owned bounded process group and report the failure.
Exclusively create the output in the caller-owned disposable evidence root;
never overwrite a report or place evidence in canonical documentation.

The internal report identifies `podway.dolgorae-connection-report/v1` and records
`probe_outcome`, `activation_qualified: false`, pinned executable identities,
per-probe checks and evidence limits. Its outcome is `pass`, `fail`, `unsupported`
or `incomplete`. Exit codes are 0 for metadata pass, 1 for failure, 2 for invalid
arguments/unsafe or existing output target, 3 for unsupported contract and 4 for
incomplete observation. Usage failure does not overwrite or require a report.

Classify deterministic controls and observations with these rules:

| Observation | Outcome |
|---|---|
| Every selected response satisfies supported validation, process-status, version and executable-identity checks | `pass` |
| Identifiable unsupported family/version, or explicit unsupported operation validated under the frozen recognized error contract and stable-code mapping | `unsupported` |
| Complete syntactically malformed JSON, a complete schema-valid recognized error whose stable code is outside the unsupported-operation mapping, a supported instance with invalid fields/command/data, inconsistent complete metadata, exact version disagreement, replaced executable, conclusively observed output ceiling violation, or nonzero status contradicting a complete valid supported success response | `fail` |
| Missing executable, launch failure, deadline, absent/truncated/untrustworthy observation or missing/corrupt local validation definitions, without an already conclusive defect | `incomplete` |

If observations differ, choose `fail > unsupported > incomplete > pass` and
retain every per-probe result. Missing output or timeout does not erase an
already established failure. Unidentified human error prose cannot establish an
unsupported contract. Preserve completed checks on incomplete observation and
infer no executor/task state from missing output. No outcome creates, repairs or
redispatches backend work. Capture no credentials. Metadata pass never supplies
an accepted task/result, worker disposition, receipt or activation qualification.

## 5. Failure handling and compatibility boundaries

- Invalid project config uses existing `WORKSPACE_CONFIG_INVALID` (exit 5,
  nonretryable, `{}` details) on the new query. Do not reuse the current legacy
  reset guidance for this route or change other routes' error precedence.
- Add `USER_CONFIG_INVALID` (exit 5, nonretryable, `{}` details) for invalid HOME,
  user config syntax/schema/bounds, unsafe paths, read errors or replacement.
  Its summary is `User configuration is invalid or unavailable.` Do not expose
  file contents, credentials or native home paths in machine details.
- Preserve current Git/selector errors. Unknown name/options use
  `REQUEST_INVALID`. Expected configuration failures are not `INTERNAL_ERROR`.
- Missing backend never affects settings lookup or Direct work. Unsupported
  real integration remains unavailable; no implicit Direct fallback is added.
- New assets/routes/errors require coherent manifest and adapter-catalog
  generation through existing tools. Consumers re-pin the complete manifest;
  no new generation, changed old family or inferred cross-version support occurs.
- No Store schema, predecessor declaration or automatic migration/reset is added.
  A later installed candidate must follow existing writer-contract admission.
- Turning permission off preserves historical outcomes and recovery. Settings
  are not persisted into Procedure digests, session snapshots or receipt identity.

## 6. Roadmap ownership, dependencies, and traceability

The roadmap owns numeric order and all status. Task delivery is bounded as follows:

| Task | Required delivery and handoff |
|---|---|
| INTEG-001 | Reconcile ADR-0041/0042, the separated user-settings namespace and root AGENTS.md global-state wording; specify the narrow settings exception in applicable specification/architecture owners without claiming other global runtime authority. Preserve accepted historical ADR text. |
| INTEG-002 | Deliver new config schemas/types/parsers, strict optional merge and descriptor-safe Engine read seam; prove HOME-root worktree/query/init/removal isolation, preserving v1/v2/default bytes and Direct independence. |
| INTEG-003 | Deliver typed Engine query, CLI/JSON result, error catalog/union/route memberships, help/completion, coherent manifest/catalog generation and Store-free query behavior with an authentic previous-manifest Store. |
| INTEG-004 | Deliver the pinned local metadata/error validation bundle and verifier, exact prefix comparison, bounded child/IO cleanup and deterministic outcome controls and an honest observation against an existing explicitly selected Dolgorae when available. Report unsupported/incomplete real-path limits without backend mutation. |
| INTEG-005 | Integrate the explicit manifest continuity/refusal/query matrix and all regressions, pass development checks, promote durable owners and prepare later independent Epic validation/explicit acceptance and dossier retirement. |

Required enduring owners include the CLI/JSON/error and release specifications,
system/repository architecture and testing guidance. The existing live candidate
keeps both Primary-host mappings, controller/configuration ownership, complete
byte domains, actual accepted-task/result/round/disposition proof, response-loss
recovery and live comparative acceptance. No TYPED-wide dependency is selected.

## 7. Verification and release acceptance

- Config tests cover all missing/true/false precedence combinations, empty nested
  objects, v1/v2 inheritance, unchanged default/canonical bytes, unknown/new schema
  refusal, null/type/duplicate/YAML hazards and every ceiling at/over its boundary.
- Engine/CLI tests use owned worktrees and private HOME fixtures. Cover pre-init
  query, absent user/project files, malformed user with explicit project override,
  invalid project before user, absent/relative HOME, unsafe links/nonregular files,
  unreadable inputs and deterministic root/file replacement with no side effects.
- In an owned HOME-root worktree, retain project/user schema separation and valid
  settings lookup. A separate worktree must read the user document without
  misinterpreting the HOME-root workspace config. Init and explicitly confirmed
  `.podway` removal preserve user settings; use no real user home.
- Verify result/error schemas, both generations, rejected flags/names, human output,
  help/completion and exhaustive catalog/Protocol/output/error membership agreement.
- Prove zero Store/lease creation or access on query and zero external process launch.
  Preserve receipt/recovery successes and refusals; policy changes never fabricate
  acceptance. Use the explicit Store matrix below, rather than assuming a new
  manifest can continue an undeclared previous writer.
- Verifier controls accept stateless version output and the specified Machine4
  capabilities command/data. Test valid prefix/suffix comparison, actual suffix or
  version disagreement, wrong command/fields, identifiable unsupported versions,
  explicit recognized unsupported errors, complete schema-valid recognized errors
  outside the unsupported-operation map (`fail`), complete malformed JSON versus
  timeout-truncated output, absent output and output ceilings,
  timeout, changed binaries and contradictory process status. Test outcome priority,
  bounded reaping, exclusive report creation and offline frozen-definition lookup.
  These prove metadata inspection, never real adapter/model execution.
- Use focused registered suites and existing Make targets first, then required
  `make test` before sharing executable/contract delivery. Route long/noisy checks
  through configured Gaori commands. Review complete diff/status after commands.

### Manifest-specific acceptance

| Fixture | Required behavior |
|---|---|
| Store created by the candidate with its own manifest | Direct reads, fresh writes and exact original-request replay preserve behavior with absent/poisoned HOME and either valid project toggle, without user-global lookup. |
| Authentic Store created by a verified selected previous-manifest CLI | Candidate reads, fresh writes and retained-original-request replay refuse `WORKSPACE_CONTRACT_MISMATCH`, preserving Store/session/filesystem state and creating no side effects. |
| The same previous-manifest Store with valid Git, project/user config and HOME | `integration.show` succeeds without SQLite or maintenance-lease admission; invalid config or unavailable HOME still rejects through its normal query contract. |

Follow [supported Store continuity](../implementation-tips/testing.md#supported-store-continuity)
and the [upgrade contract](../specs/operations/release-and-packaging.md#upgrade).
Create authentic previous Stores through the verified previous CLI in owned
fixtures; retain exact binary/manifest and original request/response identities.
Do not fabricate a writer row, add a predecessor or migrate/reset/rewrite a Store
to obtain a pass. These cases grant no release or installation authority.

Readiness closeout requires implemented local behavior, the matrix above, honest
connection evidence/limits, durable documentation and separate whole-Epic validation
and owner acceptance. Unavailable future exports do not block this bounded scope;
they remain explicit unmet prerequisites for real integration in the Candidate.
Release/distribution, provider-sensitive live QA, installation and default/backend
activation require separate authorization. No `make dist` claim is part of INTEG.

## 8. References

- [ADR-0041](../architecture-decision-records/0041-user-integration-settings.md)
- [ADR-0042](../architecture-decision-records/0042-separate-user-settings-namespace.md)
- [Generic CLI/JSON integration](../architecture-decision-records/0010-generic-cli-json-integration.md)
- [Direct architecture](../architecture-decision-records/0038-remove-the-daemon.md)
- [Contract generations](../architecture-decision-records/0039-opt-in-contract-generations.md)
- [Delegation contract](../specs/interfaces/supervised-delegation.md)
- [Adapter mapping review](../implementation-tips/delegation-adapter-contract-review.md)
- [Release compatibility](../specs/operations/release-and-packaging.md)
- [Testing guidance](../implementation-tips/testing.md)
