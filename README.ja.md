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

### GitHub Releases から（推奨）

[Releases](https://github.com/owa/git-smart-commit/releases) からお使いのプラットフォーム用のバイナリをダウンロードしてください。

#### macOS (Apple Silicon)
```bash
curl -L https://github.com/owa/git-smart-commit/releases/latest/download/git-sc-aarch64-apple-darwin.tar.gz | tar xz
sudo mv git-sc /usr/local/bin/
```

#### macOS (Intel)
```bash
curl -L https://github.com/owa/git-smart-commit/releases/latest/download/git-sc-x86_64-apple-darwin.tar.gz | tar xz
sudo mv git-sc /usr/local/bin/
```

#### Linux (x86_64)
```bash
curl -L https://github.com/owa/git-smart-commit/releases/latest/download/git-sc-x86_64-unknown-linux-gnu.tar.gz | tar xz
sudo mv git-sc /usr/local/bin/
```

#### Linux (ARM64)
```bash
curl -L https://github.com/owa/git-smart-commit/releases/latest/download/git-sc-aarch64-unknown-linux-gnu.tar.gz | tar xz
sudo mv git-sc /usr/local/bin/
```

#### Windows

[Releases](https://github.com/owa/git-smart-commit/releases) から `git-sc-x86_64-pc-windows-msvc.zip` をダウンロードし、展開して PATH に追加してください。

### ソースから

```bash
# リポジトリをクローン
git clone https://github.com/owa/git-smart-commit.git
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

# プレフィックススクリプト設定（オプション）
# リモートURLに基づいて外部スクリプトを実行し、コミットメッセージのプレフィックスを生成
[[prefix_scripts]]
url_pattern = "https://gitlab.example.com/"
script = "/path/to/prefix-generate.py"

# プレフィックスルール設定（オプション）
# リモートURLに基づいてプレフィックス形式を指定
[[prefix_rules]]
url_pattern = "https://github.com/myorg/"
prefix_type = "conventional"  # conventional, bracket, colon, emoji, plain, none, またはカスタム形式
```

### 設定オプション

| オプション | 説明 | デフォルト |
|-----------|------|-----------|
| `providers` | AI プロバイダーの優先順序 | `["gemini", "codex", "claude"]` |
| `language` | コミットメッセージの言語 | `"Japanese"` |
| `models.gemini` | Gemini CLI のモデル | `"flash"` |
| `models.codex` | Codex CLI のモデル | `"gpt-5.1-codex-mini"` |
| `models.claude` | Claude CLI のモデル | `"haiku"` |
| `prefix_scripts` | プレフィックス生成用外部スクリプト | `[]` |
| `prefix_rules` | URLベースのプレフィックス形式設定 | `[]` |

### プレフィックス判定の優先順位

1. **prefix_scripts**（最優先）: `url_pattern` に前方一致すればスクリプトを実行
2. **prefix_rules**: `url_pattern` に前方一致すれば指定された `prefix_type` を使用
3. **Auto**（デフォルト）: 過去5件のコミットから形式を自動判定

### プレフィックスルール

URLベースでプレフィックス形式を指定できます。

| prefix_type | 動作 |
|-------------|------|
| `conventional` | Conventional Commits形式（feat:, fix:, docs:, etc.） |
| `bracket` | ブラケット形式（[Add], [Fix], [Update], etc.） |
| `colon` | コロン形式（Add:, Fix:, Update:, etc.） |
| `emoji` | 絵文字形式（✨, 🐛, 📝, etc.） |
| `plain` | プレフィックスなし（本文のみ） |
| `none` | プレフィックスなし（本文のみ） |
| その他 | カスタム形式として指定 |

### プレフィックススクリプト

リモートURLに基づいてコミットメッセージのプレフィックスを生成する外部スクリプトを設定できます。リモートURLが指定した `url_pattern` で始まる場合、スクリプトがリモートURLとブランチ名を引数として実行されます。

**スクリプトの終了コードによる動作：**

| 終了コード | 出力 | 動作 |
|-----------|------|------|
| `0` | 内容あり | 出力をカスタムプレフィックスとして使用 |
| `0` | 空 | プレフィックスなし（本文のみ） |
| `1` | - | AI生成のメッセージをそのまま使用 |

スクリプト呼び出し例：
```bash
/path/to/prefix-generate.py "git@example.com:org/repo.git" "feature/my-branch"
```

スクリプト例（疑似コード）：
```bash
#!/bin/bash
# ブランチ名や外部APIからプレフィックスを生成
PREFIX=$(generate_prefix "$1" "$2")
if [ -n "$PREFIX" ]; then
    echo -n "$PREFIX"
    exit 0
else
    exit 1  # AI生成フォーマットを使用
fi
```

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
| `make tag-release` | GitHub Actions リリース用の git タグを作成 |
| `make tag-release-push` | タグを作成してプッシュしリリースをトリガー |
| `make test` | テスト実行 |
| `make fmt` | コードフォーマット |
| `make check` | clippy と check を実行 |
| `make clean` | ビルド成果物をクリーン |
| `make help` | 利用可能なコマンド一覧表示 |

## 使い方

```bash
# ステージされた変更のコミットメッセージを生成
git-sc

# アンステージの変更も含めて全てをステージしてコミット
git-sc -a

# 確認プロンプトなしでメッセージ生成
git-sc -y

# コミットせずにメッセージをプレビュー（ドライラン）
git-sc -n

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
| `--all` | `-a` | アンステージの変更も含めて全てをステージしてコミット |
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

## Claude Code との連携

Claude Code の Hooks 機能を使用して、セッション終了時に自動でコミットメッセージを生成できます。

`~/.claude/settings.json` に以下を追加：

```json
{
  "hooks": {
    "Stop": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "git-sc --all --yes"
          }
        ]
      }
    ]
  }
}
```

これにより、Claude Code のセッション終了時に変更があれば自動的にコミットされます。変更がない場合は正常終了します。

## ライセンス

MIT
