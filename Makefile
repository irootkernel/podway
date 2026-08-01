SHELL := /bin/sh

RUST_TOOLCHAIN := 1.97.1
RUST_TOOLCHAIN_CARGO := $(shell rustup which --toolchain $(RUST_TOOLCHAIN) cargo 2>/dev/null)
ifeq ($(RUST_TOOLCHAIN_CARGO),)
$(error Rust $(RUST_TOOLCHAIN) is required; install it with rustup)
endif
RUST_TOOLCHAIN_BIN := $(dir $(RUST_TOOLCHAIN_CARGO))
RUST_TOOLCHAIN_ENV = PATH="$(RUST_TOOLCHAIN_BIN):$${PATH}"
DIST_DIR ?= dist
PRESET_DIR ?= docs/presets
PRESET_VALIDATOR ?= target/debug/podway
export PRESET_ID PRESET_NAME PRESET_DESCRIPTION PRESET_FILE PRESET_DIR PRESET_VALIDATOR

.PHONY: test test-prepare test-rust test-unit test-int test-fuzzing test-e2e \
	toolchain sync-docs-assets format vet lint architecture architecture-static preset-validator \
	preset-create preset-import preset-tool-test contract-manifest dist dist-qualify

toolchain:
	@$(RUST_TOOLCHAIN_ENV) cargo --version
	@$(RUST_TOOLCHAIN_ENV) rustc --version

test:
	$(MAKE) test-prepare
	$(MAKE) test-rust
	$(MAKE) test-fuzzing
	$(MAKE) test-e2e

test-prepare: toolchain
	$(MAKE) sync-docs-assets
	$(MAKE) format
	$(MAKE) lint
	$(MAKE) architecture-static

sync-docs-assets:
	$(RUST_TOOLCHAIN_ENV) python3 tools/sync_docs_assets.py --write

format:
	$(RUST_TOOLCHAIN_ENV) cargo fmt --all

vet:
	$(RUST_TOOLCHAIN_ENV) cargo check --workspace --all-targets --locked

lint:
	$(RUST_TOOLCHAIN_ENV) cargo clippy --workspace --all-targets --locked -- -D warnings
	$(RUST_TOOLCHAIN_ENV) cargo deny check

architecture: architecture-static
	$(RUST_TOOLCHAIN_ENV) cargo test --workspace --test 'arch_*' --locked

architecture-static:
	$(RUST_TOOLCHAIN_ENV) python3 tools/verify_docs.py
	$(RUST_TOOLCHAIN_ENV) python3 tools/verify_test_layout.py --self-test
	$(RUST_TOOLCHAIN_ENV) python3 tools/verify_test_layout.py --check
	$(RUST_TOOLCHAIN_ENV) python3 tools/verify_quality_contracts.py
	$(RUST_TOOLCHAIN_ENV) python3 tools/release_archive.py self-test
	$(RUST_TOOLCHAIN_ENV) python3 tools/qualify_distribution.py self-test
	$(RUST_TOOLCHAIN_ENV) python3 tools/verify_contracts.py --all
	$(MAKE) contract-manifest
	$(MAKE) preset-tool-test

contract-manifest:
	$(RUST_TOOLCHAIN_ENV) python3 tools/contract_manifest.py --check

preset-validator:
	$(RUST_TOOLCHAIN_ENV) cargo build --locked -p podway-cli --bin podway

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

test-rust:
	RUST_TEST_THREADS=1 $(RUST_TOOLCHAIN_ENV) cargo test --workspace --lib --bins \
		--test 'arch_*' --test 'int_*' --locked

test-unit:
	RUST_TEST_THREADS=1 $(RUST_TOOLCHAIN_ENV) cargo test --workspace --lib --bins --locked

test-int:
	$(RUST_TOOLCHAIN_ENV) python3 tools/verify_test_layout.py --check
	RUST_TEST_THREADS=1 $(RUST_TOOLCHAIN_ENV) cargo test --workspace --test 'int_*' --locked

test-fuzzing:
	python3 tools/run_fuzzing.py

test-e2e:
	$(RUST_TOOLCHAIN_ENV) python3 tools/verify_test_layout.py --check
	$(RUST_TOOLCHAIN_ENV) python3 tools/run_e2e.py

dist:
	$(MAKE) test
	$(RUST_TOOLCHAIN_ENV) cargo build --release --locked \
		-p podway-cli --bin podway -p podway-daemon --bin podwayd
	$(RUST_TOOLCHAIN_ENV) python3 tools/release_archive.py package \
		--artifact-class distribution \
		--podway target/release/podway \
		--podwayd target/release/podwayd \
		--output-dir $(DIST_DIR)

dist-qualify:
	$(RUST_TOOLCHAIN_ENV) python3 tools/qualify_distribution.py qualify \
		--output-dir $(DIST_DIR)
