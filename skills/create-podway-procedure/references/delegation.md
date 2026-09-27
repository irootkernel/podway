# Author a Delegated Action

Read this reference only when the requested workflow needs supervised Specialist
execution. First establish the exact validation binary, local/distributed
support requirements, and repository-owned target through `SKILL.md`.

## Choose responsibilities

Keep the Primary responsible for decisions and semantic acceptance. Delegate an
action only when the workflow requires Specialist execution and a qualified
integration can produce the required identities and worker settlement. Podway
does not discover providers, execute tasks, or verify the truth of receipts.

Put `delegation` on the action's placement under `graph.nodes`, alongside `use`
and `next` or `terminal`. Do not put it in `node_definitions`, attach it to a
decision, or combine it with `skip.allowed: true`. Omission means Direct; there
is no separate execution-mode flag or fallback to Direct after failure.

## Resolve all five declaration fields

| Field | Authoring decision |
| --- | --- |
| `binding_ref` | Logical integration binding selected outside Podway; 1–64 ASCII characters matching `[a-z][a-z0-9]*(?:-[a-z0-9]+)*`. It is not an executable or provider configuration. |
| `role_ref` | Exact executor-owned Role reference, at most 256 UTF-8 bytes with no control characters. |
| `role_snapshot_digest` | Immutable complete Role-content identity from the reviewed integration mapping, using lowercase `sha256:` plus 64 hex digits. Never hash a Role name as a substitute. |
| `max_rounds` | Explicit integer 1–32, chosen from the workflow's feedback budget. Podway checks the reported count, not each external assignment. |
| `after_accept` | `retain` requires a verified idle worker; `release` requires completed release. Choose based on intended worker reuse and cleanup ownership. |

All five fields are required when delegation is present. Use the exact
manifest-bound schema for the complete closed grammar. Neither the binding nor
the digest belongs to Podway configuration discovery. If real mapping facts are
missing, report the prerequisite instead of inventing operational values.

## Design evidence and correction

- Select only the required upstream evidence with `evidence_from`; make required
  sources dominate the delegated placement. Full selected evidence is read
  separately from bounded previews before constructing the external task.
- Define bounded items for the final result and any useful optional notes.
  Instruct the Primary to finish those values before submitting acceptance.
  Output recording preserves the input package digest, but later item edits
  invalidate an accepted receipt.
- Provide an intentional rework target when correction is part of the workflow.
  Retry or rework creates a fresh attempt; neither operation launches an
  executor. The integration owns result revalidation, new assignments, and
  recovery of uncertain external outcomes.
- Acceptance and action completion are separate. Do not imply that a worker's
  delivery or the Primary's receipt submission advances the graph.

## Start from the reviewed example

Use the [supervised review YAML](https://github.com/irootkernel/podway/blob/main/docs/examples/procedures/supervised-review-v2.yaml)
and its [walkthrough](https://github.com/irootkernel/podway/blob/main/docs/examples/supervised-delegation.md)
from the same pinned revision. The example is a custom Procedure, not a built-in
preset. It shows Direct preparation, a delegated review with selected evidence,
and Direct closeout, plus a declared rework target.

Its `fixture-specialist` binding and synthetic Role digest are valid only in the
documented isolated test. For real use, obtain a reviewed integration mapping
and replace the binding and Role identities together. Select the round bound
and disposition deliberately, update the Procedure identity/version according
to repository policy, and rerun the complete authoring sequence from `SKILL.md`.

Hand off the exact preview digest and start suggestion without executing it.
Authoring checks prove grammar and structural admissibility; isolated fixture
tests prove local runtime behavior; a real adapter needs separate qualification.
A successful source test does not establish stable-release support. Keep
distributed support claims tied to immutable published-release evidence.
