.PHONY: build release install clean test fmt check help

# Default target
.DEFAULT_GOAL := help

# Variables
BINARY_NAME := git-sc
INSTALL_PATH := /usr/local/bin
UNAME_S := $(shell uname -s)

# macOS では Apple Intelligence (fm-rs) を有効化
ifeq ($(UNAME_S),Darwin)
CARGO_FEATURES := --features apple-ai
else
CARGO_FEATURES :=
endif

## Build Commands

build: ## Build debug version
	cargo build $(CARGO_FEATURES)

release: ## Build release version
	cargo build --release $(CARGO_FEATURES)

## Installation

install: release ## Build release and install to /usr/local/bin
	cp target/release/$(BINARY_NAME) $(INSTALL_PATH)/
ifeq ($(UNAME_S),Darwin)
	codesign --force --sign - $(INSTALL_PATH)/$(BINARY_NAME)
endif

## Development

test: ## Run tests
	cargo test

fmt: ## Format code
	cargo fmt

check: ## Run clippy and check
	cargo clippy $(CARGO_FEATURES) -- -D warnings
	cargo check $(CARGO_FEATURES)

clean: ## Clean build artifacts
	cargo clean

## Help

help: ## Show this help message
	@echo "git-sc Build Commands"
	@echo ""
	@echo "Usage: make [target]"
	@echo ""
	@echo "Targets:"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-20s\033[0m %s\n", $$1, $$2}'
	@echo ""
	@echo "Release:"
	@echo "  Use GitHub Actions > Release > Run workflow"
