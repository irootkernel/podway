# Contributor Documentation

This directory is the canonical source for the information and machine-readable
contracts needed to understand and contribute to Podway. The repository-root
`schemas/`, `spec/`, and `presets/` directories are generated mirrors used by the
build and release archive; edit their sources here instead.

## Start here

1. [Project](project.md) explains the problem, goals, vocabulary, and boundaries.
2. [Repository structure](structure.md) maps crates, assets, tests, and dependency rules.
3. [Architecture](architecture.md) follows a request through the CLI, daemon, queue, and store.
4. [Contributing](contributing.md) gives implementation tips and verification commands.
5. [Roadmap](roadmap.md) records the completed v0.1.0 implementation sequence.

## Detailed reference

- [Product](reference/product/00-product-overview.md): detailed workflows, terminology,
  goals, and non-goals.
- [Architecture](reference/architecture/10-system-architecture.md): daemon, worktree,
  service, and Rust implementation contracts.
- [Domain](reference/domain/20-domain-model.md): procedures, items, transitions, and
  session lifecycle.
- [Interfaces](reference/interfaces/30-cli-specification.md): CLI, JSON, IPC, errors,
  and exit codes.
- [Storage](reference/storage/40-sqlite-model.md): schema, transactions, recovery,
  retention, and reset behavior.
- [Operations](reference/operations/50-security-and-trust.md): trust boundary,
  observability, packaging, installation, and upgrades.
- [Quality](reference/quality/60-testing-and-conformance.md): tests, acceptance criteria,
  and requirements traceability.
- [Risk register](reference/risks.md), [ADRs](adr/), and [worked examples](examples/).

## Canonical assets

- [JSON schemas](schemas/) define versioned authoring and response shapes.
- [Specifications](spec/) contain the SQLite DDL, command and error catalogs,
  transition matrix, and LaunchAgent template.
- [Built-in preset sources](presets/) are embedded in the product after validation.

Run `make sync-docs-assets` after editing these directories. Never edit the root
mirrors directly.

## Precedence

When sources disagree, resolve them in this order:

1. accepted [Architecture Decision Records](adr/);
2. machine-readable [schemas](schemas/) and [specifications](spec/);
3. detailed behavioral documents under `reference/`;
4. core contributor guides;
5. examples.

Fix every affected source when resolving a contradiction. There are no intentional
contract disagreements in the v0.1.0 baseline.
