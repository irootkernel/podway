# Procedure and Item Specification

## File format

Procedure definitions are YAML or JSON documents conforming to `podway.procedure/v1`. The normative structure is [`../../schemas/procedure-v1.schema.json`](../../schemas/procedure-v1.schema.json).

Example:

```yaml
schema: podway.procedure/v1
id: bug-fix-custom
version: "1"
name: Custom bug fix

stages:
  - id: reproduce
    title: Reproduce the problem
    instructions:
      - Observe the failure before changing implementation files.
      - Record expected and actual behavior.
    items:
      - id: reproduced
        type: confirm
        prompt: The problem was reproduced.
        required: true
      - id: expected-behavior
        type: text
        prompt: Describe expected behavior.
        required: true
        min_length: 1
        max_length: 8000

  - id: fix
    title: Implement the fix
    items:
      - id: implementation-complete
        type: confirm
        prompt: The intended implementation is complete.
        required: true

rework:
  allow_return_to: any_previous
```

## Common fields

### Procedure

| Field | Required | Rules |
|---|---:|---|
| `schema` | yes | Exact value `podway.procedure/v1` |
| `id` | yes | Kebab-case, 1 to 64 characters |
| `version` | yes | Non-empty string, maximum 64 characters |
| `name` | yes | 1 to 120 Unicode scalar values |
| `description` | no | Maximum 4000 characters |
| `stages` | yes | 1 to 64 stages |
| `rework` | yes | Return policy object |

Unknown fields are rejected.

### Stage

| Field | Required | Rules |
|---|---:|---|
| `id` | yes | Kebab-case, unique within procedure |
| `title` | yes | 1 to 120 characters |
| `instructions` | no | 0 to 32 strings, each at most 2000 characters |
| `items` | no | 0 to 128 item definitions |
| `skip` | no | Explicit skip policy |

A stage with no required items is valid but produces a validation warning. It still requires an explicit `complete` command.

### Item common fields

| Field | Required | Rules |
|---|---:|---|
| `id` | yes | Kebab-case, unique within stage |
| `type` | yes | One supported type |
| `prompt` | yes | 1 to 500 characters |
| `help` | no | Maximum 4000 characters |
| `required` | yes | Boolean |

Item IDs only need to be unique within a stage. CLI item commands always target the active stage, so ambiguity across stages is harmless.

## Item types

### `confirm`

```yaml
- id: baseline-inspected
  type: confirm
  prompt: Existing behavior and relevant code were inspected.
  required: true
```

Allowed stored value: `true`. `uncheck` clears the value.

### `text`

```yaml
- id: acceptance-criteria
  type: text
  prompt: State the acceptance criteria.
  required: true
  min_length: 1
  max_length: 8000
  multiline: true
```

Rules:

- default `min_length`: 0;
- default `max_length`: 8000;
- hard maximum `max_length`: 65536;
- length is counted after trimming leading and trailing Unicode whitespace for satisfaction;
- the original string is stored, subject to valid UTF-8;
- `multiline` defaults to true and controls CLI help, not storage.

### `choice`

```yaml
- id: risk-level
  type: choice
  prompt: Select the change risk.
  required: true
  choices:
    - low
    - medium
    - high
```

Rules:

- 1 to 64 choices;
- each choice is a unique non-empty string of at most 120 characters;
- matching is exact and case-sensitive;
- choices preserve author order for display.

### `integer`

```yaml
- id: affected-tests
  type: integer
  prompt: Number of affected tests inspected.
  required: false
  minimum: 0
  maximum: 1000000
```

Rules:

- signed 64-bit integer storage;
- `minimum` and `maximum` are optional;
- minimum cannot exceed maximum.

### `list`

```yaml
- id: touched-components
  type: list
  prompt: List the affected components.
  required: true
  min_items: 1
  max_items: 100
  max_item_length: 500
  unique: true
```

Rules:

