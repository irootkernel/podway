SHELL := /bin/sh

RUST_TOOLCHAIN := 1.97.1
RUST_TOOLCHAIN_CARGO := $(shell rustup which --toolchain $(RUST_TOOLCHAIN) cargo 2>/dev/null)
ifeq ($(RUST_TOOLCHAIN_CARGO),)
$(error Rust $(RUST_TOOLCHAIN) is required; install it with rustup)
endif
RUST_TOOLCHAIN_BIN := $(dir $(RUST_TOOLCHAIN_CARGO))
RUST_TOOLCHAIN_ENV = PATH="$(RUST_TOOLCHAIN_BIN):$${PATH}"
RUST_GATE_ENV = CARGO_INCREMENTAL=0 $(RUST_TOOLCHAIN_ENV)
TEST_THREADS ?= 4
DIST_DIR ?= dist
PRESET_DIR ?= assets/presets
PRESET_VALIDATOR ?= target/debug/podway
export PRESET_ID PRESET_NAME PRESET_DESCRIPTION PRESET_FILE PRESET_DIR PRESET_VALIDATOR

.PHONY: test test-prepare release-prepare dist-preflight test-rust test-unit test-int test-fuzzing test-e2e contract-verifier-test \
	toolchain format format-check vet lint lint-all architecture architecture-static preset-validator \
	preset-create preset-import preset-tool-test contract-manifest dist \
	dev-daemon dev-runtime-test

toolchain:
	@$(RUST_TOOLCHAIN_ENV) cargo --version
	@$(RUST_TOOLCHAIN_ENV) rustc --version

test:
	$(MAKE) test-prepare
	$(MAKE) test-rust
	$(MAKE) contract-verifier-test
	$(MAKE) test-e2e
	$(MAKE) preset-tool-test PRESET_VALIDATOR_READY=1
	$(MAKE) dev-runtime-test

test-prepare: toolchain
	$(MAKE) format-check
	$(MAKE) lint
	$(MAKE) architecture-static

release-prepare:
	$(MAKE) lint-all
	$(RUST_TOOLCHAIN_ENV) python3 tools/verify_contracts.py --sentinels
	$(RUST_TOOLCHAIN_ENV) python3 tools/release_evidence.py self-test
	$(RUST_TOOLCHAIN_ENV) python3 tools/release_archive.py self-test
	$(RUST_TOOLCHAIN_ENV) python3 tools/qualify_distribution.py self-test
	$(RUST_TOOLCHAIN_ENV) python3 tools/create_dolgorae_handoff.py self-test
	$(RUST_TOOLCHAIN_ENV) python3 tools/verify_release_bundle.py self-test

dist-preflight:
	$(RUST_TOOLCHAIN_ENV) python3 tools/release_archive.py preflight

format:
	$(RUST_TOOLCHAIN_ENV) cargo fmt --all

format-check:
	$(RUST_TOOLCHAIN_ENV) cargo fmt --all -- --check

vet:
	$(RUST_TOOLCHAIN_ENV) cargo check --workspace --all-targets --locked

lint:
	$(RUST_GATE_ENV) cargo clippy --workspace --lib --bins --locked -- -D warnings
	$(RUST_TOOLCHAIN_ENV) cargo deny check

lint-all:
	$(RUST_GATE_ENV) cargo clippy --workspace --all-targets --locked -- -D warnings

architecture: architecture-static
	$(RUST_GATE_ENV) cargo test --workspace --test 'arch_*' --locked -- \
		--test-threads=$(TEST_THREADS)

architecture-static:
	$(RUST_TOOLCHAIN_ENV) python3 tools/verify_docs.py
	$(RUST_TOOLCHAIN_ENV) python3 tools/verify_test_layout.py --check
	$(RUST_TOOLCHAIN_ENV) python3 tools/verify_quality_contracts.py
	$(RUST_TOOLCHAIN_ENV) python3 tools/verify_contracts.py --check

contract-manifest:
	$(RUST_TOOLCHAIN_ENV) python3 tools/contract_manifest.py --check

