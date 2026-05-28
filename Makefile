.PHONY: build release install clean test fmt check help

# デフォルトターゲット
.DEFAULT_GOAL := help

# 変数
BINARY_NAME := git-sc
INSTALL_PATH := /usr/local/bin
UNAME_S := $(shell uname -s)

# macOS では Apple Intelligence (fm-rs) を有効化
ifeq ($(UNAME_S),Darwin)
CARGO_FEATURES := --features apple-ai
else
CARGO_FEATURES :=
endif

## ビルドコマンド

build: ## デバッグビルドを作成
	cargo build $(CARGO_FEATURES)

release: ## リリースビルドを作成
	cargo build --release $(CARGO_FEATURES)

## インストール

install: release ## リリースビルドを作成して /usr/local/bin にインストール
	cp target/release/$(BINARY_NAME) $(INSTALL_PATH)/
ifeq ($(UNAME_S),Darwin)
	codesign --force --sign - $(INSTALL_PATH)/$(BINARY_NAME)
endif

## 開発

test: ## テストを実行
	cargo test

fmt: ## コードをフォーマット
	cargo fmt

check: ## clippy と cargo check を実行
	cargo clippy $(CARGO_FEATURES) -- -D warnings
	cargo check $(CARGO_FEATURES)

clean: ## ビルド成果物を削除
	cargo clean

## ヘルプ

help: ## このヘルプを表示
	@echo "git-sc ビルドコマンド"
	@echo ""
	@echo "Usage: make [target]"
	@echo ""
	@echo "Targets:"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-20s\033[0m %s\n", $$1, $$2}'
	@echo ""
	@echo "リリース:"
	@echo "  GitHub Actions > Release > Run workflow を使用"
