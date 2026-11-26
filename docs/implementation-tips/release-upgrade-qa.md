# Release Upgrade Hands-on QA

Run this independent repository cluster during every full Aquarium release-QA
pass. Select the branch by the exact writer generations and manifests. The
`v0.2.12` to `v0.2.13` cutover established the daemon-era refusal boundary.
The Procedure v3 and Execution Group v1 cutover has a separate direct-to-direct
boundary from the `v0.2.17` Procedure v2 writer (Store 15) to Store 16.
Retain the frozen daemon-era coverage and verify the applicable exact
previous-release boundary. This hands-on QA is separate from `make dist` and
its package provenance.

## Inputs and evidence boundary

Require the exact clean candidate and intended stable version, native Apple
Silicon macOS, and the selected previous stable release's authenticated Apple
Silicon archive, checksum and provenance. Select the latest non-draft,
non-prerelease stable release reachable from the candidate or an exact baseline
explicitly confirmed by Master. A confirmed initial release has no previous
release. Record that boundary before execution.

Pin the previous release tag and peeled source commit, archive name and
SHA-256, provenance, and extracted binary SHA-256. Check the release, target,
source, build and manifest identities against the selected published assets.
Use the previous release's verified provenance and packaged manifest for its
identity. Check the candidate's identity through `version --json --identity`.
Record actual signing and notarization facts as published. A matching checksum and provenance do not
establish a signature. Compare both binaries' SQLite generations and writer
manifest identities with their owning source. The version string and shared
Output v4 and Error v2 envelopes alone cannot distinguish writers or dictate
which error detail family applies.

Require verified frozen historical fixtures for the daemon-era branch and
real previous-writer Stores for the direct branch. Missing or ambiguous inputs,
unsupported execution hosts, missing authentic fixture provenance or failure to
isolate the exact candidate makes the applicable pass `INCOMPLETE`. A
reproducible mismatch against the owning contract is a finding.

## Safety boundary

Use only the assigned private evidence root, physically resolved with `pwd -P`,
with standalone Git repositories, explicit absolute worktree paths and
bounded child cleanup. Keep source HEAD, porcelain status and the common Git
worktree registry for integrity comparisons. Do not use `git worktree add`
against the source repository or run a candidate against an installed/managed
workspace. Keep installation and account/tool/service state outside the pass.

Before dispatch, the Coordinator may download only the selected previous
release's archive, checksum and provenance into the assigned fixture without
new authentication. Verify regular non-symlink files and the exact package.
Reject extraction paths with links, traversal, absolute names, devices or
unexpected members. Inspect daemon-era inputs without launching the old daemon,
invoking its service commands or creating account runtime. In a direct-to-direct
pass, the provenance- and checksum-verified previous CLI may run in owned
fixtures to create authentic previous-release Stores and to verify its
refusal of the successor Store. Establish its identity from the package before
running either fixture. The previous CLI remains outside all other workspace,
maintenance and adoption work in this pass.

Use a candidate archive only when its provenance binds the exact clean
candidate. Otherwise build the exact candidate `podway` using the pinned
toolchain, `--locked`, release profile, offline Cargo networking and a private
`CARGO_TARGET_DIR`. Verify the candidate's binary SHA-256, full Output v4 and
version-result v2 identity and manifest v2. Invoke the exact executable with
an explicit `--worktree` and use its applicable presentation schema. Worker
execution stays offline after dispatch. Mode switches, global runtime roots,
sockets and installed service controls have no role in these fixtures.

For each fixture prepare configuration, local source, `.gitignore`, Git
metadata and unrelated tracked and untracked sentinels. Before a refusal,
snapshot the full relevant trees: names, file types, modes, identities and
content digests, including SQLite main/WAL and the presence or absence of
SHM and maintenance markers. Compare before and after each invocation without
opening the rejected database through an inspection tool. For a no-SHM
case, establish the absence at snapshot time and verify it again afterward.
Preserve an existing SHM file when testing an input that already has one.

## Historical daemon-era inputs

For the `v0.2.12` to `v0.2.13` boundary, the original contract treats Store
generations 1–13 and old maintenance markers as unsupported. Use the reviewed
[frozen inputs and producer recipes](../../tests/fixtures/storage/daemon-era-v2dfc/README.md).
They are historical rejection witnesses. The existing `podway-store` test
fixture writer materializes checkpointed and committed WAL-only header cases
from the ordered recipes. The populated generation-13 case retains a genuine
legacy current session, completed job and delegation receipt. Qualify that
populated meaning only for the populated case. Pin producer commits and
recipe hashes, and supply verified fixture copies in the owned area. Fixture
materialization grants no permission to edit a live Store or generate a
surrogate for an authentic previous direct writer.

