# ADR-0009: Store Artifact Metadata Only

- Status: Accepted
- Date: 2026-07-13

## Context

Copying artifact bytes into Podway would create storage growth, retention, privacy, backup, and secret-handling obligations. The product needs references sufficient to guard the current stage.

## Decision

Artifact items store only location type, path or reference, SHA-256 digest, byte size, and media type. Local files are hashed read-only and revalidated at completion. External references are not fetched.

## Consequences

Positive:

- bounded database size;
- no artifact archive or encryption feature;
- worktree remains source of local bytes;
- no network dependency.

Negative:

- external content availability is not guaranteed;
- local content can change after completion;
- users must retain artifacts through their normal project systems.
