# Aquarium Development Service

## Scope and ownership

Podway implements Aquarium's `managed-service` producer contract v2. Aquarium
owns immutable publication, its generic locks and leases, the `current` and
`pending` selectors, strict controller-result validation, and the separate
approval for applying one exact plan token. Podway owns the bundle contents,
the `dev.aquarium.podwayd` LaunchAgent, runtime metadata, daemon lifecycle,
readiness, rollback, and bounded recovery state.

Building, committing, or publishing a generation never activates it. Publication
changes only Aquarium's `pending/podway` selector. The prior current generation
and its daemon remain selected until an approved controller apply succeeds.

## Producer contract

The repository provides these targets:

```text
make aquarium-dev-describe
make aquarium-dev-build AQUARIUM_DEV_OUTPUT=<absolute-empty-directory>
```

Describe is read-only and emits exactly one
`aquarium-dev-producer-description/v2` object. Build accepts only a clean
checkout whose `HEAD` is local `main`, uses committed source bytes, writes its
build scratch and final output below the supplied absolute empty directory, and
emits `.aquarium-manifest.json` using
`aquarium-dev-artifact-manifest/v2`.
If construction fails, the producer removes only its partial `.build`, `bundle`,
and manifest paths so the same still-owned output directory is retryable.

The immutable bundle layout is:

```text
bundle/bin/podway
bundle/libexec/podway
bundle/libexec/podwayd
bundle/libexec/aquarium-dev-service
bundle/manifest.json
```

The development version is `v0.2.8-dev.<sha12>`. The external manifest binds the
complete bundle digest using Aquarium's canonical path-and-file digest algorithm.
The internal manifest binds the full Git SHA, the payload digest before the
manifest is added, every executable digest, and the controller and managed
runtime protocol versions.

`bundle/bin/podway` resolves relative to its own immutable generation and
executes only `bundle/libexec/podway --dev`. It never searches `PATH`, resolves
an installed production binary, or falls back to the production endpoint.

## Controller interface

The bundled controller exposes only:

```text
aquarium-dev-service status --json --runtime-root <absolute-root>
aquarium-dev-service plan --json --runtime-root <absolute-root> \
  --generation-root <absolute-generation>
aquarium-dev-service apply --json --runtime-root <absolute-root> \
  --generation-root <absolute-generation> --plan-token <exact-token>
```

Status and plan are read-only. They validate canonical generation containment,
the external and internal manifests, the full bundle digest, and every
executable identity. Results use the closed Aquarium status, plan, and result
schemas registered under `assets/schemas/`.

A plan selects `install`, `activate`, `repair`, `defer`, or `no-change`. Every
non-deferred action carries a SHA-256 token over the action, canonical runtime
root, observed active generation, busy and recovery state, target Git SHA, and
target bundle digest.
A busy replacement returns `defer` and no token. Apply first verifies the token
without mutation, then serializes under the producer-owned service lock and
repeats the plan before changing service state.

The token is an unkeyed `podway.aquarium-service-plan-token/v1` state-integrity
digest, not an authorization or same-user security boundary. Aquarium owns the
separate human approval and must obtain a new token after a controller-protocol
upgrade or any observed-state change.

## Runtime and activation

All producer state is below the runtime root supplied by Aquarium. It includes
the fixed LaunchAgent plist, `podway.managed-runtime/v3` metadata, a controller
lock, exact active-generation state, and at most one bounded recovery record.
The plist label is `dev.aquarium.podwayd`; it executes the exact generation's
`libexec/podwayd --dev`, supplies that runtime root through `PODWAY_DEV_HOME`,
and never uses a mutable `PATH` entry. Aquarium development runtime validation
requires its sealed executable entries to use owner-only mode `0500`; contributor
and release-qualification runtimes retain their private `0755` executable mode.

Replacement rechecks that the current daemon is ready and has no queued or
running work, other admitted clients, or maintenance operation. Missing activity
counts fail closed as busy. It then boots out the prior LaunchAgent. Podway's signal shutdown
closes new connection admission and drains admitted handlers before removing its
socket. The controller publishes exact target runtime metadata, state, and plist,
bootstraps the target, and accepts success only when the bundled CLI observes a
ready mode-`dev` daemon with the target daemon path and source commit.

The controller never touches the installed production label, runtime root,
socket, registry, logs, or service metadata. A durable Podway session without
queued or running execution does not make the development service busy.

## Failure and recovery

Before stopping an active generation, apply writes a bounded adjacent recovery
record containing the exact target and prior state. If target readiness fails,
the controller boots out the target and restores the prior exact manifest,
runtime metadata, plist, and daemon. It reports a successful rollback only after
the prior generation is ready with its original identity.

If restoration cannot be proven, the recovery record remains with bounded debt
and status reports `recovery_required`. Apply fails and Aquarium therefore keeps
the old `current` selector and exact pending target. A later `repair` plan remains
token-bound to that observed state. The original prior identity remains bounded
recovery evidence even if its leased generation later becomes unavailable; that
absence cannot prevent an exact pending target from being repaired, but another
failed target start cannot claim prior restoration. The controller never claims
rollback of an unproven service or advances Aquarium selectors itself.
