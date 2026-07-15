//! サブプロセス実行と出力解釈
//!
//! タイムアウト付きのプロセス待機(stdin writer / stdout / stderr の 3 スレッドを
//! 並行実行してパイプ双方向デッドロックを防ぐ)、一時ファイルの RAII 管理、
//! プロバイダー出力の検証・エラー抽出を担当する。並行設計の不変条件は
//! `run_process_with_timeout` のコメントを参照。

use std::fs;
use std::io::Write;
use std::process::{Child, ExitStatus};

use colored::Colorize;

use crate::error::AppError;

use super::service::{AiProvider, AiService};

/// 一時ファイルの RAII ガード。Drop 時に自動でクリーンアップする。
///
/// シンボリックリンク攻撃を防ぐため、`create_new(true)` で排他的に作成する。
pub(super) struct TempFile {
    path: std::path::PathBuf,
}

impl TempFile {
    /// 一意な一時ファイルを排他的に作成し、内容を書き込む。
    ///
    /// `create_new(true)` により既存ファイルやシンボリックリンクを追従しない。
    pub(super) fn create_with_content(content: &[u8]) -> Result<Self, AppError> {
        use std::fs::OpenOptions;
        use std::time::{SystemTime, UNIX_EPOCH};

        let temp_dir = std::env::temp_dir();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();

        for attempt in 0..100 {
            let path = temp_dir.join(format!(
                "git-sc-prompt-{}-{}-{}.txt",
                std::process::id(),
                timestamp,
                attempt
            ));

            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                // プロンプトには差分内容が含まれるため、他ユーザーに読めない権限で作成する。
                options.mode(0o600);
            }

            match options.open(&path) {
                Ok(mut file) => {
                    // 書き込み/sync失敗時はファイルを削除してからエラーを返す
                    if let Err(e) = file.write_all(content).and_then(|_| file.sync_all()) {
                        drop(file);
                        let _ = fs::remove_file(&path);
                        return Err(AppError::AiProviderError(format!(
                            "Failed to write temp file: {}",
                            e
                        )));
                    }
                    drop(file); // 明示的にファイルハンドルを閉じる
                    return Ok(Self { path });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    return Err(AppError::AiProviderError(format!(
                        "Failed to create temp file: {}",
                        e
                    )));
                }
            }
        }

        Err(AppError::AiProviderError(
            "Failed to create unique temp file".to_string(),
        ))
    }

    pub(super) fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

impl AiService {
    /// デバッグ行を出力する
    ///
    /// silent モード(--generate-for)は「stdout には生成メッセージのみ」が契約のため、
    /// デバッグ出力は stderr へ逃がす。通常モードでは従来どおり stdout に出す。
    pub(super) fn emit_debug_line(silent: bool, line: &str) {
        if silent {
            eprintln!("{}", line);
        } else {
            println!("{}", line);
        }
    }

