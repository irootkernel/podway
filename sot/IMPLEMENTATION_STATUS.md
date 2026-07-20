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

## Current release boundary

Completion of `G001` through `G008` does not mean the complete product is released. `G007` and `G008` still require the final validation-batch close, and `G009` native Apple Silicon release acceptance remains incomplete.

## Evidence rules

- `complete / verified` means the goal's scoped implementation or design gate has direct current-repository evidence.
- Evidence paths outside `sot/` are repository-relative code spans, not normative package links.
- Design validation, implementation verification, and final release acceptance remain separate claims.
- `docs/60-quality/61-product-acceptance.md` remains the authority for complete-product acceptance.
