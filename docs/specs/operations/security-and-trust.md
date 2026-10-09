# Security and Trust

## Security objective

Podway is a same-user local reliability tool. It prevents accidental wrong-worktree
mutation, unsafe path access, malformed input effects and ambiguous concurrent
writes through bounded inputs, native path validation, writer identity and fences.
It is not a security boundary against software running as the same OS user.
There is no socket authentication or service namespace in the direct product.

## Trust model

Normal operation relies on the invoking user's binary, local filesystem and
tracked Procedure definitions. Recorded confirmations, text, artifact references,
external check results, worker exit status and Procedure authorship are not proof
of semantic correctness. Local database state is not protected against malicious
same-user alteration. Podway enforces procedural completeness, not truth.

A `check_result` is structurally bound. Podway validates required fields, bounds,
digests, operation identity and the accepted outcome relationship. It does not
compare the supplied input basis with worktree state, retrieve executor-owned
bytes, execute the operation, authenticate the executor or verify the summary.
Attempt invalidation supplies ordinary attempt freshness only.

Typed conditions and guards evaluate bounded recorded values. A met predicate
performs no external lookup, authenticates no actor and establishes no factual
truth about text, integers, choices, lists or check results.

A supervised-delegation receipt is also structural. Podway validates declaration,
Role, package identity, bounded execution provenance, rounds, disposition and final
local item-state currency. The integration owns permission, result judgment,
executor-owned bytes and actual worker settlement. A host label or digest grants
no approval. No credential belongs in receipt provenance. The
[adapter mapping review](../../implementation-tips/delegation-adapter-contract-review.md)
records integration prerequisites; local fixture success does not establish live
adapter or model support.

## Why there is no workspace access key

A same-user process able to invoke Podway can also read or alter accessible
worktree state and inspect processes. A locally stored key would not create a
meaningful same-user boundary. Podway instead uses private runtime permissions,
canonical Git worktree identity, Store writer identity and explicit concurrency
fences. Workspace UUID, Procedure/package digests and caller-supplied actor labels
provide identification and correlation, not authentication or authorization.

## Threats and controls

| Threat | Control | Residual limitation |
|---|---|---|
| Wrong worktree is selected | Native Git discovery, UUID/binding validation and precommit revalidation | User can intentionally target any accessible worktree |
| Config/source/runtime path escapes or is replaced | Descriptor-safe containment, nofollow opens and bounded source reads | Malicious same-user code can still alter accessible files |
| Procedure executes code | Data-only bounded schema; no expressions, plugins or execution hooks | Recorded assertions can be false |
| Concurrent callers overwrite state | SQLite IMMEDIATE transaction with workspace/session/attempt/item fences | Callers must reconcile conflicts; FIFO fairness is not promised |
| Lost response repeats a mutation | Atomic success outcome, exact-key replay and read-only lookup | Retention limits apply; full removal deletes runtime retention |
| Runtime is copied to another worktree | Store binding compared with canonical Git common/admin identity | No global scan or authentication of offline copies |
| Incompatible binary writes state | Manifest writer identity checked before writable foreign open | No current production predecessor is declared |
| Malformed input exhausts resources | Byte/depth/string/collection limits and typed aggregate input bound | Same-user software can intentionally consume local resources |
| Task/artifact bytes become global diagnostics | Worktree-only journal with fixed command summaries; no daemon log sink | Local database and public output still contain declared task state |
| Remote attack surface appears | No network client/listener, remote include or telemetry | External reference truth is unverified |

## Filesystem protections

Runtime directories and created Store/lock files use private permissions where
supported. Runtime is confined to `.podway/runtime/`; Podway neither reads nor
creates `~/.podway`. Descriptor-safe opens reject symlink/replacement uncertainty
and preserve lossless internal paths. Local artifacts are opened read-only within
the worktree and hashed in bounded buffers rather than loaded whole.

Ordinary Store access uses a shared native maintenance lease. Cold write-free
reads lacking required WAL/SHM state use stronger exclusion and immutable/heap WAL
snapshots, without creating sidecars. Exclusive maintenance reserves intent before
waiting for existing holders; typed markers survive interrupted reset/removal.
An advisory lock coordinates cooperating Podway invocations and is not an access
control mechanism against the same user.

