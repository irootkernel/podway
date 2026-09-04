# Release Upgrade Hands-on QA

This repository-specific scenario verifies that one stable Podway release can
hand durable state to the next stable release candidate and that the candidate
still honors the supported deletion boundaries. Run it once during every full
Aquarium release-QA pass. It is not a `make dist` stage, does not replace the
packaged conformance suite, and does not publish a tracked receipt.

## Inputs and result

The scenario requires:

- the exact clean candidate commit and intended stable version selected by
  release QA;
- the previous release established by the pass: the latest non-draft,
  non-prerelease GitHub Release whose tag is reachable from that candidate, or an
  exact tag explicitly confirmed by the user;
- that release's Apple Silicon archive, checksum, and provenance;
- the canonical SQLite schema version declared under
  [Upgrade](../specs/operations/release-and-packaging.md#upgrade) at the selected
  previous tag and candidate commit; and
- native Apple Silicon macOS.

A confirmed first release has no upgrade baseline, so this scenario is not
applicable. Missing or ambiguous release assets, failed release discovery,
unsupported hosts, and an inability to build or run an exact isolated candidate
make the QA pass `INCOMPLETE`. A selected previous schema below v10 is also
`INCOMPLETE`: those Stores have no named-mode identity and migrate only in
production. When both versions are v10 or later, a schema-version difference
remains in scope and the candidate must migrate the Store forward in mode `dev`.
A reproducible mismatch against an expected behavior below is a finding.

## Safety boundary

Use the caller-assigned scenario fixture below its existing physical release-QA
evidence root. For Aquarium release QA, use the worker's assigned fixture; do not
create another top-level evidence root. Resolve the fixture with `pwd -P`, prove
that it is a private directory contained by the physical evidence root, and use
only that resolved path. Keep every download, extraction, build output, Git
fixture, runtime namespace, command transcript, and result below the scenario
fixture. Record the source repository's status before the scenario and prove it
has the same `HEAD` and `git status --porcelain` afterward, with no new or removed
entry in its common Git directory's `worktrees` registry.

Before dispatch, the release-QA coordinator may download only the selected
previous release's archive, checksum, and provenance into the assigned scenario
fixture. It must verify that the three files are regular non-symlink files, that
the checksum names and matches the archive, and that provenance names the
selected version, Apple Silicon target, archive digest, and matching `podway` and
`podwayd` identities. The dispatched worker receives only those verified local
files and retains Aquarium's no-network and no-credential constraints. It safely
extracts the archive without accepting absolute paths, parent traversal, links,
devices, or unexpected top-level members. Run the extracted binaries directly;
never install them.

Prefer a candidate archive only when its provenance binds it to the exact
candidate commit. Otherwise build only `podway` and `podwayd` from the exact
candidate with the pinned toolchain, `--locked`, release profile, offline Cargo
networking, and `CARGO_TARGET_DIR` below the scenario fixture. Confirm both
candidate binaries report the expected source commit, target, and one shared
build identity through `podway version --json --identity` and `podway --mode dev
daemon status --json` once the daemon is ready.

Do not use `prod`, `release-qa`, an installed service, `podway daemon install`,
`uninstall`, `start`, or `stop`, or `launchctl`. Run both generations as
foreground processes in mode `dev`. Pass both
`--mode dev` and
`PODWAY_DEV_HOME=<scenario-fixture>/runtime` explicitly on every daemon and
daemon-backed CLI invocation; do not rely on ambient defaults or an exported
environment. Omitting the mode selector selects `prod`, where `PODWAY_DEV_HOME`
has no effect. Omitting `PODWAY_DEV_HOME` in mode `dev` instead selects the
user-global `~/.podway/modes/dev` namespace outside the evidence root. Before
spawning any process, have the controlled runner fail closed unless its argument
list contains `--mode dev` and its environment map contains the exact nonempty
fixture runtime path. Also require the UTF-8 byte length of
`<scenario-fixture>/runtime/run/podwayd.sock` to be less than 104 bytes. After the
daemon becomes ready and before the first workspace mutation, require `podway
--mode dev daemon status --json` to report `configured_socket_path`,
`effective_socket_path`, and `socket_path` below that runtime root. A failed
preflight, an overlong socket path, or any reported path mismatch is
`INCOMPLETE`. Put all fixture Git worktrees below
`<scenario-fixture>/worktrees`. Create them as standalone repositories with `git
init`; never use `git worktree add` against the source repository. This raw
development endpoint is disposable and distinct from the installed production
namespace, Aquarium's persistent development runtime, and the purpose-bound
`make dist` release-qualification runtime.

After acquiring the previous assets, run the behavioral scenarios offline. Do
not read or mutate user-global Podway state, installed LaunchAgents, Aquarium
state, credentials, or Git worktrees outside the evidence root. Never edit a
Podway database, registry, socket, or runtime file to manufacture an outcome.

## Previous-generation setup and one-way handoff

1. Start the previous `podwayd --mode dev` against the disposable runtime and
   wait until the previous CLI reports that exact daemon ready.
2. Create the active-state worktree, initialize it with the previous CLI, and
   start and begin `small-change-v2`. Record `scope-summary` at the `inspect`
   placement. Capture the standard `status --json` projection and `observe
   --json`, including workspace UUID, session UUID and revision, Procedure ID,
   version and digest, current placement, attempt identity, item revision and
   value, and allowed actions.
3. Create the retained-state worktree. Complete the entire `small-change-v2` path
   with valid values, using nonempty English text for textual items. Select
   `ready`, record the closeout, attach a current `handed-off` disposition, and
   archive the terminal session. Capture `archive list` and `archive show
   --session-id <id> --verbose` in JSON.
4. With the previous CLI and the explicit disposable `PODWAY_DEV_HOME`
   assignment, run `podway --mode dev terminate`, then prove that the previous
   daemon exited and released its socket. Start the candidate daemon against the
   same runtime root, wait for readiness, and repeat the required socket-path
   checks. Do not start the previous generation again after this handoff.

## Scenario 1: active state survives the handoff

Query the active-state worktree with the candidate CLI. Every captured durable
field must retain the same value, except fields whose public contract explicitly
permits a new observation time or daemon identity. When the schema versions
differ at v10 or later, this open must migrate the previous Store forward without
losing those fields. The candidate must accept a normal `complete` transition and
advance the same session to `implement`.

Failure to open or migrate the previous Store, silent initialization of
replacement state, identity drift, lost or changed evidence, a different cursor,
or inability to continue the admitted Procedure is a finding.

## Scenario 2: retained state survives the handoff

Query the retained-state worktree with the candidate CLI. Both archive commands
must return the same archive slot, session identity, terminal revision, Procedure
identity, disposition, and verbose trace and decision-history identities and
evidence digests. Terminal archive projection does not expose prior attempt item
values as current `item_values`; compare their retained evidence identities
instead. The inactive session must remain immutable and must not become current
merely because a newer binary opened the workspace.

## Scenario 3: candidate deletion boundaries remain exact

Exercise these operations through the candidate CLI, using identities and
revision fences obtained from fresh JSON reads:

1. Run confirmed `archive purge` for the inactive session from scenario 2. The
   selected archive must disappear, while workspace initialization, Git state,
   and any unrelated current session remain unchanged.
2. Run confirmed `reset --all` against the active-state worktree from scenario 1.
   Current and inactive runtime history, attempts, jobs, receipts, and Store data
   must be removed and reinitialized under the new workspace UUID reported by
   the reset result. Workspace configuration, supported Procedure files,
   `.gitignore`, Git metadata, tracked files, and untracked non-Podway files must
   survive.
3. After confirming `status` reports no current session, run confirmed `workspace
   remove` using that new exact workspace UUID. Its complete `.podway` tree must
   disappear, while the Git worktree, `.git` relationship, HEAD, tracked files
   outside `.podway`, and unrelated untracked files remain unchanged. The command
   deliberately deletes tracked `.podway` content; that deletion is expected.

An over-broad deletion, a retained object that the command promises to delete,
loss of preserved Git or workspace content, or a result that cannot be
reconciled through the public read API is a finding.

## Cleanup and evidence

With the candidate CLI and the explicit disposable `PODWAY_DEV_HOME` assignment,
run `podway --mode dev terminate` and prove that neither generation has a live
process or socket. Remove the exact runtime, extracted previous archive,
candidate build directory, and disposable worktrees. Retain only bounded text or
JSON transcripts and the scenario result below the same assigned scenario
fixture; never retain copied credentials, databases, sockets, or executables.

The scenario passes only when all three behavioral scenarios and cleanup pass and
the source repository retains the recorded `HEAD` and porcelain status with no
change to its common Git directory's `worktrees` entries. Report the selected
previous release, candidate commit, commands, controlled environment, expected
and observed outcomes, evidence paths, findings, and gaps through Aquarium's
normal release-QA result. Do not create a Podway-specific evidence schema or add
the result to release provenance.
