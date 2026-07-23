# ADR-0013: Release and Support Only Native Apple Silicon macOS

- Status: Accepted
- Date: 2026-07-23
- Supersedes: the future-platform portion of ADR-0005

## Context

ADR-0005 selected Rust and described Apple Silicon macOS as the first platform,
while leaving open later Linux and Windows service backends. Podway's actual
service, release builder, qualification environment, and distribution format are
all specific to a native macOS LaunchAgent on Apple Silicon. The project does not
intend to publish or support another platform.

## Decision

Podway has one public release and support tuple:

```text
target triple: aarch64-apple-darwin
binary architecture: thin arm64 Mach-O
host operating system: macOS
host architecture: arm64
execution: native, not translated
```

Universal, fat, relabeled, cross-built, Rosetta-translated, Intel macOS, Linux,
and Windows artifacts are not Podway releases. Documentation and roadmaps do not
promise a later port or service backend for those platforms.

Existing conditional Linux code may remain as an internal implementation aid. It
is not a supported product surface, release target, compatibility promise, or
reason to weaken native macOS conformance. Removing that code or making non-macOS
compilation fail is outside this decision.

The Rust implementation decision in ADR-0005 remains accepted. Only its future
platform-expansion statement is superseded.

## Consequences

- Release engineering has one artifact tuple and one native verification host.
- Platform-specific contracts may state macOS and LaunchAgent behavior directly.
- Non-macOS code paths receive no release, compatibility, or support guarantee.
- A future decision to publish another platform would require a superseding ADR,
  a complete service contract, and its own native release gate.

