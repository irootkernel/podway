# Author a Delegated Action

Read for explicitly requested Specialist execution after resolving source ownership,
exact validating binary and local/distributed support.

The Primary owns decisions and semantic acceptance. The integration supplies
execution identities and settled workers; Podway does not discover providers,
execute tasks or establish receipt truth. Put `delegation` on an action placement in a v3 base or v1 group
under `graph.nodes`, beside `use` and `next`/`terminal`, not in `node_definitions`.
Decisions and skip-enabled actions cannot delegate. Omission remains Direct;
there is no execution-mode switch or fallback after failure.

## Required fields

| Field | Source and bound |
| --- | --- |
| `binding_ref` | Integration-selected logical binding; 1..64 ASCII characters matching `[a-z][a-z0-9]*(?:-[a-z0-9]+)*`; no command/provider configuration |
| `role_ref` | Exact executor-owned Role reference, at most 256 UTF-8 bytes, no control characters |
| `role_snapshot_digest` | Reviewed immutable complete Role-content identity: lowercase `sha256:` plus 64 hex digits; a Role-name hash is insufficient |
| `max_rounds` | Explicit integer 1..32 from the workflow's feedback budget |
| `after_accept` | `retain` requires verified `idle`; `release` requires completed release and `released` |

All five fields are required. Follow the complete closed manifest-bound grammar.
Missing real mapping facts are a prerequisite, not permission to invent values.
Podway checks reported rounds; the integration accounts for assignments.

## Evidence and correction

Select needed upstream evidence through `evidence_from`; required sources dominate
the delegated placement. Complete selected values are read separately from previews
before constructing the external task.

Declare bounded final-result items and optional notes. Finish these values before
acceptance. Recording preserves the input package digest. Any committed item-state
change after acceptance, including cosmetic, optional-item or reverting edits,
makes the receipt stale; true no-ops and rolled-back mutations preserve it.

Provide owned rework routes when correction is required. Retry/rework creates a
fresh attempt without dispatching an executor. Integration owns reassignment,
result revalidation and recovery of uncertain external effects. Acceptance and
completion are distinct; delivery/submission never advances the cursor alone.

## Scoped handoff

An admitted declaration is instantiated per attempt. Package and receipt use
successor scope and exact execution identity, including activation and group
digest for group attempts. `delegation accept --stdin` uses the current closed
`podway.delegation-accept-input/v2` context. A package or receipt from a previous
execution never qualifies a new, same-definition activation. Settle affected
workers and reconcile uncertain integration effects before a boundary change.

Hand off the validated source pair, canonical digests, exact base start suggestion,
and current binding and Role ownership. Qualify representative delegated paths
only with separate runtime authority and exact source/CLI pairing. Follow the
[supervised review walkthrough](../../../docs/examples/supervised-delegation.md)
from the exact compatible source commit or release tag. With an installed
skill, open `docs/examples/supervised-delegation.md` in that source revision.
The walkthrough uses the v3 example with a synthetic executor. The retained
`supervised-review-v2.yaml` requires its original writer. Live adapters need
their own qualification.