Exercise candidate observation, outcome lookup and a normal Store-backed
mutation against each applicable frozen legacy input. Require the executing
writer's own Error v2 `LEGACY_RUNTIME_STATE_UNSUPPORTED` details and explicit
reset-required recipe. Check unchanged file bytes, modes, identities and names,
with no SHM/marker creation, state effect or successful outcome. A refusal must
preserve the populated generation-13 evidence unchanged. Retain the historical
regression even when a later direct writer is the selected preceding stable
release. Use the independent [explicit reset](#explicit-reset-and-exact-replay)
and [fresh lifecycle](#fresh-lifecycle-and-deletion-boundaries) scenarios for
applicable recovery, using the actual candidate's supported Procedure model.

## Direct-to-direct boundary for Store 16

For the `v0.2.17` to Store 16 case, build the authentic old Store with the
verified previous-release CLI in an owned Git fixture. Retain its initialized
writer/workspace identity, admitted Procedure v2, actual active attempt, recorded
work and known successful key to exercise lookup against real history. Retain
its closed native envelopes as source provenance. Keep any original installed
or managed workspace outside both fixture operation and reset.

| Executing writer and input | Expected refusal |
| --- | --- |
| Store 16 candidate opening an authentic `v0.2.17` Store 15 | Error v2, exit `5`, `LEGACY_RUNTIME_STATE_UNSUPPORTED`, `podway.legacy-runtime-state-unsupported-details/v2`, `store_schema_version: 15`. Human stderr appends the four Store 15 hints below. The explicit reset recipe requires separate authorization. |
| Verified `v0.2.17` writer opening a successor Store 16 | Error v2, exit `5`, `WORKSPACE_SCHEMA_UNSUPPORTED`, `podway.workspace-schema-unsupported-details/v1`, actual generation `16`, supported generation `15`. Use a compatible writer. |
| Store 16 `start` with a well-formed Procedure v2 source on an admitted successor workspace | Error v2, exit `1`, `PROCEDURE_SCHEMA_UNSUPPORTED`, `podway.procedure-source-error-details/v1`, `expected_schema: podway.procedure/v3`, `reason: unsupported_schema`, `known_source_schema: podway.procedure/v2`. |
| Store 16 candidate opening an authentic `v0.2.18` Store 16 with a different undeclared manifest digest | Error v2, exit `5`, `WORKSPACE_CONTRACT_MISMATCH`, `podway.workspace-contract-mismatch-details/v1` (gen 1) or `v2` with `actual_product_version: "v0.2.18"` (gen 2), byte-identical main/WAL, no SHM, and no outcome replay. Explicit reset requires separate authorization. |
| Verified `v0.2.18` writer opening a newer Store 16 with a different manifest digest | Error v2, exit `5`, `WORKSPACE_CONTRACT_MISMATCH` under `v0.2.18`'s error contract, byte-identical main/WAL, no SHM, and no outcome replay. |

In human `status`, expect these hints after the error and its recovery reason:

```text
hint: stored Store generation: 15
hint: to keep history, finish or abandon the session with its original writer: Podway v0.2.13 through v0.2.17
hint: with a verified v0.2.15-v0.2.17 binary, --contract-generation 2 --json observe may name the writer release; otherwise use retained provenance
hint: reset --all destroys current and retained history and requires explicit authorization
```

Run the focused development regression with an absolute path to the verified
`v0.2.17` binary documented in [Supported Store continuity](testing.md#supported-store-continuity):

```bash
PODWAY_TEST_LEGACY_V017_BINARY=/absolute/verified/podway \
  make test-e2e-exact E2E_TEST=podway-cli::e2e_suite::e2e_hands_usability::authentic_store_15_preserves_json_and_adds_human_history_guidance
```

The opt-in case verifies the writer hash, copies the stopped writer's original
Store bytes to a no-SHM fixture, and compares the two JSON presentations and the
unchanged Store. Run it explicitly to establish authentic Store 15 coverage.

Both Store refusal cases classify by generation before opening SQLite for
recovery, writes or SHM creation. `WORKSPACE_CONTRACT_MISMATCH` describes a
separate writer-digest mismatch in an admitted generation. Store 16 declares
no predecessor. Neither presentation selector restores Procedure v2 grammar or
permits conversion of its state. Source support must be checked separately
from the Store gate on a compatible successor workspace.
See the [upgrade contract](../specs/operations/release-and-packaging.md#upgrade).

For the forward Store case, run actual successor `observe`, `outcome lookup`
and a Store-backed session mutation. Preserve the original writer's old key
for the lookup and use an explicit key for the declined new request. A lookup
on the refused Store itself returns the admission error. The unchanged owned
snapshot establishes absence of a new success outcome. For the v2 source
case, use an admitted fixture and check the Procedure bytes, SQLite main and
WAL, path identities and known rejected-key miss. A start request in an
admitted Store may refresh existing transient SHM bytes even when source
validation rejects it. Record any observed SHM change separately. Verify the
successor source through a fresh lifecycle.

For the reverse Store case, invoke the previous CLI only through the scoped
ordinary read and session-mutation refusal commands on the owned successor
Store. Select actual Store-backed calls rather than a local authoring route or
an unfenced source preview. Require unchanged files, no new sidecars/markers
and no successful outcome. The prior release's own error contract supplies the
expected result. Use no previous-writer reset, removal or migration to obtain a
refusal. In future direct passes, select the expected transition from both
exact authorities. An unchanged writer identity needs continuity evidence. A
declared predecessor needs the release's specified forward-admission evidence.
An unsupported generation or undeclared digest needs unchanged refusal evidence.
This cutover supplies no future migration or downgrade promise.

## Explicit reset and exact replay

After retaining the refusal comparison, obtain distinct destructive authority
covering only the exact disposable fixture and all of its runtime history.
Confirm no process or other writer still uses it. With an unsupported Store
there is no supported UUID-fenced continuation with the candidate. Use a new
explicit unfenced discard request with the candidate CLI:

```bash
podway --worktree <owned-disposable-worktree> --json reset --all --yes --idempotency-key <new-key>
```

Require an Output v4 success for `workspace.reset_all`, a fresh workspace UUID
and the candidate Store generation and writer digest. Verify through current
workspace and session reads that no old current or archived session or outcomes
became current. Record deliberate loss of the selected fixture's runtime
history. Verify preserved configuration, user Procedure source, `.gitignore`,
Git and the unrelated tracked/untracked sentinels. Legacy mode configuration is
retired only where the maintenance contract already says so. The reset does
not rewrite old Procedure documents or v2 preset pins to v3. Select compatible
successor sources explicitly.

Replay the exact original unfenced request and key against that fixture.
Require the complete original success response and no second UUID or other
state effect. Inspect current state separately. Lost output or an interruption
retains the same request and marker-backed recovery boundary. See the
[maintenance contract](../specs/storage/recovery-retention-and-maintenance.md).

## Fresh lifecycle and deletion boundaries

For Store 16, start and begin with a supported base such as `small-change-v3`.
Record the current anchor's plan items and activate the exact expected default
or custom group using its current cursor, plan binding, working-copy basis,
digest and caller key. Use an empty `prior_work` for first activation. Follow
the actual group's declared actions/decisions, record current evidence, then
return from its terminal to the declared base successor. Complete the base
session and record its real terminal disposition. The untracked
`small-change-v3` needs no goal assertion. For a tracked base, supply a current
goal and follow its declared final assessment. Use separate CLI processes,
scoped fences and exact replay/lookup where applicable. Check every envelope
against the executing CLI's contract. [Procedure v3 walkthrough](../examples/continuation-v3.md)

For a full release-QA pass, retain and inspect the terminal session through
archive list/show and verify that confirmed purge deletes only the selected
inactive history. Run separately authorized reset-all with a new key to prove
preserved user/configuration files and fresh UUID. Use the applicable workspace
UUID and confirmation rules for approved `workspace remove`. Verify the chosen
`.podway` tree is removed and the Git worktree, HEAD and unrelated files survive.
Reinitialize through the supported CLI and verify fresh current operation. The
historical daemon cutover uses the exact candidate's own known-good Procedure
and lifecycle rules. No case substitutes manual runtime-file edits for the
public operations. A work-unit cutover check can establish only the branches it
actually executed and does not waive these full-release obligations.

## Cleanup and scope of the result

Bound and reap every owned child and verify no owned process or lock remains.
Remove the exact owned extraction, build, staging and fixture paths after saving
bounded evidence, and compare source repository and worktree-registry state.
Installed/managed writer retirement is separate real-upgrade work through the
original owners, outside this QA pass.

Report the two writer identities, selected release/provenance, actual fixture
producers, controlled commands and exit/envelope pairs, original and final
snapshots, exact scope of reset and its replay, preserved files, fresh lifecycle,
reverse refusals and cleanup. Report explicit coverage limits and findings.
Pass only when all applicable cases and cleanup pass. A confirmation rerun uses
the authorized frozen full-pass matrix. Settling real v2 sessions with their
original writer, approving a discard, installing a release or activating an
external consumer each needs the applicable separate authority.
