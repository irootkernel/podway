# Project

## Goal

Podway helps a person or local automation complete one current task in one Git
worktree according to an explicit ordered procedure. It answers five questions:

1. Which procedure governs this task?
2. Which stage and attempt are active?
3. Which required items or blockers remain?
4. Which Podway action is valid next?
5. Which stages must be repeated after rework?

Success means `status` explains the current state, `next` gives actionable missing
items, invalid advancement fails without mutation, and retry or return cannot hide
work that must be repeated.

## Principles

- Focus on the current task instead of building a project-management history.
- Make the next valid action explicit for both humans and automation.
- Treat retry, return, and reopen as normal lifecycle transitions.
- Serialize mutations durably per worktree while allowing independent worktrees
  to progress concurrently.
- Define procedures as validated data, never executable expressions or commands.
- Use versioned JSON as the automation contract.
- Keep state with the worktree that owns it.
- State the same-user local trust boundary without presenting it as authentication.

## Product composition

Podway ships `podway`, the user-facing CLI, and `podwayd`, a user-scoped daemon.
The daemon is the sole normal writer of `.podway/runtime/state.sqlite3`. CLI
mutations become durable FIFO jobs; reads observe coherent committed state.

The initial catalog contains `sw-dev`, `bug-fix`, `docs-only`, and `analysis`.
Custom YAML procedures use the same parser, validation, canonicalization, and
runtime model as built-in presets.

## Non-goals

Podway is not a workflow server, CI system, command runner, Git automation layer,
review database, artifact store, long-term evidence ledger, or multi-user access
control system. It performs no network I/O, stores artifact metadata rather than
artifact bytes, and exposes no Git mutation API.

## Supported boundary

Podway publishes and supports only native Apple Silicon macOS on the tuple
`{triple: aarch64-apple-darwin, arch: arm64, host_arch: arm64, mach_o_arch: arm64}`
and installs the daemon as a per-user LaunchAgent. Linux, Windows, Intel macOS,
translated, universal, fat, and cross-built artifacts are not release targets.

The implemented v1 baseline remains usable, but the expanded automation contract
is a v0.1.0 target until every release-blocking roadmap task and its executable
evidence are complete.

## Core lifecycle

```text
initialize worktree
  -> start one session
  -> satisfy the active stage
  -> complete exactly one stage
  -> retry or return when work must be repeated
  -> complete the final stage
  -> optionally reopen before reset
  -> reset for the next task
```

For the full domain language and invariants, see the
[domain model](reference/domain/20-domain-model.md),
[state transitions](reference/domain/22-state-transitions.md), and
[goals and non-goals](reference/product/01-goals-and-non-goals.md).