- default `min_items`: 0;
- default `max_items`: 100;
- explicit `max_items` range: 1 through 1000;
- default `max_item_length`: 500;
- blank entries are rejected;
- `unique` defaults to true;
- when unique, exact string equality determines duplication;
- insertion order is preserved.

### `artifact`

```yaml
- id: regression-case
  type: artifact
  prompt: Reference the regression case.
  required: true
  allowed_media_types:
    - text/plain
    - application/json
```

Rules:

- one artifact value per item;
- `allowed_media_types` is optional, with at most 64 exact media types;
- local paths must remain inside the worktree;
- local paths are hashed by the daemon using streaming SHA-256;
- when `--media-type` is absent, the daemon uses a versioned embedded file-extension mapping and falls back to `application/octet-stream`; it does not call an OS metadata service or inspect remote data;
- media types are lowercase ASCII `type/subtype` values without parameters in v1;
- local required artifacts are revalidated for path, size, and digest at stage completion; the stored media type is not redetected;
- an external reference requires caller-supplied digest, size, and media type;
- Podway never reads an external reference or guarantees its later availability.

## Skip policy

A stage is non-skippable by default.

```yaml
skip:
  allowed: true
  reason_required: true
```

`reason_required` defaults to true whenever skipping is allowed. Setting `allowed: false` with other skip fields is invalid.

Skipping does not require required items to be satisfied. It records a reason when required and advances one stage.

## Return policy

Allow any earlier stage:

```yaml
rework:
  allow_return_to: any_previous
```

Allow only selected stages:

```yaml
rework:
  allow_return_to:
    - implement
    - verify
```

Validation rules:

- every listed stage ID must exist;
- duplicate destinations are rejected;
- the first stage may be listed but can only be a return destination from a later stage;
- return is always backward; the policy does not create graph edges or forward skips.

## Procedure resolution

`podway start` accepts exactly one source:

```bash
podway start --preset sw-dev --task "..."
podway start --procedure .podway/procedures/custom.yaml --task "..."
```

Resolution rules:

- built-in preset names are exact;
- procedure paths are resolved within configured `procedure_paths` or by explicit worktree-relative path;
- remote URLs are rejected;
- includes and inheritance are not supported in v1;
- duplicate preset and custom IDs do not conflict because the source is explicit.

## Semantic validation

Validation rejects:

- empty stages;
- duplicate stage IDs;
- duplicate item IDs in a stage;
- unknown item types or fields;
- invalid constraints;
- unsatisfiable required items;
- invalid return destinations;
- unsupported schema version;
- path escapes, remote includes, executable constructs, YAML aliases that exceed parser limits, or excessively deep documents;
- non-deterministically canonicalizable values.

Validation warns about:

- a stage with no required items;
- excessive stage or instruction size near hard limits;
- a skippable final stage;
- `any_previous` where a smaller allowlist appears safer;
- repeated prompts;
- optional items that appear necessary from their prompt.

Warnings do not prevent start unless `--warnings-as-errors` is supplied.

## Canonicalization

Podway Canonical JSON v1 is used for procedure and request digests.

Algorithm:

1. parse YAML or JSON into typed schema values;
2. reject duplicate object keys;
3. apply all documented defaults;
4. normalize enums and IDs exactly as stored;
5. preserve array order;
6. serialize UTF-8 JSON with object keys in lexicographic byte order;
7. serialize integers in minimal decimal form;
8. emit no insignificant whitespace;
9. compute SHA-256 over the exact bytes.

String content is not Unicode-normalized. Authors should use stable source encoding. The canonicalizer must produce identical bytes on all supported platforms.

## Snapshot behavior

At start, the daemon stores:

- source kind and display location;
- full canonical JSON;
- procedure ID and version;
- canonical digest;
- validation warnings accepted at start;
- snapshot creation timestamp.

The running session never re-reads source procedure files. `status` may report that the source file changed, but source drift cannot alter stage semantics.
