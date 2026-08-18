# Podway 0.2.4 release candidate notes

Podway 0.2.4 is a release candidate and has not been published. These notes do
not claim a publication date or an existing `v0.2.4` tag.

## Changes since 0.2.3

- Separate session preparation from execution: `start` now creates a prepared
  revision-0 session, while `begin` atomically creates attempt 1 and an optional
  initial goal.
- Add terminal ownership dispositions and make default reset or replacement
  eligible only for prepared sessions or terminal revisions with a current
  disposition.
- Preserve explicitly confirmed force reset and force replacement with bounded
  progress summaries, plus read-only eligibility previews with exact fences.
- Emit prepared-lifecycle durable jobs through closed v4 wrappers while keeping
  released v3 wrappers unchanged, and restore complete job status, list, lookup,
  and cold-reopen read-back.
- Reject individual and batched item mutations against prepared sessions with
  `SESSION_NOT_RUNNING` before attempt or item fences and without durable
  admission or state changes.
- Migrate released schema-v3 and schema-v4 workspaces to schema-v5 on cold access
  and rebuild missing registry metadata through the sole-writer activation path.
- Bind reduced patch-release evidence to the exact immutable commit that passed
  `make test` and reject symbolic baselines or non-regular release inputs.

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

- `podway-0.2.4-aarch64-apple-darwin.tar.gz`;
- `podway-0.2.4-aarch64-apple-darwin.tar.gz.sha256`;
- `podway-0.2.4-aarch64-apple-darwin.provenance.json`;
- `podway-0.2.4-aarch64-apple-darwin.dolgorae-handoff.json`.

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

The Podway 0.2.4 Apple Silicon release candidate is unsigned and not notarized.
Users must verify the attached SHA-256 checksum before installing a published
artifact.

- Only native Apple Silicon macOS is supported.
- The service is a per-user LaunchAgent. It starts after GUI login and does not
  run before login.
