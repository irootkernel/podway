# Podway v0.1.2 Contract Recovery and Native Release

## Status and authority

- Document state: `Adopted`
- Owning roadmap epic: `REL12`
- Target product release: `v0.1.2`
- Repository scope: Podway only
- Last accepted planning baseline: August 3, 2026

This dossier is the source of truth for the unfinished v0.1.2 implementation and
release plan. The [active roadmap](../roadmap/) owns execution order and task
status. Accepted ADRs, canonical assets, and specifications continue to own
implemented product behavior. Each roadmap task must promote its completed
behavior into those sources rather than treating this dossier as a permanent
runtime specification.

## Verified context

The published v0.1.1 archive and its two executables have the expected source,
architecture, and content identities. The verified release baseline is:

| Identity | Value |
|---|---|
| Release tag | `v0.1.1` |
| Annotated tag object | `6a73f955d2126957a8d81af4ab4b5e769f318fa1` |
| Source commit | `8a1f2ad4727617239cae4cd809bcedf215c93c07` |
| Source tree | `05f3068b10aed7ab0599eba4ff80714197b285c9` |
| Archive SHA-256 | `8e101b325b5ff58641c1e1cf06283e8cb738a23b3c38af35bab35f65b76de6da` |
| `podway` SHA-256 | `df62e08655bcea97a1609661884f6204340a64d3335a5f095fa8ae96a3aaed99` |
| `podwayd` SHA-256 | `9e7e1c0a45ef20baf3aec8355463657e59e1ad6e0209854c343837f11558f95b` |
| Contract manifest | `sha256:f128d90b7006605cbbbf82e5304667269601e73674239ff608784c86e0212a85` |
| GitHub release ID | `363845461` |

The protected v0.1.1 asset-ID baseline is:

| Asset | GitHub asset ID |
|---|---|
| Provenance | `499105302` |
| Archive | `499105304` |
| Detached archive checksum | `499105305` |
| Dolgorae handoff | `499105306` |

The published CLI identity result contains
`"schema":"podway.version-result/v1"`. The published daemon identity result
does not. Both responses otherwise identify the same build, source, target,
product version, contract manifest, and supported IPC set.

The current archive validator accepts both results because it extracts `result`
and compares selected fields. The packaged qualification validator performs the
same kind of partial comparison. Neither validator proves that the complete
envelope and result satisfy the schemas shipped in the archive. This is the
release-blocking v0.1.1 defect.

## Goal

Publish v0.1.2 as the official native Apple Silicon Podway runtime for the
Dolgorae MVP-E4D baseline. Completion requires all of the following:

- schema-conformant CLI and daemon identity responses;
- one identical closed identity reported by both binaries;
- release validation against only the exact packaged contract set;
- a clean native `aarch64-apple-darwin` distribution and complete handoff;
- an explicit v1 compatibility decision;
- early qualification failure when another Podway daemon owns the production
  singleton lock;
- immutable publication when supported; and
- independent verification of the bytes downloaded after publication.

Compilation or unit-test success alone is not release completion.

## Non-goals

- Do not modify the Dolgorae repository.
- Do not modify, replace, delete, or republish the v0.1.1 tag, release, or assets.
- Do not weaken a public v1 schema to accept malformed output.
- Do not introduce a new output, error, procedure, or result contract version
  unless the compatibility audit discovers a genuine released breaking change.
- Do not add another supported platform, architecture, translated build, or
  universal binary.
- Do not bypass the production singleton lock for packaged conformance.
- Do not expose test-only isolation capability from distribution binaries.
- Do not claim reproducible compilation unless independent clean builds prove
  identical binary bytes. Deterministic archive construction remains required.

## Contract decisions

### Version identity result

`podway version --json --identity` and
`podwayd version --json --identity` must each emit one newline-terminated
`podway.output/v1` document with:

- `command` equal to `version`;
- a `result` that validates against `podway.version-result/v1`;
- `result.schema` equal to `podway.version-result/v1`; and
- the complete product, version, target, build, source, manifest, and IPC
  identity required by the existing result schema.

The shared Rust identity value will own the result discriminator. Both binaries
will construct the public response through the validated protocol envelope path;
the daemon will no longer assemble this envelope as unchecked JSON. The CLI's
runtime daemon probe will parse and validate the complete response rather than
accepting a bare result or selected fields.

The two result objects must be exactly equal after parsing. Envelope correlation
and timestamp fields are expected to differ and are validated but not compared.

### Envelope openness and result closure

The common `podway.output/v1` envelope deliberately permits additive envelope
fields. v0.1.2 will not close it or impose a private shape stricter than the
published schema. The command-selected `podway.version-result/v1` object remains
closed: missing, unknown, or mismatched result fields are invalid.

### Authoritative packaged schema registry

An internal release contract verifier will use the existing Rust JSON Schema
implementation. It will receive a contract root and construct a registry only
from schema assets listed by that root's contract manifest.

The verifier must:

