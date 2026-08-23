# Procedure v2 Authoring

## Resolve the source of truth

- Work in one Git worktree and follow its authority for workflow meaning, naming, location, compatibility, and verification. If it has no Procedure location policy, use `.podway/procedures/<procedure-id>.yaml` under the configured default procedure path.
- Treat the installed CLI as the grammar authority for that validation run. Use `podway help procedure.<operation>`, the minimal scaffold, and `podway preset show sw-dev-v2`; do not copy syntax from an unrelated checkout, unreleased branch, or differently versioned example.
- Preserve an existing repository-owned Procedure unless the request authorizes its revision. Creating a Procedure does not authorize replacing another canonical file.
- When Podway or valid workspace configuration is missing, report the exact prerequisite and stop. Initialization, repair, installation, and daemon control belong to a separate runtime workflow.

## Select features from requirements

- Use an action item only for evidence that an external actor can actually produce and substantiate.
- Use `check_result` when an external operation needs a stable structural identity and a closed `pass`, `fail`, or `inconclusive` result. Its operation digest versions the exact abstract operation definition; it never proves that the operation ran.
- Use `required_when` only for same-action typed conditions. Keep the dependent item optional in its base declaration and make the condition exhaustive over values that can reach completion.
- Use decision guards only for mechanically decidable predicates over required, fresh, explicitly selected evidence. Leave semantic judgments and explicit user choices unguarded.
- Give every decision option criteria that state when to select it and distinguish options that route to the same node. These Procedure criteria guide a runtime choice; they are not session goal criteria or claims that Podway judged the choice true.
- Use lists for repeated values that need count, uniqueness, or per-entry bounds. Enable a multi-page maximum only when the workflow genuinely needs it; Podway is not an evidence archive.
- Use artifacts only for stable references to repository-local metadata. Podway does not store or attest artifact bytes.
- Prefer an explicit choice such as `not-applicable` over skipping a phase that owns a decision. Do not make a required evidence source skippable.

## Build evidence flow deliberately

- Give every consumer only the source item IDs it needs. A reference to a decision record has no item selector.
- Required sources must dominate their consumers. Branch-specific sources are optional and must not be treated as missing work when their branch did not execute.
- Every guard source must be required, fresh, selected by the guarded decision placement, and typed for the predicate operator.
- Make guarded option sets exhaustive for every reachable source value. Split options that share a route when the predicate language cannot express a safe disjunction.
- Route each correction to the phase that owns it. `manual_rework.allowed_targets` is a separate operator allowlist and does not replace option-specific `effect: rework` routes.
- Keep goal assessment near terminal closeout and cite the smallest fresh evidence set that can substantiate every criterion.

## Bound the complete document

- Set meaningful text, list, and artifact bounds from the workflow's actual evidence shape.
- Give counters a non-negative minimum and positive ordinals a minimum of one.
- Keep identifiers stable, concise, and unique. Change the Procedure version whenever canonical semantics change; the canonical digest binds the exact admitted bytes.
- Keep recorded summaries bounded and store logs, transcripts, source payloads, and generated documents outside Podway. Record only stable references, digests, and adjudicated conclusions.

## Review the trust claim

For every item and option, ask what Podway can enforce mechanically and what remains a caller assertion. Rewrite prompts or criteria that imply Podway executed a command, verified Git state, approved a design, reviewed a change, or established semantic truth.

Review the graph projection for unreachable routes, false phase ownership, missing failure paths, accidental loops, and a terminal path that bypasses required evidence. Validation success proves structural admissibility, not workflow correctness.

## Resolve authoring failures

- On formatting drift, inspect the canonical rendering before using `podway procedure format <file> --write`; then review the resulting source diff and rerun the complete authoring sequence.
- On validation, vet, lint, or check findings, correct the owning declaration or graph. Do not weaken warnings, bounds, evidence requirements, guards, or rework ownership merely to pass the tool.
- On `WORKSPACE_CONFIG_INVALID` or a target outside configured `procedure_paths`, leave the Procedure candidate undistributed and hand off the exact readiness problem to `$use-podway` under separate user intent.
- Treat preview's canonical digest and exact start suggestion as the consumption handoff. Never reconstruct the digest or start command manually, and never execute the suggestion as part of authoring.
