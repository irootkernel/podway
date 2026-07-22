# Implementation Status

This document is an informational implementation-progress index. Normative product
behavior remains in the SOT documents and machine-readable contracts.

## Goal mapping

| Goal | Design scope | Status | Completed | Primary evidence |
|---|---|---|---|---|
| `G001` | Reconcile and revalidate the source-of-truth baseline | **complete / verified** | 2026-07-13 | `VALIDATION_REPORT.md` |
| `G002` | Phase 0: repository and executable contract lock | **complete / verified** | 2026-07-14 | `contracts/locks/`; `contracts/handoffs/`; `make test-prepare` |
| `G003` | Phase 1: domain, configuration, and presets | **complete / verified** | 2026-07-14 | `make test-unit`; `make test-int` |
| `G004` | Phases 2–3: persistence queue and Git boundary | **complete / verified** | 2026-07-15 | `make test-int` |
| `G005` | Phase 4: daemon IPC and real production vertical slice | **complete / verified** | 2026-07-15 | `make test-e2e` |
| `G006` | Phase 5: CLI and versioned JSON | **complete / verified** | 2026-07-16 | `make test-int`; `make test-e2e` |
| `G007` | Phase 6: macOS service integration and packaging behavior | **complete / verified** | 2026-07-22 | `make test`; distribution archive E2E |
| `G008` | Phase 7: preset dogfooding and UX correction | **complete / verified** | 2026-07-22 | `make test-e2e` |

## Release readiness

The repository has one required release gate: `make test`. It runs
`test-prepare`, `test-unit`, `test-int`, `test-fuzzing`, and `test-e2e`
sequentially. A revision is release-ready when this command succeeds locally. No independent signature,
approval quorum, holdout run, qualification archive, or attestation bundle is
required.

Because `test-prepare` synchronizes generated assets and applies formatting, release
tags and archives must use the resulting formatted tree. Signing, notarization, and
publication metadata describe a distributed artifact but do not change whether the
tested revision is release-ready.

## Evidence rules

- `complete / verified` means the implementation is exercised by the current local gate.
- The status applies only after a repository-root `make test` succeeds for the exact resulting tree.
- Evidence paths outside `sot/` are repository-relative implementation references.
- A successful command applies to the exact resulting source tree; later changes require a new run.
- `docs/60-quality/61-product-acceptance.md` remains the product-acceptance authority.
