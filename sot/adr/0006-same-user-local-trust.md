# ADR-0006: Use Same-User Local Trust Without an Access Key

- Status: Accepted
- Date: 2026-07-13

## Context

A proposed per-worktree key would be accessible to the same user who can invoke the CLI, read the worktree, and inspect user files. It would add credential lifecycle complexity without protecting against the relevant same-user adversary.

## Decision

Podway uses user-private socket permissions, peer UID checks, Git identity, workspace UUIDs, revisions, and idempotency. It does not issue a worktree access key and does not claim cryptographic authentication.

## Consequences

Positive:

- simpler setup and recovery;
- honest assurance model;
- fewer secrets and redaction requirements.

Negative:

- any process running as the same user can invoke Podway;
- Podway cannot serve as audit proof or a malicious-local-process boundary.
