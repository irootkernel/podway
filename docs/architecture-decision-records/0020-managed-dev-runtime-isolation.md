# ADR-0020: Managed Dev Runtime Isolation

- Status: Accepted
- Date: 2026-08-16
- Supersedes: the assertion in ADR-0012 that every daemon instance contends on
  the production singleton lock

## Context

The release gate must exercise the packaged CLI and daemon without installing a
LaunchAgent. The existing `--dev` mode provides that foreground lifecycle, but it
keeps the singleton lock under the effective user's production Podway home.
Consequently, `make dist` requires stopping a healthy installed daemon even though
qualification otherwise uses temporary state.

Contributor tooling already creates a private runtime topology, but its account
override is debug-only and cannot safely qualify release-profile binaries.

## Decision

`--dev` remains the only foreground mode. When `PODWAY_DEV_HOME` points to a
private managed root containing `podway.managed-dev-runtime/v2` metadata, the
daemon validates the complete topology and exact daemon snapshot before startup.
The metadata declares either `contributor` or `release-qualification` purpose.

A valid managed runtime derives its singleton lock from its declared private
`account/` root and keeps its socket, registry, and logs under `dev/`. It admits
workspace selectors only beneath its canonical `sandbox/` root, checks the
selector before Git resolution, and checks Git's canonical worktree root before
Store inspection or mutation. Metadata presence is authoritative: malformed,
unsafe, mismatched, or tampered metadata fails startup without falling back to
production paths.

Raw `--dev` without managed metadata preserves the ADR-0012 topology and therefore
shares the production singleton lock. Normal service mode remains one daemon per
effective user. Release qualification uses a short owner-private root directly
under `/private/tmp`, snapshots the extracted binaries into that root, and runs
those snapshots with `--dev`. Release-purpose metadata does not enable debug-only
development admission.

`make dist` preflight checks the native host and clean tree but does not probe or
modify production runtime files. Only the packaged live scenarios start a daemon,
and that daemon is contained in the managed release-qualification root.

## Rejected alternatives

- A new qualification CLI mode duplicates the lifecycle already owned by
  `--dev` and expands the public command grammar.
- Stopping or replacing the installed daemon makes release verification depend on
  unrelated live user state.
- Reusing only a different socket still contends on the production lock and does
  not isolate registry or logs.
- An environment-only account override for release binaries lacks a reviewable,
  fail-closed topology and exact-binary identity.

## Consequences

- `make dist` can run while the installed production daemon remains active.
- Contributor and release qualification share one validated topology contract,
  while their admission capabilities remain distinct.
- Managed runtimes cannot inspect or mutate worktrees outside their sandbox.
- Raw `--dev` compatibility is retained, including production-lock contention.
