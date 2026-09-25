# 設定

git-sc が読む設定を、プロバイダーのフォールバックチェーンから任意の連携機能まで、すべてまとめています。設定ファイルが無くても git-sc は動きます。短い設定例は [README](../README.ja.md#設定) にあります。

## 初期設定

`git-sc init` で設定ファイルを初期化するか、`~/.config/git-sc/config.toml` を手動で作成します:

```bash
git-sc init
```

これにより、既定値を書き込んだ設定ファイルが `~/.config/git-sc/config.toml` に作成されます。

既存の設定を上書きするには `--force` を使用します:

```bash
git-sc init --force
```

## 階層的設定

git-sc の設定は階層構造になっており、プロジェクト単位で上書きできます:

| ファイル | スコープ | 説明 |
|---------|---------|------|
| `~/.config/git-sc/config.toml` | グローバル | ユーザー全体の既定の設定 |
| `.git-sc` | プロジェクト | リポジトリ固有の上書き設定（リポジトリルートに配置） |

プロジェクト設定はグローバル設定を上書きします。プロジェクト設定に書かれていないフィールドは、グローバル設定の値を引き継ぎます。そのため、上書きしたいフィールドだけを書けば足ります。`[models]` セクションも、一部のキーだけを書けます。

## 設定例

```toml
# AIプロバイダーの優先順位
# "antigravity" は旧 Gemini CLI の後継 (`agy`)。"gemini" と書いても後方互換のため同じプロバイダーとして扱う
providers = ["opencode", "grok", "antigravity", "codex", "claude", "apple-intelligence"]

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
# Antigravity CLI (`agy`) は `--model` に対応。`antigravity` の値はそのまま
# `agy --model "<名前>"` に渡されます。表示名 (例: "GPT-OSS 120B (Medium)"、
# "Gemini 3.5 Flash (Low)") と slug (例: "gpt-oss-120b-medium"、
# "gemini-3.5-flash-low") のどちらでも指定できます。`agy models` がどちらを表示するかは
# agy のバージョンで変わります (1.0.x は表示名、1.1.10 は slug)。未知の名前は
# 非ゼロ終了で明示的に弾かれ、既定モデルへ黙って落ちることはないため、
# 打ち間違いはそのステップの失敗として現れます。
# 空文字列なら `--model` を省略し agy 自身の既定モデルに委ねます。
# 旧 `gemini = "..."` キーは後方互換の入力エイリアスとして受理され、`antigravity` に
# 昇格します(両方指定した場合は `antigravity` が優先)。
# Grok CLI は `-m` に対応。`grok models` が返す ID (例: "grok-4.5") をそのまま指定します。
# 空文字列なら `-m` を省略し grok 自身の既定モデルに委ねます。
[models]
antigravity = "GPT-OSS 120B (Medium)"
codex = "gpt-5.6-luna"
claude = "haiku"
opencode = ""
grok = ""

# プロバイダークールダウン（分）
provider_cooldown_minutes = 60

# プロバイダータイムアウト（秒）
provider_timeout_seconds = 60
```

## 設定オプション

| オプション | 説明 | 既定値 |
|-----------|------|-----------|
| `providers` | プロバイダーのフォールバックチェーン。各要素はプロバイダー名の文字列、または `{provider, model, command, env, name}` テーブル（下記「応用: プロバイダーフォールバックチェーン」を参照。`antigravity` を推奨、`gemini` も後方互換で受理） | `["opencode", "grok", "antigravity", "codex", "claude", "apple-intelligence"]`（`apple-intelligence` は `apple-ai` 機能付きでビルドした macOS 版のみ） |
| `language` | コミットメッセージの言語 | `"Japanese"` |
| `prefix_type` | コミットプレフィックス形式 | 自動検出 |
| `auto_push` | コミット後に自動プッシュ | `false` |
| `codex_reasoning_effort` | Codex の `-c model_reasoning_effort` に渡す値（`low`、`medium`、`high`、`xhigh`、または空文字列で省略） | `"low"` |
| `models.*` | 各プロバイダーのモデル | 設定例を参照 |
| `provider_cooldown_minutes` | 失敗したプロバイダーのクールダウン。極端に大きい値は実質無期限として扱う | `60` |
| `provider_timeout_seconds` | プロバイダー呼び出しのタイムアウト | `60` |
| `prefix_rules` | URL ベースのプレフィックス形式 | `[]` |
| `prefix_scripts` | 外部プレフィックススクリプト | `[]` |
| `ai_usage` | `ai-usage` CLI による残量ゲート（「残量ゲート」を参照） | 無効 |
| `dev_log` | 開発者向け生成ログ（グローバル設定のみ。「開発者向け生成ログ」を参照） | 無効 |

既存のグローバル設定ファイルは自動では書き換えられません。現在の Codex の既定モデルは `gpt-5.6-luna` です。既存の設定で使うには、`~/.config/git-sc/config.toml` の `models.codex` を更新してください。**Codex CLI を更新したら必ず確認してください。** 以前の既定値 `gpt-5.4-mini` は Codex から削除済みです。削除されたモデル名を指定しても、Codex が別のモデルで代わりに応答することはありません。HTTP 400 が返り、git-sc はこれをプロバイダーの失敗として扱ってクールダウンに入れます。そのため、古い `models.codex` が残っていると、Codex は毎回フォールバックチェーンから黙って外れます。この既定値は、API で利用でき、一覧に表示され、推論レベル `medium` に対応する Codex モデルの `input_tokens` を比べて、2026年9月17日（JST）に選び直したものです。計測条件は、空ディレクトリ、プロンプト `Reply ok.`、`--ignore-user-config --ignore-rules --ephemeral --sandbox read-only`、`model_reasoning_effort='medium'` です。結果は `gpt-5.6-luna` = 19609、`gpt-5.5` = 20181、`gpt-5.6-sol` = 21174、`gpt-5.6-terra` = 21174、`gpt-6-astra` = 22035 でした。いずれの試行も最終出力が `ok` でツール呼び出しはなく、2 回目の計測でも全モデルが同じ値を再現しました。

Antigravity（`agy`）の既定モデルは `GPT-OSS 120B (Medium)` で、実測に基づいて選んでいます。`agy` 1.1.10 で print モードに `--output-format json` が加わり、リクエストごとの `usage` を取得できるようになりました。これで Codex と同じ `input_tokens` の比較ができます。それ以前の agy には機械可読な使用量の出力がなく、この既定値は公開価格を根拠にしていました。計測は 2026年8月4日（JST）に `agy` 1.1.10 で行い、空ディレクトリと固定プロンプト `Reply ok.` を使いました。結果は `gpt-oss-120b-medium` = 13680、`gemini-3.5-flash-medium` = 16994、`gemini-3.5-flash-low` = 16998、`gemini-3.1-pro-low` = 17684、`gemini-3.6-flash-low` = 18175、`gemini-3.6-flash-medium` = 18176、`claude-sonnet-4-6` = 19346 でした。いずれも 1 ターンで成功し、`gpt-oss-120b-medium` が次点より約 19% 少なかったため、既定値を維持しています。これは 1 リクエストあたりの最小オーバーヘッドの比較で、実作業での品質を保証するものではありません。既存の設定で使うには、`~/.config/git-sc/config.toml` の `models.antigravity` を追加・更新するか、`""` を指定して agy 自身の既定モデルに任せてください。

プロバイダーを並び替える前に、クールダウン状態に残っている旧エイリアスを正規化します。そのため、`gemini` / `agy` のクールダウンは `antigravity` に、旧 `apple-ai` / `apple_intelligence` のキーは `apple-intelligence` に、引き続き適用されます。`--debug` 付きで実行すると、設定の `providers` に旧 `gemini` エイリアスが残っている場合は、「`antigravity` に正規化される」という注意が 1 回だけ表示されます。

## 応用: プロバイダーフォールバックチェーン（モデル/アカウント/コマンド）

`providers` の各要素は、プロバイダー名のみの文字列に加えて、`model` / `command` / `env` を持つテーブルでも書けます。テーブルを使うと、同じプロバイダーをモデルやアカウントを変えて何度も並べたフォールバックチェーンを組めます。1 つのプロバイダーがモデル系統ごと、またはアカウントや契約ごとにクォータを分けている場合に役立ちます。

```toml
providers = [
  # 同じプロバイダー・別アカウント(env で CODEX_HOME / CLAUDE_CONFIG_DIR を切替)
  { provider = "codex", model = "gpt-5.6-luna", env = { CODEX_HOME = "~/.codex" } },       # アカウント1
  { provider = "codex", model = "gpt-5.6-luna", env = { CODEX_HOME = "~/.codex-work" } },  # アカウント2
  # 同じプロバイダー・別モデル系統(クォータが別)
  { provider = "antigravity", model = "Gemini 3.5 Flash (Low)" },
  { provider = "antigravity", model = "GPT-OSS 120B (Medium)" },
  # 従来どおり文字列(プロバイダー名のみ)も使えます
  "claude",
]
```

ステップごとのフィールド:

| フィールド | 説明 |
|------------|------|
| `provider` | 必須。CLI への引数の渡し方を決めるプロバイダーの種類（`codex` / `antigravity` / `claude` / `opencode` / `grok` / `apple-intelligence`。`gemini` / `agy` はエイリアス）。 |
| `model` | 任意。このステップのモデル。省略時は `[models].<provider>`、それもなければ各 CLI の既定モデルを使う。 |
| `command` | 任意。プロバイダー既定のバイナリの代わりに実行するバイナリ（と固定引数）。ラッパースクリプトなど。`~` は展開される。Codex の `--disable hooks` などの標準引数は引き続き付与される。 |
| `env` | 任意。このステップの起動時に `Command::env()` で明示的に設定する環境変数。値の `~` は展開され、キーは POSIX の変数名でなければならない。動的ローダーやインタープリターの事前ロード系のキー（`LD_PRELOAD`、`DYLD_INSERT_LIBRARIES`、`NODE_OPTIONS`、`PYTHONPATH` など）は、プロジェクト側の `.git-sc` からのコード注入を防ぐため、大文字小文字を区別せずに設定エラーとして拒否する。 |
| `name` | 任意。クールダウンキーとログ表示に使う識別子。省略時は `provider` + `model` + `env` + `command` の組み合わせから決まった規則で導出する。 |

**アカウントの切り替え（推奨: `env`）。** Codex と Claude Code は、`CODEX_HOME` / `CLAUDE_CONFIG_DIR` を見て使うアカウントと認証情報を決めます。これらをステップごとに `env` で設定すると、クォータが別々のアカウントをまたいでフォールバックできます。git-sc は `Command::env()` で明示的に上書きするので、git-sc を起動したシェルで `CODEX_HOME` / `CLAUDE_CONFIG_DIR` が export されていても、起動される CLI はその影響を受けません。（`command` でラッパースクリプトを使う方法もありますが、`env` のほうが明示的で `--debug` にも表示されるため、こちらを推奨します。）

**独立したクールダウン。** クールダウンキーには `provider` + `model` + `env`（と `command`、または明示した `name`）が含まれるため、ステップごとに独立して優先度が下がります。`codex` のアカウント 1 がレート制限に達しても、`codex` のアカウント 2 や、別モデルの `antigravity` は引き続き使えます。

## prefix_type の値

| 値 | 例 | 説明 |
|----|-----|------|
| `conventional` | `feat: add new feature` | Conventional Commits 形式 |
| `bracket` | `[Add] new feature` | ブラケット形式 |
| `colon` | `Add: new feature` | シンプルなコロン形式 |
| `emoji` | `✨ add new feature` | 絵文字形式 |
| `plain` | `Add new feature` | プレフィックスなし |
| `none` | `Add new feature` | プレフィックスなし（`plain` と同じ） |

## プレフィックスルール

リモート URL でコミット形式を指定:

```toml
[[prefix_rules]]
url_pattern = "github\\.com[:/]myorg/"
prefix_type = "conventional"  # conventional, bracket, colon, emoji, plain, none
```

一致したルールの `prefix_type` には、上記の有効な値を書く必要があります。値が無効なルールは警告を出してスキップされ、後続のプレフィックスルール、設定済みの `prefix_type`、自動判定の順に判定が続きます。

## プレフィックススクリプト

外部スクリプトでカスタムプレフィックスを生成:

```toml
[[prefix_scripts]]
url_pattern = "^https://gitlab\\.example\\.com/"
script = "/path/to/prefix-generate.py"
```

プレフィックススクリプトが有効な `prefix_type` の名前（`conventional`、`bracket`、`emoji` など）を返した場合、その出力はプレフィックスの文字列としてではなく、その `prefix_type` を指定したものとして扱われます。そのため、ブランチ名やリモート URL に応じて、スクリプトでコミット形式を切り替えられます。

プレフィックスの文字列をそのまま返す場合は、`echo` などの一般的なスクリプト出力に含まれる末尾の改行（`\n` / `\r\n`）だけを取り除きます。意図して付けた末尾のスペースは残ります。

プレフィックススクリプトが空の出力を返した場合（終了コード `0` で標準出力なし）、git-sc は生成したメッセージをそのまま使います。ただし先頭が Conventional Commits の type プレフィックス（例: `feat:`、`fix(scope):`、`feat!:`）の場合に限り、そのプレフィックスを取り除きます。

プレフィックススクリプトが終了コード `1` で終了した場合、git-sc はプレフィックスを付けず、AI が生成したメッセージをそのまま使います。0 と 1 以外の終了コードはスクリプトの実行失敗として扱い、次に一致するプレフィックススクリプト、プレフィックスルール、設定済みの `prefix_type`、自動判定の順にフォールバックします。

プロジェクトの `.git-sc` に書いた相対パスの `script` は Git リポジトリのルートから解決され、スクリプトの作業ディレクトリも Git のルートになります。

```bash
#!/bin/bash
# 例: "conventional" を返すと Conventional Commits 形式が適用される
echo "conventional"
```

## 自動プッシュ

設定ファイルで自動プッシュを有効にできます:

```toml
# ~/.config/git-sc/config.toml または .git-sc に記述
auto_push = true
```

有効にすると、`git-sc` はコミットまたは squash 成功後に `git push` を実行します。

## 残量ゲート（`ai-usage` 連携）

`ai-usage` CLI がインストールされている場合、残量が尽きかけているアカウントのプロバイダーを、呼び出す前にフォールバックチェーンから外せます。既定では無効です。

```toml
[ai_usage]
enabled = true
command = ["ai-usage", "--json"]  # 省略可（実行ファイルのパスの `~` は展開されます）
threshold_percent = 95            # この使用率以上のステップを除外する
window = "nearest"                # weekly / five_hour / nearest (両者のうち高い方)
timeout_seconds = 10
```

git-sc は起動時にこのコマンドを 1 回だけ実行し、フォールバックチェーンの各ステップを対応するアカウントの使用率と照合します。`threshold_percent` 以上のステップは **その実行に限り** チェーンから外れます（プロバイダーのクールダウン状態には影響しません）。

各ステップがどのアカウントに属するかは、次のフィールドで指定できます。

```toml
[[providers]]
provider = "codex"
ai_usage_profile = "Work"          # ai-usage の `profile` と完全一致 (大文字小文字を区別)
env = { CODEX_HOME = "~/.codex-work" }

[[providers]]
provider = "antigravity"
ai_usage_group = "Claude&GPT"      # `group_label` と一致 (大文字小文字を区別しない)
```

`ai_usage_group` があるのは、1 つのアカウントの枠がモデル系統ごとに分かれている場合があるためです。Antigravity は `Gemini` と `Claude&GPT` という別々の枠を報告し、それぞれ独立に消費されます。`ai_usage_profile` を指定しない場合、git-sc はそのプロバイダーで最も使用率の低いアカウントを基準に判定します。ただしこれは **判定にしか影響しません**。ステップが実際にどのアカウントで動くかは `env` が決めるため、判定と実行を一致させたい場合は `ai_usage_profile` と `env` の両方を指定してください。

失敗時の扱いは意図的に非対称です。コマンドを実行できない・タイムアウトする・出力を解釈できない場合は、チェーンをそのままにしてコミットを続行します（補助ツールの不調でコミットを止めてはならないため）。一方、使用率の取得に成功したうえで **すべてのステップ** が閾値超過だった場合は、既定のチェーンへフォールバックせずエラーで停止します。フォールバックすると、ゲートが今しがた除外したプロバイダーをそのまま呼ぶことになるためです。

プロジェクトの `.git-sc` からはフィールド単位で上書きできます。書かなかったフィールドはグローバル設定の値がそのまま残ります。

## 開発者向け生成ログ

どのプロンプトを渡して何が返ってきたかを記録します。プロンプトを変更したときの効果を、印象ではなく実データで比較するためのものです。既定では無効です。

```toml
# ~/.config/git-sc/config.toml のみ有効（プロジェクトの .git-sc からは有効化できません）
[dev_log]
enabled = true
content = "metadata"   # metadata | full
retention_days = 14
max_total_mb = 500
```

1 回の実行につき 1 つの JSON ファイルを `~/.config/git-sc/logs/YYYY-MM-DD/` に書き出します。記録する内容は、プロンプトのハッシュと差分の統計、各プロバイダーの試行、実行の結末です。試行ごとに整形前の生応答・モデル・所要時間・品質判定と、採用/引き直し/フォールバックのどれだったかを残し、結末にはコミットした場合のハッシュも含めます。実行ごとに別のファイルに書くので、`git-sc` を同時に走らせても記録が混ざりません。また、一時ファイルに書き切ってから rename して公開するため、書きかけのファイルが完成品として解析対象に紛れ込むこともありません。

ファイル群は、そのまま JSONL に流し込んで解析できます:

```bash
find ~/.config/git-sc/logs -name '*.json' | sort | xargs jq -c .

# 例: プロバイダーごとに壊れた件名が出た回数を数える
find ~/.config/git-sc/logs -name '*.json' | xargs jq -r \
  '.attempts[] | select(.findings | length > 0) | "\(.provider)\t\(.findings[0])"' | sort | uniq -c
```

**取り扱いの注意。** 既定の `content = "metadata"` はプロンプトの統計とハッシュだけを残し、本文は残しません。プロバイダーの標準エラー出力も記録しません。Codex は標準エラー出力にプロンプトをそのまま書き出すので、記録すると差分が別の経路でログに入ってしまうためです。`content = "full"` は実際に送ったプロンプトをそのまま保存するので、ステージ済みの差分が平文でディスクに残ります。プロバイダーの生応答はどちらの詳細度でも残します。整形後のメッセージだけでは生成事故を追えないためです。環境変数の上書きは名前だけを記録し、値は残しません。ログは `0700` のディレクトリ内に `0600` で作成し、`retention_days` を過ぎたものと `max_total_mb` を超えた分（古い順）を削除します。この掃除は 1 日 1 回までです。書き込みに失敗した場合は警告を 1 行出すだけで（`--quiet` では出しません）、コミットはそのまま続行します。

グローバル設定専用にしているのは意図的です。clone したリポジトリの `.git-sc` が、ログ出力を有効にしたり、手元のコードの書き出し先を決めたりできてはいけないためです。プロジェクト側の `[dev_log]` は警告を出して無視します。
