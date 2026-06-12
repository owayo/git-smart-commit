//! プロバイダー別のコマンド構築とデバッグ表示
//!
//! 各 AI CLI の引数仕様(プロンプトの受け渡し方法・モデル指定・出力ファイル)を
//! ここに閉じ込める。プラットフォーム固有の制約(Windows の cmd /C 経由起動と
//! その安全性ガード、ARG_MAX 事前チェック)もこのモジュールが担当する。

use std::fs;
use std::process::{Command, Stdio};

use colored::Colorize;

use crate::error::AppError;

use super::process::TempFile;
use super::service::{AiProvider, AiService};

impl AiService {
    /// デバッグ用にコマンド文字列をフォーマット
    pub(super) fn format_command_for_debug(
        &self,
        provider: &AiProvider,
        prompt: &str,
        temp_file_path: Option<&std::path::Path>,
    ) -> String {
        let escaped_prompt = prompt.replace('\'', "'\\''");
        match provider {
            AiProvider::Antigravity => {
                // Antigravity CLI (`agy`) はモデル選択フラグも `--debug` フラグも持たない。
                // プロンプトは `-p` で 1 引数として渡す。
                format!("agy -p '{}'", escaped_prompt)
            }
            AiProvider::Codex => {
                let model_arg = if self.models.codex.is_empty() {
                    String::new()
                } else {
                    format!(" --model '{}'", self.models.codex)
                };
                let effort_arg = if self.codex_reasoning_effort.is_empty() {
                    String::new()
                } else {
                    format!(
                        " -c model_reasoning_effort='{}'",
                        self.codex_reasoning_effort
                    )
                };
                let output_arg = temp_file_path
                    .and_then(|p| p.to_str())
                    .map(|s| s.replace('\\', "/"))
                    .unwrap_or_else(|| "<output_file>".to_string());
                format!(
                    "echo '{}' | codex --disable hooks{} exec{} -o '{}'",
                    escaped_prompt, effort_arg, model_arg, output_arg
                )
            }
            AiProvider::Claude => {
                let model_arg = if self.models.claude.is_empty() {
                    String::new()
                } else {
                    format!(" --model '{}'", self.models.claude)
                };
                format!("echo '{}' | claude{} -p", escaped_prompt, model_arg)
            }
            AiProvider::Opencode => {
                let file_display = temp_file_path
                    .and_then(|p| p.to_str())
                    .map(|s| s.replace('\\', "/"))
                    .unwrap_or_else(|| "<temp_file>".to_string());
                let model_arg = if self.models.opencode.is_empty() {
                    String::new()
                } else {
                    format!(" -m '{}'", self.models.opencode)
                };
                format!(
                    "opencode run 'Follow the instructions in the attached file exactly. Output only the commit message.'{} -f '{}' --print-logs",
                    model_arg, file_display
                )
            }
            AiProvider::AppleIntelligence => {
                format!("echo '{}' | apple-ai", escaped_prompt)
            }
        }
    }

