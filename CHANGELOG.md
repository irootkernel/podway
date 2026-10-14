# Changelog

This file records concise shipped outcomes and the planned next stable release.

## v0.2.15 - Unreleased

## v0.2.14 - 2026-10-13

### Changed

- Rewrite the runtime and Procedure-authoring skills around the current Direct
  model, shorten references, and preserve lifecycle, recovery, evidence, Goal
  and delegation safeguards.
- Pin optional skill installation and examples to `v0.2.14`, clarify writer
  recovery and workspace reset, and correct contributor and adapter terminology.
- Require fresh runtime state to use `v0.2.14` with workspaces bound to the
  `v0.2.13` manifest: no writer predecessor is declared. Finish or abandon old
  sessions with their original writer before upgrading; an explicitly confirmed
  unfenced reset-all discards runtime history without backup or conversion.

## v0.2.13 - 2026-10-12

### Changed

- Run Podway commands directly without a daemon, preserve crash-safe retries,
  and return replacement guidance for retired commands and explicit reset
  guidance for legacy workspaces.
- Publish the direct contract generation: Output v4, Error v2, version-result v2,
  manifest v2, observation v5 and SQLite generation 15, without daemon-era
  compatibility or a declared Store predecessor.
- Retire daemon, job, IPC, socket, account-runtime and workspace-mode commands,
  flags and error families, including exit class 3. Retired syntax returns
  `COMMAND_RETIRED` (exit 2); `job lookup` points to `outcome lookup`.
- Reconcile uncertain mutations through read-only `outcome lookup` using the
  original key and request. Only committed successes retain exact replayable
  responses; rejected requests retain no outcome.
- Refuse legacy generations 1–14 unchanged. Explicitly confirmed reset discards
  runtime history and creates a fresh workspace without backup or migration;
  an older binary cannot reset or remove a newer Store.
- Require compatible Aquarium and Dolgorae releases before installation. Finish
  legacy sessions, retire every old writer through its original owner, install
  the qualified single CLI, separately retire legacy account state, then confirm
  reset of each selected worktree. Contract corrections land before `v0.2.13`
  so consumers pin the final contract identity once.

### Fixed

- Replace discontinued YAML dependencies with maintained Norway forks while
  preserving Procedure and workspace configuration contracts.
- Preserve maintenance refusals when recovery diagnostics race with workspace removal.
- Align product and lifecycle documentation, worked examples and the archive-purge catalog with the direct contract, and retire obsolete prototype feedback.
- Reuse immutable Procedure and item data during writes, and require executable native qualifications, nonempty exact tests and frozen development bundles.
- Preserve removal exclusion through final cleanup and report observed maintenance markers with original-key recovery guidance.
- Recover invalid current workspaces through confirmed unfenced reset or removal, and refuse newer Stores with compatible-version guidance.
- Hash local artifacts before writer admission and reject changed file stamps without reading artifact bytes inside the transaction.
- Apply timeouts only to lock waits, permit an immediate attempt with no remaining budget, and preserve the original budget through interactive prompts.
- Preserve independent item writes across concurrent changes, report current stale revisions, and re-evaluate concurrent session starts against current state.

- Return valid JSON errors for local CLI failures and oversized output, honor a late `--json`, and provide retired-family help and outcome identity completion.

- Apply workspace outcome retention to archive purge while preserving original session outcomes.
- Honor consumer-attempt fences on the first evidence page and its continuations.
- Resume maintenance with original idempotency keys that begin with a hyphen.
- Verify delegation receipt digests at the Store acceptance boundary.
- Preserve original maintenance request fences and force in retries and recovery commands.
- Reject invalid typed batch counts and duplicate items before preparing mutations.
- Preserve maintenance guidance when workspace removal interrupts a configuration read.

## v0.2.12 - 2026-09-29

### Added

- Add local opt-in supervised delegation for custom Procedure v2 actions with work-package, acceptance, and receipt commands and explicit completion gating.
- Add a tested custom Procedure and step-by-step Podway skill guidance for supervised delegation.

### Fixed

- Keep `daemon status` responsive with many active worktrees by reading live job counts without copying workspace databases.
- Bound `daemon wait-ready` by one deadline and enter Podway sessions through worktree observation.
- Shorten daemon startup recovery by avoiding repeated database inspections and idle job drains while retaining guarded Store validation.
- Return the declared detached admission response for supervised-delegation acceptance so callers can reconcile its durable job.
- Preserve decision-node evidence in delegated work packages and account for it in package size limits.
- Correct the optional agent-policy template to accept delegated observation responses.

