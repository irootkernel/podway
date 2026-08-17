# Architecture Decision Records

Architecture Decision Records explain decisions that constrain more than one
implementation area or would be expensive to rediscover. Accepted ADRs outrank
narrative architecture and specifications when sources conflict.

## Status and numbering

- Use the next four-digit identifier and a descriptive lowercase filename.
- Record `Proposed`, `Accepted`, `Superseded`, or `Rejected` near the title.
- Never rewrite the decision of an accepted ADR. Add a new ADR and link the
  supersession in both records.
- Small local implementation choices belong in implementation tips, not an ADR.

## Index

- [ADR-0001](0001-current-task-session-focus.md): focus on the current task session
- [ADR-0002](0002-single-active-stage.md): one active stage attempt
- [ADR-0003](0003-daemon-single-writer.md): daemon as sole normal writer
- [ADR-0004](0004-worktree-local-state.md): worktree-local task state
- [ADR-0005](0005-rust-and-macos-first.md): Rust implementation and macOS delivery
- [ADR-0006](0006-same-user-local-trust.md): same-user local trust
- [ADR-0007](0007-stage-items-not-evidence-ledger.md): typed stage items
- [ADR-0008](0008-relational-state-not-event-sourcing.md): relational current state
- [ADR-0009](0009-artifact-metadata-only.md): artifact metadata only
- [ADR-0010](0010-generic-cli-json-integration.md): generic CLI and JSON integration
- [ADR-0011](0011-local-make-test-release-gate.md): local release gate
- [ADR-0012](0012-explicit-daemon-endpoint-and-canonical-per-user-podway-home.md): canonical user home and endpoint
- [ADR-0013](0013-native-apple-silicon-macos-only.md): native Apple Silicon macOS support
- [ADR-0014](0014-single-canonical-asset-tree.md): one canonical build asset tree
- [ADR-0015](0015-constrained-single-cursor-graph.md): constrained single-cursor graph (superseded by ADR-0017)
- [ADR-0016](0016-recorded-item-workflow-memory.md): recorded-item workflow memory
- [ADR-0017](0017-single-cursor-convergence.md): single-cursor convergence
- [ADR-0018](0018-v2-success-envelope.md): versioned Procedure v2 success envelope
- [ADR-0019](0019-procedure-v2-only-product.md): Procedure v2-only product and unified success envelope
- [ADR-0020](0020-managed-dev-runtime-isolation.md): isolated managed `--dev` runtimes
- [ADR-0021](0021-separate-session-preparation-from-execution.md): separate session preparation from execution
