<p align="center">
  <img src="docs/images/app.png" width="128" alt="git-sc">
</p>

<h1 align="center">git-sc</h1>

<p align="center">
  AI コーディングエージェントによるスマートコミットメッセージ生成 CLI
</p>

<!-- standard:badges:start -->
<h3 align="center">対応プラットフォーム</h3>

<p align="center">
  <img src="https://img.shields.io/badge/Linux-FCC624?logo=linux&amp;logoColor=black" alt="Linux">
  <img src="https://img.shields.io/badge/macOS-000000?logo=apple&amp;logoColor=white" alt="macOS">
  <img src="https://img.shields.io/badge/Windows-0078D6" alt="Windows">
</p>

<p align="center">
  <a href="https://github.com/owayo/git-smart-commit/actions/workflows/ci.yml"><img src="https://github.com/owayo/git-smart-commit/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
  <a href="https://github.com/owayo/git-smart-commit/releases/latest"><img src="https://img.shields.io/github/v/release/owayo/git-smart-commit" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/owayo/git-smart-commit" alt="License"></a>
</p>

<p align="center">
  <a href="README.md">English</a> |
  <a href="README.ja.md">日本語</a>
</p>
<!-- standard:badges:end -->

---

git-sc は、ステージした差分とリポジトリで使われているコミット形式を読み取り、手元で使える AI にコミットメッセージを書かせます。使える AI は Antigravity CLI、Codex CLI、Claude Code、opencode、Grok CLI と、端末内で動く Apple Intelligence です。プロバイダーが失敗したり利用枠を使い切ったりしたときは、フォールバックチェーンの次のステップが引き継ぎます。

コーディングエージェントの Stop フックから無人で動かす前提で作っています（`git-sc --all --yes --quiet`）。エージェントが残した作業は、その内容を説明するメッセージ付きでコミットされます。git-sc 自身は API キーを持たず、CLI のプロバイダーはそれぞれの CLI のログインを使います。

## 機能

