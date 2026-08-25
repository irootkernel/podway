---
name: create-podway-procedure
description: Create, revise, review, or harden a custom Podway Procedure v2 from repository-owned workflow requirements. Use for Procedure source and its authoring evidence, not for starting, advancing, recovering, or deleting Podway sessions.
---

# Create a Podway Procedure

## Preserve ownership and runtime boundaries

- Treat the requesting repository as the owner of workflow meaning, canonical Procedure paths, supported Podway versions, tests, and distribution policy. Podway owns the Procedure v2 grammar and runtime semantics.
- Produce declarative Procedure v2 source only. Do not add command execution, Git mutation, arbitrary expressions, plugins, parallel cursors, or another workflow engine.
- Do not initialize a workspace, inspect or mutate a session, control a daemon, install Podway, or alter runtime state. Use `$use-podway` only for a separately requested runtime operation.
- Prefer an existing built-in or repository-owned Procedure when it already expresses the required gates. Do not create a custom graph merely to rename phases.

## Establish authoring readiness

1. Resolve one Git worktree and run `podway version --json` plus `podway version --json --identity` when supported. Record the exact manifest and source provenance, not only semver. If Podway is unavailable, stop and report the missing prerequisite; do not install it.
2. Read the repository authorities and `.podway/config.yaml`. Resolve the canonical target from repository policy and configured `procedure_paths`; when neither chooses a different path, use `.podway/procedures/<procedure-id>.yaml`.
3. If the workspace configuration is absent, invalid, or excludes the intended target, stop before editing and request a separate `$use-podway` initialization or diagnosis. Do not treat `WORKSPACE_CONFIG_INVALID` as permission to repair the workspace.
4. For a worktree-local Procedure, record the exact Podway binary identity used for validation. Require a minimum immutable stable release only when the Procedure will be distributed or maintained in other worktrees; never promote an unreleased source build to released-support evidence.

## Establish the authoring contract

Before editing, read the repository authorities and identify:

- the workflow purpose and the cases that should select this Procedure;
- the owner of every action, decision, external result, and terminal claim;
- required inputs, accepted outcomes, rework owners, and goal-tracking need;
- the canonical target path and whether the Procedure is local or distributed;
- the installed validation version and, only for a distributed Procedure, its minimum supported stable release.

Stop when unresolved product intent would change the graph, trust boundary, compatibility, or completion claim. Do not infer those decisions from a preset example.

Read [references/authoring.md](references/authoring.md) and the complete [capability inventory](references/capabilities.md) before creating or changing Procedure source. For a Procedure shipped to other repositories or installed as a managed bundle, also read [references/distribution.md](references/distribution.md).

## Author the smallest complete graph

1. Inspect built-ins with `podway preset list` and use only identifiers returned by that exact binary. When present, use `podway preset show analysis-v2` for source-backed research or technical analysis and `podway preset show sw-dev-v2` for the broadest software-development example. If a referenced preset is absent, do not infer it from the skill or another release; inspect an available preset or author the required custom Procedure. No single preset is the complete grammar; inspect a smaller preset when its boundary better matches the requested workflow.
2. Read command behavior with `podway help procedure.<operation>` and scaffold with `podway procedure scaffold --template minimal` when starting from nothing. Neither command help nor the minimal scaffold is a complete YAML grammar; reconcile the capability inventory with the exact manifest-bound schema. The scaffold writes to stdout; place the reviewed result only at the repository-owned canonical target.
3. Model one cursor and one active attempt. Use explicit action evidence, decisions, advance routes, owned rework routes, and terminal outcomes; omit phases and features that do not enforce a verified requirement.
4. Record narrative guidance in English while preserving exact commands, paths, hashes, identifiers, enumerated values, and product names verbatim.
5. Review the complete source diff before treating the Procedure as a candidate.

## Verify the candidate

Run the repository's stricter checks plus the complete Podway authoring sequence:

```bash
podway procedure format <file> --check
podway procedure validate <file>
podway procedure vet <file>
podway procedure lint <file> --warnings-as-errors
podway procedure check <file> --warnings-as-errors
podway --json procedure preview <worktree-relative-file>
```

Use `podway procedure graph <file> --format mermaid` only as a review projection. The canonical source and digest are authoritative.

For a local Procedure, the authoring checks and preview are the required qualification unless repository authority requires more. For a distributed Procedure, follow [references/distribution.md](references/distribution.md) and keep runtime path qualification under separate runtime authorization.

Report the selection boundary, canonical path, Procedure ID/version/digest, exact validation binary identity and manifest provenance, graph and evidence decisions, exact checks and outcomes, compatibility impact, released-support evidence, and any unverified assumption. Copy the exact start suggestion returned by preview, including `--expect-procedure-digest`, into the handoff without executing it. Report distributed runtime proof separately when it exists. Do not claim that Podway executed external work or judged recorded evidence true.
