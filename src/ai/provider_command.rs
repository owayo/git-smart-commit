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
use crate::config::ProviderStep;

impl AiService {
    /// デバッグ用にコマンド文字列をフォーマット
    ///
    /// `model` は解決済みモデル(空なら省略)、`step` は env と command(実行バイナリ)の
    /// 表示に使う。env は `KEY='val' ` の形でコマンドの前に表示する。
    pub(super) fn format_command_for_debug(
        &self,
        provider: &AiProvider,
        model: &str,
        step: &ProviderStep,
        prompt: &str,
        temp_file_path: Option<&std::path::Path>,
    ) -> String {
        let escaped_prompt = prompt.replace('\'', "'\\''");
        // 明示上書きされる env を `KEY='val' ` の形でコマンド前に表示する。
        let env_prefix = step
            .env
            .iter()
            .map(|(k, v)| format!("{k}='{}' ", v.replace('\'', "'\\''")))
            .collect::<String>();
        // 実行バイナリ(command 指定があれば優先)と、その固定引数。
        let bin = step
            .command
            .as_ref()
            .and_then(|c| c.first())
            .map(|s| s.as_str())
            .unwrap_or_else(|| provider.command());
        let fixed_args = step
            .command
            .as_ref()
            .map(|c| {
                c.iter()
                    .skip(1)
                    .map(|a| format!(" '{}'", a.replace('\'', "'\\''")))
                    .collect::<String>()
            })
            .unwrap_or_default();
        // --model 用(antigravity/codex/claude)。opencode は -m を使うので別途組み立てる。
        let model_arg = if model.is_empty() {
            String::new()
        } else {
            format!(" --model '{}'", model)
        };
        match provider {
            AiProvider::Antigravity => {
                // Antigravity CLI (`agy`) は `--model` に対応。空でなければ付与する。
                // プロンプトは `-p` で 1 引数として渡す(`--debug` フラグは無い)。
                format!(
                    "{env_prefix}{bin}{fixed_args}{model_arg} -p '{}'",
                    escaped_prompt
                )
            }
            AiProvider::Codex => {
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
                    "echo '{}' | {env_prefix}{bin}{fixed_args} --disable hooks{} exec{} -o '{}'",
                    escaped_prompt, effort_arg, model_arg, output_arg
                )
            }
            AiProvider::Claude => {
                format!(
                    "echo '{}' | {env_prefix}{bin}{fixed_args}{model_arg} -p",
                    escaped_prompt
                )
            }
            AiProvider::Opencode => {
                let file_display = temp_file_path
                    .and_then(|p| p.to_str())
                    .map(|s| s.replace('\\', "/"))
                    .unwrap_or_else(|| "<temp_file>".to_string());
                let opencode_model = if model.is_empty() {
                    String::new()
                } else {
                    format!(" -m '{}'", model)
                };
                format!(
                    "{env_prefix}{bin}{fixed_args} run 'Follow the instructions in the attached file exactly. Output only the commit message.'{} -f '{}' --print-logs",
                    opencode_model, file_display
                )
            }
            AiProvider::AppleIntelligence => {
                format!("echo '{}' | {env_prefix}{bin}", escaped_prompt)
            }
        }
    }

    /// プロバイダー固有の Command を構築する。
    /// 返り値: (Command, stdin を使用するか)
    pub(super) fn build_provider_command(
        &self,
        provider: &AiProvider,
        step: &ProviderStep,
        model: &str,
        prompt: &str,
        temp_file: Option<&TempFile>,
        codex_output_file: Option<&TempFile>,
    ) -> Result<(Command, bool), AppError> {
        // 実行バイナリ + 固定引数を解決する。command 指定があればそれを優先し、
        // 無ければ provider 既定バイナリ(codex/agy/claude/opencode)を使う。
        // command の先頭バイナリの `~` は Config::load 時に展開済み。
        let argv: Vec<String> = match step.command.as_ref().filter(|c| !c.is_empty()) {
            Some(c) => c.clone(),
            None => vec![provider.command().to_string()],
        };
        let (bin, fixed_args) = argv
            .split_first()
            .ok_or_else(|| AppError::AiProviderError("empty command".to_string()))?;

        // Windows: cmd /C 経由で渡すトークンに cmd.exe メタ文字が含まれると任意コマンド実行に
        // 繋がる(後述 build 時の cmd /C 起動と Rust の引数クォート仕様による。詳細は
        // windows_cmd_arg_has_metachar を参照)。設定由来の bin / 固定引数 / model /
        // codex の reasoning_effort を事前検証し、危険なら明示エラーで次プロバイダーへ
        // フォールバックさせる(Antigravity の Windows ブロックと同じ fail-safe 方針)。
        #[cfg(windows)]
        {
            let mut tokens: Vec<&str> = vec![bin.as_str()];
            tokens.extend(fixed_args.iter().map(String::as_str));
            if !model.is_empty() {
                tokens.push(model);
            }
            if matches!(provider, AiProvider::Codex) && !self.codex_reasoning_effort.is_empty() {
                tokens.push(self.codex_reasoning_effort.as_str());
            }
            if let Some(bad) = tokens
                .into_iter()
                .find(|&t| Self::windows_cmd_arg_has_metachar(t))
            {
                return Err(AppError::AiProviderError(format!(
                    "refusing to launch {} via cmd.exe: argument {:?} contains a cmd.exe \
                     metacharacter (&, |, <, >, ^, %, !, \", newline) that cannot be passed safely",
                    provider.name(),
                    bad
                )));
            }
        }

        // Windows: cmd /C 経由で実行する（npm等でインストールされた .cmd ラッパーに対応するため）
        // Rust の Command::new() は .cmd/.bat ファイルを直接実行できないため、cmd /C が必要
        #[cfg(windows)]
        let mut cmd = {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(bin);
            c
        };
        #[cfg(not(windows))]
        let mut cmd = Command::new(bin);

        // command で指定された固定引数(ラッパーに渡す追加フラグ等)を先に積む。
        cmd.args(fixed_args);

        // アカウント切替等の env を明示的に上書きする(token-burn の継承バグ対策)。
        // env_clear() はしない(PATH/HOME 継承が必要)。明示上書きは親シェルの値に勝つ。
        for (key, value) in &step.env {
            cmd.env(key, value);
        }

        // プロバイダー固有の引数を追加
        // model が空文字列の場合、モデルパラメータを省略する
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
                // Antigravity CLI (`agy`) は `--model` に対応(空でなければ付与)。`--debug` は無い。
                // プロンプトは `-p` 引数で渡す。長大な diff で OS の ARG_MAX を超えないよう
                // 事前にプロンプト長をチェックし、超過時は明確なエラーで失敗させる。
                #[cfg(not(windows))]
                {
                    Self::check_arg_size_limit(prompt)?;
                    // モデル指定がある場合のみ `--model` を付与(空文字列なら agy 既定に委ねる)
                    if !model.is_empty() {
                        cmd.args(["--model", model]);
                    }
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
                if !model.is_empty() {
                    cmd.args(["--model", model]);
                }
                if let Some(output_file) = codex_output_file {
                    cmd.arg("-o").arg(output_file.path());
                }
                true
            }
            AiProvider::Claude => {
                if !model.is_empty() {
                    cmd.args(["--model", model]);
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
                    if !model.is_empty() {
                        cmd.args(["-m", model]);
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
        step: &ProviderStep,
        model: &str,
        prompt: &str,
        temp_file: Option<&TempFile>,
        silent: bool,
    ) {
        let cmd_str = self.format_command_for_debug(
            provider,
            model,
            step,
            prompt,
            temp_file.map(|tf| tf.path()),
        );
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
        // env 明示上書きと cooldown_key を表示する(どの step に何を渡したか = バグ調査の起点)。
        if !step.env.is_empty() {
            Self::emit_debug_line(silent, &"  env (explicit override):".dimmed().to_string());
            for (key, value) in &step.env {
                Self::emit_debug_line(silent, &format!("    {key}={value}"));
            }
        }
        Self::emit_debug_line(
            silent,
            &format!("  cooldown_key: {}", step.cooldown_key())
                .dimmed()
                .to_string(),
        );
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

    /// Windows の `cmd /C` 経由起動で危険な cmd.exe メタ文字を含むかを判定する。
    ///
    /// Windows では全プロバイダーを `cmd /C <bin> <args...>` で起動する。Rust 標準の
    /// 引数クォートは「空白・タブ・空文字」のときだけ引用し、`&`/`|`/`<`/`>`/`^` などの
    /// cmd.exe メタ文字は素通しする。そのため空白を含まない細工値(例: `model = "x&calc"`)は
    /// 引用されず `cmd /C codex --model x&calc ...` となり、cmd.exe が `&` をコマンド区切りと
    /// 解釈して任意コマンドを実行してしまう(CVE-2024-24576 と同クラス)。安全に渡せる確実な
    /// 手段が無いため、これらのメタ文字を含むトークンは拒否してフォールバックさせる。
    /// 空白や括弧を含む正規のモデル名(例: "GPT-OSS 120B (Medium)")は Rust が引用するため対象外。
    ///
    /// この純粋関数はプラットフォーム非依存でテスト可能だが、呼び出すガードは Windows 限定の
    /// ため、非テストの非 Windows ビルドでは未使用 dead_code にならないよう cfg を絞る。
    #[cfg(any(windows, test))]
    pub(super) fn windows_cmd_arg_has_metachar(arg: &str) -> bool {
        arg.chars().any(|c| {
            matches!(
                c,
                '&' | '|' | '<' | '>' | '^' | '%' | '!' | '"' | '\r' | '\n'
            )
        })
    }
}
