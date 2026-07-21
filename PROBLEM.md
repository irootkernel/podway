# Podway — Incomplete Work and Handoff

## Status: superseded (2026-07-21)

The handoff below is preserved verbatim as a historical record of an interrupted implementation effort. A recovery effort (11 fix commits plus this checkpoint, `b6fbd4e..d50660d`) has since closed most of what it raised. The recovery outcome is summarized first; the original handoff follows unchanged.

## Recovery outcome (2026-07-21)

- **Store concurrent-init race** (the "Known Active Failure" below): root-caused and fixed. `e81fafc` converges orphaned-ownership-marker recovery (distinguishes live from dead owners by the marker flock; fixes a stat-then-open TOCTOU); `253ac6a` guards the orphan-marker reap against an `O_EXLOCK` creation-gap race. The three concurrent publication tests ran 4000/4000 hang-free stress iterations post-fix; two new regression tests pin the fixed behaviors.
- **Full workspace suite: green.** 683 passed / 0 failed, confirmed on multiple runs, including `cargo test --workspace --locked` as command #4 (exit 0) in `artifacts/g042/g042-test-report.json`.
- **G042 semantic closure:** matrix/migration-evidence/policy/verifier pins refrozen; hermetic G036 replay completed all 50 sandboxed commands on the Apple-Silicon host (`84713db`, generator `tools/generate_g036_report.py`); the terminal G042 gate regenerated with all 7 commands exit 0 (generator `tools/generate_g042_report.py`, `artifacts/g042/g042-test-report.json`). One G042 sub-item remains open: the original process's cleanup/architecture/executor-QA red-team gate rerun on a frozen snapshot — an independent two-lens review was performed instead, in-session.
- **G043:** thin-arm64 `MH_EXECUTE` executable validation is now enforced in both qualification tools (`09a09d1`). The remaining items still need the self-hosted release pipeline and human PGP attestations — unchanged, honestly open.
- **Phase 0 evidence chain:** contracts, locks, and receipts regenerated and green (`verify_contracts --all`, `import_sot --check`, `phase0_receipts --check`, `run_verification --check`).
- **Daemon observability:** the shutdown-linearization race fixed (`0d6832e`), verified 5000/5000 isolated stress iterations plus 25 clean full-binary runs.
- **Known monitored item:** one unreproduced PAC-044 flake observation (a fresh `session.start` error after ~31s under parallel load); 210+ subsequent full-binary runs have been clean, including a 150-run instrumented hunt.
- **Not claimed:** this is not a release-ready or aggregate-acceptance claim. `G009` (Phase 8 final Apple Silicon release acceptance) remains pending per `sot/IMPLEMENTATION_STATUS.md`.
- Durable details live in `sot/IMPLEMENTATION_STATUS.md` ("Phase 8 sub-checkpoints" table) and `artifacts/g036/`, `artifacts/g042/` (local, gitignored, not reproducible from a fresh clone).

## Original handoff (historical, 2026-07-20)

# Podway — Incomplete Work and Handoff

## Purpose

This document describes work that remains incomplete after a large, interrupted implementation effort. It is intended as a handoff to another expert AI. Do not assume the current working tree is release-ready, internally consistent, or fully attributable.

## Mandatory Product Scope

- Support **Apple Silicon macOS only**.
- The supported native target is arm64 Apple Darwin (`aarch64-apple-darwin`).
- Intel (`x86_64`) and Rosetta support are out of scope.
- Intel/Rosetta references may remain only where they explicitly test rejection of unsupported environments.
- Do not restore the abandoned dual-architecture release or qualification design.
- The source-of-truth requirements live under `./sot/`.

## Repository State Warning

- The working tree contains a very large set of uncommitted changes accumulated across many implementation and review passes.
- No intermediate checkpoint commits were created during this effort.
- Changes from multiple agents and pre-existing user work may be mixed in the same files.
- Do not revert, reset, stash, mass-format, or overwrite the working tree before first producing a complete inventory and backup strategy.
- Do not assume that every modified file belongs to one feature or one author.
- The prior inline Ultragoal was dropped at the user's request; incomplete durable stories were not marked complete.

