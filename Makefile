.PHONY: build release release-major release-minor install clean test fmt check help tag-release

# Default target
.DEFAULT_GOAL := help

# Variables
BINARY_NAME := git-sc
INSTALL_PATH := /usr/local/bin

## Build Commands

build: ## Build debug version (no version bump)
	cargo build

release: ## Build release version (no version bump)
	cargo build --release

release-patch: ## Bump patch version and build release (0.1.0 -> 0.1.1)
	./scripts/bump-version.sh patch
	cargo build --release

release-minor: ## Bump minor version and build release (0.1.0 -> 0.2.0)
	./scripts/bump-version.sh minor
	cargo build --release

release-major: ## Bump major version and build release (0.1.0 -> 1.0.0)
	./scripts/bump-version.sh major
	cargo build --release

## Installation

install: release ## Build release and install to /usr/local/bin
	cp target/release/$(BINARY_NAME) $(INSTALL_PATH)/

install-release: release-patch install ## Bump version, build, and install

## Release (GitHub Actions)

tag-release: ## Create a git tag for release (triggers GitHub Actions build)
	@VERSION=$$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/'); \
	echo "Creating tag v$$VERSION..."; \
	git tag -a "v$$VERSION" -m "Release v$$VERSION"; \
	echo "Tag created. Push with: git push origin v$$VERSION"

tag-release-push: ## Commit version files, create tag, and push for release
	@VERSION=$$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/'); \
	echo "Committing Cargo.toml and Cargo.lock for v$$VERSION..."; \
	git add Cargo.toml Cargo.lock; \
	git commit -m "Release v$$VERSION" || echo "Nothing to commit"; \
	echo "Creating and pushing tag v$$VERSION..."; \
	git tag -a "v$$VERSION" -m "Release v$$VERSION"; \
	git push origin HEAD && git push origin "v$$VERSION"

## Development

test: ## Run tests
	cargo test

fmt: ## Format code
	cargo fmt

check: ## Run clippy and check
	cargo clippy -- -D warnings
	cargo check

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
