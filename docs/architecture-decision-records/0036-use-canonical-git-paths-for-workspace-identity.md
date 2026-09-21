# ADR-0036: Use Canonical Git Paths for Workspace Identity

- Status: Accepted
- Date: 2026-09-21
- Supersedes in part: [ADR-0030](0030-retire-workspaces-and-prune-stale-registry-entries.md)
- Preserves: [ADR-0003](0003-daemon-single-writer.md), [ADR-0004](0004-worktree-local-state.md), [ADR-0006](0006-same-user-local-trust.md)
- Delivery owner: [V2PID](../roadmap/README.md#v2pid-canonical-path-workspace-identity)

## Context

The released workspace identity hashed filesystem device, inode, object type,
and birth-time metadata for the Git common directory, worktree administration
directory, and worktree root. On macOS, a normal reboot can change the mounted
volume's device identifier while every repository path and Git relationship
remains unchanged. That made most existing workspaces fail with
`WORKSPACE_ID_CONFLICT` after reboot even though no workspace had been copied,
moved, or modified.

Those object identifiers are useful while one request is resolving paths and
checking for replacement races. They are not a stable cross-boot identity.

## Decision

The durable Git identity is derived only from lossless canonical paths:

- parent identity: the canonical Git common-directory path;
- child identity: the canonical worktree-administration path;
- location evidence: the canonical worktree-root path.

Role-tagged hashes remain an internal fixed-size representation, but their only
filesystem input is the canonical path bytes. Device, inode, object type, and
birth time remain request-local descriptor evidence for race detection and are
never persisted as workspace identity. Descriptor-backed evidence is compared
through the final pre-Store revalidation and then stripped before a scheduler
context becomes long-lived.
Mutation-capable Store opens repeat that validation after SQLite has bound the
database pathname and before connection pragmas, migration, recovery, or domain
mutation. A final Store path is never created by an existing-database open;
new-database creation uses only a fully initialized private temporary followed by
guarded atomic publication. Recovery that records an initialized Store must reopen
that Store in existing-only mode. New-database creation also validates before
publication and on the published connection.

The workspace UUID remains the logical generation fence. A linked worktree root
may move when its common and administration paths remain unchanged and Git's
reciprocal link validates. Moving the common directory, moving a main worktree,
or copying a complete runtime changes the durable path identity and fails closed;
the operator must explicitly reset or use a separately specified path-repair
operation rather than receiving an implicit adoption.

Schema v13 names the identity-generation boundary. A supported v12 Store may
migrate only when its stored `last_validated_root` exactly equals the freshly
resolved canonical root and its workspace UUID is unchanged. The migration
rewrites the two persisted Git identity hashes to canonical-path values in the
same transaction as the schema ledger update. A v13 mismatch is not migrated.
An in-flight v12 reset retains its pre-migration digest through marker replay,
including after predecessor deletion. A completed v12 reset may replay only from
an immutable successful reset terminal receipt under the same idempotency key;
other command families retain exact digest matching.

Confirmed reset may recover when global registry metadata is absent if the local
Store binding supplies one unambiguous workspace UUID and its stored root exactly
matches the selected worktree. The reset marker and compare-and-swap protocol
still fence the destructive transition. A registry entry for that predecessor or
marker target UUID at any other root is a copied-or-moved-generation conflict and
fails before marker publication or Store deletion; reset never removes or adopts
that other-root entry. An occupied or ambiguous selected root remains a hard
failure.

Destructive reset and workspace mode switching renew request-local Git evidence
at every filesystem and registry mutation boundary. Reset fences the marker's
predecessor and target UUIDs even when either UUID was absent from the first
registry snapshot. Mode-switch recovery reopens and validates the target Store
before publishing its target registry entry. Mode-switch marker v2 also persists
the canonical root, common-directory, and worktree-administration identities.
Marker and config publication is descriptor-relative, while registry publication
renews authority under the registry lock at its durable linearization point.
Legacy v1 recovery upgrades only from a live matching plan, exact surviving
source authority, or—when complete—exact target config, Store, and registry
authority; an identity-unprovable incomplete legacy marker fails closed.

## Rejected alternatives

- Continue persisting `st_dev`, inode, or birth time: these values do not provide
  the required cross-boot stability on the supported macOS platform.
- Persist both filesystem and path identity indefinitely: this keeps two
  competing authorities and leaves ordinary startup dependent on unstable data.
- Silently adopt every moved or copied database: this can duplicate a workspace
  UUID and bypass explicit destructive recovery.
- Repair the global registry manually: registry files are daemon-owned metadata
  and are not a supported recovery interface.

## Consequences

Reboot and remount events no longer alter durable workspace identity when the
canonical Git paths are unchanged. Existing v12 workspaces migrate in place on
their first successful open; reset remains available for unreadable or explicitly
discarded state, including a proven local binding whose registry entry is gone.

Path changes are now visible by design. Descriptor metadata still protects each
individual resolution against replacement races, but it no longer claims to be
durable identity across requests.
