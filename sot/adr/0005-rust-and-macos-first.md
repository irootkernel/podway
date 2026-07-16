# ADR-0005: Implement in Rust and Deliver macOS First

- Status: Accepted
- Date: 2026-07-13

## Context

Podway needs a reliable local daemon, Unix-domain IPC, SQLite transactions, Git worktree inspection, strong domain types, and distributable binaries. Delivery time is limited, so the first release needs one complete platform rather than shallow multi-platform support.

## Decision

Podway is implemented in Rust. The first complete release targets macOS and uses a user LaunchAgent. Apple Silicon and Intel artifacts are built. Linux support may later add a systemd user-service backend while preserving all public semantics.

## Consequences

Positive:

- strong type and memory safety for daemon and state logic;
- single-language CLI and daemon;
- suitable native binaries and platform abstractions;
- focused service and filesystem testing.

Negative:

- Rust learning and compile-time costs;
- macOS service integration is platform-specific;
- Linux and Windows users wait for later ports.
