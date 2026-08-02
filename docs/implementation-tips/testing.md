# Testing

## Verification layers

```bash
make test-prepare   # formatting, lint, static checks, contracts, architecture
make test-rust      # unit, architecture, and integration tests
make test-unit      # focused library and binary tests
make test-int       # focused component integration tests
make test-fuzzing   # bounded deterministic protocol fuzzing
make test-e2e       # real user journeys and archive construction
make test           # complete release-readiness gate
make test-if-needed # reuse an exact current-tree receipt or run make test
```

Run the narrowest relevant layer while iterating and `make test` before treating a
revision as release-ready. The complete gate records
`target/podway-test-gate-v1.json` only after every layer passes. The receipt binds
the Git commit, all non-ignored source bytes, `Cargo.lock`, and the selected Rust
toolchain binaries.

Cargo integration files are modules of each crate's `int_suite`; run one exact
test with:

```bash
cargo test -p <package> --test int_suite <source>::<function> -- --exact
```

CLI E2E sources use `e2e_suite`. The test-layout verifier rejects sources omitted
from or duplicated across aggregate suites.

## Optional diagnostics

- `python3 tools/run_g005_vertical.py` exercises the production command path.
- `python3 tools/run_g008_dogfood.py` exercises all four built-in presets.
- The pinned fuzz targets cover canonical JSON, selectors, response compatibility,
  and procedure parsing.

Optional diagnostics provide investigation support; they do not replace
`make test` as the release gate. See the normative [testing and conformance
specification](../specs/quality/testing-and-conformance.md).