1. validate the manifest shape and recompute its canonical digest without the
   `digest` member;
2. verify every manifest member against its recorded SHA-256;
3. require a complete and unique schema inventory and unique `$id` values;
4. resolve `$ref` only by manifest-registered `$id` or packaged schema path;
5. reject network, external filesystem, missing, stale, duplicated, and unknown
   contract references;
6. validate the complete identity envelope against `output-v1.schema.json`;
7. separately validate its result against `version-result-v1.schema.json`; and
8. compare the closed result with the expected source, target, version, and
   contract identities.

Source-tree tests and extracted-distribution qualification must call the same
verifier with different contract roots. Release Python code may orchestrate the
checks but must not implement a weaker parallel schema interpretation.

### Regression evidence

The contract fixture set will contain the exact malformed v0.1.1 daemon identity
shape. It must be rejected for missing `result.schema`. Generated mutations will
also cover:

- a wrong result discriminator;
- an unknown result field;
- each missing required identity field;
- a wrong outer discriminator or command;
- CLI and daemon identity drift;
- contract-manifest digest drift; and
- source-commit drift.

The repository gate must remain offline. A separate release audit may download
the actual v0.1.1 asset and demonstrate the same rejection, but that network
operation is not a normal regression-test dependency.

## V1 compatibility boundary

The released v0.1.0 and v0.1.1 copies of these public schemas are identical:

- `procedure-v1.schema.json`;
- `output-v1.schema.json`;
- `error-v1.schema.json`;
- `status-result-v1.schema.json`;
- `next-result-v1.schema.json`; and
- `version-result-v1.schema.json`.

Before v0.1.0, commit `754ff5d7e764e74234f0b22a0a6fe255bfa09ea4`
changed `list.max_items.minimum` from zero to one. The core domain constructor
already rejected zero, while the configuration layer and schema were aligned in
that commit. Therefore v0.1.2 will preserve `minimum: 1`; restoring zero would
reintroduce a schema/runtime contradiction.

This was not a breaking change between public releases and does not require a v2
procedure contract. A consumer that pinned an earlier pre-release snapshot must
migrate any schema-only assumption or document using `max_items: 0` to a value in
the supported range `1..=1000`. The release notes and Dolgorae handoff must state
this boundary without claiming that zero was ever accepted at runtime.

The compatibility task must compare the exact schema bytes and behavior again
before freezing v0.1.2. Any additional breaking difference stops REL12 and
requires a new contract identifier and a separate migration design.

## Qualification and release evidence

### Early singleton preflight

The release preflight will derive the effective account's fixed production lock
path without trusting ambient `HOME`. If the existing lock file is safe to open,
it will attempt the same non-blocking exclusive `flock` class used by the daemon.

- A missing runtime directory or lock file means no current owner and passes.
- Lock contention fails before formatting, compilation, tests, or fuzzing with an
  instruction to stop the production service or foreground dev daemon.
- An unsafe owner, mode, type, or symlink fails closed.
- A successful probe releases the lock immediately and never inspects, removes,
  or replaces a socket.

This is an early diagnostic, not a lease for the rest of qualification. The
packaged daemon must still acquire the same production singleton lock, so a race
after preflight remains safely rejected.

### Archive and manifest

The deterministic archive layout remains rooted at
`podway-0.1.2-aarch64-apple-darwin/`. It must contain the two thin arm64 Mach-O
executables, complete manifest-bound contract assets, presets, specifications,
fixtures, completions, license, README, and release notes required by the release
specification.

Archive determinism means identical archive bytes for fixed staged input bytes:
sorted paths, fixed ownership, fixed modes, fixed timestamps, and deterministic
gzip metadata. Binary reproducibility across independent compiler invocations is
not implied unless it is separately demonstrated.

### Provenance

The v1 provenance shape will preserve its existing fields and add these required
members:

- `product`: `podway`;
- `source_tree`: the qualified Git tree;
- `release_gate_result`: `passed`; and
- `packaged_conformance`: an object containing `result` and the exact scenario
  list.

Packaging may initially record packaged conformance as `pending`. Only successful
qualification of the extracted final archive may atomically replace it with
`passed`. Handoff generation and publication verification must reject a pending,
missing, unknown, or malformed provenance field.

The final provenance continues to bind version, source commit, clean-tree state,
Rust toolchain, Cargo.lock SHA-256, target, archive name and SHA-256, both binary
SHA-256 values, build identity, contract-manifest identity, release gate, and
signing/notarization status.

### Dolgorae handoff

The v1 handoff will preserve its current fields and add:

- `product`: `podway`;
- `source.clean`: `true`;
- `release_gate_result`: `passed`; and
- `release_status`: the exact provenance signing and notarization status.

It must bind the final provenance by name and SHA-256 and repeat the archive,
binary, build, source, target, toolchain, contract, and packaged-conformance
identities required for Dolgorae pinning. The final verifier must compare these
values bidirectionally rather than merely checking their presence.