An undeclared writer or legacy Store is refused before read-write opening, because
SQLite recovery/sidecar creation could otherwise mutate refused state. Legacy
reset is explicitly destructive and does not convert old sessions. The current
compiled predecessor list is empty; tested Store migration support does not imply
an active Engine migration route. A future predecessor declaration and its route
require their separately adopted release/open-development change.

## Procedure and YAML hardening

The parser MUST reject duplicate keys, unknown fields, unsupported tags, remote
includes and executable values; bound document size, aliases, nesting, strings and
collections; and validate semantics before immutable snapshot creation. JSON
follows equivalent bounds. Direct typed input and public output each retain the
1 MiB aggregate budget. Metadata limits do not authorize storing artifact bytes.

## Direct input and output hardening

There is no IPC frame or endpoint. Bounded stdin decoders and typed Direct API
serialization validate the complete aggregate before cloning/preparation. Public
output uses closed canonical output v4/error v2 schemas and versioned result
families. Missing, unknown, mistyped, oversized and cross-command fields fail
closed. Schema retrieval is offline and restricted to embedded or manifest-bound
resources; Podway never fetches a referenced schema over the network.

Retired commands and flags return `COMMAND_RETIRED` before Store access or stdin
consumption. The [JSON contract](../interfaces/json-contract.md) and
[automation requirements](../interfaces/automation-client-contract.md#direct-execution-requirements)
own exact bounds, retry semantics and compatibility generation.

## Artifact handling

Local artifacts retain worktree-relative location, digest, size and media type,
not content. Required local artifacts are hashed before writer admission for
completion and decisions. The transaction reobserves safe descriptor stamps and
item/worktree identities without reading artifact bytes. External references remain
caller-provided and unverified.
A digest identifies bytes when the relevant bytes were hashed; it authenticates
no actor and does not establish that the artifact remains unchanged afterward.

## Secrets

Podway has no secret-management feature. Users SHOULD NOT record credentials in
titles, item values, artifact references, Procedure prompts or execution provenance.
`record --stdin` can reduce shell-history exposure, but recorded text remains local
state and is not a protected secret store.

## Logging and diagnostics

No daemon or bootstrap log, log sink, rotation policy or global diagnostic store
remains. New successful mutations journal only time, committed sequence, a fixed
event and command summary; they do not copy task titles, values, artifact locations,
keys, environments or raw source chains into that summary. Error envelopes carry
only the bounded remediation fields required by their canonical contracts.
An opaque internal diagnostic ID is correlation, not a promise of a searchable
log record. See [Observability](observability.md).

## Network and telemetry policy

Podway opens no network listener, performs no network requests, sends no telemetry,
checks for no updates and loads no remote schemas, presets, Procedures or artifacts.
A future network capability requires a new accepted ADR with visible configuration
and a threat model. Development/release tools are separate from product execution;
their explicit external delivery authority does not change this runtime policy.

## Binary and supply-chain security

The application commits its lockfile, pins the toolchain and reviews dependency
licenses/vulnerabilities in the development preparation gate. Full distribution
requires native Apple Silicon execution, complete manifest/offline-schema closure,
checksums, provenance, packaged scenarios and release-hook absence on exact
executable bytes. Signing and notarization status MUST be reported truthfully;
current release evidence declares `unsigned` and `not-attempted`, not completed
signing or notarization. There is no dynamic plugin loader or service installation.

Release readiness, publication, installation, old-service retirement and external
consumer adoption are separate decisions. A non-distribution native dry run is
nonpublishable. The [release workflow](../../implementation-tips/release.md) owns
the full and explicitly confirmed reduced patch gates.

## Explicit limitations

Podway does not protect against root, malicious same-user software, direct database
modification, fabricated assertions, malicious binary replacement, worktree
deletion, untrusted external references or artifacts changed after completion.
Private paths, digests, receipts and locks must never be presented as stronger
assurance. Tests prove their selected boundary, not arbitrary real executor work.
