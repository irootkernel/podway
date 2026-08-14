# Podway 0.2.1 release candidate notes

Podway 0.2.1 is a release candidate and has not been published. These notes do not claim a publication date or an existing `v0.2.1` tag.

## Changes since 0.2.0

- Make Procedure v2 the only supported authoring and runtime model, removing Procedure v1 parsing, commands, presets, public success schemas, and runtime paths.
- Emit all successful commands through the closed `podway.output/v3` envelope while retaining `podway.error/v1` for failures and current procedure-independent `/v1` contracts.
- Move worktree persistence to schema-v4 and fail closed before migration when legacy Procedure v1 state is present.
- Ship two built-in Procedure v2 presets: `sw-dev-v2` and `bug-fix-v2`.
- Add optional source-distributed `use-podway` guidance for AI coding agents.

## Compatibility and migration

Procedure v2 is now the only supported authoring and runtime model. This is an intentionally compatibility-breaking release: Procedure v1 inputs and commands are no longer accepted, and consumers must use `podway.output/v3` success envelopes.

Existing Procedure v2 state and identities remain supported. Empty predecessor databases migrate transactionally to canonical schema-v4. A database containing legacy Procedure v1 state fails closed with `LEGACY_PROCEDURE_STATE_UNSUPPORTED`; Podway does not convert or discard that state automatically. After any desired backup, recovery requires an explicit confirmed `podway reset --all`.

The supported release target remains native Apple Silicon macOS: `aarch64-apple-darwin` with thin `arm64` Mach-O `podway` and `podwayd` binaries.

Podway is a same-user local tool. Its IPC endpoint and worktree state are trusted only within the operating-system user account that owns them. It does not provide a multi-user access-control boundary.

## Distribution metadata

The qualified, unpublished distribution contains these exact top-level artifacts:

- `podway-0.2.1-aarch64-apple-darwin.tar.gz`;
- `podway-0.2.1-aarch64-apple-darwin.tar.gz.sha256`;
- `podway-0.2.1-aarch64-apple-darwin.provenance.json`;
- `podway-0.2.1-aarch64-apple-darwin.dolgorae-handoff.json`.

The archive contains both binaries, shell completions, the two built-in Procedure v2 presets, public schemas and specifications, canonicalization fixtures, the contract manifest, README, release notes, and license. Provenance records the source and build identities, artifact class, target, checksums, contract-manifest identity, qualification results, and signing/notarization status.

## Admission and release boundary

This qualified release candidate admits Procedure v2 sessions normally and does not contain the development-only admission unlock. Publication may publish only the unchanged qualified artifacts after explicit release authorization and must reverify the published bytes.

Development-only v2 admission remains limited to feature-enabled binaries, disposable workspaces, development mode, and isolated socket and state directories. That state has no migration-preservation promise and is not part of a release artifact.

## Signing and known limitations

The Podway 0.2.1 Apple Silicon release candidate is unsigned and not notarized. Users must verify the attached SHA-256 checksum before installing a published artifact.

- Only native Apple Silicon macOS is supported.
- The service is a per-user LaunchAgent. It starts after GUI login and does not run before login.
