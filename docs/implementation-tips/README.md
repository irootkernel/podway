# Implementation Tips

Use these guides when changing Podway:

- [Repository Workflow](repository-workflow.md): prerequisites, code ownership,
  worktrees, and canonical assets.
- [Testing](testing.md): focused checks and the complete release gate.
- [Documentation](documentation.md): document placement, precedence, and links.
- [Release](release.md): source readiness, packaging, and publication handoff.

## Core implementation rules

- Keep domain transitions pure and pass clocks, IDs, digests, and prepared inputs
  explicitly.
- Preserve the daemon's single-writer and per-worktree serialization boundaries.
- Bound user-controlled input, queues, frame sizes, file reads, and shutdown waits.
- Avoid panics on user input and keep public errors stable and structured.
- Treat paths as platform bytes internally and validate display strings only at
  public serialization boundaries.
- Check containment without following untrusted symlinks.
- Keep migrations forward-only, deterministic, and transactional.
- Add a regression test for every state, queue, recovery, or path-safety bug.
- Prefer exhaustive matches and explicit failure over hidden fallback.

Before editing, identify the owning crate, read the relevant architecture, ADR,
specification, and machine asset, and determine which invariant or public contract
the change affects.
