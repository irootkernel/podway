# Built-in Presets

## Principles

Built-in presets are versioned procedure data. They do not receive privileged transition logic. Every preset must validate through the same parser, schema, canonicalizer, and domain engine as a custom worktree procedure.

The normative YAML files are in [`../../../assets/presets/`](../../../assets/presets/).

Each preset is designed for one current task and favors short explicit items over historical reporting.

The catalog retains four Procedure v1 presets and ships two implemented Procedure v2
presets in v0.2.0, which admits v2 sessions through the normal runtime and excludes
the development-only admission unlock.

## Procedure v1 presets

### `sw-dev`

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

### `bug-fix`

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

### `docs-only`

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

### `analysis`

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

## Procedure v2 presets

### `sw-dev-v2`

Purpose: software delivery with fresh verification, review, rework, and
goal-directed closeout.

```text
implement
  -> capture-baseline (skippable with a reason)
  -> test-after-impl
  -> decide-after-impl-test
       passed -> review-change -> test-after-review
       failed -> implement (rework)
  -> decide-after-review-test
       passed -> assess-session-goal
       failed -> implement (rework)
  -> outcome finalization -> closeout confirmation -> record-closeout
```

Key safeguards:

- test decisions read back the exact recorded test evidence;
- a post-review test reruns before goal assessment;
- failed verification and incomplete closeout route to declared rework targets;
- goal assessment maps every outcome to a declared path;
- manual rework is limited to `implement`, `test-after-impl`, and
  `review-change`.

### `bug-fix-v2`

Purpose: defect correction from reproduction through regression, verification,
review, rework, and goal-directed closeout.

```text
reproduce -> diagnose -> establish-regression -> implement -> verify
  -> decide-verification
       passed -> review -> decide-review
       failed -> implement (rework)
  -> assess-session-goal
  -> outcome finalization -> closeout confirmation -> record-closeout
```

Key safeguards:

- reproduction, diagnosis, and regression evidence precede implementation;
- verification and review decisions read back the recorded source actions;
- failed verification and requested changes return to implementation;
- goal assessment and closeout confirmation cover achieved, not-achieved, and
  superseded outcomes;
- manual rework is limited to declared defect-workflow targets.

## Versioning

Preset `id` and `version` are stored in the procedure snapshot. Updating a built-in preset affects only new sessions. Existing sessions continue using their embedded snapshot.

A behavior-changing preset update increments its Procedure version. The retained
v1 presets use version `"1"`; `sw-dev-v2` and `bug-fix-v2` use version `"2"` and
the `podway.procedure/v2` schema. Product releases may ship more than one preset
version only when compatibility or migration needs justify it.

## Adding a preset

The retained v1 catalog is fixed at exactly `sw-dev`, `bug-fix`, `docs-only`, and
`analysis`. The v0.2.0 v2 catalog is fixed at exactly `sw-dev-v2` and `bug-fix-v2`.
The contributor commands below only prepare canonical source candidates; they do not
modify either embedded catalog or make another preset shippable by themselves.

Create a validated scaffold directly in `assets/presets/`:

```sh
make preset-create \
  PRESET_ID=release-check \
  PRESET_NAME='Release Check' \
  PRESET_DESCRIPTION='Prepare and verify a release candidate.'
```

Import an existing procedure while preserving its exact bytes:

```sh
make preset-import PRESET_FILE=/path/to/release-check.yaml
```

Both targets use the repository's pinned Rust toolchain to build the real `podway`
validator, reject invalid or oversized input, and never overwrite an existing preset.
`PRESET_DIR` may point to a temporary directory for evaluation; its default is the
canonical `assets/presets/` directory.

A new built-in preset requires:

1. repeated real-world use not covered well by the existing four;
2. a complete YAML procedure;
3. help text and rework examples;
4. schema validation and canonical digest test;
5. an end-to-end complete scenario;
6. applicable retry, return, decision, or rework coverage;
7. product review confirming it does not add domain-specific core logic.

Candidate future presets such as release, incident response, or security review
are not part of the v0.2 catalog.
