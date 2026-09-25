# プロバイダーの注意

特別な扱いが要るプロバイダーを git-sc がどう起動するか、プラットフォームによって何が変わるかをまとめています。どのプロバイダーをどの順に、どのモデルやアカウントで使うかは [configuration.ja.md](configuration.ja.md) で設定します。

## Grok CLI

Grok プロバイダーは Grok Build TUI（`grok`、xAI）を使います。この CLI はコーディングエージェントで、plan モード、セッションをまたぐ memory、web 検索、ツール実行が既定で有効です。そのため git-sc は、1 ターンの純粋関数として振る舞うよう、次のフラグで制約をかけて起動します。

| フラグ | 目的 |
|--------|------|
| `--output-format plain` | 対話 TUI ではなくヘッドレスのテキスト出力にする |
| `--sandbox read-only` | Codex のサンドボックスと同様にファイル書き込みとネットワークを禁止する |
| `--no-plan` / `--no-memory` | plan モードとセッションをまたぐ memory を無効化する（どちらも既定で有効） |
| `--disable-web-search` | web の取得と検索を無効化する |
| `--max-turns 1` | ツールのループを 1 ターンで打ち切る |
| `--verbatim` | CLI 側でプロンプトを書き換えさせない |
| `--prompt-file <一時ファイル>` | 大きな diff で `ARG_MAX` や cmd.exe のメタ文字問題を避ける |

- **モデル**: ステップの `model` > `[models].grok` > 空（grok 自身の既定に委ねる）の順で解決します。空でない場合は `grok models` が返す ID（現在は `grok-4.5` のみ）を `-m "<id>"` として渡します。同梱の既定値は空にしてあるので、今後より安価なモデルが追加されても、git-sc の新しいリリースを待たずに追随できます。
- **入手**: Grok CLI は [cmux](https://github.com/manaflow-ai/cmux) に同梱されています（`/Applications/cmux.app/Contents/Resources/bin/grok`）。`grok` が `PATH` にない場合、このステップはスキップされて次のプロバイダーへ進みます。

## Apple Intelligence

Apple Intelligence プロバイダーは、[fm-rs](https://github.com/blacktop/fm-rs)（Apple の [Foundation Models](https://developer.apple.com/documentation/foundationmodels) フレームワークの Rust バインディング）を使い、推論をすべてデバイス上で行います。API キーやネットワーク接続は不要です。

- **動作要件**: macOS 26（Tahoe）以降、Apple Silicon、システム設定で Apple Intelligence が有効であること
- **仕組み**: Apple Intelligence が有効な状態で実行すると（macOS では既定で有効）、git-sc は fm-rs 経由で Foundation Models を直接呼び出します。生成のたびに、コミットメッセージ生成用の指示（`instructions`）を設定した `LanguageModelSession` を作成します。指示は解決済みのプレフィックス形式から組み立てるので、`prefix_type = "none"` / `"bracket"` / `"emoji"` や直近コミットからの自動判定がそのまま反映されます（常に Conventional Commits を強制することはありません）。
- **コンテキスト長**: オンデバイスモデルのコンテキストは **4096 トークン**で、ほかのプロバイダーより桁違いに小さいです。git-sc は生成前にプロンプトのトークン数を計測し、収まらない場合は変更ファイル一覧を残したまま diff 本文を縮約してプロンプトを組み直します。この場合はメッセージが変更の一部だけを見て書かれることになるため、警告を表示します。縮約しても収まらないときは、コミットを失敗させずに次のプロバイダーへ進みます。
- **タイムアウト**: `provider_timeout_seconds`（既定 60 秒）が適用されます。CLI プロバイダーと同じ設定です。
- **失敗の扱い**: プロンプト起因の失敗（コンテキスト長超過・安全ガードレール・拒否・非対応言語）はクールダウンに入れません。クールダウンに入るのは、モデルが現在使えないことを示す失敗（アセット未取得・レート制限・タイムアウト）だけです。この失敗種別の判定には macOS 27 SDK 以降でのビルドが必要で、macOS 26 ではすべてプロバイダー側の失敗として扱われます。
- **ビルド**: `cargo build --features apple-ai`（macOS では `make build` / `make install` で自動的に有効）。macOS 27 以降でビルドすると Foundation Models 27 の機能（正確なトークン数計測・型付きエラー・応答ごとのトークン使用量）も有効になります。macOS 26 も引き続きサポートします。
- **クロスプラットフォーム**: Linux/Windows では Apple Intelligence は利用できず、自動的にスキップされます。

## プラットフォームの注意

- **Windows**: Antigravity CLI（`agy`）プロバイダーは、明示的なエラーを出してスキップされます。Windows では、npm でインストールされる `.cmd` シムに対応するため、すべてのプロバイダーを `cmd /C` 経由で起動します。ところが cmd.exe は、複数行の diff を含むプロンプトをコマンドライン引数として安全に受け取れず、渡すとコマンドラインが壊れます。これは CVE-2024-24576 と同じ種類のコマンドインジェクションの経路でもあります。フォールバックチェーンは次のプロバイダーへ進みます。プロンプトを標準入力や一時ファイルで受け取るプロバイダー（codex、claude、opencode、grok）は影響を受けません。
