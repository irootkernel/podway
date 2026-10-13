---
name: create-podway-procedure
description: Author, review, revise and validate custom Podway Procedure v2 source from repository-owned workflow requirements. This skill hands off declarative source without operating sessions or changing workspace runtime state.
---

# Create a Podway Procedure

## Establish ownership and readiness

The requesting repository owns workflow meaning, canonical paths, compatibility,
tests and distribution. Podway owns the Procedure v2 grammar and runtime semantics.
Prefer an existing built-in or repository Procedure when its gates already fit.
Do not add execution hooks, Git mutation, expressions, plugins or parallel cursors.

1. Resolve one Git worktree. Run `podway version --json` for its compact version and
   `podway version --json --identity` for Output v4, version-result v2 and
   contract-manifest v2. Record exact build/source/manifest identity, not semver
   alone. Missing Podway is a prerequisite, not installation authority.
2. Read repository guidance and `.podway/config.yaml`. Resolve the canonical target
   through policy and `procedure_paths`; otherwise use
   `.podway/procedures/<procedure-id>.yaml`.
3. On absent configuration, `WORKSPACE_CONFIG_INVALID` or an excluded target, stop
   before editing and hand off initialization/diagnosis to `$use-podway` under
   separate intent. Authoring does not authorize workspace or session mutation.
4. Establish task purpose, action/decision owners, evidence producers, accepted
   outcomes, correction routes and goal need. Resolve ambiguity that would change
   the graph, trust claim or supported contract before writing it.

Read [authoring](references/authoring.md) and the complete
[capability inventory](references/capabilities.md) before changing source.
For a managed/distributed bundle, also read [distribution](references/distribution.md)
and identify the minimum immutable released manifest supporting every feature
and those current identity families. Validate with that exact release binary.
A local Procedure needs exact validation-binary provenance, not a new release pin.

Delegation requires explicit intent, an exact admitting schema and a reviewed
integration mapping for all identities. It applies only to non-skippable action
placements; omission remains Direct. Read [delegation](references/delegation.md)
before declaring it. Source support does not supply a live executor or permit
invented binding refs or Role digests.

## Author and verify

Inspect exact-binary sources with `podway preset list`, then
`podway preset show analysis-v2` for analysis or `podway preset show sw-dev-v2`
for broad development.
Use only returned presets. Consult `podway help procedure.<operation>` and the
manifest-bound schema; neither a preset nor minimal scaffold defines the grammar.
`podway procedure scaffold --template minimal` writes stdout, not a canonical file.

Write the smallest graph with one cursor/attempt, owned actions and decisions,
bounded evidence, explicit advance/rework routes and terminal outcomes. Keep
narrative guidance in English. Review the complete canonical source diff.

Run repository-required checks plus the following from the owning worktree root:

```bash
podway procedure format <file> --check
podway procedure validate <file>
podway procedure vet <file>
podway procedure lint <file> --warnings-as-errors
podway procedure check <file> --warnings-as-errors
podway --json procedure preview <worktree-relative-file>
```

`podway procedure graph <file> --format mermaid` is a review projection; source and digest
remain authoritative. For local source, these checks/preview suffice unless the
repository requires more. Distributed accepted-path evidence needs separate runtime
authorization under [distribution](references/distribution.md).

Report path, ID/version/digest, selection boundary, exact validation identity,
checks/outcomes, evidence and correction decisions, compatibility and remaining
uncertainty. Copy preview's exact start suggestion, including
`--expect-procedure-digest`, without executing it. Keep structural validation,
runtime qualification, released support and real integration proof distinct.