## What Was Partially Implemented

The working tree contains substantial implementation and test work in these areas, but the aggregate result has not received a clean final verification:

- Apple-Silicon-only release and qualification policy.
- Workspace repair, job cancellation, and doctor behavior.
- Daemon and service lifecycle hardening.
- Qualification artifact identity and dependency binding.
- Temporary-file ownership, locking, cleanup, and crash recovery.
- Protocol error-catalog and receipt-validation hardening.
- Git worktree confinement, safety, and non-mutation proofs.
- Observability shutdown linearization and counter snapshot consistency.
- Release/publication handoff validation.
- Product acceptance and SOT evidence matrices.

These are implementation claims only. They are not proof that the corresponding SOT requirements are complete.

## Known Verification Results

The following checks were observed passing before work stopped:

- `cargo check --workspace --all-targets --locked`
- Python bytecode compilation for the qualification scripts.
- Focused regressions for the modified daemon fragmented-request test.
- Focused regressions for two modified observability race tests.

A final clean full-workspace test run was **not** obtained.

## Known Active Failure

The last full Rust test attempt stopped on a store concurrent-initialization/reset-lifecycle race in:

- `crates/podway-store/tests/phase2_reset_lifecycle.rs`

A stress reproduction showed both contenders could fail rather than one successfully establishing the workspace:

- First contender: `StorageUnavailableV1 { reason: StorageIo }`
- Second contender: `StorageIntegrityV1 { check: WorkspaceIdentity }`

The exact production root cause was not resolved. Investigate synchronization, ownership-marker publication, SQLite creation/open ordering, workspace-identity publication, cleanup ordering, and recovery semantics. Do not weaken the test merely to accept two failures unless the SOT explicitly requires that behavior.

## Incomplete Durable Work

### G042 — Verification and Semantic Closure

The following work was not completed:

1. Run the focused semantic and runtime test set against the final working tree.
2. Resolve remaining G042 verification blockers, including stale or mismatched evidence locators and source digests.
3. Complete hermetic G036 replay and prove its full input/tool/runtime closure.
4. Ensure the product acceptance matrix represents direct, non-circular proofs for all required PAC criteria.
5. Rerun the cleanup review, architecture review, and executor QA/red-team gates on one frozen snapshot.
6. Produce a valid durable checkpoint and evidence receipt.

### G043 — Lifecycle and Qualification Security Closure

The following work was not completed:

1. Make production version compatibility mandatory at every required lifecycle boundary.
2. Validate that the shipped executable is a thin arm64 macOS `MH_EXECUTE` Mach-O.
3. Reject x86_64, translated/Rosetta, universal/fat, relabeled, or mismatched artifacts.
4. Verify qualification snapshots, staged dependencies, sandbox profile identity, executable identity, and lifecycle drift detection end to end.
5. Run lifecycle, service, CLI, release, publication, and qualification verification on Apple Silicon.
6. Complete final cleanup, architecture review, and executor QA/red-team gates.
7. Produce a valid durable checkpoint and evidence receipt.

### Aggregate Acceptance

The following final work was not completed:

1. Map every requirement under `./sot/` to an implementation location and direct test/evidence artifact.
2. Identify requirements that remain missing, partially implemented, circularly proven, or represented only by policy assertions.
3. Run the complete workspace and SOT verification suite on the final frozen tree.
4. Regenerate all stale hashes, source locators, evidence matrices, reports, and qualification receipts from that same frozen tree.
5. Prove Apple-Silicon-only release and publication behavior end to end.
6. Obtain clean cleanup, architecture, product, code, and adversarial QA verdicts.
7. Create a fresh aggregate acceptance receipt only after all required stories and blockers are genuinely complete.


## Explicit Non-Claims

At handoff time, none of the following can be claimed:

- All `./sot/` functionality is implemented.
- The complete Rust test suite passes.
- The current working tree is release-ready.
- The qualification/release evidence is fresh and internally consistent.
- The remaining store race is understood or fixed.
- G042, G043, or aggregate acceptance is complete.
