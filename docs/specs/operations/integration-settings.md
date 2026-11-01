# Integration Settings

## Authority and availability

[ADR-0041](../../architecture-decision-records/0041-user-integration-settings.md)
permits a narrow read-only integration-settings exception to the worktree-local
Direct model. [ADR-0042](../../architecture-decision-records/0042-separate-user-settings-namespace.md)
supersedes only its user-settings location. Accepted ADR bodies remain historical
authority; the effective path below follows ADR-0042.

Workspace v3 and user-config v1 parsing, pure permission resolution and the
explicit Engine native-read seam are implemented. Default initialization still
writes workspace v1. The explicit CLI/JSON permission query and its public
configuration errors and repository-only metadata verification are implemented.
The integrated normal Store compatibility matrix remains INTEG-005 work.
The [roadmap](../../roadmap/README.md#integ-dolgorae-integration-readiness)
owns implementation status and the [execution dossier](../../todo/TODO-podway-dolgorae-integration-readiness.md)
owns the unfinished delivery and verification plan. Adoption alone changes no
installed binary, existing configuration or Direct behavior.

## Namespace and resolution

Worktree settings remain `<worktree>/.podway/config.yaml`. User settings use only
`$HOME/.config/podway/config.yaml`; they are outside workspace initialization,
runtime, maintenance and removal ownership. A HOME-root Git worktree keeps its
ordinary project configuration in `.podway` and its separate user document in
`.config/podway`. Initializing or explicitly removing that worktree's `.podway`
tree MUST preserve the user document. There is no old-user-path migration,
`XDG_CONFIG_HOME`, passwd lookup, alternate-root flag or user-configuration writer.

Closed workspace v3 and user-config v1 documents expose optional
`integrations.dolgorae.enabled`. Missing objects or values inherit; explicit
`true` and `false` are distinct strict booleans. Resolve project value, then user
value, then `false`. Workspace v3 excludes legacy `mode` and `job_queue`; released
workspace v1/v2 schemas, canonical serialization and default v1 initialization
bytes MUST remain unchanged. No configuration is rewritten automatically.

Only the explicit query reads user settings. It validates syntax/options and
contract generation, discovers Git, validates project config, resolves HOME,
validates user config, then merges values. Invalid user configuration MUST fail
even when a project override wins. Missing directories/files mean inheritance;
a present empty file is invalid. Each document is one UTF-8 YAML document bounded
to 65,536 bytes, depth 16 and 1,024 nodes. Reject unknown fields, duplicate keys,
nulls, wrong types, anchors, aliases, tags and merge keys.

HOME selection requires a lossless nonempty absolute existing directory. After
canonicalizing/opening that root, use descriptor-relative nofollow reads of
`.config`, `podway` and `config.yaml`. Require real directories and a regular
bounded file; unsafe, unreadable or changed witnesses fail closed. Create no
files or directories. Two bounded configuration reads are not an atomic snapshot
across both files.

Ordinary Direct commands MUST NOT read HOME/user settings or probe Dolgorae,
even when project configuration enables the permission. Core, Store, Procedure
snapshots, digests and receipt identity acquire no user-settings dependency.

## Public query

`podway integration show dolgorae` maps to `integration.show`. Engine performs
this Git/config-only read before or after workspace initialization without
opening SQLite, acquiring a maintenance lease or reading workspace/session
identity. It launches no external process and reads no Dolgorae private home.

Both accepted contract generations use Output v4 with the same new closed result:

```json
{"schema":"podway.integration-settings-result/v1","integration":"dolgorae","enabled":false,"source":"default"}
```

`source` is exactly `project`, `user` or `default`. The envelope contains neither
`workspace` nor `session`. Accept worktree, timeout, rendering and contract-generation
options; reject mutation keys, identity/revision fences and destructive flags as
`REQUEST_INVALID`. Reject unknown integration names before configuration I/O.
The query takes no stdin document and preserves existing syntax, retired-command
and generation precedence. Human output explains permission and source without
claiming connection readiness.

Invalid project input uses `WORKSPACE_CONFIG_INVALID` without legacy reset advice.
Invalid or unavailable HOME/user input uses `USER_CONFIG_INVALID`,
exit 5, nonretryable, empty details and summary
`User configuration is invalid or unavailable.` Neither machine failure exposes
file contents or native home paths. Preserve existing Git/selector failures.
Expected configuration failures are not internal failures.

Config owns document parsing and pure resolution; Engine owns native reads and
typed failures; Protocol owns public result/error validation; CLI owns grammar,
rendering, help and completion. Register new routes, result/error selections and
manifest/adapter-catalog identities coherently without widening old families.
No Store predecessor, schema migration or reset is introduced.

## Permission and external ownership

Enabled permission is neither capability discovery nor proof of actual execution.
A cooperating external adapter must adopt this query before starting new
assignments. Disabled permission does not cancel accepted work or disable result
lookup, reconciliation, receipts, semantic acceptance or explicit completion.
Unknown prior assignments MUST NOT be redispatched with a new key. Podway does
not enforce external dispatch or make an unmodified adapter toggle-aware.

Podway gains no provider adapter, worker scheduler, model invocation, command
runner, network client, credential store, account state, resident process, global
registry or implicit Direct fallback. Dolgorae source, configuration, installation
and account state remain externally owned.

Repository-only verification may inspect an explicitly selected existing
Dolgorae executable using bounded public metadata probes and frozen offline
success/error definitions. Preserve process status, complete response validation,
exact executable identity and version comparison as separate checks. Reap only
verifier-owned children and write reports only in caller-owned disposable roots.
A metadata pass establishes no accepted task/result, receipt, worker disposition,
real adapter qualification or activation authority. Missing or unsupported exports
remain explicit integration limitations. Release, installation and consumer
adoption retain their separate authorization boundaries.

## Repository-only metadata verification

Run the repository tool with explicit executable files and an exclusively new
report in an existing caller-owned canonical directory outside canonical source:

```sh
python3 tools/verify_dolgorae_connection.py --podway /absolute/path/to/podway \
  --dolgorae /absolute/path/to/dolgorae --output /private/tmp/owned-run/metadata.json
```

`--timeout-seconds` is an integer from 1 through 60, default 15. The selected
Python must provide the dependencies in `tools/requirements-dev.txt`; missing
validation libraries or missing/corrupt offline definitions produce `incomplete`.
No installation, executable search, Profile selection or fallback is performed.
The verifier resolves regular executable files up to 128 MiB, pins their SHA-256 and native
witnesses, and rechecks both selected bindings after the probes. Only these
commands run, with no stdin:

1. selected Podway `--json version --identity`;
2. selected Dolgorae `version --json`;
3. selected Dolgorae `runtime capabilities`.

One monotonic deadline covers pinning, validation setup and all probes. Each
probe retains at most 1 MiB stdout and 64 KiB stderr. It may observe one extra
byte to establish a conclusive ceiling violation, discards that byte and records
observed counts separately from retained counts/digests. Cleanup kills only its own process group and has bounded leader-reap
and group-observation allowances of one second each after the probe budget.
On Darwin, native group inspection excludes zombies, which retain no descriptors;
unknown group state prevents successful cleanup. No general host process scan is
performed. Other hosts cannot make a stronger cleanup claim than their available
native observation; the supported platform remains native Apple Silicon macOS.

Full offline JSON Schema validation uses Podway Output v4/version-result v2,
stateless Dolgorae version-output v1 and Machine v4 `capabilities` with capabilities
v2 data. The [frozen registry](../../../contracts/dolgorae-metadata-v1/README.md)
retains original upstream bytes, embedded resource scopes and complete success/
error references from its locked commit; IDs never authorize URL retrieval.
Recognized errors must validate completely. Only the frozen stable codes
`PROTOCOL_VERSION_UNSUPPORTED` and `UNSUPPORTED_SCHEMA_VERSION` map to
`unsupported`; valid other errors fail. Human stderr text never identifies an
unsupported operation. Schema validation enforces the definitions' actual bounds;
a timestamp pattern alone is not an additional calendar-validity guarantee.

After both Dolgorae success contracts validate, compare exactly
`version.version == "v" + capabilities.data.dolgorae_version`, preserving every
suffix and character's case. Complete malformed JSON, supported invalid fields,
wrong commands, inconsistent metadata, contradictory natural nonzero success,
changed executables and conclusive output ceiling violations fail. Identifiable
unsupported families or versions are unsupported. Missing executables, launch
failures, deadlines and absent/truncated observations are incomplete unless a
conclusive defect has already been observed. Natural process status, verifier
termination, full response validation and cleanup are separate evidence fields.

The outcome order is `fail > unsupported > incomplete > pass`, with exit codes
1, 3, 4 and 0 respectively. Invalid arguments or unsafe/existing output paths exit
2 without an output requirement or overwrite. Reports use the internal schema
`podway.dolgorae-connection-report/v1`, retain pinned identities, per-probe checks,
limits and `probe_outcome`, and always set `activation_qualified` to `false`.
They record the configured timeout, byte ceilings and cleanup allowances, with
output counts/digests rather than raw stdout/stderr or credentials.
A metadata result never proves task/result/receipt or worker disposition, and
never authorizes backend repair, activation or a replacement assignment.
