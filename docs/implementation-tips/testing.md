# Testing

## Verification layers

```bash
make test-prepare   # formatting, lint, static checks, contracts, architecture
make test-rust      # unit, architecture, and integration tests
make test-unit      # focused library and binary tests
make test-int       # focused component integration tests
make test-fuzzing   # bounded deterministic protocol fuzzing
make test-e2e       # real user journeys through debug product binaries
make dev-runtime-test # isolated contributor runtime self-test
make test           # required development gate
make dist           # complete gate, qualification, handoff, and final verification
```

Run the narrowest relevant layer while iterating and `make test` before sharing a
development revision. `make dist` always reruns that gate, adds bounded fuzzing,
all-target Clippy, and qualification of the release-profile distribution before
creating the handoff and independently verifying the final bundle.

Rust unit, architecture, and integration targets use four test workers by default;
set `TEST_THREADS=<n>` on a Make invocation to tune that bound. Real-binary E2E
remains serial. Make-driven Cargo gates disable incremental compilation to avoid
accumulating large numbers of codegen objects; ordinary direct `cargo build`
commands retain Cargo's incremental default.

Cargo integration files are modules of each crate's `int_suite`; run one exact
test with:

```bash
cargo test -p <package> --test int_suite <source>::<function> -- --exact
```

CLI E2E sources use `e2e_suite`. The test-layout verifier rejects sources omitted
from or duplicated across aggregate suites.

## Optional diagnostics

- `python3 tools/run_g005_vertical.py` exercises the production command path.
- `python3 tools/run_g008_dogfood.py` exercises the four retained v1 presets.
- `python3 tools/dev_runtime.py self-test` exercises the implemented `sw-dev-v2`
  workflow in isolated disposable state; the broader development gate also
  verifies both shipped v2 preset identities and the contributor-only admission
  override independently of public release admission.
- The pinned fuzz targets cover canonical JSON, selectors, response compatibility,
  and procedure parsing.

For an isolated contributor daemon and disposable sandbox that does not hold the
production singleton lock, use the
[contributor development runtime](dev-runtime.md).

Optional diagnostics provide investigation support; they do not replace
`make test` as the development gate or `make dist` as the release gate. See the normative [testing and conformance
specification](../specs/quality/testing-and-conformance.md).