## v0.2.11 - 2026-09-23

### Changed

- Raise the Procedure v2 definition and graph-placement ceiling from 64 to 1,024,
  with bounded iterative graph analysis, complete counter projections, and
  aggregate `next` and observation admission that preserves the 1 MiB frame.
- Stop failing archival and start when a worktree already retains 32 inactive
  sessions. `archive list` now reports the exact total, a newest-first window, and the oldest summary.
- Derive durable workspace identity from canonical Git paths, with an in-place schema-v13 migration for exact-root schema-v10 through schema-v12 workspaces that preserves runtime-mode ownership.

### Fixed

- Preserve structured `WORKSPACE_MODE_MISMATCH` details, including pre-admission state, instead of masking mutation preflight failures as `INTERNAL_ERROR`.
- Show Procedure authoring diagnostic codes and the `procedure check` remediation path in
  human-readable start failures, including the shell-safe custom Procedure path,
  without changing the JSON error contract.
- Make bounded human diagnostic output disclose truncation, distinguish canonical
  projection character and encoded-byte failures, and verify payload-recipe test
  locators against their current source symbols.
- Return `PRESET_NOT_FOUND` instead of `UNSUPPORTED_V2_CAPABILITY` when a
  Procedure v2 start or replacement names an unknown built-in preset.
- Let confirmed `reset --all` recover an exact local Store binding when its
  global registry entry is missing.
- Keep an admitted reset marker until the target scheduler is active, replay
  schema-v12 reset markers and terminal receipts across the identity migration,
  and revalidate Git authority after SQLite binds its path but before Store mutation.
- Prevent existing-workspace activation and recorded mode-switch recovery from
  recreating an absent or empty Store, bind mode-switch recovery to canonical Git
  identity, and publish mode-switch files and registry entries at guarded atomic
  boundaries.

## v0.2.10 - 2026-09-14

### Added

- Preview and retire one ordinary account runtime or a frozen all-mode selection
  through bounded confirmation tokens and fail-closed preflight, with managed-runtime exclusions, idle-daemon coordination, crash-safe recovery, exact partial-failure retry, and preservation of worktree state and unrelated namespaces.
- Explain why a stored check result remains unsatisfied with a closed
  `unsatisfied_reason` and a CLI comparison of declared and recorded operation identities.

### Changed

- Clarify how Podway cooperates with Codex context management and goals, preserving
  authorized session boundaries, safe recovery, and evidence-based completion.

### Removed

- Remove the never-emitted check-result entry branch from the atomic item-record
  result schema.

## v0.2.9 - 2026-09-06

### Added

- Accept top-level `-h` and `--help` as aliases for `podway help`.

### Fixed

- Prefix CLI version text and shared CLI/daemon identity versions with `v`.
- Preserve committed job admission identity and report a structured `INTERNAL_ERROR`
  when a mutation response cannot be reconstructed.
- Let `workspace repair` restore a missing registry entry after validating
  the worktree's Git and durable Store identity.
- Let confirmed `reset --all` reach daemon recovery when normal workspace status
  fails, including after Git identity replacement.
- Allow UUID-fenced `workspace remove` retries to succeed after an earlier removal
  deletes the workspace configuration.

## v0.2.8 - 2026-09-03

### Added

- Add bounded named runtime modes with complete isolated daemon namespaces and preserve `--dev` as the exact alias for mode `dev`.
- Bind each workspace config, registry generation, and SQLite store to one runtime mode, with token-bound idle switching that deletes current and archived runtime history while preserving Git and worktree content, plus marker-backed replay recovery.
- Add an immutable Aquarium managed-service bundle with token-bound idle activation, exact-generation rollback, and bounded recovery debt.
- Move contributor and packaged release qualification to purpose-bound private named runtimes with exact executable identity and bounded cleanup.

### Fixed

- Accept Aquarium-sealed managed-service executables when validating the exact development runtime.
- Reject symbolic links anywhere in immutable Aquarium managed-service bundles during digest and generation validation.
- Route the bundled Aquarium CLI to its active managed runtime without production or default-development fallback.
- Admit `workspace mode plan` and `workspace mode apply` through their typed client request contracts.
- Align the Dolgorae adapter handoff's released workspace upgrade boundary with SQLite schema-v10.

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
