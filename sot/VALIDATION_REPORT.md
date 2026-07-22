# Design Package Validation Report

**Design version:** 1.0.1-design  
**Validation date:** 2026-07-13  
**Result:** passed

## Scope

This report covers the internal consistency of the reconciled design package for complete public release `v0.1.0`. It does not certify an implementation, runtime behavior, security posture, or release binary.

## Design baseline binding

- Recorded S0 payload: `022167d808f5f0f85711bfdfa94d1a0165de711a6eda51bb9209e9e873ea342d`.
- Recorded S0 index: `9cf070eb8e64d46dffd1b33580d547ed631efa6ca91ed0867f868f7190972c12`.
- S1 transaction: 11 payload-bound derivative files applied with full-package drift guards.
- Initial migration: non-file `uninitialized-database`, predecessor `schema-0-uninitialized`, result `schema-v1`.
- Public and storage contract identifiers remain v1.
- Migration behavior is revalidated by the repository-local integration suite.

## Checks performed

| Check | Result |
|---|---|
| All Markdown relative links resolve inside the package | passed, 104 links |
| All JSON documents parse | passed, 15 files |
| All YAML documents parse | passed, 7 files |
| All JSON Schemas satisfy Draft 2020-12 structural and local-reference validation | passed, 8 schemas |
| All four built-in presets validate against `podway.procedure/v1` | passed |
| The custom procedure example validates against `podway.procedure/v1` | passed |
| The workspace configuration example validates against `podway.workspace/v1` | passed |
| All JSON examples validate against their declared schemas | passed, 6 examples |
| The reference SQLite DDL creates a fresh database | passed |
| SQLite `PRAGMA user_version` equals `1` | passed |
| Required relational tables and foreign-key integrity checks succeed | passed, 11 tables |
| Public error references resolve to the machine-readable catalog | passed, 58 codes |
| Canonical command catalog contains every required command variant | passed, 43 entries |
| State-transition matrix covers every state-changing command variant | passed, 20 rows |
| Preset stage IDs and per-stage item IDs are unique | passed |
| Gate S version, migration, sequencing, and delivery contracts are cross-consistent | passed |
| Package contains no Hangul text | passed |
| Package contains no em dash or en dash characters | passed |
| Package manifest lists every regular file | passed, 79 files |

## Contracts validated together

The following combinations were validated as a single contract set:

1. `schemas/procedure-v1.schema.json`, the four files in `presets/`, and the custom procedure example.
2. `schemas/workspace-v1.schema.json` and `examples/.podway/config.yaml`.
3. Public JSON schemas and every file in `examples/json/`.
4. `spec/command-catalog.yaml`, `spec/state-transition-matrix.csv`, the CLI specification, and the state-transition specification.
5. `spec/error-codes.json` and error identifiers referenced throughout the package.
6. `spec/sqlite-v1.sql`, storage/lifecycle design, migration testing, product acceptance, and release packaging.
7. The recorded S0 baseline, S1 transaction, requirements traceability, implementation plan, and decision record.

## Required implementation revalidation

The development repository must reproduce these checks through `make test` and add implementation conformance tests. Successful package validation does not replace:

- pure-domain property tests;
- production schema-0/uninitialized to schema-v1 migration tests using `uninitialized-database`;
- real daemon crash-injection tests;
- concurrent worktree and same-item conflict tests;
- IPC framing fuzz tests;
- macOS LaunchAgent integration tests;
- release acceptance against `docs/60-quality/61-product-acceptance.md`.

## Integrity

`checksums.sha256` is regenerated only after this report is finalized and then contains the SHA-256 digest of every regular package file except `checksums.sha256` itself. Verify from the package root with:

```bash
shasum -a 256 -c checksums.sha256
```
