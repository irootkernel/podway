# Security and Trust

## Security objective

Podway is a same-user local reliability tool. Its security objective is to prevent accidental cross-worktree mutation, unsafe path access, malformed configuration effects, unauthorized other-user socket access, and ambiguous concurrent writes.

It is not a security boundary against software running as the same OS user.

## Trust model

Trusted for normal operation:

- the logged-in OS user;
- `podway` and `podwayd` binaries installed for that user;
- the local worktree and its tracked procedure definitions;
- explicit user or agent item assertions.

Not trusted as proof of correctness:

- a checked confirmation;
- user-supplied text;
- external artifact metadata;
- task-worker exit status;
- procedure authorship;
- local state against malicious same-user modification.

Podway enforces procedural completeness, not truth.

## Why there is no workspace access key

A worktree access key would be stored in a location readable by the same user that can:

- invoke the CLI;
- connect to the daemon socket;
- read the worktree;
- read user application-support files;
- inspect user processes.

It would add issuance, storage, rotation, recovery, and redaction complexity without creating a meaningful boundary against same-user malware. Podway therefore uses:

- user-private socket permissions;
- peer UID verification;
- Git worktree identity;
- workspace UUID conflict detection;
- explicit concurrency preconditions.

The workspace UUID is identification, not authentication.

## Threats and controls

| Threat | Control | Residual limitation |
|---|---|---|
| Another OS user sends mutations | Socket directory `0700`, socket `0600`, peer UID check | Root can bypass |
| CLI targets wrong path | Independent daemon worktree discovery and UUID check | Same user can intentionally target any accessible worktree |
| Config path escapes worktree | Canonical path containment and symlink checks | Same user can modify files between checks |
| Procedure executes code | Data-only schema, no expressions/plugins/includes | Procedural assertions can still be false |
| Concurrent callers overwrite state | FIFO queue, attempt/session/item revisions | Caller must handle conflicts correctly |
| Lost response repeats mutation | Idempotency binding and terminal receipt | Reuse after retention boundary is not guaranteed for workspace maintenance |
| Worktree copy duplicates state | Live workspace UUID conflict detection | Offline copies are not globally discoverable |
| Malformed IPC exhausts daemon | Frame, depth, string, collection, and queue bounds | Same user can still consume local CPU intentionally |
| Artifact bytes leak into DB | Metadata-only model, streaming hash | Paths and media types remain metadata |
| Logs leak task content | Structured redacted logging | Fatal library messages require review |
| Remote attack | No network listener or network client | External reference truth is not checked |

## Filesystem protections

- daemon runtime and registry directories are user-private;
- workspace runtime directory is `0700` where supported;
- database and socket files are `0600` where supported;
- path canonicalization occurs immediately before file access;
- `.podway/runtime` cannot be a symlink;
- local artifact files are opened read-only;
- no artifact path may escape the worktree;
- file hashing uses bounded buffers and handles sparse or large files without full-memory loading.

## Procedure and YAML hardening

The parser MUST:

- reject duplicate mapping keys;
- bound document size and nesting depth;
- bound aliases or disable YAML features that permit expansion attacks;
- reject unknown fields;
- reject tags that instantiate arbitrary types;
- reject remote includes and executable values;
- validate all string and collection limits before snapshot creation.

JSON input follows equivalent bounds.

## IPC hardening

- local Unix-domain socket only;
- strict frame length before allocation;
- maximum 1 MiB payload;
- one request per connection;
- bounded parse depth;
- explicit protocol version;
- peer UID check;
- no environment-variable expansion in request fields;
- no file path trusted without daemon-side canonicalization.

## Artifact handling

For local paths, Podway stores worktree-relative location, digest, size, and media type. It does not store content.

For external references, metadata is caller-provided and unverified. The UI labels the source as `reference`, not as verified content.

Podway does not treat digest presence as cryptographic actor authentication. It only identifies bytes when those bytes are available for hashing.

## Secrets

Podway has no secret-management feature. Users SHOULD NOT place credentials in:

- task titles;
- item text;
- list values;
- artifact references;
- procedure prompts;
- daemon command arguments.

The CLI supports `set --stdin` to reduce shell-history exposure for ordinary text, but it does not make Podway a safe secret store.

## Logging and diagnostics

Normal logs contain identifiers, command names, durations, queue states, and error codes. They exclude:

- full request payloads;
- item values;
- task titles;
- artifact locations;
- environment variables;
- procedure canonical JSON.

Diagnostic IDs correlate user-visible internal errors with logs without exposing full state.

## Network and telemetry policy

Podway v1:

- opens no network listener;
- performs no HTTP or other network requests;
- sends no telemetry;
- checks for no updates automatically;
- loads no remote schemas, presets, procedures, or artifacts.

Any future network capability requires a new accepted ADR and an explicit user-visible configuration and threat model.

## Binary and supply-chain security

Distribution practice SHOULD include:

- reproducible or independently verifiable builds where practical;
- committed dependency lockfile;
- dependency license and vulnerability review;
- code signing and notarization for public macOS artifacts;
- published SHA-256 checksums;
- no runtime dynamic plugin loading;
- least-privilege user service installation.

These distribution measures describe published artifacts. They are not additional
release-readiness gates beyond the repository-root `make test` command.

## Explicit limitations

Podway does not protect against:

- root;
- malicious same-user software;
- direct database modification while the daemon is stopped;
- fabricated confirmations or text;
- an artifact changed immediately after completion;
- deletion of the worktree;
- malicious modification of the Podway binaries;
- untrusted external reference content.

These limitations must be stated in help and product documentation without implying stronger assurance.
