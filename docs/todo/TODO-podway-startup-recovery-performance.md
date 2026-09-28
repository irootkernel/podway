# Startup Recovery Performance at Large Store Size

## Context

- Document state: `Candidate`
- Owning roadmap epic: none
- Release target: not assigned

An isolated development daemon with 22 initialized worktrees and about 164 MB
of checkpointed SQLite main files originally remained in `recovering` beyond
180 seconds after restart. The fixture used supported Podway commands to create
six bounded record batches per worktree. Before restart, with about 138 MB of
main files and 115 MB of WAL files, 20 live `daemon status` calls completed in
at most 14.32 ms. The cold-start result was distinct from live status latency.

`V2DHP-003` removed repeated database copies and complete integrity scans from
startup activation while retaining full validation at the guarded Store open.
It also skips the worker drain for a proven empty recovered queue. An
optimization trial on the same 22-worktree fixture reached `ready` in 47.31
seconds. The final startup-only routing passed a two-worktree restart with
14,925,824 bytes of checkpointed main databases in 7.76 seconds; the 22-worktree
fixture was not rerun after that routing correction. A repeatable latency target
for larger Stores, more worktrees, or a more heavily loaded host remains open.

## Goal

Characterize cold recovery at larger scales and decide whether further bounded
improvements are needed without weakening durable-state validation, recovery
ordering, or truthful readiness reporting.

## Non-goals

- Do not skip integrity checks, treat `recovering` as `ready`, or increase the
  default readiness deadline without a measured product decision.
- Do not change Store schema, retained history, or runtime-reset safety as an
  assumed first step.

## Rough scope

Add repeatable timing at larger Store sizes and worktree counts using supported
commands. Attribute the remaining time to SQLite checks, codec validation,
migration, and job recovery. Compare bounded improvements while preserving
failure and crash-recovery behavior.

## Open decisions

- What cold-start latency target is appropriate for the supported workload?
- Can the remaining validation be made incremental or parallel without changing
  an accepted integrity or single-writer decision?
- Should operators receive additional progress detail or guidance when recovery
  legitimately outlasts the default `wait-ready` deadline?

## Roadmap promotion conditions

Promote only after profiling at the larger target scale identifies the dominant
work, the safety boundary and latency target are agreed, and any architectural
change has an accepted ADR.
The adopted plan must include restart, corruption, crash, and large-state
regressions as well as an exact end-to-end benchmark.

## References

- [Daemon status and readiness](../specs/operations/observability.md#daemon-status)
- [Recovery and maintenance](../specs/storage/recovery-retention-and-maintenance.md)
- [Readiness CLI](../specs/interfaces/cli-specification.md#command-groups)
