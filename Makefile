# Development tasks for git-sc. Run `make` with no arguments to list the targets.
#
# Tool versions are pinned in mise.toml. When mise is available, every tool runs through
# `mise exec --`, so the pinned versions are used even when mise is not activated in the shell
# (for example when make is started from an IDE or a GUI). SYSTEM_TOOLS=1 uses the tools on PATH
# instead (the versions are then not guaranteed).
#
# On macOS, build, release, install and lint enable the Apple Intelligence provider (the apple-ai
# feature, which compiles Swift through fm-rs and needs Xcode with the macOS 26 SDK or later).
# The release workflow builds the distributed binaries without it.
#
# Only GNU Make 3.81 features are used (the make that ships with macOS):
# no .ONESHELL, .SHELLFLAGS, $(file ...) or !=.

.DEFAULT_GOAL := help

BINARY_NAME := git-sc
INSTALL_PATH ?= /usr/local/bin
# Cargo.lock is committed, so resolve dependencies exactly as CI does
CARGO_FLAGS ?= --locked
UNAME_S := $(shell uname -s)

# Apple Intelligence (fm-rs) is available only on macOS
ifeq ($(UNAME_S),Darwin)
CARGO_FEATURES := --features apple-ai
else
CARGO_FEATURES :=
endif

# ---- Toolchain ------------------------------------------------------------------
# Look for mise on PATH, then in the usual install locations (make started from a GUI may not
# inherit the shell's PATH). Override with make MISE=/path/to/mise.
# To try the behavior without mise, empty the candidates with MISE_CANDIDATES=.
MISE_CANDIDATES ?= $(HOME)/.local/bin/mise /opt/homebrew/bin/mise /usr/local/bin/mise
ifeq ($(SYSTEM_TOOLS),1)
RUN :=
else
ifndef MISE
MISE := $(firstword $(shell command -v mise 2>/dev/null) $(wildcard $(MISE_CANDIDATES)))
endif
ifeq ($(MISE),)
ifneq ($(filter-out help,$(or $(MAKECMDGOALS),help)),)
$(error mise was not found. Install it from https://mise.jdx.dev, or add SYSTEM_TOOLS=1 to use the tools on PATH)
endif
endif
RUN := $(if $(MISE),$(MISE) exec --,)
endif

.PHONY: help setup build release test lint fmt fmt-check check ci install uninstall clean

## Setup

setup: ## Install the toolchain (mise) and dependencies
	@if [ -n "$(MISE)" ]; then "$(MISE)" install; fi
	$(RUN) cargo fetch $(CARGO_FLAGS)

## Build

build: ## Build a debug binary (with Apple Intelligence on macOS)
	$(RUN) cargo build $(CARGO_FLAGS) $(CARGO_FEATURES)

release: ## Build a release binary (with Apple Intelligence on macOS)
	$(RUN) cargo build --release $(CARGO_FLAGS) $(CARGO_FEATURES)

## Checks

test: ## Run the tests
	$(RUN) cargo test $(CARGO_FLAGS)

# Linux CI lints the build without apple-ai, and the distributed macOS binaries are built without
# it too, so on macOS lint checks both builds. Code that only the Apple path uses (such as an error
# variant that only ai/apple.rs constructs) is dead code in the other build.
lint: ## Run clippy with warnings as errors (on macOS, with and without Apple Intelligence)
	$(RUN) cargo clippy $(CARGO_FLAGS) --all-targets -- -D warnings
ifneq ($(CARGO_FEATURES),)
	$(RUN) cargo clippy $(CARGO_FLAGS) $(CARGO_FEATURES) --all-targets -- -D warnings
endif

fmt: ## Format the code (rewrites files)
	$(RUN) cargo fmt --all

fmt-check: ## Check the formatting (no changes)
	$(RUN) cargo fmt --all -- --check

check: fmt-check lint ## Run fmt-check and lint (no changes)

ci: check test ## Run the same checks as CI (no changes)

## Install

# Replace the binary through a temporary file and a rename instead of copying over it. macOS
# caches the code signature check per inode, so a binary copied over one that is running (or ran
# a moment ago) is killed with SIGKILL right after it starts (exit 137). The temporary file sits
# in the same directory so that the rename swaps the inode. The binary is not re-signed: the linker
# already signs it ad hoc, and a fixed identifier would not keep permissions across versions.
# The trap removes the temporary file when copying fails.
install: release ## Install the release binary to INSTALL_PATH (default /usr/local/bin)
	@mkdir -p "$(INSTALL_PATH)"
	@set -eu; \
		temp_path=$$(mktemp "$(INSTALL_PATH)/.$(BINARY_NAME).tmp.XXXXXX"); \
		trap 'rm -f "$$temp_path"' 0 1 2 15; \
		cp "target/release/$(BINARY_NAME)" "$$temp_path"; \
		chmod 0755 "$$temp_path"; \
		mv -f "$$temp_path" "$(INSTALL_PATH)/$(BINARY_NAME)"

uninstall: ## Remove the binary from INSTALL_PATH
	rm -f "$(INSTALL_PATH)/$(BINARY_NAME)"

clean: ## Remove build artifacts
	$(RUN) cargo clean

## Help

help: ## Show this help
	@echo "Development tasks for $(BINARY_NAME)"
	@echo ""
	@echo "Usage: make <target>"
	@echo ""
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}'
	@echo ""
	@echo "Tool versions are pinned in mise.toml. Run make setup first."
	@echo "Release: GitHub Actions > Release > Run workflow"
