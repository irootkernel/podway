SHELL := /bin/sh

RUST_TOOLCHAIN := 1.97.1
RUST_TOOLCHAIN_CARGO := $(shell rustup which --toolchain $(RUST_TOOLCHAIN) cargo 2>/dev/null)
ifeq ($(RUST_TOOLCHAIN_CARGO),)
$(error Rust $(RUST_TOOLCHAIN) is required; install it with rustup)
endif
RUST_TOOLCHAIN_BIN := $(dir $(RUST_TOOLCHAIN_CARGO))
RUST_TOOLCHAIN_ENV = PATH="$(RUST_TOOLCHAIN_BIN):$${PATH}"

.PHONY: test test-prepare test-prepare-core test-unit test-int test-e2e \
	toolchain codegen format vet lint architecture

toolchain:
	@$(RUST_TOOLCHAIN_ENV) cargo --version
	@$(RUST_TOOLCHAIN_ENV) rustc --version

test:
	$(MAKE) test-prepare
	$(MAKE) test-unit
	$(MAKE) test-int
	$(MAKE) test-e2e

test-prepare:
	$(MAKE) test-prepare-core
	$(RUST_TOOLCHAIN_ENV) python3 tools/phase0_receipts.py --check

test-prepare-core: toolchain
	$(MAKE) codegen
	$(MAKE) format
	$(MAKE) vet
	$(MAKE) lint
	$(MAKE) architecture

codegen:
	$(RUST_TOOLCHAIN_ENV) python3 tools/import_sot.py --write

format:
	$(RUST_TOOLCHAIN_ENV) cargo fmt --all

vet:
	$(RUST_TOOLCHAIN_ENV) cargo check --workspace --all-targets --locked

lint:
	$(RUST_TOOLCHAIN_ENV) cargo clippy --workspace --all-targets --locked -- -D warnings
	$(RUST_TOOLCHAIN_ENV) cargo deny check

architecture:
	$(RUST_TOOLCHAIN_ENV) python3 tools/verify_test_layout.py --self-test
	$(RUST_TOOLCHAIN_ENV) python3 tools/verify_test_layout.py --check
	$(RUST_TOOLCHAIN_ENV) python3 tools/verify_quality_contracts.py
	$(RUST_TOOLCHAIN_ENV) cargo test --workspace --test 'arch_*' --locked
	$(RUST_TOOLCHAIN_ENV) python3 tools/verify_contracts.py --all
	$(RUST_TOOLCHAIN_ENV) python3 tools/verify_verification_runner.py

test-unit:
	$(RUST_TOOLCHAIN_ENV) cargo test --workspace --lib --bins --locked
	$(RUST_TOOLCHAIN_ENV) cargo test --workspace --doc --locked

test-int:
	$(RUST_TOOLCHAIN_ENV) python3 tools/verify_test_layout.py --check
	$(RUST_TOOLCHAIN_ENV) cargo test --workspace --test 'int_*' --locked

test-e2e:
	$(RUST_TOOLCHAIN_ENV) python3 tools/verify_test_layout.py --check
	$(RUST_TOOLCHAIN_ENV) python3 tools/run_e2e.py
