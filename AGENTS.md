# AGENTS.md

Repository guidance for AI coding agents working on Podway.

The core behavior below is the complete local authority for how agents inspect,
implement, and verify work in this repository. These rules favor correctness and
caution over speed; apply them proportionally for trivial work.

## Core Behavior

### 1. Inspect Before Acting

**Resolve repository facts before making implementation decisions. Do not hide
uncertainty.**

Before implementing:

- Read the requested code, its relevant tests, and the nearest authoritative
  document or machine contract before changing anything.
- Resolve discoverable facts from the repository first. Follow the authority order
  and ownership boundaries below instead of asking the user for information the
  repository already provides.
- State material assumptions when they affect scope, design, compatibility,
  migration, or verification.
- If multiple interpretations would produce materially different outcomes, present
  the alternatives and recommend one instead of choosing silently.
- Surface meaningful trade-offs. Point out a simpler approach when it satisfies the
  same requirement with less complexity or risk.
- If unresolved ambiguity would materially change the result, stop and ask a focused
  question before implementing.
- Push back when a request conflicts with repository authority, product boundaries,
  safety, or the user's stated goal.

### 2. Prefer the Smallest Complete Solution

**Use the minimum implementation that fully satisfies the verified requirement. Add
nothing speculative.**

- Implement only what the request requires.
- Reuse established crate boundaries, domain types, public envelopes, error models,
  and test patterns before introducing a new abstraction.
- Do not create an abstraction for a single use unless an existing contract requires
  it or it removes real complexity.
- Do not add speculative features, configurability, compatibility layers, or
  extension points.
- Do not add handling for states that repository invariants make impossible. Add
  defensive handling at real trust, persistence, concurrency, process, protocol,
  and filesystem boundaries.
- Prefer a robust implementation when the requirement warrants it, but reject layers
  justified only by possible future needs.
- If the implementation is substantially larger than the behavior it provides,
  simplify it before reporting completion.

Ask whether a senior maintainer would consider the solution overcomplicated. If so,
reduce it.

### 3. Make Surgical Changes

**Touch only what the requested outcome and its verification require. Clean up only
what the change makes obsolete.**

When editing existing code:

- Do not refactor, reformat, rename, or clean up adjacent code unless the task
  requires it.
- Match the local Rust, Python, YAML, JSON, SQL, and documentation style.
- Mention unrelated defects or dead code instead of modifying them without
  authorization.
- Preserve unrelated user changes in a dirty worktree.

When the change creates obsolete code:

- Remove imports, variables, functions, files, contract entries, generated
  references, or documentation made obsolete by the change.
- Do not remove pre-existing dead code or unrelated artifacts unless the request
  includes that cleanup.

Every changed line must be traceable to the requested outcome or to verification of
that outcome.

### 4. Work Toward Verifiable Goals

**Define success before implementation and continue until the result is proved or
concretely blocked.**

- Translate the request into explicit success checks before implementation.
- For a bug, reproduce the failure when practical and add or identify a regression
  check that fails for the right reason before making it pass.
- For a behavior or contract change, update tests for the success path, relevant
  failure paths, and compatibility boundary.
- For a refactor, establish the relevant behavior and checks before editing, then run
  them again afterward.
- Run the narrowest relevant checks while iterating, then the repository-standard
  gate appropriate to the claim.
- Use `Makefile` targets for repository-standard formatting, linting, contract
  verification, testing, fuzzing, and distribution work.
- Do not treat scaffolding, compilation alone, mocked success, or a focused test as
  proof of complete product behavior when the acceptance criteria require a real
  CLI/daemon, persistence, concurrency, crash, or release path.
- Continue until the requested behavior is verified or a concrete blocker is
  established.
- Report skipped checks with the reason and distinguish unverified assumptions from
  confirmed results.

For multi-step work, keep a short plan in which every step has a corresponding
verification.

## master Preferences

- Use English for internal planning, but never reveal private chain-of-thought.
  Provide concise conclusions and useful evidence instead.
- Respond to master in Korean using polite speech. When directly addressing the
  user, use exactly `master`.
