# Release Upgrade Hands-on QA

Run this repository-specific cluster during every full Aquarium release-QA pass.
For the `v0.2.12` to `v0.2.13` cutover, upgrade means unchanged legacy refusal,
then an explicitly confirmed destructive reset and a working fresh direct
workspace. There is no daemon handoff, transparent migration, or named runtime
mode. This independent QA is not a `make dist` stage or release-provenance receipt.

## Inputs and result

Require the exact clean candidate and intended stable version, the selected
previous stable release, its verified Apple Silicon archive/checksum/provenance,
native Apple Silicon macOS, and verified frozen historical Store fixtures.
Select the latest non-draft, non-prerelease release reachable from the candidate
or an exact baseline explicitly confirmed by Master. Compare its declared SQLite
generation and writer contract with the candidate authority before execution.
A confirmed first release has no upgrade baseline and is not applicable.

Generations 1–13 are legacy regardless of their old runtime mode. `v0.2.13`
declares no writer predecessor. A later direct-to-direct upgrade may migrate only
a digest explicitly declared by the release or following open-development
commit; a version difference alone does not authorize conversion. Follow
[Upgrade](../specs/operations/release-and-packaging.md#upgrade) for that contract.

Missing or ambiguous assets, unsupported hosts, absent authentic historical
fixtures, or an inability to run the isolated exact candidate makes the pass
`INCOMPLETE`. A reproducible expected-behavior mismatch is a finding.

## Safety boundary

Use only the caller-assigned private fixture below the physical release-QA
evidence root, resolved with `pwd -P`. Keep extraction, builds, standalone Git
repositories, frozen-input staging, commands and results there. Never use
`git worktree add` against the source repository. Record source HEAD, porcelain
status, and common-Git worktree entries and prove they are unchanged afterward.

Before dispatch, the coordinator may download only the selected previous
release's archive, checksum and provenance into this fixture, without new
authentication. Verify regular non-symlink files, archive name/digest, selected
version/target, and the previous generation's own binary identities. Extract
without absolute paths, traversal, links, devices or unexpected members. Inspect
these previous assets; do not launch an old installed or extracted daemon, invoke
old service commands, or create an old account runtime.

Use a candidate archive only if its provenance binds the exact candidate commit.
Otherwise build only the current `podway` with the pinned toolchain, `--locked`,
release profile, offline Cargo networking and a private `CARGO_TARGET_DIR`.
Validate its full Output v4/version-result v2 identity and manifest v2 against
that candidate. Invoke this exact CLI directly with explicit `--worktree` paths;
no `--mode`, `--dev`, socket, `PODWAY_DEV_HOME`, LaunchAgent or `launchctl` is used.
The worker retains the no-network/no-credential boundary after dispatch.

## Historical input preparation

Use the reviewed frozen recipes and producer provenance in
[`tests/fixtures/storage/daemon-era-v2dfc`](../../tests/fixtures/storage/daemon-era-v2dfc/README.md).
They are rejected historical evidence, not current canonical DDL or a migration
source. The existing `podway-store` test-support fixture writer materializes each
generation's actual main file plus committed WAL without SHM from the frozen
ordered SQL. Its checkpointed and WAL-only header cases cover generations 1–13.
The separately frozen `generation-13-populated.sql` preserves a genuine legacy
current session, completed job and delegation receipt from the original producer.
Do not claim populated semantic coverage for the other generations.

Have the coordinator supply verified private fixture copies from those reviewed
recipes, including checkpointed and WAL-only layouts, the populated generation-13
case, and the frozen old marker cases. Record original producer commits and recipe
hashes. Fixture materialization is an explicitly bounded historical test-input
step before candidate execution; it is not a production conversion route or
permission to edit the candidate's Store. Do not invent old state by patching a
live database or run an old daemon to fill a missing prerequisite.

Place copies only in disposable standalone Git worktrees. Include configuration,
a supported local Procedure, `.gitignore`, tracked files and unrelated untracked
sentinels. Before invoking the candidate, record the full `.podway` tree's names,
bytes, modes and file identities, plus main/WAL hashes and absence of SHM.

## Scenario 1: legacy refusal leaves old state unchanged

Against each legacy worktree, run actual read and mutation commands with explicit
keys/fences as applicable. Include observation, outcome lookup and a normal
session mutation. Require validated Error v2
`LEGACY_RUNTIME_STATE_UNSUPPORTED`, its explicit reset-required recovery recipe,
and no outcome or state effect. Candidate classification must not initialize,
migrate, checkpoint, open a legacy Store for writes, or create SHM.

Compare every original file and marker with the pre-invocation snapshot; require
identical bytes, names, modes and identities and no new files. Preserve the genuine
populated generation-13 state and evidence unchanged during refusal. A successful
open, silent replacement, changed old data, or new SQLite sidecar is a finding.

## Scenario 2: explicit reset establishes fresh current state

After retaining the refusal proof, authorize destructive reset only for the exact
assigned fixture. Run the current CLI with an explicit stable key:

```bash
podway --worktree <owned-legacy-worktree> --json reset --all --yes --idempotency-key <new-key>
```

Require a validated success with a fresh workspace UUID, current schema generation
and writer manifest digest, and no legacy current or archived session, job or
receipt. Old runtime history is deliberately discarded; report that loss rather
than claiming migration or archive preservation. Configuration, local Procedure
files, `.gitignore`, Git metadata and unrelated tracked/untracked files survive.
Replay the same reset key and original preconditions; require its original full
response and no second identity change. Reconcile through current read APIs.

## Scenario 3: the fresh candidate lifecycle and deletion boundaries work

In the reset worktree, start and begin canonical `small-change-v2`, record its
required evidence with nonempty English text, and traverse its declared successful
path and Goal closeout using current JSON-derived revision/attempt/item fences.
Validate every Output v4/Error v2 response. Require the expected current identity,
normal progress, durable evidence and exact replay across separate CLI processes.
Archive a terminal session with a current disposition and verify bounded immutable
archive list/show evidence. Confirm purge removes only the selected inactive
session without altering Git or another current session.

Then run confirmed reset-all with a new key, verify preserved user/config files
and a fresh UUID, and use that exact UUID and canonical-root confirmation for
`workspace remove`. The whole `.podway` tree must disappear while the Git worktree,
HEAD and unrelated files survive. Reinitialize through the supported CLI and
require a new working current workspace. Do not manually delete runtime files to
stand in for these public operations.

## Direct-to-direct upgrades

When the selected previous release and the candidate are both direct
generations (SQLite generation 15, `v0.2.13` or later), upgrade QA additionally
covers the writer-refusal boundary in both directions. The candidate declares
no writer predecessor, so each release's manifest digest differs and each
binary refuses the other's Store. The coordinator's verified previous-release
CLI (see Safety boundary) may run against owned fixtures only, for exactly two
purposes: creating authentic previous-release Stores, and confirming that it
refuses the new Store. It runs without daemons, installation or account state;
extracted previous assets stay inspect-only otherwise.

1. **Refusal leaves the Store unchanged.** Start from a Store written by the
   previous CLI in an owned fixture. Record the full `.podway` tree's names,
   bytes, modes and file identities, then run candidate read and mutation
   commands. Require validated Error v2 `WORKSPACE_CONTRACT_MISMATCH` with no
   outcome, no state effect, no new files and no SQLite sidecars.
2. **Confirmed unfenced reset, with exact replay.** After read-only diagnosis
   and explicit discard authorization, run the candidate's
   `reset --all --yes --idempotency-key <new-key>` and require a fresh UUID and
   current writer digest with user files preserved; replay the same key and
   request and require the original full response with no second identity
   change (as in Scenario 2).
3. **A fresh lifecycle.** In the reset worktree, run one complete current
   lifecycle — start, begin, record, advance, disposition and archive — with
   exact replay across processes (as in Scenario 3).
4. **The previous CLI refuses the new Store.** Point the verified
   previous-release CLI at a worktree whose Store the candidate wrote. Require
   its fail-closed writer refusal with no destructive reset, no marker and no
   byte change. Invoke only ordinary read and session-mutation commands for
   this refusal check; never invoke the previous CLI's reset, removal or
   migration routes.

## Cleanup and evidence

Bound and reap every candidate/fixture child and prove no owned process or lock
remains. Remove exact extracted/build/staging/worktree paths, retain only bounded
text/JSON evidence, and verify the source repository and worktree registry stayed
unchanged. Never control installed or Aquarium legacy services as cleanup.

Report selected previous release, candidate/manifest identities, fixture producer
and recipe hashes, controlled commands, complete envelopes, immutable refusal
comparisons, destructive-reset result, fresh lifecycle and deletion observations,
cleanup, findings and gaps through Aquarium's normal result. This cluster passes
only when all applicable scenarios and cleanup pass. A confirmation rerun is
allowed only when this cluster belongs to the frozen full-pass matrix.