preset-validator:
	@if [ "$(PRESET_VALIDATOR_READY)" = "1" ]; then \
		test -x "$(PRESET_VALIDATOR)" || { echo "prepared preset validator is missing: $(PRESET_VALIDATOR)" >&2; exit 1; }; \
	else \
		$(RUST_TOOLCHAIN_ENV) cargo build --locked -p podway-cli --bin podway; \
	fi

preset-create: preset-validator
	@test -n "$$PRESET_ID" || { echo "PRESET_ID is required" >&2; exit 2; }
	@test -n "$$PRESET_NAME" || { echo "PRESET_NAME is required" >&2; exit 2; }
	@test -n "$$PRESET_DESCRIPTION" || { echo "PRESET_DESCRIPTION is required" >&2; exit 2; }
	$(RUST_TOOLCHAIN_ENV) python3 tools/manage_presets.py create \
		--id "$$PRESET_ID" \
		--name "$$PRESET_NAME" \
		--description "$$PRESET_DESCRIPTION" \
		--output-dir "$$PRESET_DIR" \
		--podway "$$PRESET_VALIDATOR"

preset-import: preset-validator
	@test -n "$$PRESET_FILE" || { echo "PRESET_FILE is required" >&2; exit 2; }
	$(RUST_TOOLCHAIN_ENV) python3 tools/manage_presets.py import \
		--source "$$PRESET_FILE" \
		--output-dir "$$PRESET_DIR" \
		--podway "$$PRESET_VALIDATOR"

preset-tool-test: preset-validator
	$(RUST_TOOLCHAIN_ENV) python3 tools/verify_preset_tooling.py --podway "$$PRESET_VALIDATOR"

dev-daemon:
	$(RUST_TOOLCHAIN_ENV) python3 tools/dev_runtime.py daemon

dev-runtime-test:
	$(RUST_TOOLCHAIN_ENV) python3 tools/dev_runtime.py self-test

test-rust:
	$(RUST_GATE_ENV) cargo test --workspace --lib --bins \
		--test 'arch_*' --test 'int_*' --locked -- \
		--test-threads=$(TEST_THREADS)

test-unit:
	$(RUST_GATE_ENV) cargo test --workspace --lib --bins --locked -- \
		--test-threads=$(TEST_THREADS)

test-int:
	$(RUST_TOOLCHAIN_ENV) python3 tools/verify_test_layout.py --check
	$(RUST_GATE_ENV) cargo test --workspace --test 'int_*' --locked -- \
		--test-threads=$(TEST_THREADS)

contract-verifier-test:
	$(RUST_GATE_ENV) cargo clippy --offline --locked -p podway-protocol \
		--features release-contract-verifier --lib --bin podway-contract-verifier \
		--test int_suite -- -D warnings
	$(RUST_GATE_ENV) cargo test --offline --locked -p podway-protocol \
		--features release-contract-verifier --test int_suite \
		int_release_contract_verifier -- --test-threads=$(TEST_THREADS)

test-fuzzing:
	$(RUST_GATE_ENV) python3 tools/run_fuzzing.py

test-e2e:
	$(RUST_TOOLCHAIN_ENV) python3 tools/verify_test_layout.py --check
	$(RUST_GATE_ENV) python3 tools/run_e2e.py

dist:
	$(MAKE) dist-preflight
	$(MAKE) test
	$(MAKE) release-prepare
	$(MAKE) test-fuzzing
	$(RUST_GATE_ENV) cargo build --release --locked \
		-p podway-cli --bin podway -p podway-daemon --bin podwayd
	$(RUST_TOOLCHAIN_ENV) python3 tools/release_archive.py package \
		--artifact-class distribution \
		--podway target/release/podway \
		--podwayd target/release/podwayd \
		--output-dir $(DIST_DIR)
	$(RUST_TOOLCHAIN_ENV) python3 tools/qualify_distribution.py qualify \
		--output-dir $(DIST_DIR)
	$(RUST_TOOLCHAIN_ENV) python3 tools/create_dolgorae_handoff.py create \
		--output-dir $(DIST_DIR)
	$(RUST_TOOLCHAIN_ENV) python3 tools/verify_release_bundle.py verify \
		--output-dir $(DIST_DIR)