- Keep code, comments, documentation, prompts, templates, CLI/help text, logs,
  reports, schemas, and artifacts in English unless master explicitly requests
  another language.

## Repository Authorities

Start with `docs/README.md`. When sources disagree, use its precedence order:

1. accepted ADRs under `docs/architecture-decision-records/`;
2. canonical machine assets under `assets/` and executable contracts under
   `contracts/`;
3. behavioral specifications under `docs/specs/`;
4. architecture and implementation guidance under `docs/architecture/` and
   `docs/implementation-tips/`;
5. the active `docs/roadmap/README.md` and adopted TODO design dossiers for
   unfinished work;
6. examples, TODO candidates, deferred feedback, and archived roadmap history.

Apply these distinctions as well:

- Use current source, tests, SQLite migrations, and runtime evidence to determine
  existing implementation reality.
- Treat a mismatch between implementation and an accepted ADR, canonical asset, or
  normative specification as a conformance failure. Do not silently choose one side.
- Use the active roadmap for adopted work, ordering, and status. Use an adopted TODO
  dossier for the decision-complete plan of unfinished work; neither overrides an
  implemented higher-authority contract.
- Update every affected specification, machine asset, test, and roadmap entry when a
  behavior change crosses those boundaries.
- Use `Makefile` as the entry point for repository-standard checks. Read the nearest
  relevant authority rather than copying detailed feature design into this file.

## Architecture and Ownership

- `podway-core` owns pure domain values, invariants, transitions, item satisfaction,
  status derivation, and domain errors. Keep infrastructure out of it.
- `podway-config` owns workspace and Procedure parsing, semantic validation,
  canonicalization, digests, and path-safe local Procedure resolution.
- `podway-protocol` owns IPC framing, public envelopes, compatibility, bounded
  decoding, and public error serialization.
- `podway-store`, `podway-git`, and `podway-service` own SQLite persistence, read-only
  Git/worktree discovery, and macOS service integration respectively.
- `podway-presets` embeds and validates the reviewable YAML in `assets/presets/`.
- `podway-daemon` composes infrastructure and owns the socket server, registry,
  scheduler, durable jobs, workers, and observability.
- `podway-cli` owns command grammar, daemon communication, rendering, help, and shell
  completion. It must not reach into store internals.
- Preserve the dependency direction documented in
  `docs/architecture/repository-structure.md`; do not create cycles or reverse
  infrastructure dependencies.

## Product and Runtime Invariants

- Podway is a local procedure guard for one task in one Git worktree. It is not a
  project manager, CI system, shell runner, Git mutation layer, arbitrary workflow
  engine, evidence archive, AI runtime, or remote collaboration service.
- Keep the Procedure v2 single-cursor graph lifecycle ordered with one active
  attempt. Do not introduce parallel active nodes, undeclared routes, expressions,
  plugins, or execution hooks without a new accepted architecture decision and
  explicitly adopted work.
- Podway enforces formal progression conditions; it does not execute the work or
  judge the semantic truth of recorded results.
- Preserve the daemon as the sole normal writer and exactly one executing mutation
  per worktree. Keep mutations atomic, ordered, idempotent, and fail-closed on stale
  revisions, attempts, identities, or unsupported state.
- Keep authoritative task state under the owning worktree's `.podway/runtime/`.
  Global state is limited to the documented per-user endpoint, registry metadata,
  socket, and bounded logs.
- Podway must not mutate Git, make network requests, execute configured commands,
  store artifact bytes, or act as a security boundary against same-user processes.
- Treat public JSON, IPC, schemas, error codes, command routes, canonicalization,
  SQLite layout, and packaged manifest identity as compatibility-sensitive contracts.
  Preserve stable machine fields and error semantics; automation must never depend
  on human-readable output.
- Keep input, frames, queues, collections, paths, timeouts, logs, and concurrency
  bounded. Avoid panics on user-controlled input and preserve non-UTF-8-safe internal
  path handling.
- Use the Rust toolchain pinned by `rust-toolchain.toml`. Podway's supported release
  target is native Apple Silicon macOS only unless repository authority changes.

## Canonical Assets and Generated Outputs

