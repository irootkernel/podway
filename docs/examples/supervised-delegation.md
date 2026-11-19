# Supervised Review Walkthrough

The [supervised-review-v3 Procedure](procedures/supervised-review-v3.yaml)
records a supervised result on a base Action before its design anchor.
The native qualifier uses the exact Procedure source and a deterministic
fixture executor. Supported deployed integrations need their own mapping
and qualification. See the [delegation contract](../specs/interfaces/supervised-delegation.md)
and [adapter mapping requirements](../implementation-tips/delegation-adapter-contract-review.md#integration-handoff).

The [supervised-review-v2 Procedure](procedures/supervised-review-v2.yaml)
is the retained example for its original writer.

## Review and acceptance

| Placement | Caller responsibility |
| --- | --- |
| `source` | Record the review input and complete the preparation Action. |
| `work` | Read the exact package and evidence. Settle the external assignment, record the result, and accept it against the current attempt. |
| `plan` | Decide on the bounded follow-up and activate the selected group explicitly. |
| `finish` | Close the base after the selected group returns. |

The example's `fixture-specialist` binding, `reviewer` role and digest belong to
the test executor. The digest identifies the UTF-8 JSON string `"reviewer-v1"`.
For a real assignment, use the admitted role-content and agent-configuration
digests and the actual acknowledgement and settlement evidence.

Read `delegation package --json` at the delegated Action. The package pins
the workspace, session, base digest, active scope, node, attempt, and selected
evidence. A group package also pins both the execution and the group definition.
Use `evidence read --source source --item input` for the exact input value.

Record the final Action items before submitting
`podway.delegation-accept-input/v2` through `delegation accept --stdin`.
Keep the package digest, current scoped cursor and caller-owned key.
`after_accept: retain` requires an idle worker in this example.
A `release` declaration requires a released worker.

The current receipt admits completion only while its accepted item set stays
applicable. Editing an accepted value makes the receipt stale. Recover a lost
response with `outcome lookup` and the original key. Exact replay returns the
original success. Once the Action advances, its immutable receipt stays
available with historical applicability.

## Run the isolated scenario

```bash
make test-e2e-exact E2E_TEST=podway-cli::e2e_suite::e2e_v2sdg008_qualification::supervised_delegation_preserves_documented_example_hosts_and_recovery
```

The qualifier executes the copied native CLI in owned worktrees, validates
the exact basis and receipt, exercises both labeled fixture hosts, and checks
response recovery and late-result refusal. It records the source and binary
hashes, Procedure digest and direct contract identity. The fixture host labels
represent deterministic local execution.

For a built CLI, the example-only diagnostic is available through
`tools/python tools/qualify_supervised_delegation.py --podway target/debug/podway --scenario example`.
