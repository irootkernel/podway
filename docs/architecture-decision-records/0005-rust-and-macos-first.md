# ADR-0005: Implement in Rust and Deliver macOS First

- Status: Accepted
- Date: 2026-07-13
- Platform scope: Partially superseded by ADR-0013

## Context

Podway needs a reliable local daemon, Unix-domain IPC, SQLite transactions, Git worktree inspection, strong domain types, and distributable binaries. Delivery time is limited, so the first release needs one complete platform rather than shallow multi-platform support.

## Decision

Podway is implemented in Rust. The release targets only the Apple Silicon tuple
`{triple: aarch64-apple-darwin, arch: arm64, host_arch: arm64, mach_o_arch: arm64}`
and uses a user LaunchAgent. ADR-0013 supersedes the former option for later Linux
and Windows release backends; the Rust implementation decision remains in force.

## Consequences

Positive:

- strong type and memory safety for daemon and state logic;
- single-language CLI and daemon;
- suitable native binaries and platform abstractions;
- focused service and filesystem testing.

Negative:

- Rust learning and compile-time costs;
- macOS service integration is platform-specific;
- Linux, Windows, Intel macOS, translated, and cross-built artifacts are not
  release or support targets.
