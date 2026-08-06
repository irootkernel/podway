# Agent Session

This walkthrough drives the same `bug-fix` session as the [example session](example-session.md), but entirely through the JSON automation contract, the way a script or AI agent should operate Podway. The session state matches the known-answer payloads under [`json/`](json/), so every response shape referenced here is decoder-verified.

Ground rules from the [automation workflow](../specs/product/user-workflows.md#automation-and-ai-assisted-use):

- Parse JSON fields and stable error codes. Human-readable text is not an API.
- Perform only the active-stage work, then record it. An external worker finishing does not complete a stage; the caller records the required items and invokes `podway complete`.

## Read the authoritative state

An agent needs no memory of how the session got here. Two read-only commands return everything:

```bash
podway status --json
podway next --json
```

[`json/status-result.json`](json/status-result.json) is the full state: the session is on the `verify` stage, attempt 2, at session revision `17`. The fields an agent should extract are:

- `session.id` and `session.revision` — mutation preconditions;
- `current.attempt_id` — the active attempt that item updates bind to;
- `items[].revision` — per-item preconditions (`0` for an unset item);
- `stages[].status` — `done`, `current`, `redo`, or `pending`;
- `queue.pending_mutations` — whether queued jobs may still change this snapshot.

[`json/next-result.json`](json/next-result.json) is the work order: `missing_required_items` lists exactly what still blocks completion, `allowed_actions` lists the transitions that are currently legal, and `suggestions[].argv` carries a ready-to-run command vector for each missing item.

## Execute the suggestions with explicit preconditions

Fill each suggestion's placeholder, then add the preconditions read from status and an explicit idempotency key ([preconditions reference](../specs/interfaces/cli-specification.md#automatic-and-explicit-preconditions)):

```bash
podway check regression-check-passed \
  --if-attempt 6f8e7dc4-6502-4857-9d38-1a4afedb50e4 \
  --if-item-revision 0 \
  --idempotency-key login-race-verify-2-regression \
  --json

podway set verification-note \
  "The race test and the surrounding authentication suites passed." \
  --if-attempt 6f8e7dc4-6502-4857-9d38-1a4afedb50e4 \
  --if-item-revision 0 \
  --idempotency-key login-race-verify-2-note \
  --json
```

Every successful mutation returns a `podway.output/v1` envelope reporting `revision_before` and `revision_after` ([`json/output-complete.json`](json/output-complete.json)). Feed the latest revision forward instead of assuming it:

```bash
podway complete \
  --if-session-revision <revision-after-from-the-last-response> \
  --if-attempt 6f8e7dc4-6502-4857-9d38-1a4afedb50e4 \
  --idempotency-key login-race-verify-2-complete \
  --json
```

## When the gate refuses

Completing while required items are missing returns a `podway.error/v1` object with code `REQUIRED_ITEMS_MISSING` and the exact missing item IDs ([`json/error-required-items.json`](json/error-required-items.json)). Its `retryable` field is `false`: resubmitting the identical request cannot succeed. Satisfy `details.missing_item_ids` first.

Stale preconditions fail the same way instead of overwriting newer state: when another actor advanced the session, a pinned `--if-attempt` or `--if-item-revision` no longer matches and the mutation is rejected. The correct response to any precondition failure is to re-read `status --json` and re-derive the action, never to resubmit blindly.

## Recover a lost response

If the connection or client dies after a mutation may have started transmitting, the CLI reports `MUTATION_OUTCOME_UNKNOWN` together with the idempotency key ([errors reference](../specs/interfaces/errors-and-exit-codes.md)). This is not a cancellation: the job may have been admitted and may still succeed. Resolve it read-only with the same key:

```bash
podway job lookup --idempotency-key login-race-verify-2-complete --json
```

`job lookup` returns the durable terminal response without replaying the mutation ([job commands](../specs/interfaces/cli-specification.md#job-commands)). If it shows no admitted job, resubmit with the same key: the same key and the same canonical request produce one logical mutation, and a conflicting request under the same key fails with `IDEMPOTENCY_KEY_REUSED`.

## Rework through the same contract

Rework commands are ordinary JSON mutations. When review work finds an earlier defect:

```bash
podway return --to fix \
  --reason "review found an unhandled cancellation path" \
  --json
```

After the return, `status --json` reports the reached downstream stages as `redo`, and `next --json` describes the fresh `fix` attempt. The agent does not need to remember that a return happened; the state says so.

## Separation of duties

Podway records claims; it is deliberately [not an automatic judge of semantic correctness](../specs/product/goals-and-non-goals.md#non-goals). `podway check regression-check-passed` records that the caller asserts the check passed. When one automated actor both performs and attests all work, the procedure gate is only as trustworthy as that actor.

The operational mitigation is to split stage ownership across principals: the actor that implements does not attest verification or review. A different process — another agent or a human — reads the same `status --json` and `next --json`, performs independent verification, and records those stage items itself. The [multi-actor recipe](../specs/product/user-workflows.md#multiple-actors-on-one-stage) shows the item-level mechanics of several actors recording on one attempt. Podway does not authenticate actors ([same-user trust](../architecture-decision-records/0006-same-user-local-trust.md)), so the split is an orchestration convention rather than an enforced boundary — but the recorded attempts, item revisions, and rework reasons make how each gate was satisfied visible. The adopted v0.2 design strengthens the recorded side of this pattern with immutable decision records that bind selected routes to referenced evidence ([roadmap](../roadmap/README.md)).
