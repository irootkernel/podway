# Author a Delegated Action

Read for explicitly requested Specialist execution after resolving source ownership,
exact validating binary and local/distributed support.

The Primary owns decisions and semantic acceptance. The integration supplies
execution identities and settled workers; Podway does not discover providers,
execute tasks or establish receipt truth. Put `delegation` on an action placement
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

## Example and handoff

Use the [review YAML](https://github.com/irootkernel/podway/blob/v0.2.15/docs/examples/procedures/supervised-review-v2.yaml)
and [walkthrough](https://github.com/irootkernel/podway/blob/v0.2.15/docs/examples/supervised-delegation.md)
after `v0.2.15` is published; before publication, use both from the exact reviewed
candidate commit with its matching CLI. When validating another release, select both from
its exact tag; keep example bytes and instructions at one pinned revision.
This is custom source, not a preset. Its binding and
synthetic Role digest apply only to the isolated fixture. Real use needs the
reviewed binding/Role mapping, chosen rounds/disposition and full authoring checks.

Hand off preview's exact digest/start suggestion without executing it. Grammar
validation, isolated runtime paths, released support and a live adapter are separate
claims; source tests do not qualify real execution.
