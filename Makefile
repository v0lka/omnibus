# Omnibus — local quality gates
#
# Encodes the locally automatable subset of specs/architecture/quality-gates.md
# and roadmap tasks 1.1.2-1.1.3 (docs/development/omnibus-roadmap.md):
#
#   gate       aggregate gate: specs + lint + build + test (default target)
#   build      cargo build --workspace (AC 1.1.2: workspace compiles, omnibusd --version)
#   lint       cargo fmt --check
#              + cargo clippy --workspace --all-targets -D warnings
#              + the four R4 connascence/boundary rules (roadmap 1.1.3, quality-gates R4),
#                each proven red/green by negative fixtures:
#                  R4-COP     no tuple types in trait signatures (contracts only)
#                  R4-NATIVE  zero OS dependencies in omnibus-contracts
#                  R4-CON     capability literals only in the registry file
#                  R4-EDGES   dependency direction (adapters <- contracts only;
#                             no adapter->adapter, no adapter->core)
#   test       cargo test --workspace — carries the contract suite (R2, as traits
#              arrive), the R4 negative fixtures, the R5/R6 security seeds and the
#              R3 ladder suite (as code appears)
#   run-debug  cargo run --bin omnibusd (debug profile)
#   specs      python3 specs/tools/validate_specs.py
#
# CI note (roadmap 1.1.3 AC): this file is the locally runnable half of the gate
# set; the same commands are the CI jobs. A red gate has no override.

CARGO ?= cargo
PYTHON ?= python3

.PHONY: gate build lint test specs run-debug clean help

.DEFAULT_GOAL := gate

gate: specs lint build test ## run every quality gate in order (default)
	@echo "gate: specs, lint, build, test — all green"

build: ## cargo build --workspace
	$(CARGO) build --workspace

lint: ## fmt --check + clippy -D warnings + R4 lints (real tree + red/green fixtures)
	$(CARGO) fmt --all -- --check
	$(CARGO) clippy --workspace --all-targets -- -D warnings
	$(CARGO) run -p omnibus-lints --bin omnibus-lints
	$(CARGO) test -p omnibus-lints --test fixtures

test: ## cargo test --workspace (R2 contract suite, R4 fixtures, R5/R6 security, R3 ladder)
	$(CARGO) test --workspace

specs: ## validate the spec system (structure, index, links, coverage, paths)
	$(PYTHON) specs/tools/validate_specs.py

run-debug: ## run the omnibusd daemon in the debug profile
	$(CARGO) run --bin omnibusd

clean: ## remove build artifacts
	$(CARGO) clean

help: ## list targets
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk -F':.*## ' '{printf "  %-10s %s\n", $$1, $$2}'
