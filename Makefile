# git-sc の開発タスク。引数なしの make でターゲット一覧を表示する。
#
# mise.toml の固定バージョンを mise exec -- 経由で使う。
# IDE や GUI など、シェルで mise を有効にしていない環境にも適用する。
# SYSTEM_TOOLS=1 では PATH 上のツールを使うため、バージョンは保証しない。
#
# macOS の build / release / install / lint では Apple Intelligence を有効にする。
# apple-ai は fm-rs 経由で Swift をコンパイルするため、macOS 26 以降の SDK を備えた
# Xcode が必要。リリースワークフローの配布バイナリでは有効にしない。
#
# macOS 付属の GNU Make 3.81 に合わせ、.ONESHELL / .SHELLFLAGS / $(file ...) / != は使わない。

.DEFAULT_GOAL := help

BINARY_NAME := git-sc
INSTALL_PATH ?= /usr/local/bin
# CI と同じ Cargo.lock で依存を解決する。
CARGO_FLAGS ?= --locked
UNAME_S := $(shell uname -s)

# Apple Intelligence（fm-rs）は macOS のみ対応。
ifeq ($(UNAME_S),Darwin)
CARGO_FEATURES := --features apple-ai
else
CARGO_FEATURES :=
endif

# ---- ツールチェーン ---------------------------------------------------------------
# PATH、一般的なインストール先の順で mise を探す。GUI 起動ではシェルの PATH を
# 継承しない場合がある。make MISE=/path/to/mise で上書きできる。
# mise がない場合を検証するときは MISE_CANDIDATES= で候補を空にする。
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

## 準備

setup: ## Install the toolchain (mise) and dependencies
	@if [ -n "$(MISE)" ]; then "$(MISE)" install; fi
	$(RUN) cargo fetch $(CARGO_FLAGS)

## ビルド

build: ## Build a debug binary (with Apple Intelligence on macOS)
	$(RUN) cargo build $(CARGO_FLAGS) $(CARGO_FEATURES)

release: ## Build a release binary (with Apple Intelligence on macOS)
	$(RUN) cargo build --release $(CARGO_FLAGS) $(CARGO_FEATURES)

## 検証

test: ## Run the tests
	$(RUN) cargo test $(CARGO_FLAGS)

# Linux CI と macOS 配布版は apple-ai を使わないため、macOS でも両構成を検査する。
# ai/apple.rs だけが構築するエラー型などは、機能を外すと未使用になる。
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

## インストール

# 同じディレクトリの一時ファイルから rename し、実行ファイルの inode を置き換える。
# macOS は署名検証を inode ごとにキャッシュするため、直接上書きすると起動直後に
# SIGKILL（exit 137）になる場合がある。リンカーの ad hoc 署名を保持し、再署名しない。
# 固定の識別子による再署名では、バージョン間で権限を維持できない。
# コピー失敗時は trap で一時ファイルを削除する。
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

## ヘルプ

help: ## Show this help
	@echo "Development tasks for $(BINARY_NAME)"
	@echo ""
	@echo "Usage: make <target>"
	@echo ""
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}'
	@echo ""
	@echo "Tool versions are pinned in mise.toml. Run make setup first."
	@echo "Release: GitHub Actions > Release > Run workflow"
