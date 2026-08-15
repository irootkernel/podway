# Podway 0.2.3 release candidate notes

Podway 0.2.3 is a release candidate and has not been published. These notes do
not claim a publication date or an existing `v0.2.3` tag.

## Changes since 0.2.2

- During LaunchAgent replacement, wait for launchd to report the prior label as
  unloaded before requesting the replacement bootstrap.
- Recover an authenticated `prepared` service publication by rerunning the same
  `podway daemon install` command without requiring an internal socket override.
- Keep prepared endpoints unavailable to ordinary daemon clients and other
  lifecycle commands until installation makes the receipt durable.
- Preserve `DAEMON_UNAVAILABLE` and its public details schema while making human
  service-lifecycle diagnostics distinguish the failure category.

## Compatibility and migration

Podway remains Procedure v2-only. No existing public contract identifier or
SQLite schema version changes in this release. Existing Procedure v2 state and
identities remain supported, and empty predecessor databases still migrate
transactionally to canonical schema-v4.

A database containing legacy Procedure v1 state fails closed with `LEGACY_PROCEDURE_STATE_UNSUPPORTED`; Podway does not convert or discard that state
automatically. After any desired backup, recovery requires an explicit confirmed
`podway reset --all`.

The supported release target remains native Apple Silicon macOS:
`aarch64-apple-darwin` with thin `arm64` Mach-O `podway` and `podwayd` binaries.
Podway remains a same-user local tool rather than a multi-user security boundary.

## Distribution metadata

The qualified, unpublished distribution contains these exact top-level artifacts:

- `podway-0.2.3-aarch64-apple-darwin.tar.gz`;
- `podway-0.2.3-aarch64-apple-darwin.tar.gz.sha256`;
- `podway-0.2.3-aarch64-apple-darwin.provenance.json`;
- `podway-0.2.3-aarch64-apple-darwin.dolgorae-handoff.json`.

The archive contains both binaries, shell completions, and three built-in Procedure v2 presets: `bug-fix-v2`, `small-change-v2`, and `sw-dev-v2`. It also
contains public schemas and specifications, canonicalization fixtures, the
contract manifest, README, release notes, and license. Provenance records the
source and build identities, target, checksums, qualification results, and
signing and notarization status.

## Admission and integration boundary

This qualified release candidate admits Procedure v2 sessions normally and does not contain the development-only admission unlock. Publication may publish only the unchanged qualified artifacts after explicit release authorization.

No MCP server or MCP transport is included. Automation integrates through the
CLI and its versioned JSON and local IPC contracts; the optional source-distributed
`use-podway` skill provides agent guidance over those interfaces.

## Signing and known limitations

The Podway 0.2.3 Apple Silicon release candidate is unsigned and not notarized.
Users must verify the attached SHA-256 checksum before installing a published
artifact.

- Only native Apple Silicon macOS is supported.
- The service is a per-user LaunchAgent. It starts after GUI login and does not
  run before login.
