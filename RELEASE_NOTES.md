# Podway 0.2.6 release candidate notes

Podway 0.2.6 is a release candidate and has not been published. These notes do
not claim a publication date or an existing `v0.2.6` tag.

## Changes since 0.2.5

- Scale Procedure v2 definitions and atomic item recording to 128 items, align
  text and list bounds, and return structured diagnostics at each limit.
- Replace whole-value evidence embedding with bounded previews and stable
  `evidence read` pagination through digest-bound continuation tokens.
- Add structurally bound external check results that record caller-supplied
  operation identity, input digest, outcome, summary, and optional diagnostics.
- Retain up to 32 immutable inactive terminal sessions per worktree with explicit
  archive list, show, and purge commands and no automatic eviction.
- Make `start` resolve existing prepared, running, and terminal state explicitly,
  including atomic preservation of superseded running work.
- Add bounded typed predicates for conditional required items and decision-option
  guards without introducing expressions or execution hooks.
- Ship four reviewable reference Procedures, including `analysis-v2`, and split
  Procedure authoring guidance from runtime lifecycle guidance.

## Compatibility and migration

Podway remains Procedure v2-only. Existing running, completed, cancelled, and
prepared Procedure v2 state remains supported. Empty predecessors and released
schema-v5 databases migrate transactionally through canonical schema-v9.

Schema-v6 admits external check-result item values. Schema-v7 adds immutable
inactive sessions, schema-v8 adds successor-linked `superseded` dispositions, and
schema-v9 permits multiple ordered evidence references to select different items
from the same source node. Each migration preserves supported predecessor state,
checks foreign keys before commit, and advances its ledger atomically.

A database containing legacy Procedure v1 state fails closed with `LEGACY_PROCEDURE_STATE_UNSUPPORTED`;
Podway does not convert or discard that state automatically. After any desired
backup, recovery requires an explicit confirmed `podway reset --all`. A database
newer than schema-v9 also fails closed without mutation.

Selected evidence is no longer embedded whole in a progression response.
`session.next` uses `podway.next-result/v3` and `session.observe` uses
`podway.observation-result/v3`; both carry evidence identity, digest, and total
size. The new `podway.evidence-read-result/v1` family returns one bounded page and
continues through `next_page_token`. Compact status remains
`podway.status-result/v3`, with released result families retained for their
declared compatibility boundaries.

The former `start --replace-eligible` and `start --replace` options are removed.
Automation instead selects explicit behavior with `--on-existing preserve` or
`--on-existing delete`, including the required reason, progress summary, and
confirmation fences for destructive paths. Plain `reset` remains destructive and
never archives implicitly.

The supported release target remains native Apple Silicon macOS:
`aarch64-apple-darwin` with thin `arm64` Mach-O `podway` and `podwayd` binaries.
Podway remains a same-user local tool rather than a multi-user security boundary.

## Distribution metadata

The qualified, unpublished distribution contains these exact top-level artifacts:

- `podway-0.2.6-aarch64-apple-darwin.tar.gz`;
- `podway-0.2.6-aarch64-apple-darwin.tar.gz.sha256`;
- `podway-0.2.6-aarch64-apple-darwin.provenance.json`;
- `podway-0.2.6-aarch64-apple-darwin.dolgorae-handoff.json`.

The archive contains both binaries, shell completions, and four built-in Procedure v2 presets:
`analysis-v2`, `bug-fix-v2`, `small-change-v2`, and `sw-dev-v2`. It also contains public schemas
and specifications, canonicalization fixtures, the contract manifest, README,
release notes, and license. Provenance records the source and build identities,
target, checksums, qualification results, and signing and notarization status.

## Admission and integration boundary

This release candidate admits Procedure v2 sessions normally and does not contain the development-only admission unlock.
Publication may publish only unchanged qualified artifacts after explicit release authorization.

No MCP server or MCP transport is included. Automation integrates through the
CLI and its versioned JSON and local IPC contracts; the source-distributed
`use-podway` and `create-podway-procedure` skills provide runtime and authoring
guidance over those interfaces.

During LaunchAgent replacement, Podway waits for launchd to report the prior
label as unloaded before requesting the replacement bootstrap. Refresh an
installed service with `podway daemon install` only after installing the matching
`podway` and `podwayd` binaries together.

## Signing and known limitations

The Podway 0.2.6 Apple Silicon release candidate is unsigned and not notarized.
Users must verify the attached SHA-256 checksum before installing a published
artifact.

- Only native Apple Silicon macOS is supported.
- The service is a per-user LaunchAgent. It starts after GUI login and does not
  run before login.
