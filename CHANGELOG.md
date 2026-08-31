# Changelog

This file records concise shipped outcomes and the planned next stable release.

## v0.2.8 - Unreleased

### Added

- Add bounded named runtime modes with complete isolated daemon namespaces and preserve `--dev` as the exact alias for mode `dev`.

## v0.2.7 - 2026-08-31

### Added

- Add confirmed, marker-recoverable removal of one selected Podway workspace while preserving its Git worktree.

### Changed

- Replace single-release candidate notes with this cumulative changelog and ship it in distribution archives.
- Teach the source-distributed `use-podway` skill to distinguish session cleanup from complete workspace removal and require exact target confirmation.

### Fixed

- Correct the public IPC mutation example to include the command-level worktree selector required by decoding.
- Prune only conclusively missing exact workspace registry generations while retaining ambiguous recovery failures.

## v0.2.6 - 2026-08-26

### Added

- Add structurally bound external check results with caller-supplied operation identity, input digest, outcome, summary, and optional diagnostics.
- Add bounded typed predicates for conditional required items and decision-option guards without expressions or execution hooks.
- Add the reviewable `analysis-v2` Procedure and separate Procedure authoring guidance from runtime lifecycle guidance.
- Serve phase-aware daemon status during startup and recovery, reject normal admission until ready, and add bounded `daemon wait-ready` behavior reused by service installation.

### Changed

- Scale Procedure v2 definitions and atomic item recording to 128 items, align text and list bounds, and return structured diagnostics at each limit.
- Replace whole-value evidence embedding with bounded previews and stable `evidence read` pagination through digest-bound continuation tokens.
- Retain up to 32 immutable inactive terminal sessions per worktree with explicit archive list, show, and purge commands and no automatic eviction.
- Make `start` resolve existing prepared, running, and terminal state explicitly, including atomic preservation of superseded running work.

### Fixed

- Preserve retained goal history when a successor session starts and keep `workspace doctor` available when normal workspace state is unreadable.

## v0.2.5 - 2026-08-19

### Added

- Replace legacy daemon text logs with bounded `podway.daemon-log/v1` JSONL and request, workspace, session, job, and diagnostic correlation without raw paths or caller values.

### Changed

- Move service bootstrap diagnostics to a daemon-owned bounded rotating stream and redirect LaunchAgent standard output and error descriptors to `/dev/null`.
- Keep bootstrap failures machine-readable while excluding filesystem paths and other raw internal error text from their public `message` field.

### Fixed

- Preserve eligible and force reset modes when released schema-v4 terminal receipts migrate to schema-v5, including retained job reads and replay after cold reopen.
- Preserve explicit `reset --all` recovery when workspace identity is readable but disposable full-store openability or internal-codec inspection fails.
- Serialize concurrent daemon log writers and refresh active-file identity and length before every write to preserve the retained per-file bound.

## v0.2.4 - 2026-08-18

### Added

- Separate session preparation from execution so `start` creates a prepared revision-0 session and `begin` creates attempt 1 with an optional initial goal.
- Add terminal ownership dispositions, exact-fenced eligibility previews, and explicitly confirmed force reset and replacement with bounded progress summaries.
- Emit prepared-lifecycle durable jobs through closed v4 wrappers while retaining released v3 wrappers and complete job read-back.

### Changed

- Migrate released schema-v3 and schema-v4 workspaces to schema-v5 on cold access and rebuild missing registry metadata through the sole-writer activation path.

### Fixed

- Reject item mutations against prepared sessions before attempt or item fences and without durable admission or state changes.
- Bind reduced patch-release evidence to the exact immutable commit that passed `make test` and reject symbolic baselines or non-regular release inputs.

## v0.2.3 - 2026-08-16

### Fixed

- Wait for launchd to report the prior LaunchAgent label as unloaded before requesting replacement bootstrap.
- Recover an authenticated prepared service publication by rerunning `podway daemon install` without an internal socket override.
- Keep prepared endpoints unavailable to ordinary clients and lifecycle commands until installation makes the receipt durable.
- Preserve `DAEMON_UNAVAILABLE` and its public details schema while distinguishing human service-lifecycle failure categories.

## v0.2.2 - 2026-08-16

### Added

- Add self-contained session observation with bounded active inputs, current workflow memory, and fenced mutation templates.
- Add atomic multi-item recording through the closed `podway record --stdin` contract.
- Add structured read-only recovery recipes to common automation errors.
- Ship the lightweight `small-change-v2` preset alongside `bug-fix-v2` and `sw-dev-v2`.

### Fixed

- Prevent different workspace UUIDs from owning the same canonical root and let confirmed reset converge a proven legacy duplicate-root registry generation.
- Preserve reset terminal replay after cold reopen across job status, unfiltered list, and lookup reads.

## v0.2.1 - 2026-08-15

### Added

- Ship the `sw-dev-v2` and `bug-fix-v2` presets and optional source-distributed `use-podway` guidance for AI coding agents.

### Changed

- Make Procedure v2 the only supported authoring and runtime model and remove Procedure v1 parsing, commands, presets, success schemas, and runtime paths.
- Emit successes through `podway.output/v3`, retain `podway.error/v1` failures and procedure-independent `/v1` contracts, and migrate worktree persistence to schema-v4.

## v0.2.0 - 2026-08-12

### Added

- Add the versioned Procedure v2 contract and closed result families while preserving the released v1 Procedure, output, error, status, next, version, IPC, and automation contracts.
- Add Procedure v2 validation, formatting, vetting, linting, source conversion, graph projections, previews, and scaffolds.
- Add the durable Procedure v2 runtime lifecycle with typed items, decisions, retry, skip, rework, blockers, goals, criterion assessments, and bounded history.
- Ship six built-in v1 and v2 presets and extend native qualification across CLI, daemon, queues, jobs, concurrency, recovery, SQLite reopen, isolation, and replay.

## v0.1.2 - 2026-08-03

### Added

- Add an offline Rust verifier for source and packaged contract manifests, schema registries, references, complete envelopes, and binary identity.
- Add an early production singleton diagnostic and close provenance, Dolgorae handoff, packaged conformance, and final bundle verification.

### Fixed

- Repair CLI and daemon build identity so both emit the same complete `podway.version-result/v1` object.
- Make runtime daemon probes decode the complete `podway.ipc/v1` response and reject malformed version results.

## v0.1.1 - 2026-08-03

### Changed

- Simplify local development and distribution gates by removing cached test receipts and duplicate release-only test execution.

### Fixed

- Normalize daemon version command grammar and align packaged daemon identity probes with the public interface.
- Stabilize inactive-workspace SQLite reconciliation tests by comparing logical state instead of transient WAL and SHM layouts.
- Handle macOS process-group reuse after a launchctl child exits and contain recursive crash-test output within the parent test run.

## v0.1.0 - 2026-08-02

### Added

- Publish the initial native Apple Silicon macOS release with the Podway CLI, daemon, LaunchAgent service, public v1 contracts, deterministic archives, and packaged conformance.
