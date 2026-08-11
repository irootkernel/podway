# Authoring a Custom Procedure

A procedure is a YAML checklist that Podway can enforce: ordered stages, typed items that record the evidence for each stage, and an explicit rework policy. This tutorial converts a team checklist into a worktree-local procedure. The finished result is the shipped example [`.podway/procedures/custom-bug-fix.yaml`](.podway/procedures/custom-bug-fix.yaml).

## 1. Start from the checklist

The team's service bug-fix checklist:

1. Reproduce in a controlled environment and write the failure down.
2. State the diagnosed cause.
3. Implement the fix.
4. Verify, and file the verification report.
5. Declare the result ready.

## 2. Shape the stages

Each numbered step becomes a stage with a stable `id` and a `title`. Order is meaning: a session walks the list top to bottom with one active stage at a time.

```yaml
schema: podway.procedure/v1
id: service-bug-fix
version: "1"
name: Service Bug Fix
stages:
  - id: reproduce
    title: Reproduce in a controlled environment
  - id: diagnose
    title: Diagnose
  - id: implement
    title: Implement
  - id: verify
    title: Verify
  - id: finish
    title: Finish
rework:
  allow_return_to: any_previous
```

This already validates, but it gates nothing: a stage with no required items completes without recording anything.

## 3. Type the evidence

For every stage, ask: what must exist before this stage may be left? Encode each answer as an item. Six item types are available ([schema](../../assets/schemas/procedure-v1.schema.json), [specification](../specs/domain/procedure-and-item-specification.md)):

| Type | Records | Useful bounds |
|---|---|---|
| `confirm` | an explicit yes | — |
| `text` | prose | `min_length`, `max_length`, `multiline` |
| `list` | multiple entries | `min_items`, `max_items`, `unique` |
| `choice` | one of the declared options | `choices` |
| `integer` | a number | `minimum`, `maximum` |
| `artifact` | a file or reference with digest and size | `allowed_media_types` |

For example, the `verify` stage requires an explicit confirmation and a report reference:

```yaml
  - id: verify
    title: Verify
    items:
      - id: verification-passed
        type: confirm
        prompt: Relevant verification passed.
        required: true
      - id: verification-report
        type: artifact
        prompt: Reference the verification report.
        required: true
```

Prefer few, sharp items over many vague ones: every `required: true` item is a hard gate on `podway complete`.

## 4. Decide the rework policy

`rework.allow_return_to` declares where `podway return` may go: `any_previous`, or an explicit list when only some backward transitions are legitimate.

```yaml
rework:
  allow_return_to:
    - reproduce
    - diagnose
    - implement
    - verify
```

## 5. Validate and start

Store the file under `.podway/procedures/` — that directory is tracked by Git; only `.podway/runtime/` is ignored. Then:

```bash
podway procedure validate .podway/procedures/service-bug-fix.yaml
podway procedure show .podway/procedures/service-bug-fix.yaml
podway start --procedure .podway/procedures/service-bug-fix.yaml \
  --task "fix duplicate login session creation"
```

The daemon validates the file and stores an immutable canonical snapshot at start; later edits to the file apply only to future sessions.

The v1 presets under [`assets/presets/`](../../assets/presets/) are examples of
this schema, and the [integration session](integration-session.md) walks through
a second custom v1 procedure built the same way. For Procedure v2, use the
implemented scaffold, format, check, preview, graph, and conversion commands in
the [CLI specification](../specs/interfaces/cli-specification.md#procedure-commands),
then follow the [Procedure v2 workflow](v2-workflow.md) in a managed disposable
runtime.
