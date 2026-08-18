# Release Workflow

The repository-root `make test` command is the development gate. `make dist` is
the complete release gate and must run from the exact clean revision being
packaged.

When the v2 contract surface changes, regenerate the reviewable downstream
adapter catalog from repository authorities. Refresh the manifest once so the
generator can validate changed schema bytes, then refresh it again to bind the
new adapter-catalog bytes:

```bash
python3 tools/contract_manifest.py --write
python3 tools/create_dolgorae_handoff.py prepare-adapter
python3 tools/contract_manifest.py --write
```

This only updates Podway's manifest-bound prepared contract. It does not modify
the Dolgorae repository, qualify a distribution, or establish downstream
acceptance. `python3 tools/create_dolgorae_handoff.py self-test` rejects catalog,
schema-pin, route, error, migration, reactivation, and handoff drift.

After version and release-note preparation:

1. run `make dist` on native Apple Silicon macOS;
2. inspect the final verifier receipt for the archive, checksum, provenance, and
   Dolgorae handoff;
3. confirm signing and notarization statements match the artifact;
4. tag the exact source revision and publish all required files.

`make dist` always runs the development gate and bounded fuzzing before it builds
thin arm64 binaries and qualifies the packaged CLI/daemon contract in isolated dev
mode. Distribution construction rejects dirty trees, translated execution,
non-arm64 binaries, version mismatches, and stale layouts.

The native-host and clean-worktree preflight runs before the expensive test gate,
while packaging repeats those checks before writing release artifacts. It does not
inspect or acquire the production singleton lock. Packaged live qualification
creates a private purpose `release-qualification` managed runtime under
`/private/tmp` and uses the existing `--dev` lifecycle, so an installed production
daemon may remain active throughout `make dist`.

## Runtime isolation and cleanup

`make dist` owns packaged runtime qualification end to end. Do not replace that
step with a raw `podwayd --dev` process or point its CLI, daemon, account home,
socket, registry, logs, or sandbox at installed production state. The qualifier
uses the extracted matching binary pair in an owner-private
`podway.managed-dev-runtime/v2` root with purpose `release-qualification`; the
installed production LaunchAgent may remain running.

Cleanup is part of qualification, not optional follow-up. Each scenario must stop
its temporary daemon, and a successful full gate must prove that no qualification
daemon process or socket remains. If a gate fails or is interrupted, do not
publish or retry until the exact helper-owned process, socket, and root have been
reconciled. Inspect and clean only the identified owner-private qualification
state; never use broad `/private/tmp` deletion and never edit production databases,
registry data, sockets, service metadata, or LaunchAgent files to simulate cleanup.

The persistent contributor runtime is separate from the per-run qualification
root. Stop its foreground daemon and then run:

```bash
python3 tools/dev_runtime.py clean --yes
```

The helper validates ownership, layout, the isolated lock, and endpoint idleness
before rename-to-trash deletion. A failed cleanup is a blocker: preserve and
report the exact recoverable path instead of deleting around the guard.

`make dist-patch` does not run packaged runtime qualification. Its readiness comes
from the confirmed tested baseline and reduced-gate provenance and handoff; do not
claim that a temporary release-qualification runtime, packaged scenarios, or their
cleanup ran.

Qualification changes packaged-conformance evidence from `pending` to `passed`
only after all extracted scenarios succeed; handoff generation rejects anything
else. The last `make dist` command independently re-extracts and cross-checks the
complete archive/provenance/handoff identity set.

See the normative [release and packaging specification](../specs/operations/release-and-packaging.md)
and the [active roadmap](../roadmap/).
