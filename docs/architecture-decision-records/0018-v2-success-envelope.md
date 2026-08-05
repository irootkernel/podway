# ADR-0018: Version the Procedure v2 Success Envelope

- Status: Accepted
- Date: 2026-08-04

## Context

Procedure v2 success results use closed, version-aware result families. Keeping
those results inside `podway.output/v1` would make an older peer accept an outer
envelope whose command result it cannot safely interpret. Job reconciliation
also preserves complete terminal envelopes, so the compatibility boundary must
remain explicit when a v2 result is nested in a job result.

Errors do not have the same ambiguity: `podway.error/v1` already selects behavior
by its registered error code, and v2 runtime errors can use a separate closed
details discriminator without changing the outer error envelope.

## Decision

Procedure v2 success responses use the additive `podway.output/v2` envelope.
It retains the v1 envelope fields and openness rules, but its command-to-result
mapping accepts only the registered Procedure v2 result families.

`job.lookup`, `job.status`, and `job.wait` use
`podway.job-lookup-result/v2` and `podway.job-result/v2` when observing a v2
job. A terminal job stores and returns the complete original `podway.output/v2`
or `podway.error/v1` envelope, or the existing closed cancellation summary.

Failures retain `podway.error/v1`. Every registered Procedure v2 runtime code
uses the closed `podway.v2-runtime-error-details/v1` family with a `kind` equal
to the outer error code. V1 sessions and released v1 result families remain
unchanged.

## Consequences

- peers can reject an unsupported v2 success envelope before interpreting its
  result;
- job reconciliation preserves the same explicit version boundary as the
  original response;
- the error envelope remains compatible while v2 error details are closed and
  code-bound;
- implementations must validate the complete serialized v2 envelope, including
  nested terminal job responses, against the existing frame limit.
