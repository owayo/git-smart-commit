// AIサービスモジュール
//
// service: AiService 本体(プロバイダー選択・フォールバックのオーケストレーション)
// prompt: プロンプト構築とメッセージ整形
// provider_command: プロバイダー別のコマンド構築・デバッグ表示
// process: サブプロセス実行(タイムアウト・並行 I/O)と出力解釈
// apple: Apple Intelligence ネイティブ呼び出し (macOS + apple-ai feature 限定)
#[cfg(all(target_os = "macos", feature = "apple-ai"))]
mod apple;
mod process;
mod prompt;
mod provider_command;
mod service;

pub use service::{AiProvider, AiService};