- Edit canonical presets, public schemas, and executable specifications only in
  `assets/presets/`, `assets/schemas/`, and `assets/specifications/`.
- Treat `contracts/`, `quality/`, `release/`, and `tests/fixtures/` according to their
  documented executable-contract and evidence roles; they are not documentation
  mirrors.
- Never create a second source tree or hand-edit a derived copy. Change the canonical
  source and use the documented `Makefile` or repository tool workflow.
- Keep temporary plans, logs, generated reports, fuzz corpora, release output, and
  host-local evidence out of canonical documentation and source trees. Use ignored
  locations such as `artifacts/`, `dist/`, `target/`, and `.podway/` as documented.
- Do not rewrite accepted ADR decisions. Add a new ADR with the next identifier and
  link supersession in both records when an architectural decision changes.

## Development skill references

- Use `$aquarium:task-handler` for one named roadmap task.
- Use `$aquarium:dev-setup` to diagnose or configure development tooling.
- Use `$use-mulgae` for an authorized Mulgae review, run inspection, finding
  follow-up, configuration diagnosis, cleanup plan, or recovery.
- Use `$use-gaori` when a selected long or noisy check is routed through Gaori
  or existing Gaori evidence must be inspected.
- Repository-specific rules below override defaults from the referenced skills.

## Verification

### Release Gate Selection

- This subsection is the single source of truth for agent-operated release gate
  selection and confirmation. Documentation that describes `make dist` applies to
  the full gate and does not override this agent workflow.
- When master requests a release, first ask whether to use the full gate or the
  reduced patch gate. Do not infer the choice from an earlier release.
- Use `make dist` for the full gate.
- The reduced gate is available only for an exact patch-version increment after
  the same clean code candidate passed `make test`. Ask master separately whether
  that `make test` run succeeded. Proceed only after an explicit yes.
- Record the tested baseline commit before changing version metadata, then run
  `make dist-patch PRIOR_MAKE_TEST_PASSED=yes PATCH_BASE_COMMIT=<sha>` from the
  clean version-bumped release commit.
- The reduced gate permits only canonical version and release-metadata changes
  after the tested baseline. Any other change, missing or negative confirmation,
  or failed check stops the release. Do not fall back to the full gate without a
  new instruction.
- A successful reduced gate establishes patch release readiness through its own
  provenance and handoff contract. Do not claim that omitted fuzzing or packaged
  runtime qualification ran, and do not add a `skipped` result to patch evidence.
