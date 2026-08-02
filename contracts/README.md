# Executable Repository Contracts

This directory contains versioned inputs consumed directly by Podway's repository
verification tools. It is intentionally separate from `docs/`, which is the
canonical home for human-readable product and contributor documentation.

- root JSON files define repository contracts and schemas;
- `interfaces/` freezes internal interface contracts;
- canonical build assets live under `assets/` and are verified without generated
  mirrors.

Raw logs, fuzz corpora, and machine-specific output belong under ignored
`artifacts/`. The repository-root `make test` command is the sole release-readiness
gate.