- **マルチプロバイダー対応**: Antigravity CLI（`agy`、Gemini CLI の後継）、Codex CLI、Claude Code、opencode、Grok CLI、Apple Intelligence を自動フォールバック付きでサポート。同じプロバイダーを異なるモデル・アカウントで複数回並べられる（[プロバイダーのフォールバックチェーン](docs/configuration.ja.md)）
- **スマートクールダウン**: 失敗したステップの優先度を 1 時間（設定可能）下げ、同じステップで失敗を繰り返さないようにする。クールダウンはプロバイダー・モデル・アカウントの組ごとに管理するので、1 つのアカウントやモデルがレート制限に達しても、ほかのステップは巻き添えにならない
- **フォーマット自動検出**: 過去のコミットから形式を自動判断（Conventional、Bracket、Emoji など）
- **空リポジトリ対応**: コミットがまだないリポジトリでも、形式の自動判定はエラーにならず Conventional Commits 形式を使う（Git のロケールに依存しない）
- **インタラクティブ**: コミット前に確認プロンプト表示（`-y` でスキップ可能）
- **ドライラン**: コミットせずにメッセージをプレビュー（`-n`）
- **Quiet モード**: フック/スクリプト向けに進捗出力を抑制（`-q`）
- **本文サポート**: 箇条書き本文付きの詳細なコミットメッセージを生成（`-b`）
- **Amend/Squash/Reword**: 既存コミットのメッセージを再生成
- **安全な一時ファイル**: Unix/macOS では AI プロンプト、Codex の最終応答、reword メッセージの一時ファイルを group/other から読めない権限で作成
- **エージェントコンテキスト**: [claw-hooks](https://github.com/owayo/claw-hooks) と連携し、エージェントが何をしていたかを受け取って、その意図を反映したメッセージを生成

## 動作環境

- **Git**: 必須
- **AI プロバイダー**（少なくとも 1 つ）:
  - Antigravity CLI（`agy`、Gemini CLI の後継）: https://antigravity.google/docs/gcli-migration を参照（旧 Gemini CLI は 2026年6月18日に停止）
  - Codex CLI: `npm install -g @openai/codex`
  - Claude Code: `curl -fsSL https://claude.ai/install.sh | bash`
  - opencode: `curl -fsSL https://opencode.ai/install | bash`
  - Grok CLI（`grok`、xAI）: [cmux](https://github.com/manaflow-ai/cmux) に同梱（`/Applications/cmux.app/Contents/Resources/bin/grok`）。`grok` が `PATH` にない場合、このステップはスキップされて次のプロバイダーへ進みます。
  - Apple Intelligence: macOS 26 以降と Apple Silicon が必要で、ソースからビルドした場合だけ使えます（後述）

プロバイダーごとの起動のしかたと、プラットフォームによる違いは [docs/providers.ja.md](docs/providers.ja.md) にまとめています。

## インストール

<!-- standard:install:start -->
### Homebrew (macOS/Linux)

```bash
brew install owayo/git-sc/git-sc
```

### winget (Windows)

```powershell
winget install owayo.git-sc
```

### Cargo

Rust 1.98 以上が必要です。

```bash
cargo install --git https://github.com/owayo/git-smart-commit --locked
```

### GitHub Releases から

[Releases](https://github.com/owayo/git-smart-commit/releases/latest) から自分の環境のアーカイブを取得して展開し、`git-sc` を `PATH` の通った場所に置きます。各リリースには、取得したファイルを確かめるための `SHA256SUMS` も添付しています。

| プラットフォーム | ファイル |
|---|---|
| Linux (x86_64) | `git-sc-x86_64-unknown-linux-gnu.tar.gz` |
| Linux (ARM64) | `git-sc-aarch64-unknown-linux-gnu.tar.gz` |
| macOS (Intel) | `git-sc-x86_64-apple-darwin.tar.gz` |
| macOS (Apple Silicon) | `git-sc-aarch64-apple-darwin.tar.gz` |
| Windows (x86_64) | `git-sc-x86_64-pc-windows-msvc.zip` |

macOS でブラウザから取得した場合は、実行の前に隔離属性を外します: `xattr -d com.apple.quarantine git-sc`。

### ソースから

[mise](https://mise.jdx.dev/) が必要です (Rust のツールチェーンは `mise.toml` で固定しています)。

```bash
git clone https://github.com/owayo/git-smart-commit.git
cd git-smart-commit
make install
```

`make install` は `/usr/local/bin` に入れます。場所を変えるときは `INSTALL_PATH` を指定します (例: `make install INSTALL_PATH="$HOME/.local/bin"`)。
<!-- standard:install:end -->

winget で入れた後は、新しいターミナルを開いてください。portable パッケージは `PATH` を書き換えるだけなので、起動中のシェルには反映されません。

Homebrew・winget・Cargo・GitHub Releases のバイナリには Apple Intelligence が入っていません。macOS でソースから入れると有効になります。`make install` は `apple-ai` 機能を付けてビルドし、一時ファイルにコピーしてから、インストール済みのバイナリをアトミックに置き換えます。再インストール時に、inode 単位の古いコード署名検証キャッシュが残る問題を防ぐためです。

## クイックスタート

```bash
# 任意: 既定値を書いた ~/.config/git-sc/config.toml を作る (無くても既定値で動く)
git-sc init

# ステージした変更のメッセージを、コミットせずに確かめる
git-sc -n

# メッセージを生成し、確認してコミットする
git-sc
```

## 使い方

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

スクリプトやフックから使うときの注意:

- `--yes` は無人実行に必須です。標準入力が閉じている（スクリプトやフックから呼ばれた）と、確認プロンプトは `[Y/n]` の既定値を採らずにエラーで中止します。
- `--quiet` は進捗の表示を抑え、エラーは残します。`--generate-for` と併用すると標準出力には生成したメッセージだけが出るので、そのままパイプに渡せます。
- `--amend`・`--squash`・`--reword` は履歴を書き換えます。rebase の途中だった、生成中に `HEAD` が動いた、のように作業中にリポジトリの状態が変わったときは、推測で進めずに止まります。

すべてのコマンドとオプション、履歴を書き換える各モードが実行前に確かめる内容は [docs/cli-reference.ja.md](docs/cli-reference.ja.md) を参照してください。

## 設定

設定ファイルが無くても動きます。設定は任意の 2 つのファイルから読み、プロジェクトの設定がグローバルの設定をフィールド単位で上書きします。

| ファイル | スコープ | 説明 |
|---------|---------|------|
| `~/.config/git-sc/config.toml` | グローバル | ユーザー全体の既定の設定（`git-sc init` で作成、`--force` で上書き） |
| `.git-sc` | プロジェクト | リポジトリ固有の上書き設定。Git リポジトリのルートから読む |

```toml
# プロバイダーのフォールバックチェーン (この順に試す)
providers = ["opencode", "grok", "antigravity", "codex", "claude", "apple-intelligence"]
language = "Japanese"            # コミットメッセージの言語
prefix_type = "conventional"     # conventional, bracket, colon, emoji, plain, none (既定: 自動検出)
auto_push = true                 # コミットに成功したら git push する (既定: false)

[models]
antigravity = "GPT-OSS 120B (Medium)"
codex = "gpt-5.6-luna"
claude = "haiku"
opencode = ""                    # "" なら各 CLI の既定モデルに任せる
grok = ""
```

- 既定値が変わっても、既存の設定ファイルは書き換えられません。Codex CLI を更新したら、`models.codex` が Codex で使えるモデルを指しているか確かめてください。削除されたモデルのままだと呼び出しが毎回失敗し、Codex はフォールバックチェーンから黙って外れます。
- プロジェクトの `.git-sc` には、git-sc が起動する実行ファイルを書けます（`providers[].command`、`prefix_scripts[].script`、`ai_usage.command`）。信頼できないリポジトリで git-sc を実行する前に、その内容を確認してください。
- `.git-sc-ignore`（gitignore 形式、リポジトリのルートに置く）のパターンに一致するファイルは、AI に送る差分から除外されます。

設定項目の一覧と、フォールバックチェーン・プレフィックス・残量ゲート・開発者向け生成ログの詳しい設定は [docs/configuration.ja.md](docs/configuration.ja.md) にあります。AI に何を送り、それをどう守っているかは [docs/diff-processing.ja.md](docs/diff-processing.ja.md) で説明しています。

## Claude Code との連携

Claude Code が止まるたびにコミットするには、`~/.claude/settings.json` に次を追加します。

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

[claw-hooks](https://github.com/owayo/claw-hooks) と併用すると、エージェントが直前に行った作業の要約が `CLAW_HOOKS_AGENT_MESSAGE` で git-sc に渡り、差分だけでは読み取れない変更の意図までメッセージに反映されます。VS Code では拡張機能 [Git-SC (Smart Commit)](https://marketplace.visualstudio.com/items?itemName=owayo.vscode-git-smart-commit) から使えます。連携の詳細は [docs/integrations.ja.md](docs/integrations.ja.md) にあります。

## 開発

<!-- standard:dev:start -->
[mise](https://mise.jdx.dev/) が必要です。ツールの版は `mise.toml` で固定しています。

```bash
make setup   # ツールチェーン (mise) と依存を取得する
make ci      # CI と同じ検査 (書き換えない)
```

| コマンド | 説明 |
|---|---|
| `make setup` | ツールチェーン (mise) と依存を取得する |
| `make build` | デバッグ版をビルドする |
| `make release` | リリース版をビルドする |
| `make test` | テストを実行する |
| `make lint` | clippy を警告ゼロで通す |
| `make fmt` | コードを整形する (書き換える) |
| `make fmt-check` | 整形済みかを確かめる (書き換えない) |
| `make check` | 整形と静的検査 (書き換えない) |
| `make ci` | CI と同じ検査 (書き換えない) |
| `make install` | リリース版を INSTALL_PATH (既定 /usr/local/bin) に入れる |
| `make uninstall` | INSTALL_PATH から取り除く |
| `make clean` | ビルド成果物を消す |

`make` でターゲットの一覧を表示します。リリースは GitHub Actions で行います (**Actions → Release → Run workflow**)。
<!-- standard:dev:end -->

macOS の `make build`・`make release`・`make install`・`make lint` は、Apple Intelligence プロバイダーもビルドします。このプロバイダーは [fm-rs](https://github.com/blacktop/fm-rs) 経由で Swift のコードをビルドするので、macOS 26 SDK 以降を含む Xcode が必要です。Linux の CI と配布するバイナリはこの機能なしでビルドするため、`make lint` は機能なしと機能ありの両方で clippy を通します。

## ライセンス

<!-- standard:license:start -->
[MIT](LICENSE)
<!-- standard:license:end -->
