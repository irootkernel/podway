# ADR-0039: Opt-In Contract Generations with Deprecation

- Status: Accepted
- Acceptance date: 2026-10-22
- Date: 2026-10-22
- Acceptance owner: the owner, recorded with the GUIDE registration
- Revision: refined before registration after independent Claude Opus and Gemini Argon reviews; the owner accepted the revised text on 2026-10-22
- Delivery owner: [GUIDE](../roadmap/README.md#guide-agent-guidance-and-contract-generation-2)
- Implementation guidance: [Contract generations](../specs/interfaces/json-contract.md#contract-generations)
- Preserves: [ADR-0010](0010-generic-cli-json-integration.md), [ADR-0019](0019-procedure-v2-only-product.md), [ADR-0038](0038-remove-the-daemon.md)
- Extends: [ADR-0027](0027-state-aware-session-start.md) with optional session fences on plain start
- Superseded in part by: [ADR-0040](0040-deprecation-in-changelog-changed-section.md)

This ADR accepts an opt-in successor contract generation, the deprecation of the
current one, and fenced plain start. Delivery follows the roadmap order, and the
existing executable contracts keep describing shipped behavior until `GUIDE-007`
through `GUIDE-011` implement these decisions. Acceptance does not activate that
behavior. It also does not authorize a release, an installation, or a change to an
external consumer.

## Context

Podway's public machine contract consists of Output v4, Error v2, and the closed
result and detail families they select. Aquarium and Dolgorae pin the exact contract
manifest. A peer whose manifest digest differs fails closed on the whole contract
set. The manifest digest includes the product version, so consumers already re-pin
on every release. A released closed family never gains a member: a materially
changed payload needs a new schema identifier
([JSON contract](../specs/interfaces/json-contract.md)).

Hands-on agent sessions on 2026-10-22 found these gaps. All of them have existed
since `v0.2.13`:

- **Observation templates.** Prepared and terminal templates in
  `podway.observation-result/v5` carry fences only in `preconditions`. Their argv
  omits both the fences and the idempotency-key placeholder. The disposition template
  lacks its required subcommand. A disposed terminal session offers no archive
  template.
- **Citation errors.** `CRITERION_CITATION_INVALID` details name the rejected
  citation but not the evidence sources and items the active attempt can cite.
- **Writer mismatch errors.** `WORKSPACE_CONTRACT_MISMATCH` details report the
  expected and actual manifest digests, but not which release wrote the Store.
- **Plain start.** It accepts no session fences, so automation cannot fence the start
  that archives a disposed terminal session.

Each contract-level fix needs new closed families. Replacing the current families
outright would break every consumer at once, and Podway has no mechanism to offer a
successor while consumers migrate.

## Decision

1. **Contract generation.** A contract generation is a closed, named selection of
   result and error-detail families. All generations share:
   - the Output v4 and Error v2 envelopes;
   - routes, error codes, exit classes, retryability and messages;
   - Store layout, writer identity, and the contract manifest.

   Generation 1 is the direct contract shipped since `v0.2.13`. Generation 2 is
   generation 1 with three substitutions:
   - `session.observe` returns `podway.observation-result/v6`;
   - `CRITERION_CITATION_INVALID` uses `podway.criterion-citation-invalid-details/v1`;
   - `WORKSPACE_CONTRACT_MISMATCH` uses
     `podway.workspace-contract-mismatch-details/v2`.
2. **Selection.** A global option `--contract-generation <1|2>` selects the
   generation for one invocation. The default is 1 while generation 1 is supported.
   - **Validation.** The value is checked after parsing and before local execution or
     reading stdin. Any other value fails with `REQUEST_INVALID` (exit 2), attributed
     to the parsed route; this includes an empty value, `0`, `3` and `v2`. Retired
     syntax keeps `COMMAND_RETIRED` precedence.
   - **Accepted everywhere.** Every command accepts the option, so a consumer may pass
     it unconditionally. This includes the local commands `help`, `version`,
     `completions`, `preset` and `procedure`, where the option has no effect, and a
     top-level `-h` or `--help` that follows it. Human mode honors the same selection.
   - **Never embedded.** Templates and recovery recipes never contain the option;
     callers append their own global options.
   - **Not a selection channel.** Environment variables, workspace configuration and
     stored preferences do not select a generation.
3. **Presentation only.** The selection travels with the invocation, like the
   lock-wait budget. It is excluded from:
   - canonical request identity and idempotency;
   - stored outcomes, exact replay and outcome lookup;
   - Store state and writer identity.

   A generation may differ only in families that Podway never persists. A future
   generation that changes a persisted family needs a new decision.
4. **Schema evolution.**
   - Released families never change.
   - The Output v4 command-to-result selection and the Error v2 code-to-details
     selection may list one closed family per generation.
   - The `details_schema` field in the error catalog keeps naming the default
     generation's family, so the catalog bytes stay unchanged.
   - A canonical `assets/specifications/contract-generations.json`
     (`podway.contract-generations/v1`, enrolled in the manifest) declares the
     `default` generation and, for each generation, its selections and status. The
     status is one of `reserved`, `supported`, `deprecated` or `retired`.
5. **Generation 2 contents.**
   - **`podway.observation-result/v6`.** Running templates stay identical to v5.
     Lifecycle templates carry `--if-workspace-uuid`, `--if-session-id` and
     `--if-session-revision` with current values, then
     `--idempotency-key <idempotency-key>`. Semantic values appear only as declared
     placeholders. The templates offered depend on the session state:
     - **Prepared session:** begin, reset, and replacement start by preset and by
       Procedure.
     - **Completed or cancelled session without a current disposition:**
       `disposition handed-off` and `disposition not-required`.
     - **Disposed terminal session:** archive, reset, and fenced plain start by
       preset and by Procedure.

     Fenced plain start templates carry the `session.start` command, which their
     execution, replay and outcome lookup record. Prepared replacement templates keep
     `session.start_replace`. Archive, reset and both kinds of start require explicit
     authorization.
   - **`podway.criterion-citation-invalid-details/v1`** adds the bounded evidence
     source nodes and item identifiers that the active attempt can cite. They are
     computed with the same rule that admits citations.
   - **`podway.workspace-contract-mismatch-details/v2`** adds
     `actual_product_version`. The value comes from a compiled table of released
     manifest digests and is null for an unknown digest. The details also carry a
     recovery reason that names the original writer.
6. **Fenced plain start.** In both generations, plain `start` accepts
   `--if-session-id` and `--if-session-revision`. They must be supplied together.
   Input grammar does not depend on the generation. The fences belong to the
   plain-start request only; the replacement request keeps its own preconditions.
   - **Canonical request.** Absent fences leave the canonical request bytes and
     request digest unchanged. Supplied fences become part of the canonical request.
   - **Where fences are checked.** The Engine verifies supplied fences inside the
     start writer transaction, after the idempotency lookup. An exact replay therefore
     returns its original success even after the session was archived or replaced.
     A fresh mismatch returns the identity or revision error of the fenced
     replacement path, with no effect and no outcome.
   - **Plain-start semantics are unchanged.** Only a disposed terminal session is
     archived. Any other existing session returns `SESSION_START_DECISION_REQUIRED`
     and is never deleted. A fenced plain start never enters the human interactive
     start interview.
   - **Dry run.** A fenced `--dry-run` verifies the fences read-only instead of
     taking the offline shortcut.
7. **Deprecation.** Generation 1 is deprecated from the first release that ships
   generation 2. The deprecation is declared in:
   - this ADR and the specifications;
   - the CHANGELOG `Deprecated` section;
   - `contract-generations.json`;
   - help text.

   No runtime warning is emitted. A warning would change the default output bytes and
   would be stored inside mutation outcomes, which would make replay depend on the
   generation. The version result and the manifest schema are not extended.
8. **Removal.** Generation 1 is removed only when all of these hold:
   - Aquarium and Dolgorae have published, qualified releases that select
     generation 2 on every invocation and consume its families.
   - At least one stable Podway release has shipped generation 2 as an opt-in, with
     packaged conformance evidence.
   - No open feedback remains against generation-2 shapes.
   - The owner records the removal decision on the roadmap.

   The planned removal release is `v0.3.0`. Removal:
   - makes generation 2 the default and keeps `--contract-generation 2` accepted;
   - adds a value-aware retirement entry, so that `--contract-generation 1` returns
     `COMMAND_RETIRED` naming its replacement;
   - deletes the families that only generation 1 uses;
   - marks generation 1 `retired` in `contract-generations.json`.

## Rejected alternatives

- **New envelopes (Output v5, Error v3).** Every consumer would have to change at
  once, and the adapter acceptance requirements that name Error v2 would break.
- **Widening released families in place.** This violates the closed-schema and
  family-growth rules of the JSON contract.
- **An environment variable.** It does not appear in argv, and sanitized environments
  drop it.
- **Workspace configuration.** It ties every consumer of a worktree to one choice and
  turns presentation into stored state.
- **Separate per-family flags.** They give no coherent contract identity.
- **Flipping the default before removal.** A consumer that does not pass the option
  would experience this as removal.
- **Runtime deprecation warnings.** See decision 7.
- **Enforcing plain-start fences in the CLI facts preflight.** That check runs before
  the idempotency lookup, so it would reject a valid exact replay once the original
  session has been archived.

## Consequences

Positive:

- Existing consumers keep working on the default generation; they only need to
  re-pin.
- Agents and new integrations can opt into executable lifecycle templates and
  actionable citation and writer diagnostics.
- Automation can fence plain start without losing exact replay.
- The deprecation path and its removal criteria are explicit before anything breaks.

Negative:

- Two observation families and two detail variants must be maintained and verified
  until removal.
- The Output v4 and Error v2 schema bytes, the manifest and the Dolgorae adapter
  catalog change, so Dolgorae must re-pin.
- The released-manifest table must be extended after each release.
- Every route gains one more global option.
