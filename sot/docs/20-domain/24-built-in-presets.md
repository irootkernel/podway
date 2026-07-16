# Built-in Presets

## Principles

Built-in presets are versioned procedure data. They do not receive privileged transition logic. Every preset must validate through the same parser, schema, canonicalizer, and domain engine as a custom worktree procedure.

The normative YAML files are in [`../../presets/`](../../presets/).

Each preset is designed for one current task and favors short explicit items over historical reporting.

## `sw-dev`

Purpose: general software change.

```text
understand
  -> inspect
  -> plan
  -> implement
  -> verify
  -> review
  -> finish
```

Key safeguards:

- goal and acceptance criteria before implementation;
- current code and behavior inspection;
- explicit plan and scope;
- implementation summary;
- verification confirmation and note;
- review and finding disposition;
- final result readiness.

Typical return: `review` to `implement`, making `verify` and `review` redo.

## `bug-fix`

Purpose: defect correction with observed baseline and regression coverage.

```text
reproduce
  -> diagnose
  -> regression
  -> fix
  -> verify
  -> review
  -> finish
```

Key safeguards:

- reproduce or precisely explain inability to reproduce;
- record expected and actual behavior;
- diagnose cause before fix;
- reference a regression test or validation case;
- verify the corrected behavior;
- review scope and regressions.

Typical retry: repeat `verify` after an invalid environment. Typical return: `review` to `fix`.

## `docs-only`

Purpose: documentation work where implementation changes are out of scope.

```text
ground-sources
  -> define-audience
  -> outline
  -> draft
  -> validate
  -> review
  -> finish
```

Key safeguards:

- identify authoritative sources;
- define audience and intended outcome;
- establish structure before drafting;
- validate correctness, links, examples, and terminology;
- review clarity and scope;
- finish with report-ready output.

Typical return: `validate` or `review` to `draft`.

## `analysis`

Purpose: bounded technical research or analysis.

```text
define-question
  -> collect-sources
  -> analyze
  -> challenge
  -> synthesize
  -> finish
```

Key safeguards:

- explicit question and decision context;
- source list and limitations;
- analysis separated from source collection;
- challenge assumptions and counterarguments;
- synthesize conclusion, uncertainty, and recommended action.

Typical retry: repeat `challenge`. Typical return: `challenge` to `collect-sources`.

## Versioning

Preset `id` and `version` are stored in the procedure snapshot. Updating a built-in preset affects only new sessions. Existing sessions continue using their embedded snapshot.

A behavior-changing preset update increments its procedure version. Product releases may ship more than one preset version only when compatibility or migration needs justify it; v1 normally exposes the latest built-in version.

## Adding a preset

A new built-in preset requires:

1. repeated real-world use not covered well by the existing four;
2. a complete YAML procedure;
3. help text and rework examples;
4. schema validation and canonical digest test;
5. an end-to-end complete scenario;
6. retry and return coverage;
7. product review confirming it does not add domain-specific core logic.

Candidate future presets such as release, incident response, or security review are not part of the first release.
