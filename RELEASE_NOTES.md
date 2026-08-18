# Podway 0.2.5 release candidate notes

Podway 0.2.5 is a release candidate and has not been published. These notes do
not claim a publication date or an existing `v0.2.5` tag.

## Changes since 0.2.4

- Preserve eligible and force reset modes when released schema-v4 terminal
  receipts are migrated to schema-v5, keeping retained job status, lookup, list,
  and replay readable after cold reopen.
- Replace legacy three-field daemon text logs with fixed-schema
  `podway.daemon-log/v1` JSONL, add bounded request, workspace, session, job, and
  diagnostic correlation, and retain at most ten 1-MiB files without raw paths
  or caller-provided values.
- Move service bootstrap diagnostics to a daemon-owned bounded rotating stream
  and redirect LaunchAgent standard output and error descriptors to `/dev/null`.
- Keep bootstrap failures machine-readable while excluding filesystem paths and
  other raw internal error text from their public `message` field.

## Compatibility and migration

Podway remains Procedure v2-only. Existing running, completed, and cancelled
Procedure v2 state remains supported. Empty predecessors and released schema-v3
or schema-v4 databases migrate transactionally to canonical schema-v5; prepared
sessions are represented only in schema-v5.

A database containing legacy Procedure v1 state fails closed with `LEGACY_PROCEDURE_STATE_UNSUPPORTED`;
Podway does not convert or discard that state automatically. After any desired
backup, recovery requires an explicit confirmed `podway reset --all`.

The observation contract advances to `podway.observation-result/v2`, compact
status to `podway.status-result/v3`, and prepared-aware lifecycle jobs use
`podway.job-result/v4` and `podway.job-lookup-result/v4`. Released result versions
remain registered for compatibility and fail closed outside their declared
command families.

The supported release target remains native Apple Silicon macOS:
`aarch64-apple-darwin` with thin `arm64` Mach-O `podway` and `podwayd` binaries.
Podway remains a same-user local tool rather than a multi-user security boundary.

## Distribution metadata

The qualified, unpublished distribution contains these exact top-level artifacts:

- `podway-0.2.5-aarch64-apple-darwin.tar.gz`;
- `podway-0.2.5-aarch64-apple-darwin.tar.gz.sha256`;
- `podway-0.2.5-aarch64-apple-darwin.provenance.json`;
- `podway-0.2.5-aarch64-apple-darwin.dolgorae-handoff.json`.

The archive contains both binaries, shell completions, and three built-in Procedure v2 presets:
`bug-fix-v2`, `small-change-v2`, and `sw-dev-v2`. It also contains public schemas
and specifications, canonicalization fixtures, the contract manifest, README,
release notes, and license. Provenance records the source and build identities,
target, checksums, qualification results, and signing and notarization status.

## Admission and integration boundary

This qualified release candidate admits Procedure v2 sessions normally and does not contain the development-only admission unlock.
Publication may publish only unchanged qualified artifacts after explicit release authorization.

No MCP server or MCP transport is included. Automation integrates through the
CLI and its versioned JSON and local IPC contracts; the optional
source-distributed `use-podway` skill provides agent guidance over those
interfaces.

During LaunchAgent replacement, Podway waits for launchd to report the prior
label as unloaded before requesting the replacement bootstrap. Refresh an
installed service with `podway daemon install` only after installing the matching
`podway` and `podwayd` binaries together.

## Signing and known limitations

The Podway 0.2.5 Apple Silicon release candidate is unsigned and not notarized.
Users must verify the attached SHA-256 checksum before installing a published
artifact.

- Only native Apple Silicon macOS is supported.
- The service is a per-user LaunchAgent. It starts after GUI login and does not
  run before login.
