# Podway 0.2.0 release candidate notes

Podway 0.2.0 is a release candidate and has not been published. These notes do not claim a publication date or an existing `v0.2.0` tag.

## Changes since 0.1.2

- Preserve the released v1 Procedure, output, error, status, next, version, IPC, and automation contracts while adding the versioned Procedure v2 contract and its closed result families.
- Add the complete v2 authoring surface for validation, formatting, vetting, linting, source conversion, graph projections, previews, and scaffolds.
- Add the v2 runtime lifecycle with durable admission and reconciliation, typed items, decisions, retry, skip, rework, blockers, goals, goal revisions, criterion assessments, and bounded history read-back.
- Ship six built-in presets: `sw-dev`, `bug-fix`, `docs-only`, `analysis`, `sw-dev-v2`, and `bug-fix-v2`.
- Extend native qualification across the real CLI and daemon, queues, detached jobs, concurrency, crash/restart recovery, SQLite reopen, endpoint isolation, and response-loss reconciliation.

## Compatibility and migration

Podway 0.2.0 preserves the public v1 contract and adds Procedure v2 as an additive, explicitly versioned contract. Existing v1 sessions and their immutable snapshots remain valid after upgrade. Procedure v2 runtime success responses use `podway.output/v2`, while the released v1 behavior and result schemas remain unchanged.

On first access, uninitialized/schema-0, schema-v1, and schema-v2 worktree databases are upgraded transactionally to canonical schema-v3. Schema-v1 and schema-v2 migration is lazy per worktree. An incomplete migration is not accepted as schema-v3, and database downgrade remains unsupported.

The supported release target remains native Apple Silicon macOS: `aarch64-apple-darwin` with thin `arm64` Mach-O `podway` and `podwayd` binaries.

Podway is a same-user local tool. Its IPC endpoint and worktree state are trusted only within the operating-system user account that owns them. It does not provide a multi-user access-control boundary.

## Distribution metadata

The qualified, unpublished distribution contains these exact top-level artifacts:

- `podway-0.2.0-aarch64-apple-darwin.tar.gz`;
- `podway-0.2.0-aarch64-apple-darwin.tar.gz.sha256`;
- `podway-0.2.0-aarch64-apple-darwin.provenance.json`;
- `podway-0.2.0-aarch64-apple-darwin.dolgorae-handoff.json`.

The archive contains both binaries, shell completions, the six presets, public schemas and specifications, canonicalization fixtures, the contract manifest, README, release notes, and license. Provenance records the source and build identities, artifact class, target, checksums, contract-manifest identity, qualification results, and signing/notarization status.

## Admission and release boundary

This qualified release candidate admits Procedure v2 sessions normally and does not contain the development-only admission unlock. V2REL-007 may publish only the unchanged qualified artifacts after explicit release authorization and owns published-byte reverification, immutable release evidence, and documentation-only closeout.

Development-only v2 admission remains limited to feature-enabled binaries, disposable workspaces, development mode, and isolated socket and state directories. That state has no migration-preservation promise and is not part of a release artifact.

## Signing and known limitations

The Podway 0.2.0 Apple Silicon release candidate is unsigned and not notarized. Users must verify the attached SHA-256 checksum before installing a published artifact.

- Only native Apple Silicon macOS is supported.
- The service is a per-user LaunchAgent. It starts after GUI login and does not run before login.