    /// stdout/stderr をスレッドで読み取り、タイムアウト付きでプロセス完了を待機する。
    /// 返り値: (ExitStatus, stdout, stderr)
    pub(super) fn run_process_with_timeout(
        &self,
        child: &mut Child,
        provider: &AiProvider,
        uses_stdin: bool,
        prompt: &str,
        silent: bool,
    ) -> Result<(ExitStatus, String, String), AppError> {
        let is_debug = self.debug;

        if is_debug {
            Self::emit_debug_line(silent, "");
            Self::emit_debug_line(
                silent,
                &"=== DEBUG: AI Provider Output (streaming) ==="
                    .yellow()
                    .bold()
                    .to_string(),
            );
            Self::emit_debug_line(silent, &"─".repeat(50).dimmed().to_string());
        }

        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();

        // stdin 書き込みスレッド (codex, claude 用)。
        // stdout/stderr リーダースレッドと「並行して」書き込むことで、
        // 大きいプロンプト(例: CLAW_HOOKS_AGENT_MESSAGE 由来の長大なコンテキスト)
        // 使用時のパイプ双方向デッドロックを防ぐ。
        // stdin 全量を同期で書き切ってから stdout を読み始める実装では、
        // 子プロセスが stdin を読み切る前に stdout/stderr へ大量出力した場合、
        // 双方のパイプバッファが満杯になって相互にブロックし、さらに write_all の
        // ブロック中はタイムアウト判定にも到達できず無期限ハングに陥る。
        let stdin_pipe = child.stdin.take();
        let stdin_thread = if uses_stdin {
            let prompt_bytes = prompt.as_bytes().to_vec();
            Some(std::thread::spawn(move || -> std::io::Result<()> {
                if let Some(mut stdin) = stdin_pipe {
                    // 書き込み結果は join 後に検査する。子が先に終了して EPIPE になった
                    // 場合は Err を返し、不完全なプロンプトを成功扱いしないよう呼び出し側で扱う。
                    stdin.write_all(&prompt_bytes)?;
                }
                // 書き込み後はスコープを抜けて stdin が drop/close され、子へ EOF を伝える。
                Ok(())
            }))
        } else {
            // stdin を使わないプロバイダー(antigravity/opencode)は
            // build_provider_command で Stdio::null 済みのため take() は None。
            drop(stdin_pipe);
            None
        };

        // stdout 読み取りスレッド（デバッグ時はリアルタイム表示）
        let stdout_thread = std::thread::spawn(move || {
            use std::io::BufRead;
            let mut buf = String::new();
            if let Some(pipe) = stdout_pipe {
                let reader = std::io::BufReader::new(pipe);
                for line in reader.lines().map_while(Result::ok) {
                    if is_debug {
                        Self::emit_debug_line(silent, &format!("  {}", line));
                    }
                    buf.push_str(&line);
                    buf.push('\n');
                }
            }
            buf
        });

        // stderr 読み取りスレッド（デバッグ時はリアルタイム表示）
        let stderr_thread = std::thread::spawn(move || {
            use colored::Colorize;
            use std::io::BufRead;
            let mut buf = String::new();
            if let Some(pipe) = stderr_pipe {
                let reader = std::io::BufReader::new(pipe);
                for line in reader.lines().map_while(Result::ok) {
                    if is_debug {
                        eprintln!("  {}", line.red());
                    }
                    buf.push_str(&line);
                    buf.push('\n');
                }
            }
            buf
        });

        // タイムアウト付きでプロセス完了を待機
        // ループからは Result を返してエラー経路でも必ずスレッドを join し、
        // 取り残された読み取りスレッドが残らないようにする
        let timeout = std::time::Duration::from_secs(self.timeout_seconds);
        let start = std::time::Instant::now();
        let wait_result: Result<ExitStatus, AppError> = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) => {
                    if start.elapsed() > timeout {
                        let _ = child.kill();
                        let _ = child.wait();
                        break Err(AppError::AiProviderError(format!(
                            "{} timed out after {} seconds",
                            provider.name(),
                            self.timeout_seconds
                        )));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(AppError::AiProviderError(format!(
                        "Failed to wait for process: {}",
                        e
                    )));
                }
            }
        };

        // ループから抜けた時点で子プロセスは終了済みなので、パイプ読み取りスレッドも
        // EOF を受けて必ず終了する。タイムアウト/エラー経路でも join してから抜ける。
        // stdin 書き込みスレッドも、子プロセスの終了に伴う EOF/EPIPE で必ず完了するため join する。
        let stdin_write_result: std::io::Result<()> = match stdin_thread {
            Some(handle) => handle.join().unwrap_or(Ok(())),
            None => Ok(()),
        };
        let stdout_str = stdout_thread.join().unwrap_or_default();
        let stderr_str = stderr_thread.join().unwrap_or_default();

        let exit_status = wait_result?;

        // stdin への書き込みが失敗したにもかかわらず子が成功扱い(exit 0)で終了した場合、
        // プロンプトが途中までしか渡っていない可能性が高い。不完全なプロンプトで生成された
        // 結果を正常な応答として誤って採用しないよう、明示的にエラーにする。
        // (子が異常終了している場合は exit_status 側のエラーを process_provider_output に委ねる)
        if exit_status.success()
            && let Err(e) = stdin_write_result
        {
            return Err(AppError::AiProviderError(format!(
                "Failed to write prompt to {} stdin: {}",
                provider.name(),
                e
            )));
        }

        if is_debug {
            Self::emit_debug_line(silent, &"─".repeat(50).dimmed().to_string());
            Self::emit_debug_line(
                silent,
                &format!(
                    "  {}: {}",
                    "exit code".dimmed(),
                    exit_status.to_string().cyan()
                ),
            );
            Self::emit_debug_line(silent, "");
        }

        Ok((exit_status, stdout_str, stderr_str))
    }

    /// プロバイダーの出力を検証し、クリーンアップ済みのメッセージを返す
    pub(super) fn process_provider_output(
        provider: &AiProvider,
        exit_status: ExitStatus,
        stdout_str: &str,
        stderr_str: &str,
    ) -> Result<String, AppError> {
        if !exit_status.success() {
            let error_msg = Self::extract_error(stderr_str, provider);
            // Claude Code はエラーメッセージを stdout に出力することがあるため、
            // stderr が空（ジェネリックフォールバック）の場合は stdout も確認する
            if matches!(provider, AiProvider::Claude) && !stdout_str.trim().is_empty() {
                let stderr_fallback = Self::extract_error("", provider);
                if error_msg == stderr_fallback {
                    return Err(AppError::AiProviderError(
                        stdout_str
                            .lines()
                            .find(|l| !l.trim().is_empty())
                            .unwrap_or(&error_msg)
                            .trim()
                            .to_string(),
                    ));
                }
            }
            return Err(AppError::AiProviderError(error_msg));
        }

        // exit code が 0 でも stderr にエラーがあれば失敗扱い
        // ただし Codex/Claude は stderr にプロンプトエコーや詳細ログを出力するため、
        // diff 内容に "error:" が含まれると誤検出するのでスキップ
        if !stderr_str.trim().is_empty()
            && !matches!(provider, AiProvider::Codex | AiProvider::Claude)
        {
            let lower = stderr_str.to_lowercase();
            if lower.contains("file not found") || lower.contains("error:") {
                let error_msg = Self::extract_error(stderr_str, provider);
                return Err(AppError::AiProviderError(error_msg));
            }
        }

        let message = stdout_str.trim().to_string();
        let message = Self::clean_message(&message);

        if message.is_empty() {
            // stderr にヒントがあればそれも含める
            if !stderr_str.trim().is_empty() {
                return Err(AppError::AiProviderError(format!(
                    "{} returned an empty response (stderr: {})",
                    provider.name(),
                    stderr_str.trim()
                )));
            }
            return Err(AppError::AiProviderError(format!(
                "{} returned an empty response",
                provider.name()
            )));
        }

        Ok(message)
    }

    /// stderrからエラーメッセージを抽出
    pub(super) fn extract_error(stderr: &str, provider: &AiProvider) -> String {
        match provider {
            AiProvider::Antigravity => {
                // 旧 Gemini CLI 由来の `[API Error: ...]` パターンが Antigravity でも残っている可能性に備えて優先的に拾う。
                for line in stderr.lines() {
                    if line.starts_with("[API Error:") {
                        return line.to_string();
                    }
                }
                // "critical error" / "Error:" を含む行
                for line in stderr.lines() {
                    let trimmed = line.trim();
                    if trimmed.contains("critical error") || trimmed.contains("Error:") {
                        return trimmed.to_string();
                    }
                }
                // 最後の手段として最初の非空行
                stderr
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("Antigravity CLI request failed")
                    .to_string()
            }
            AiProvider::Codex => {
                // Codex CLI: "ERROR:" で始まる行を優先的に探す
                // 例: "ERROR: Your access token could not be refreshed..."
                for line in stderr.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with("ERROR:") {
                        return trimmed.to_string();
                    }
                }
                // "error" を含む行を探す（小文字も含む）
                for line in stderr.lines() {
                    let trimmed = line.trim();
                    let lower = trimmed.to_lowercase();
                    if lower.contains("error") && !lower.contains("reconnecting") {
                        return trimmed.to_string();
                    }
                }
                // 最後の非空行を返す（情報メッセージを避ける）
                stderr
                    .lines()
                    .rev()
                    .find(|l| {
                        let t = l.trim();
                        !t.is_empty()
                            && !t.starts_with("Reading prompt")
                            && !t.starts_with("Reconnecting")
                    })
                    .unwrap_or("Codex API request failed")
                    .to_string()
            }
            AiProvider::Claude => {
                // 最初の非空行またはジェネリックメッセージを返す
                stderr
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("API request failed")
                    .to_string()
            }
            AiProvider::Opencode => {
                // opencode: エラー行を探す
                for line in stderr.lines() {
                    let trimmed = line.trim();
                    let lower = trimmed.to_lowercase();
                    if lower.contains("error") || lower.contains("failed") {
                        return trimmed.to_string();
                    }
                }
                // 最初の非空行またはジェネリックメッセージを返す
                stderr
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("opencode request failed")
                    .to_string()
            }
            AiProvider::Grok => {
                // grok CLI: clap 由来のエラーは `error:` プレフィックス、認証系は
                // "You need to login" / "Rate limit" / "unauthorized" などが出やすい。
                // まず error/failed を含む行を優先し、無ければ最初の非空行を返す。
                for line in stderr.lines() {
                    let trimmed = line.trim();
                    let lower = trimmed.to_lowercase();
                    if lower.contains("error") || lower.contains("failed") {
                        return trimmed.to_string();
                    }
                }
                stderr
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("Grok CLI request failed")
                    .to_string()
            }
            AiProvider::AppleIntelligence => {
                // apple-ai: "Error:" で始まる行を探す
                for line in stderr.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with("Error:") {
                        return trimmed.to_string();
                    }
                }
                stderr
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("Apple Intelligence request failed")
                    .to_string()
            }
        }
    }
}