Unsigned and not-notarized publication remains acceptable only while it is the
explicit release policy and is recorded consistently in release notes,
provenance, handoff, and the final report.

## Required product behavior

The patch release must preserve existing Dolgorae behavior:

- native thin arm64 `podway` and `podwayd` binaries;
- exact `podway 0.1.2\n` plain version output with empty stderr;
- public `podway.output/v1` and `podway.error/v1` envelopes;
- explicitly discriminated procedures, status results, and next results;
- custom procedure start and current stage/attempt observation;
- fenced, idempotent admission and reconciliation;
- isolated foreground dev-daemon conformance using the production singleton
  lock; and
- absence of test-only isolation capability in distribution binaries.

Existing conformance scenarios remain release-blocking. The identity repair does
not authorize unrelated lifecycle, IPC, storage, or command changes.

## Release execution

### Local gate

From the exact clean release commit on native Apple Silicon macOS, observe and
record at least:

```text
cargo fmt --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
make test
make dist
```

After `make dist`, independently extract the final archive and verify:

- detached archive checksum;
- safe and exact archive layout;
- thin arm64 Mach-O identity;
- exact plain-text version output;
- complete CLI and daemon JSON Schema validation;
- complete binary identity equality;
- manifest member and canonical digest reconstruction;
- archive, provenance, and handoff consistency;
- tag candidate, source commit, and source tree consistency;
- Cargo.lock digest; and
- absence of leftover sockets and daemon processes.

Any source change after a gate failure creates a new release candidate and
requires the complete clean gate again.

### Publication order

GitHub release immutability currently exists for this repository but must be
enabled before creating the v0.1.2 release because it protects future releases,
not existing ones.

1. Preserve and recheck the v0.1.1 tag, release, asset IDs, and digests.
2. Create and push an annotated `v0.1.2` tag for the exact qualified commit.
3. Enable repository release immutability.
4. Create a draft v0.1.2 release tied to the annotated tag.
5. Upload the archive, checksum, provenance, and Dolgorae handoff.
6. While still a draft, compare every GitHub-reported asset digest and download
   every asset by its immutable asset ID for independent verification.
7. Publish only after draft verification passes.
8. Confirm the published release reports `immutable=true`.
9. Download all published assets again and repeat the complete independent
   verification.
10. Recheck that no v0.1.1 identity or asset changed.

If immutable releases cease to be available, publication must explicitly record
that the release name is mutable and instruct Dolgorae to trust the pinned asset
ID, archive SHA-256, provenance, and handoff identities. This fallback requires a
documented release-time decision; it is not the default plan.

## Roadmap traceability

The [REL12 roadmap](../roadmap/README.md#rel12-podway-v012-contract-recovery-and-release)
is executed in strict task order.

| Task | Design responsibility | Acceptance boundary |
|---|---|---|
| `REL12001` | Freeze this dossier and the roadmap decomposition. | English documentation passes repository validation. |
| `REL12002` | Audit released schemas and pre-release compatibility. | No unversioned breaking change remains; migration boundary is recorded. |
| `REL12003` | Repair shared identity construction and runtime probing. | Both real binaries emit identical schema-conformant results. |
| `REL12004` | Implement packaged-schema validation and negative controls. | The same verifier rejects all specified drift in source and packaged roots. |
| `REL12005` | Harden preflight, provenance, handoff, and final consistency checks. | Held locks, pending evidence, and identity mismatches fail closed. |
| `REL12006` | Advance to 0.1.2 and produce the qualified native assets. | The exact clean commit passes every local and extracted-distribution gate. |
| `REL12007` | Publish and reverify the immutable release. | Downloaded published bytes pass independently and v0.1.1 is unchanged. |

## Final report

REL12 is complete only when the final report contains:

- v0.1.2 annotated tag object, source commit, and source tree;
- release URL and release ID;
- all asset IDs;
- archive, binary, provenance, handoff, Cargo.lock, and manifest digests;
- exact commands and their results;
- packaged conformance scenarios and outcomes;
- schema compatibility audit result and any consumer migration requirement;
- signing and notarization status;
- immutable-release status; and
- every remaining limitation.

The report must state that completion was based on the downloaded published
assets, not only on compilation, local tests, or the locally staged archive.

## References

- [JSON contract](../specs/interfaces/json-contract.md)
- [CLI specification](../specs/interfaces/cli-specification.md)
- [Automation client contract](../specs/interfaces/automation-client-contract.md)
- [Release and packaging specification](../specs/operations/release-and-packaging.md)
- [Testing and conformance](../specs/quality/testing-and-conformance.md)
- [Release workflow](../implementation-tips/release.md)
- [Contract manifest](../../contracts/contract-manifest-v1.json)
- [Version result schema](../../assets/schemas/version-result-v1.schema.json)
- Historical `podway.output/v1` envelope schema (removed from the current package by ADR-0019)
- [GitHub immutable releases](https://docs.github.com/en/enterprise-cloud@latest/code-security/concepts/supply-chain-security/immutable-releases)
