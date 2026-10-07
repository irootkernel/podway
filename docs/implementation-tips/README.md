# Implementation Tips

Use these guides when changing Podway:

- [Repository Workflow](repository-workflow.md): prerequisites, code ownership,
  worktrees, and canonical assets.
- [Testing](testing.md): focused checks and the complete release gate.
- [Direct CLI Performance Qualification](direct-cli-performance-qualification.md):
  preserved G7/G8 baselines, matched supported fixtures and actual open/latency gates.
- [Documentation](documentation.md): document placement, precedence, and links.
- [Release](release.md): source readiness, packaging, and publication handoff.
- [Release Upgrade Hands-on QA](release-upgrade-qa.md): legacy refusal, explicit
  reset and fresh candidate lifecycle/deletion verification.
- [Direct Mutation Outcome QA](release-admission-qa.md): validated success outcomes,
  rejection without outcomes, reconciliation and cold durability.

## Core implementation rules

- Keep domain transitions pure and pass clocks, IDs, digests, and prepared inputs
  explicitly.
- Preserve direct Engine transactions, per-worktree write serialization, and
  maintenance exclusion.
- Bound user-controlled input, collections, file reads, and lock waits.
- Avoid panics on user input and keep public errors stable and structured.
- Treat paths as platform bytes internally and validate display strings only at
  public serialization boundaries.
- Check containment without following untrusted symlinks.
- Keep migrations forward-only, deterministic, and transactional.
- Add a regression test for every state, recovery, or path-safety bug.
- Prefer exhaustive matches and explicit failure over hidden fallback.

Before editing, identify the owning crate, read the relevant architecture, ADR,
specification, and machine asset, and determine which invariant or public contract
the change affects.
