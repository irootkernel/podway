# Implementation Status

This document is an informational implementation-progress index. It does not change the normative design, replace release acceptance, or certify the complete `v0.1.0` product. The design-package validation scope and disclaimer in `VALIDATION_REPORT.md` remain unchanged.

## Goal mapping

| Goal | Design scope | Status | Completed | Primary evidence |
|---|---|---|---|---|
| `G001` | Gate S: reconcile and revalidate the source of truth | **complete / verified** | 2026-07-13 | `VALIDATION_REPORT.md`; accepted S0 payload `022167d808f5f0f85711bfdfa94d1a0165de711a6eda51bb9209e9e873ea342d` |
| `G002` | Phase 0: repository and executable contract lock | **complete / verified** | 2026-07-14 | `contracts/locks/phase-0a-contract-lock.json`; `contracts/locks/phase-0b-contract-lock.json`; `contracts/locks/phase-0c-contract-lock.json`; `artifacts/phase0/final-handoff-report.json` |
| `G003` | Phase 1: domain, configuration, and presets | **complete / verified** | 2026-07-14 | `crates/podway-core/tests/`; `crates/podway-config/tests/`; `crates/podway-presets/tests/` |
| `G004` | Phases 2–3: persistence queue and Git boundary | **complete / verified** | 2026-07-15 | `crates/podway-store/tests/`; `crates/podway-git/tests/` |
| `G005` | Phase 4: daemon IPC and real production vertical slice | **complete / verified** | 2026-07-15 | `tools/run_g005_vertical.py`; `crates/podway-cli/tests/phase4_production_vertical.rs` |
| `G006` | Phase 5: CLI and versioned JSON | **complete / verified** | 2026-07-16 | `crates/podway-cli/tests/phase5_cli.rs`; `crates/podway-protocol/tests/phase5_slice_contract.rs`; `crates/podway-daemon/tests/phase5_dispatch.rs` |
| `G007` | Phase 6: macOS service integration and packaging | **implementation complete / batch validation pending** | 2026-07-17 | `crates/podway-service/tests/phase6_native_service.rs`; `crates/podway-cli/tests/phase5_cli.rs`; `crates/podway-daemon/tests/phase4_daemon_binary.rs` |
| `G008` | Phase 7: preset dogfooding and UX correction | **implementation complete / batch validation pending** | 2026-07-17 | `tools/run_g008_dogfood.py`; `crates/podway-cli/tests/phase4_production_vertical.rs` |
| `G009` | Phase 8: hardening, release artifacts, and final acceptance | **pending final Apple Silicon release acceptance / incomplete** | not complete | `docs/70-delivery/70-implementation-plan.md`, Phase 8 |

## Phase 8 sub-checkpoints (informal, G009-internal)

The IDs `G010`–`G043` are **not** part of the normative design — `docs/70-delivery/70-implementation-plan.md` defines only Phase 0–8 (`G001`–`G009`). They are informal implementation checkpoints minted ad hoc while executing Phase 8/`G009`, recorded only under `artifacts/g0NN/`, which is gitignored and untracked: this evidence trail exists on the implementing machine only and is not reproducible from a fresh clone. None of these rows supersede or amend the `G009` row above; `G009` remains pending final Apple Silicon release acceptance.