- Full-gate packaged qualification must use the repository-owned isolated
  `release-qualification` runtime described in the
  [release workflow](docs/implementation-tips/release.md#runtime-isolation-and-cleanup).
  Leave any installed production LaunchAgent untouched; never stop, repoint, or
  reuse its account root, lock, socket, registry, logs, or worktree for
  qualification.
- Treat isolated runtime cleanup as part of the full gate. Success requires no
  qualification daemon process or socket to remain. After failure or interruption,
  reconcile only the exact helper-owned temporary state before retrying; never
  broadly delete `/private/tmp`. Report a cleanup blocker with its exact process or
  recoverable path. For a persistent contributor runtime, stop its foreground
  daemon and use `python3 tools/dev_runtime.py clean --yes`.

- Run an exact focused test first when practical. Cargo integration tests are
  aggregated through each crate's `int_suite`; follow the documented exact-test
  invocation rather than creating an unregistered test target.
- Use `make test-unit`, `make test-int`, `make architecture`, or another narrow
  target while iterating.
- Run `make test` before sharing a development revision that changes executable
  behavior or contracts. This is the required development gate.
- Run `make dist` only when full release or distribution readiness is in scope. It
  includes the development gate, bounded fuzzing, release build, native
  qualification, Dolgorae handoff, and final bundle verification. Use
  `make dist-patch` only under the confirmed patch conditions above.
- Optional diagnostics and direct Cargo commands support investigation but do not
  replace `make test`, `make dist`, or `make dist-patch` for their respective claims.
- For documentation-only or agent-guidance-only changes, read back the file, verify
  references and authority claims, and run `git diff --check`; broader executable
  gates are unnecessary unless the documentation changes executable commands or
  normative behavior.
- After any formatting, generation, test, or release command, inspect `git status`
  and the complete diff so generated or evidence changes are intentional.

### Mulgae Code Review Overrides

- Outside the `$aquarium:task-handler` workflow, use Mulgae only when master
  explicitly requests a review. An explicit task-handler invocation authorizes
  its task-scoped Mulgae review phase.
- Configure the `logic`, `security`, `maintainability`, `product`,
  `documentation`, and `testing` roles with ZCode as their only provider and a
  `60m` provider timeout.
- Use `--diff origin/main...HEAD` for a branch or pull request, `--stage` for
  staged changes, `--dirty` for staged and unstaged changes, or `--workspace`
  only when master explicitly requests all tracked files. Preflight the same
  target and confirm the captured paths and ZCode-only routing before review.
- Track `.mulgae/config.yaml` and `.mulgaeignore`. Keep `.mulgae/local.yaml`,
  every other `.mulgae/**` path, provider homes, credentials, raw transcripts,
  diagnostics, and exported review bundles untracked and private.

### Gaori Test Evidence Overrides

Route long or noisy repository checks through these configured command IDs:

- preparation and static checks: `prepare`;
- unit tests: `unit`;
- integration tests: `integration`;
- end-to-end tests: `e2e`;
- complete development gate: `full`;
- complete distribution gate, only for release readiness: `dist`.

For a dynamically selected Rust test, use:

```bash
gaori --json run --tag rust --tag unit -- cargo test -p <package> --test int_suite <source>::<function> -- --exact
```

Track portable `.gaori/tester.yaml` and explicitly reviewed
`.gaori/tester/rules/*.yaml`. Keep toolchain metadata, rule proposals, runs,
and every other `.gaori/**` path local; never stage configuration or rules
without explicit authorization.

### Commit Messages and Lore

Every commit title must begin with exactly one bracketed header. Inspect the active
roadmap and the adopted task dossier before choosing it:

- If the commit implements or directly verifies one specific roadmap task, use that
  task's exact ID, for example `[V2CTR-001] docs: promote v2 decisions` or the
  retained historical form `[REL12003] fix: repair version identity`.
- If no specific task owns the change but it designs an epic or corrects epic-level
  content not covered by a task, use the exact epic ID, for example
  `[V2MOD] docs: reconcile the procedure model epic`. A task ID takes precedence
  whenever one task directly owns the work.
- Do not substitute a release program ID such as `PV2GA` for a task or epic ID. If
  neither a specific roadmap task nor an epic owns the change, use `[INT]`, for
  example `[INT] docs: adopt local review tooling`.
- Keep the remainder of the title concise and imperative, matching the repository's
  conventional `type: summary` style. The header is required even for trivial
  commits.

Use the installed `lore-commits` skill whenever preparing a non-trivial commit.
After the title and any useful body, append only the git trailers that preserve
decision context not evident from the diff, such as `Constraint:`, `Rejected:`,
`Confidence:`, `Scope-risk:`, `Reversibility:`, `Directive:`, `Tested:`,
`Not-tested:`, and `Related:`. Separate trailers from the body with a blank line,
use `alternative | reason` for each `Rejected:` trailer, and omit Lore trailers for
trivial changes with no meaningful decision context.

## Repository Safety and Delivery

- Do not commit, amend, push, tag, publish, release, install, uninstall, start, stop,
  or replace a daemon or LaunchAgent without explicit authorization.
- Do not discard, overwrite, unstage, or otherwise disturb unrelated user changes.
- Do not use a user's active worktree or installed daemon as a disposable test
  target. Use isolated fixtures and temporary directories through established test
  helpers.
- Do not manually edit `.podway/` databases, runtime links, global registry data,
  sockets, service metadata, or LaunchAgent files to simulate supported behavior.
- Keep `Cargo.lock` committed and use `--locked` in repository-standard Cargo gates.
- Keep one logical task and its direct verification in one commit.
- Keep completion reports compact: state the outcome, changed files, verification
  performed, and actionable remaining risks or blockers. Distinguish development
  gate success from commit, push, release, installation, and runtime activation.
