# git-sc

AI CLI（Gemini、Codex、Claude）を使用したスマートコミットメッセージ生成ツール

`git-sc` はステージされた変更と過去のコミット履歴を分析し、AI CLI ツールを使って文脈に適したコミットメッセージを自動フォールバック付きで生成します。

## 特徴

- **マルチプロバイダー対応**: Gemini、Codex、Claude CLI を自動フォールバック付きでサポート
- **設定可能**: `~/.git-sc` でプロバイダー優先度、言語、モデルをカスタマイズ
- **フォーマット自動検出**: 過去のコミットメッセージから形式を自動判断
  - Conventional Commits (`feat:`, `fix:`, `docs:` など)
  - ブラケット形式 (`[Add]`, `[Fix]`, `[Update]` など)
  - コロン形式 (`Add:`, `Fix:`, `Update:` など)
  - 絵文字形式
  - プレーン形式
- **インタラクティブ**: コミット前に確認プロンプト表示（`-y` でスキップ可能）
- **ドライラン**: コミットせずに生成メッセージをプレビュー
- **Amend サポート**: `--amend` で直前のコミットメッセージを再生成

## 前提条件

以下の AI CLI ツールのうち、少なくとも1つがインストールされている必要があります：

- **Gemini CLI**: `npm install -g @google/gemini-cli`
- **Codex CLI**: `npm install -g @openai/codex`
- **Claude CLI**: `npm install -g @anthropic-ai/claude-code`

## インストール

### ソースから

```bash
# リポジトリをクローン
git clone https://github.com/yourusername/git-smart-commit.git
cd git-smart-commit

# ビルドしてインストール
make install
```

## 設定

初回実行時に `git-sc` は `~/.git-sc` に設定ファイルを作成します：

```toml
# AI プロバイダーの優先順序（最初に利用可能なものが使用される）
providers = [
    "gemini",
    "codex",
    "claude",
]

# コミットメッセージの言語
language = "Japanese"

# 各プロバイダーのモデル設定
[models]
gemini = "flash"
codex = "gpt-5.1-codex-mini"
claude = "haiku"
```

### 設定オプション

| オプション | 説明 | デフォルト |
|-----------|------|-----------|
| `providers` | AI プロバイダーの優先順序 | `["gemini", "codex", "claude"]` |
| `language` | コミットメッセージの言語 | `"Japanese"` |
| `models.gemini` | Gemini CLI のモデル | `"flash"` |
| `models.codex` | Codex CLI のモデル | `"gpt-5.1-codex-mini"` |
| `models.claude` | Claude CLI のモデル | `"haiku"` |

## ビルドコマンド

| コマンド | 説明 |
|---------|------|
| `make build` | デバッグビルド（バージョン更新なし） |
| `make release` | リリースビルド（バージョン更新なし） |
| `make release-patch` | パッチバージョン更新してビルド (0.1.0 → 0.1.1) |
| `make release-minor` | マイナーバージョン更新してビルド (0.1.0 → 0.2.0) |
| `make release-major` | メジャーバージョン更新してビルド (0.1.0 → 1.0.0) |
| `make install` | リリースビルドして /usr/local/bin にインストール |
| `make install-release` | バージョン更新、ビルド、インストール |
| `make test` | テスト実行 |
| `make fmt` | コードフォーマット |
| `make check` | clippy と check を実行 |
| `make clean` | ビルド成果物をクリーン |
| `make help` | 利用可能なコマンド一覧表示 |

## 使い方

```bash
# ステージされた変更のコミットメッセージを生成
git-sc

# 全変更をステージしてコミットメッセージを生成
git-sc -a

# 確認プロンプトなしでメッセージ生成
git-sc -y

# コミットせずにメッセージをプレビュー（ドライラン）
git-sc -n

# ステージ済みの変更がない場合にアンステージの変更を使用
git-sc -u

# 直前のコミットメッセージを再生成（amend）
git-sc --amend

# 言語設定を上書き
git-sc -l English

# オプションを組み合わせ
git-sc -a -y           # 全ステージして確認なしでコミット
git-sc -a -n           # 全ステージしてメッセージをプレビュー
git-sc --amend -y      # 確認なしで直前のコミットを修正
```

## オプション

| オプション | 短縮 | 説明 |
|-----------|------|------|
| `--yes` | `-y` | 確認プロンプトをスキップして直接コミット |
| `--dry-run` | `-n` | 実際にコミットせず生成メッセージを表示 |
| `--all` | `-a` | コミットメッセージ生成前に全変更をステージ |
| `--unstaged` | `-u` | ステージ済みの変更がない場合にアンステージの変更を含める |
| `--amend` | | 直前のコミットメッセージを再生成 |
| `--lang` | `-l` | 設定ファイルの言語設定を上書き |
| `--help` | `-h` | ヘルプ情報を表示 |
| `--version` | `-V` | バージョン情報を表示 |

## 動作の仕組み

1. **環境確認**: git リポジトリと AI CLI のインストールを確認
2. **設定読み込み**: `~/.git-sc` から設定を読み込み（存在しなければデフォルトを作成）
3. **変更をステージ**: `-a` フラグで任意で全変更をステージ
4. **差分取得**: ステージされた差分内容を取得
5. **フォーマット検出**: 過去のコミットを分析して形式を検出
6. **メッセージ生成**: 差分とフォーマット情報を AI CLI に送信（フォールバック付き）
7. **確認してコミット**: メッセージを表示して確認を求める

## 使用例

### Conventional Commits の場合

過去のコミットが以下の場合:
```
feat: add user authentication
fix(api): resolve rate limiting issue
```

`git-sc` は以下のようなメッセージを生成:
```
feat(auth): implement password reset flow
```

### ブラケット形式の場合

過去のコミットが以下の場合:
```
[Add] new feature
[Fix] bug in auth
```

`git-sc` は以下のようなメッセージを生成:
```
[Update] refactor user service
```

### プロバイダーフォールバック

Gemini CLI が失敗またはインストールされていない場合、`git-sc` は自動的に次のプロバイダーを試行:
```
Using Gemini...
⚠ Gemini failed: API Error
Using Codex...
✓ Commit created successfully!
```

## ライセンス

MIT
