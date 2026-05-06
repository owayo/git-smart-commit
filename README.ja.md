<p align="center">
  <img src="docs/images/app.png" width="128" alt="git-sc">
</p>

<h1 align="center">git-sc</h1>

<p align="center">
  AIコーディングエージェントによるスマートコミットメッセージ生成CLI
</p>

<p align="center">
  <a href="https://github.com/owayo/git-smart-commit/actions/workflows/release.yml">
    <img alt="Release" src="https://github.com/owayo/git-smart-commit/actions/workflows/release.yml/badge.svg?branch=main">
  </a>
  <a href="https://github.com/owayo/git-smart-commit/actions/workflows/ci.yml">
    <img alt="CI" src="https://github.com/owayo/git-smart-commit/actions/workflows/ci.yml/badge.svg?branch=main">
  </a>
  <a href="https://github.com/owayo/git-smart-commit/releases/latest">
    <img alt="Version" src="https://img.shields.io/github/v/release/owayo/git-smart-commit">
  </a>
  <a href="LICENSE">
    <img alt="License" src="https://img.shields.io/github/license/owayo/git-smart-commit">
  </a>
</p>

<p align="center">
  <a href="README.md">English</a> |
  <a href="README.ja.md">日本語</a>
</p>

---

## 特徴

- **マルチプロバイダー対応**: Gemini CLI、Codex CLI、Claude Code、opencode、Apple Intelligence を自動フォールバック付きでサポート
- **スマートクールダウン**: 失敗したプロバイダーを1時間（設定可能）優先度を下げて連続失敗を回避
- **フォーマット自動検出**: 過去のコミットから形式を自動判断（Conventional、Bracket、Emoji等）
- **空リポジトリ対応**: コミットがまだないリポジトリでも、Git のロケールに依存せず自動判定を安全にフォールバック
- **インタラクティブ**: コミット前に確認プロンプト表示（`-y` でスキップ可能）
- **ドライラン**: コミットせずにメッセージをプレビュー（`-n`）
- **Quietモード**: フック/スクリプト向けに進捗出力を抑制（`-q`）
- **本文サポート**: 箇条書き本文付きの詳細なコミットメッセージを生成（`-b`）
- **Amend/Squash/Reword**: 既存コミットのメッセージを再生成
- **エージェントコンテキスト**: [claw-hooks](https://github.com/owayo/claw-hooks) と連携し、エージェントの意図を反映したコンテキストを考慮したメッセージを生成

## 動作要件

- **OS**: macOS, Linux, Windows
- **Git**: 必須
- **AIプロバイダー**（少なくとも1つ）:
  - Gemini CLI: `npm install -g @google/gemini-cli`
  - Codex CLI: `npm install -g @openai/codex`
  - Claude Code: `curl -fsSL https://claude.ai/install.sh | bash`
  - opencode: `curl -fsSL https://opencode.ai/install | bash`
  - Apple Intelligence: macOS版に内蔵（macOS 26+、Apple Silicon必須）

## インストール

### Homebrew (macOS/Linux)

```bash
brew install owayo/git-sc/git-sc
```

### ソースから

```bash
git clone https://github.com/owayo/git-smart-commit.git
cd git-smart-commit
make install
```

### GitHub Releases から

[Releases](https://github.com/owayo/git-smart-commit/releases) からお使いのプラットフォーム用のバイナリをダウンロード。

#### macOS (Apple Silicon)

```bash
curl -L https://github.com/owayo/git-smart-commit/releases/latest/download/git-sc-aarch64-apple-darwin.tar.gz | tar xz
sudo mv git-sc /usr/local/bin/
```

#### macOS (Intel)

```bash
curl -L https://github.com/owayo/git-smart-commit/releases/latest/download/git-sc-x86_64-apple-darwin.tar.gz | tar xz
sudo mv git-sc /usr/local/bin/
```

#### Linux (x86_64)

```bash
curl -L https://github.com/owayo/git-smart-commit/releases/latest/download/git-sc-x86_64-unknown-linux-gnu.tar.gz | tar xz
sudo mv git-sc /usr/local/bin/
```

#### Linux (ARM64)

```bash
curl -L https://github.com/owayo/git-smart-commit/releases/latest/download/git-sc-aarch64-unknown-linux-gnu.tar.gz | tar xz
sudo mv git-sc /usr/local/bin/
```

#### Windows

[Releases](https://github.com/owayo/git-smart-commit/releases) から `git-sc-x86_64-pc-windows-msvc.zip` をダウンロードし、展開して PATH に追加。

## クイックスタート

```bash
# ステージされた変更のコミットメッセージを生成
git-sc

# 全ステージして確認なしでコミット
git-sc -a -y

# メッセージをプレビュー（ドライラン）
git-sc -n
```

## 使い方

### コマンド

| コマンド | 説明 |
|---------|------|
| `git-sc` | ステージされた変更のメッセージを生成 |
| `git-sc init` | 設定ファイルを初期化 |
| `git-sc -a` | 全ての変更をステージしてメッセージ生成 |
| `git-sc --amend` | 直前のコミットメッセージを再生成 |
| `git-sc --squash <BASE>` | 全コミットを1つにまとめる |
| `git-sc --reword <HASH>` | 特定コミットのメッセージを再生成 |
| `git-sc -g <HASH>` | 既存コミットからメッセージ生成（出力のみ） |

### オプション

#### 基本オプション

| オプション | 短縮 | 説明 |
|-----------|------|------|
| `--yes` | `-y` | 確認プロンプトをスキップ |
| `--dry-run` | `-n` | コミットせずにメッセージを表示 |
| `--all` | `-a` | 全ての変更をステージ |
| `--body` | `-b` | 箇条書き本文付きで生成 |

#### 操作モード

| オプション | 短縮 | 説明 |
|-----------|------|------|
| `--amend` | | 直前のコミットメッセージを再生成 |
| `--squash` | | 全コミットを1つにまとめる |
| `--reword` | | 特定コミットのメッセージを再生成 |
| `--generate-for` | `-g` | コミットdiffからメッセージ生成（出力のみ） |

操作モード（`--amend`, `--squash`, `--reword`, `--generate-for`）は同時に指定できません。複数指定した場合は、どれか1つを暗黙に選ばず、引数パース時点でエラーになります。

`--amend` の注意:
- 現在の `HEAD` が最初のコミットでも動作します。
- 無関係な staged 変更は amend 対象のコミットに混ぜず、そのまま staged として保持します。

`--reword` の注意:
- 現在の `HEAD` 履歴に含まれるコミットのみ指定できます。
- 別ブランチなど現在の履歴外ハッシュを指定すると「無効なreword対象です」エラーで失敗します。
- 対象コミット自身が merge commit の場合も、`reword` 対象として拒否されます。
- 現在の履歴で最古のコミットも reword できます（必要時は内部で `git rebase -i --root` を使用）。
- `HEAD` を reword する場合も、無関係な staged 変更は書き換え後のコミットに混ぜず、そのまま staged として保持します。

`--squash` の注意:
- 無関係な staged 変更が既にある場合、履歴を書き換える前にエラーで停止します。先に commit、unstage、または stash してください。

#### 設定

| オプション | 短縮 | 説明 |
|-----------|------|------|
| `--provider` | `-p` | AIプロバイダーを指定 (gemini, codex, claude, opencode, apple-intelligence) |
| `--lang` | `-l` | コミットメッセージの言語を上書き |

#### デバッグ・情報

| オプション | 短縮 | 説明 |
|-----------|------|------|
| `--quiet` | `-q` | 進捗メッセージを抑制 |
| `--debug` | `-d` | AIに渡すプロンプトを表示 |
| `--help` | `-h` | ヘルプを表示 |
| `--version` | `-V` | バージョンを表示 |

`--quiet` の動作:
- 通常実行 / amend / squash / reword の進捗・プレビュー・成功/キャンセル表示を抑制
- エラー出力はそのまま表示
- `--generate-for` はパイプ処理向けに生成メッセージのみを標準出力に出力

### 使用例

```bash
# 基本的な使い方
git-sc                      # ステージされた変更のメッセージ生成
git-sc -a -y                # 全ステージして直接コミット

# プレビューと本文
git-sc -n                   # ドライラン（プレビューのみ）
git-sc -b                   # 詳細な本文付き

# Amend と Squash
git-sc --amend              # 直前のコミットメッセージを再生成
git-sc --squash origin/main # フィーチャーブランチのコミットをまとめる

# 既存コミットから生成
git-sc -g abc1234           # コミットdiffからメッセージ生成
git-sc -g abc1234 -b        # 詳細な本文付き
```

## 設定

### 初期設定

`git-sc init` で設定ファイルを初期化するか、`~/.config/git-sc/config.toml` を手動で作成します:

```bash
git-sc init
```

これにより `~/.config/git-sc/config.toml` にデフォルト設定の設定ファイルが作成されます。

既存の設定を上書きするには `--force` を使用します:

```bash
git-sc init --force
```

### 階層的設定

git-sc はプロジェクトレベルでの上書きが可能な階層的設定をサポートしています:

| ファイル | スコープ | 説明 |
|---------|---------|------|
| `~/.config/git-sc/config.toml` | グローバル | ユーザー全体のデフォルト設定 |
| `.git-sc` | プロジェクト | リポジトリ固有の上書き設定（リポジトリルートに配置） |

プロジェクト設定はグローバル設定を上書きします。プロジェクト設定で指定されていないフィールドはグローバル設定から継承されます。上書きしたいフィールドのみ指定できます — `[models]` セクションの部分指定もサポートしています。

### 設定例

```toml
# AIプロバイダーの優先順位
providers = ["opencode", "gemini", "codex", "claude", "apple-intelligence"]

# コミットメッセージの言語
language = "Japanese"

# コミットプレフィックス形式（オプション）
# 値: conventional, bracket, colon, emoji, plain, none
prefix_type = "conventional"

# コミット後に自動プッシュ（オプション）
auto_push = true

# Codex 呼び出し時に `-c model_reasoning_effort=<値>` として渡す推論深度
# 値: "low"（デフォルト）/ "medium" / "high" / "xhigh" / "" (codex 既定動作を使う場合は空文字列)
codex_reasoning_effort = "low"

# モデル設定
[models]
gemini = "gemini-2.5-flash-lite"
codex = "gpt-5.3-codex-spark"
claude = "haiku"
opencode = ""

# プロバイダークールダウン（分）
provider_cooldown_minutes = 60

# プロバイダータイムアウト（秒）
provider_timeout_seconds = 60
```

### 設定オプション

| オプション | 説明 | デフォルト |
|-----------|------|-----------|
| `providers` | AIプロバイダーの優先順位 | `["opencode", "gemini", "codex", "claude", "apple-intelligence"]` |
| `language` | コミットメッセージの言語 | `"Japanese"` |
| `prefix_type` | コミットプレフィックス形式 | 自動検出 |
| `auto_push` | コミット後に自動プッシュ | `false` |
| `codex_reasoning_effort` | Codex の `-c model_reasoning_effort` に渡す値（`low`, `medium`, `high`, `xhigh`, 空文字列で省略） | `"low"` |
| `models.*` | 各プロバイダーのモデル | 設定参照 |
| `provider_cooldown_minutes` | 失敗プロバイダーのクールダウン | `60` |
| `provider_timeout_seconds` | プロバイダー呼び出しのタイムアウト | `60` |
| `prefix_rules` | URLベースのプレフィックス形式 | `[]` |
| `prefix_scripts` | 外部プレフィックススクリプト | `[]` |

既存のグローバル設定ファイルは自動では書き換えられません。現在の Codex 既定モデルを使うには、`~/.config/git-sc/config.toml` の `models.codex` を更新してください。

### prefix_type の値

| 値 | 例 | 説明 |
|----|-----|------|
| `conventional` | `feat: add feature` | Conventional Commits 形式 |
| `bracket` | `[feat] add feature` | ブラケット形式 |
| `colon` | `feat: add feature` | シンプルなコロン形式 |
| `emoji` | `:sparkles: add feature` | 絵文字形式 |
| `plain` | `Add feature` | プレフィックスなし |
| `none` | `add feature` | プレフィックスなし、小文字 |

### プレフィックスルール

リモートURLでコミット形式を指定:

```toml
[[prefix_rules]]
url_pattern = "github\\.com[:/]myorg/"
prefix_type = "conventional"  # conventional, bracket, colon, emoji, plain
```

### プレフィックススクリプト

外部スクリプトでカスタムプレフィックスを生成:

```toml
[[prefix_scripts]]
url_pattern = "^https://gitlab\\.example\\.com/"
script = "/path/to/prefix-generate.py"
```

プレフィックススクリプトが有効な `prefix_type` 名（`conventional`, `bracket`, `emoji` 等）を返した場合、リテラルなプレフィックス文字列ではなくルールモードとして解釈されます。これにより、ブランチ名やリモートURLに応じてコミットフォーマットを動的に切り替えることができます。

プレフィックススクリプトが空文字を返した場合（exit `0` かつ標準出力なし）、git-sc は生成メッセージをそのまま使います。ただし先頭が Conventional Commits の type プレフィックス（例: `feat:`, `fix(scope):`, `feat!:`）の場合のみ、そのプレフィックスを除去します。

プレフィックススクリプトが終了コード `1` で終了した場合、git-sc はプレフィックスを追加せず AI 生成メッセージをそのまま使います。それ以外の非 0 終了コードはスクリプト実行失敗として扱い、次に一致するプレフィックススクリプト、プレフィックスルール、設定済みの `prefix_type`、または自動判定へフォールバックします。

プロジェクトレベルの `.git-sc` では、相対 `script` パスは Git リポジトリのルートから解決され、スクリプトの作業ディレクトリも Git ルートになります。

```bash
#!/bin/bash
# 例: "conventional" を返すと Conventional Commits 形式が適用される
echo "conventional"
```

## 差分の処理

- 空白のみの変更は除外
- バイナリファイルは除外
- スペースや非 ASCII 文字を含むパスの引用付き diff ヘッダーも正しく解析
- `.git-sc-ignore` パターンを適用
- 10,000文字で切り詰め

### .git-sc-ignore

Git が日本語ファイル名などを quoted path としてエスケープしていても、復元後の実パスに対してパターン照合します。
rename diff では変更前と変更後の両パスを対象に照合するため、無視対象ディレクトリへの移動も一貫して除外されます。
ファイル名にスペースが含まれている場合も対応しています。Git はスペースだけを含むファイル名をクォートせずに `diff --git` ヘッダーへ出力しますが、`git-sc` は正しいパスを抽出するため、無視パターンが一貫して適用されます。
変更前後のパスが異なり、両方にスペースを含む rename ヘッダーも同じように処理します。

```gitignore
package-lock.json
yarn.lock
Cargo.lock
*.generated.ts
```

### 自動プッシュ

設定ファイルで自動プッシュを有効にできます:

```toml
# ~/.config/git-sc/config.toml または .git-sc に記述
auto_push = true
```

有効にすると、`git-sc` はコミットまたは squash 成功後に `git push` を実行します。

## VS Code 拡張機能

**[Git-SC (Smart Commit)](https://marketplace.visualstudio.com/items?itemName=owayo.vscode-git-smart-commit)** - VS Code マーケットプレイスで公開中

## Claude Code との連携

`~/.claude/settings.json` に追加:

```json
{
  "hooks": {
    "Stop": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "git-sc --all --yes --quiet"
          }
        ]
      }
    ]
  }
}
```

## エージェントコンテキスト（claw-hooks 連携）

git-sc を [claw-hooks](https://github.com/owayo/claw-hooks) と併用すると、コーディングエージェントが何を行っていたかのコンテキストが `CLAW_HOOKS_AGENT_MESSAGE` 環境変数経由で自動的に渡されます。このコンテキストはAIプロンプトに含まれ、生の差分の説明ではなく、変更の意図を反映したコミットメッセージの生成を可能にします。

**claw-hooks** は Claude Code のフックライフサイクルを管理するコンパニオンツールです。stop hook が発火する際、エージェントの最後のアクティビティサマリーを `CLAW_HOOKS_AGENT_MESSAGE` にセットしてから git-sc を呼び出します。

```bash
# claw-hooks の stop hook が自動的にセットします
# CLAW_HOOKS_AGENT_MESSAGE="認証モジュールをJWTトークン方式にリファクタリング"
git-sc -a -y -q
```

環境変数がセットされている場合、プロンプトに「Agent Context」セクションが追加され、AIが開発者の意図を優先するようガイドされます。
これは通常のコミット生成に加えて、`--amend`、`--reword`、`--squash`、`--generate-for` でも適用されます。

## 動作の仕組み

```mermaid
flowchart LR
    A[変更をステージ] --> B[差分取得]
    B --> C[フォーマット検出]
    C --> D[AIで生成]
    D --> E[確認してコミット]
```

1. **環境確認**: gitリポジトリとAIエージェントの利用可否を確認
2. **設定読み込み**: `~/.config/git-sc/config.toml` から設定を読み込み
3. **差分取得**: ステージされた変更を取得（除外設定適用）
4. **フォーマット検出**: 過去のコミットまたはルールから検出
5. **生成**: AIに送信（フォールバック付き）
6. **コミット**: 確認してコミットを作成

## Apple Intelligence

Apple Intelligence プロバイダーは、[fm-rs](https://github.com/blacktop/fm-rs)（Appleの [Foundation Models](https://developer.apple.com/documentation/foundationmodels) フレームワークのRustバインディング）を使用し、完全オンデバイス推論を行います。APIキーやネットワーク接続は不要です。

- **動作要件**: macOS 26（Tahoe）以降、Apple Silicon、システム設定でApple Intelligenceが有効であること
- **仕組み**: Apple Intelligence を有効化した状態で実行すると（macOSではデフォルト）、git-scがfm-rs経由でFoundation Modelsを直接呼び出します。コミットメッセージ生成用のinstructionsを設定した `LanguageModelSession` を毎回作成します
- **ビルド**: `cargo build --features apple-ai`（macOSでは `make build` / `make install` で自動的に有効）
- **クロスプラットフォーム**: Linux/WindowsではApple Intelligenceは利用できず、自動的にスキップされます

## ビルドコマンド

| コマンド | 説明 |
|---------|------|
| `make build` | デバッグビルド |
| `make release` | リリースビルド |
| `make install` | ビルドして /usr/local/bin にインストール |
| `make test` | テスト実行 |
| `make fmt` | コードフォーマット |
| `make check` | clippy と cargo check を実行（macOS では `apple-ai` を含む） |
| `make clean` | ビルド成果物をクリーン |

## コントリビュート

コントリビュートを歓迎します！お気軽にプルリクエストをお送りください。

## 変更履歴

バージョン履歴は [Releases](https://github.com/owayo/git-smart-commit/releases) を参照してください。

## ライセンス

[MIT](LICENSE)