    /// プロバイダー固有の Command を構築する。
    /// 返り値: (Command, stdin を使用するか)
    pub(super) fn build_provider_command(
        &self,
        provider: &AiProvider,
        prompt: &str,
        temp_file: Option<&TempFile>,
        codex_output_file: Option<&TempFile>,
    ) -> Result<(Command, bool), AppError> {
        // Windows: cmd /C 経由で実行する（npm等でインストールされた .cmd ラッパーに対応するため）
        // Rust の Command::new() は .cmd/.bat ファイルを直接実行できないため、cmd /C が必要
        #[cfg(windows)]
        let mut cmd = {
            let mut c = Command::new("cmd");
            c.args(["/C", provider.command()]);
            c
        };
        #[cfg(not(windows))]
        let mut cmd = Command::new(provider.command());

        // プロバイダー固有の引数を追加
        // 各プロバイダーの models が空文字列の場合、モデルパラメータを省略する
        let uses_stdin = match provider {
            AiProvider::Antigravity => {
                // Windows では全プロバイダーを `cmd /C` 経由で起動するが、cmd.exe は
                // Rust 標準の引数クォート(MSVCRT 方式の `\"`)を解釈しない。diff を含む
                // プロンプトは常に改行や `"` を含むため、`-p` 引数渡しでは確実に
                // コマンドラインが破損し、最悪は diff 内容を細工した任意コマンド実行
                // (CVE-2024-24576 と同クラス)に繋がる。安全に渡す手段がないため、
                // Windows では明示エラーにして次のプロバイダーへフォールバックさせる。
                #[cfg(windows)]
                {
                    let _ = prompt;
                    return Err(AppError::AiProviderError(
                        "Antigravity (agy) is not supported on Windows: \
                         prompts cannot be passed safely through cmd.exe"
                            .to_string(),
                    ));
                }
                // Antigravity CLI (`agy`) はモデル選択フラグを持たず、`--debug` フラグもない。
                // プロンプトは `-p` 引数で渡す。長大な diff で OS の ARG_MAX を超えないよう
                // 事前にプロンプト長をチェックし、超過時は明確なエラーで失敗させる。
                #[cfg(not(windows))]
                {
                    Self::check_arg_size_limit(prompt)?;
                    cmd.args(["-p", prompt]);
                    false
                }
            }
            AiProvider::Codex => {
                // Codex のフックを常に無効化する。
                // git-sc は Codex をメッセージ生成器として使用しており、
                // stop hook が発火すると git-sc が再帰的に呼ばれて
                // 先にコミットされてしまう問題を防ぐ。
                cmd.args(["--disable", "hooks"]);
                if !self.codex_reasoning_effort.is_empty() {
                    cmd.args([
                        "-c",
                        &format!("model_reasoning_effort={}", self.codex_reasoning_effort),
                    ]);
                }
                cmd.arg("exec");
                if !self.models.codex.is_empty() {
                    cmd.args(["--model", &self.models.codex]);
                }
                if let Some(output_file) = codex_output_file {
                    cmd.arg("-o").arg(output_file.path());
                }
                true
            }
            AiProvider::Claude => {
                if !self.models.claude.is_empty() {
                    cmd.args(["--model", &self.models.claude]);
                }
                cmd.arg("-p");
                true
            }
            AiProvider::Opencode => {
                // opencode run "message" [-m "provider:model"] -f <temp_file>
                // プロンプトは一時ファイル経由で渡す（ファイル内に全指示を含む）
                if let Some(tf) = temp_file {
                    // Windows: バックスラッシュをフォワードスラッシュに正規化
                    // cmd /C 経由やCLIツール間でのパス受け渡しの互換性対策
                    let path_str = tf.path().to_str().unwrap_or("").replace('\\', "/");
                    cmd.args([
                        "run",
                        "Follow the instructions in the attached file exactly. Output only the commit message.",
                    ]);
                    if !self.models.opencode.is_empty() {
                        cmd.args(["-m", self.models.opencode.as_str()]);
                    }
                    cmd.args(["-f", &path_str]);
                    // デバッグモードの場合は --print-logs を追加
                    if self.debug {
                        cmd.arg("--print-logs");
                    }
                }
                false
            }
            AiProvider::AppleIntelligence => {
                // fm-rs feature 有効時はネイティブ呼び出しのため、ここには到達しない
                return Err(AppError::AiProviderError(
                    "Apple Intelligence requires the apple-ai feature flag".to_string(),
                ));
            }
        };

        // Claude Code はネスト実行を CLAUDECODE 環境変数で検出してブロックするため、
        // git-sc から Claude を呼ぶ場合は除去する
        if matches!(provider, AiProvider::Claude) {
            cmd.env_remove("CLAUDECODE");
        }

        // stdin/stdout/stderr のパイプ設定
        if uses_stdin {
            cmd.stdin(Stdio::piped());
        } else {
            cmd.stdin(Stdio::null());
        }
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        Ok((cmd, uses_stdin))
    }

    /// デバッグモード: 実行するコマンド情報を表示
    pub(super) fn print_debug_command(
        &self,
        provider: &AiProvider,
        prompt: &str,
        temp_file: Option<&TempFile>,
        silent: bool,
    ) {
        let cmd_str =
            self.format_command_for_debug(provider, prompt, temp_file.map(|tf| tf.path()));
        Self::emit_debug_line(silent, "");
        Self::emit_debug_line(
            silent,
            &"=== DEBUG: AI Provider Command ==="
                .yellow()
                .bold()
                .to_string(),
        );
        Self::emit_debug_line(silent, &"─".repeat(50).dimmed().to_string());
        Self::emit_debug_line(silent, &cmd_str.cyan().to_string());
        // 一時ファイル使用時はファイル情報を表示
        if let Some(tf) = temp_file {
            match fs::metadata(tf.path()) {
                Ok(meta) => Self::emit_debug_line(
                    silent,
                    &format!(
                        "  {} temp_file: {} ({} bytes)",
                        "✓".green(),
                        tf.path().display(),
                        meta.len()
                    ),
                ),
                Err(e) => Self::emit_debug_line(
                    silent,
                    &format!("  {} temp_file: {} ({})", "✗".red(), tf.path().display(), e),
                ),
            }
        }
        Self::emit_debug_line(silent, &"─".repeat(50).dimmed().to_string());
        Self::emit_debug_line(silent, "");
    }

    /// プロンプト長が OS の引数長制限を超えていないか事前チェックする。
    ///
    /// macOS は約 1 MB (ARG_MAX = 1,048,576)、Linux は約 2 MB (ARG_MAX = 2,097,152) が一般的だが、
    /// 環境変数や他の引数も同じ領域を共有するため、安全側に倒して 512 KiB を上限とする。
    /// 通常運用では `MAX_DIFF_CHARS` で diff が打ち切られるため、ここに到達するのは異常ケース。
    pub(super) fn check_arg_size_limit(prompt: &str) -> Result<(), AppError> {
        const MAX_ARG_BYTES: usize = 512 * 1024;
        if prompt.len() > MAX_ARG_BYTES {
            return Err(AppError::AiProviderError(format!(
                "Prompt is too large for Antigravity CLI argument: {} bytes > {} byte limit. \
                 Reduce the diff size or use a different provider.",
                prompt.len(),
                MAX_ARG_BYTES
            )));
        }
        Ok(())
    }
}
