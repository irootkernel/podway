# ADR-0041: Allow Explicit Read-Only User Integration Settings

- Status: Accepted
- Superseded in part by: [ADR-0042](0042-separate-user-settings-namespace.md), only the user-settings location in decision 1.
- Date: 2026-11-01
- Acceptance owner: Master, through approval of the exact INTEG design document diff
- Supersedes in part: [ADR-0038](0038-remove-the-daemon.md), decision 7 only
- Preserves: [ADR-0010](0010-generic-cli-json-integration.md), [ADR-0035](0035-host-neutral-supervised-delegation.md), [ADR-0039](0039-opt-in-contract-generations.md)
- Delivery owner: [INTEG](../roadmap/README.md#integ-dolgorae-integration-readiness)
- Detailed SOT: [Integration settings](../specs/operations/integration-settings.md)

This decision is adopted only when its exact document diff is approved and
applied. Implementation remains ordered by INTEG; current executable contracts
continue to describe current behavior until the owning tasks deliver it.
Acceptance authorizes no installation, Dolgorae change or runtime activation.

## Context

ADR-0038 removed the daemon and every per-user Podway runtime. Its no-global-state
rule also excludes a small user configuration default. Master selected an
explicit default-disabled Dolgorae integration permission in user configuration,
with project overrides, while preserving independent Direct work. Podway must
still neither dispatch external workers nor manage the executor's account.

## Decision

1. Permit only read-only user integration settings at `~/.podway/config.yaml`
   on an explicit integration-settings query. This is a narrow exception to
   ADR-0038 decision 7, not a global runtime root, workspace registry, journal,
   credential store, cache, log, service or automatic configuration writer.
2. Use closed `podway.user-config/v1` and new `podway.workspace/v3` for optional
   `integrations.dolgorae.enabled`. Preserve released workspace v1/v2 and current
   v1 initialization bytes. Resolve explicit project value, then user value,
   then `false`; omission and explicit `false` remain distinct.
3. Validate project config before user config and both before merge. Malformed,
   unreadable or unsafe user input fails the explicit query even if the project
   has an override. Use only lossless, nonempty absolute HOME for home selection;
   do not introduce alternate-root, XDG or account-discovery fallbacks. Apply
   bounded descriptor-safe reads without creating files or directories.
4. Expose effective permission and its source through generic CLI/JSON,
   `integration.show`, without Store/session identity, provider discovery,
   process execution, network access or credential lookup. Both accepted contract
   generations use one new closed result family. Preserve existing envelopes
   and manifest pinning; do not silently widen old configuration families.
5. Ordinary Direct commands do not read user settings or HOME. Runtime state,
   workspace identity, maintenance and success outcomes remain worktree-local.
   Core and Store gain no infrastructure or provider dependencies.
6. Permission is not an execution fact. An external adapter must adopt and honor
   the query before new dispatch. The setting neither enables a backend by
   installation nor authenticates an actor or proves capability. Receipt,
   acceptance, result reconciliation and explicit completion remain unchanged.
7. Repository-only verification tools may inspect an explicitly selected existing
   Dolgorae binary through bounded public metadata commands. They cannot modify
   its code, configuration, installation or account, start providers or invoke
   models. Real adapter qualification and activation remain separate work.

## Rejected alternatives

- Restore a user-global runtime root: introduces authority the settings do not need.
- Read user config on every command: makes ordinary Direct work depend on home state.
- Reuse workspace v1/v2 with added fields: widens released closed contracts.
- Treat enabled as connected or qualified: confuses owner preference with executor proof.
- Change Dolgorae as part of this epic: exceeds the selected Podway-only boundary.

## Consequences

Users can manage one default and explicit worktree overrides. A broken user file
affects only explicit policy queries. The cost is one new versioned configuration
family, a narrow home read boundary and a public permission query. Existing
external adapters require separate adoption before they obey the policy.

INTEG-001 reconciles the root agent guidance and affected durable owners with this
exception. Source implementation, development verification, release, consumer
qualification and activation retain their independent acceptance boundaries.
