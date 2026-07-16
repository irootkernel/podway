# ADR-0001: Focus on the Current Task Session

- Status: Accepted
- Date: 2026-07-13

## Context

Earlier designs emphasized durable evidence, revocation, audit history, export, and post-mortem analysis. The intended product need is narrower: keep one currently running task on procedure and prevent omitted steps.

Long-term history features substantially increase data modeling, security, storage, migration, and user-interface cost while providing limited value during the active task.

## Decision

Podway manages one current task session per Git worktree. Historical attempts are retained only while that session exists because retry and return need them for correct current-state operation. `podway reset` is the deliberate history boundary.

Podway will not ship long-term evidence lifecycle, audit export, post-mortem analytics, or global task history in the first product.

## Consequences

Positive:

- smaller and clearer product model;
- simpler relational storage and deletion;
- lower sensitivity and retention burden;
- status and next can focus on actionable current work.

Negative:

- reset removes task history permanently;
- users needing durable records must use project systems such as issues, commits, or documents;
- future archive functionality would require a new product decision and migration.
