# Procedure v2 Authoring

Read this reference when the user asks to create, review, visualize, or use a custom Procedure v2.

## Prefer an existing Procedure

1. Inspect built-ins with `podway preset list`, `podway preset explain <name>`, and `podway preset show <name>`.
2. Reuse a preset when it expresses the required gates. Do not add custom graph complexity or configuration without a task requirement.
3. Keep Procedure files under `.podway/procedures/` so their reviewed source is worktree-local and versionable.

## Author Procedure v2

1. Read the exact grammar for each operation with `podway help procedure.<operation>`.
2. Start a new Procedure v2 document with `podway procedure scaffold --template minimal`. The command writes the document to stdout and never writes a file, so redirect the output to a file under `.podway/procedures/` and review it there.
3. Edit the declarative document without adding command execution, Git mutation, arbitrary expressions, plugins, unbounded collections, parallel cursors, or other behavior outside Podway's product boundary.
4. Run the authoring stages in this order:

   ```bash
   podway procedure format <file> --check
   podway procedure validate <file>
   podway procedure vet <file>
   podway procedure lint <file> --warnings-as-errors
   podway procedure check <file> --warnings-as-errors
   podway procedure preview <worktree-relative-file>
   ```

5. Use `podway procedure graph <file> --format mermaid` or another supported format only as a review projection. Treat the YAML or JSON Procedure and its canonical digest as authoritative.
6. Review purpose, action instructions, required and optional items, decisions, declared routes, manual rework targets, skip policy, resource bounds, and goal settings. Give every decision option `criteria` guidance that states when a decision-maker picks that option; `WEAK_CRITERIA_GUIDANCE` reports only guidance that is absent, one word, or a marked placeholder, never mere vagueness, and these declared option criteria are Procedure structure, not the session goal criteria recorded at runtime. Keep exactly one active cursor and attempt.
7. Review the declared recorded-item references and their read-back selection: which prior placement each consumer reads back, and which item IDs that reference selects. A required evidence source must dominate its consumer and cannot be skippable; branch-specific references are optional.
8. Start the custom Procedure only after explicit user intent, using the exact digest from the reviewed preview or validation output.

## Preserve the enforcement boundary

Podway validates declared structure and blocks progression on unsatisfied formal conditions. It does not execute instructions or determine whether recorded text, confirmations, decisions, assessments, or external test results are semantically true. Design required items so an external actor can perform and substantiate the work; do not imply stronger assurance than that actor provides.