| Goal | Scope summary | Evidence artifact | Status |
|---|---|---|---|
| `G010` | — | none | number never allocated |
| `G011` | Daemon/service typed-observability test pass | `artifacts/g011/observability-test-report.json` | stale checkpoint (predates post-checkpoint fix commits) |
| `G012` | Service crash-boundary tests and fuzz corpus | `artifacts/g012/crash-fuzz-test-report.json` | stale checkpoint |
| `G013` | Release-policy contract coverage | `artifacts/g013/release-policy-report.json`, `release-policy-test-report.json` | stale checkpoint |
| `G014` | Daemon observability contract re-verification | `artifacts/g014/observability-test-report.json` | stale checkpoint |
| `G015` | Service publication crash-boundary tests | `artifacts/g015/service-publication-test-report.json` | stale checkpoint |
| `G016` | Release trust-chain self-test, protocol, traceability, crash-registry run | `artifacts/g016/release-trust-chain-test-report.json` | stale checkpoint |
| `G017` | Production observability daemon suite | `artifacts/g017/production-observability-test-report.json` | stale checkpoint |
| `G018` | Service fuzz/crash-boundary report | `artifacts/g018/service-fuzz-test-report.json` | stale checkpoint |
| `G019` | Release-trust closure; remote publication self-declared `not_applicable` locally (needs protected GitHub Environment credentials and an immutable RC run) | `artifacts/g019/g019-quality-gate.json`, `release-trust-test-report.json` | stale checkpoint, self-declared partial |
| `G020` | Daemon observability hardening gate | `artifacts/g020/g020-quality-gate.json`, `observability-test-report.json` | stale checkpoint |
| `G021` | Full-workspace and rustup 1.85.0 rerun, fuzz gate | `artifacts/g021/g021-test-report.json` | stale checkpoint, self-flagged "final confirmation pending" |
| `G022` | — | none | number never allocated |
| `G023` | Apple-Silicon-only support-boundary claim | `artifacts/g023/apple-silicon-completion-report.json` | stale checkpoint |
| `G024` | Reset crash-window contracts C14–C16, RC target-tuple exactness | `artifacts/g024/g024-test-report.json` | stale checkpoint |
| `G025`–`G027` | — | none | numbers never allocated |
| `G028` | Publication-controller self-test | `artifacts/g028/g028-test-report.json` | stale checkpoint, self-flagged deferred verification manifest |
| `G029`–`G032` | — | none | numbers never allocated |
| `G033` | macOS lifecycle qualification controller | `artifacts/g033/g033-test-report.json` | stale checkpoint |
| `G034` | Release-publication controller binding proof | `artifacts/g034/g034-quality-gate.json`, `g034-test-report.json` | stale checkpoint |
| `G035` | Native-service and store schema/integrity/reset-lifecycle pass | `artifacts/g035/g035-quality-gate.json`, `g035-test-report.json` | stale checkpoint (reset-lifecycle evidence predates the store race fixes) |
| `G036` | Direct-evidence report backing the product-acceptance matrix (71 criteria, 50 exact commands) | `artifacts/g036/g036-test-report.json`; generator `tools/generate_g036_report.py` | regenerated from the current tree, 2026-07-21 |
| `G037` | Quality-gate re-validation over the G036 report | `artifacts/g037/g037-quality-gate.json` | stale gate (validated a superseded G036 report; no independent test report) |
| `G038` | Lifecycle proof semantics and verifier independence | `artifacts/g038/g038-test-report.json` | stale checkpoint |
| `G039` | — | none | number never allocated |
| `G040` | Semantic proof-membership attribution inside the verifier | `G040_*` constants in `tools/verify_g009_qualification.py` only | naming artifact, no checkpoint directory |
| `G041` | — | none | number never allocated |
| `G042` | Terminal gate: verifier self-test, matrix, G036 report, workspace test/clippy/fmt, diff check | `artifacts/g042/g042-test-report.json`; generator `tools/generate_g042_report.py` | regenerated from the current tree, 2026-07-21 |
| `G043` | Lifecycle and qualification security closure | none (`PROBLEM.md` open items) | incomplete; the thin-arm64 `MH_EXECUTE` executable check is now enforced by `tools/verify_g009_qualification.py` and `tools/run_g009_qualification.py`, remaining items need the release pipeline |

## Current release boundary

Completion of `G001` through `G008` does not mean the complete product is released. `G007` and `G008` still require the final validation-batch close, and `G009` native Apple Silicon release acceptance remains incomplete.

## Evidence rules

- `complete / verified` means the goal's scoped implementation or design gate has direct current-repository evidence.
- Evidence paths outside `sot/` are repository-relative code spans, not normative package links.
- Design validation, implementation verification, and final release acceptance remain separate claims.
- `docs/60-quality/61-product-acceptance.md` remains the authority for complete-product acceptance.
