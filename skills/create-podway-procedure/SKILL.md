---
name: create-podway-procedure
description: Author, review, revise and validate repository-owned Podway Procedure v3 and Execution Group v1 source. Hand off declarative source and exact identities for separately authorized runtime use.
---

# Create a Podway Procedure

## Establish ownership and readiness

The repository owns workflow meaning, canonical source paths, compatibility,
tests and distribution. Podway owns admitted grammar and transition semantics.
Prefer an existing base and group when their gates already fit.

1. Resolve the owning Git worktree. Run `podway version --json` and `podway
   version --json --identity`. Require Output v4, version-result v2 and
   contract-manifest v2, then confirm that the exact binary and manifest admit
   Procedure v3 and Execution Group v1. A missing binary is a prerequisite.
   Match the original writer and its guidance for any older Procedure v2 work.
   Compatibility of a new base and group gives no basis to rewrite an existing
   v2 pin in workspace configuration.
2. Read repository guidance and `.podway/config.yaml`. Find the canonical base
   path through project policy and configured `procedure_paths`. The default is
   `.podway/procedures/<procedure-id>.yaml`. Keep reusable groups in the
   repository-owned source locations and supply their exact documents at
   activation. A group cannot be passed directly to `start`.
3. On absent configuration, `WORKSPACE_CONFIG_INVALID` or an excluded base
   target, hand the runtime prerequisite to `$use-podway` under separate intent.
   Offline reading and validation can proceed on an authorized source file.
   Authoring grants no workspace initialization, repair or session effects.
4. Establish the base's starting work, design anchor, returned outputs, goal
   policy and closeout. For the group, identify owned actions, evidence sources,
   decision predicates, viable corrections and the single success terminal.
   Define any guarded replan and the circumstances that justify it.

Read [authoring](references/authoring.md) and the
[capability inventory](references/capabilities.md) before choosing features.
For managed or shipped source read [distribution](references/distribution.md).
For explicitly delegated base or group actions read
[delegation](references/delegation.md). Delegation needs an admitting schema,
reviewed binding and exact Role identity before it can be used.

## Author and verify

Inspect `podway preset list`, then use `podway preset show sw-dev-v3` for broad
work or `podway preset show small-change-v3` for assertion-only bounded work.
Consult the exact CLI help and manifest-bound schemas. `podway procedure
scaffold --template minimal` writes an editable base to stdout. A scaffold and
any single preset are partial examples of the grammar.

Keep one cursor and attempt. Give every node an accountable external role,
actual inputs, expected records, failure-report obligations and owned next
steps. Author evidence ports, conditional items, guards, and terminal routes
against the complete base and group together. Reserve replan for a declared
Decision with current failure evidence and a justified structural change. Do
not add execution hooks, Git effects, expressions, plugins or nested groups.

From the owning worktree, validate both source documents and review the graph:

```bash
podway procedure check <base-file> --warnings-as-errors
podway procedure check <group-file> --warnings-as-errors
podway procedure graph <base-file> --format mermaid
podway procedure graph <group-file> --format mermaid
podway --json procedure preview <worktree-relative-base-file>
podway --json procedure preview <worktree-relative-group-file>
```

`check` combines formatting, validation, graph vetting and lint. Preview
shows the admitted digest, graph and separate diagnostics. Only a ready base
preview supplies `result.start_argv`. A group preview supplies its definition
identity for activation. Compare the result and plan item declarations across
the pair. Individual source checks cannot establish pair admission or runtime
execution. Use the [distribution checks](references/distribution.md) for any
separately authorized accepted-path execution.

Report both paths, IDs, versions and canonical digests, pair-interface review,
validation-binary identity, checks and remaining limits. Hand off the base's
exact preview start suggestion with `--expect-procedure-digest`, plus the group
source and digest. The runtime caller adds the current start fences, task and
caller-owned key, then uses the current group activation envelope at the anchor.
Keep preview, runtime qualification, release status and installed adoption distinct.
