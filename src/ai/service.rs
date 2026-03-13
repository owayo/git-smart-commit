use std::fs::{self, File};
use std::io::Write;
use std::process::{Child, Command, ExitStatus, Stdio};

use colored::Colorize;

use crate::config::{Config, ModelsConfig};
use crate::error::AppError;
use crate::state::State;

/// 一時ファイルの RAII ガード。Drop 時に自動でクリーンアップする。
struct TempFile {
    path: std::path::PathBuf,
}

impl TempFile {
    /// 一時ファイルを作成し、内容を書き込む。sync_all でディスクにフラッシュ。
    fn create_with_content(path: std::path::PathBuf, content: &[u8]) -> Result<Self, AppError> {
        let mut file = File::create(&path)
            .map_err(|e| AppError::AiProviderError(format!("Failed to create temp file: {}", e)))?;
        file.write_all(content)
            .map_err(|e| AppError::AiProviderError(format!("Failed to write temp file: {}", e)))?;
        file.sync_all()
            .map_err(|e| AppError::AiProviderError(format!("Failed to sync temp file: {}", e)))?;
        drop(file); // 明示的にファイルハンドルを閉じる
        Ok(Self { path })
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Conventional Commits プレフィックスの詳細説明
const CONVENTIONAL_COMMITS_GUIDE: &str = "\
Use Conventional Commits format. Choose the prefix that best matches the change:\n\
- feat: new feature or functionality added\n\
- fix: bug fix\n\
- docs: documentation only changes (README, comments, JSDoc)\n\
- style: code style changes (formatting, whitespace, semicolons) with no logic change\n\
- refactor: code restructuring without adding features or fixing bugs\n\
- perf: performance improvement\n\
- test: adding or correcting tests\n\
- build: changes to build system or dependencies (Cargo.toml, package.json, Makefile)\n\
- ci: CI/CD configuration changes (GitHub Actions, GitLab CI)\n\
- chore: maintenance tasks that don't modify src or test files\n\
- revert: reverting a previous commit";

/// AIプロバイダーの種類
#[derive(Debug, Clone, Copy)]
pub enum AiProvider {
    Gemini,
    Codex,
    Claude,
    Opencode,
    AppleIntelligence,
}

impl AiProvider {
    pub fn name(&self) -> &'static str {
        match self {
            AiProvider::Gemini => "Gemini CLI",
            AiProvider::Codex => "Codex CLI",
            AiProvider::Claude => "Claude Code",
            AiProvider::Opencode => "opencode",
            AiProvider::AppleIntelligence => "Apple Intelligence",
        }
    }

    fn command(&self) -> &'static str {
        match self {
            AiProvider::Gemini => "gemini",
            AiProvider::Codex => "codex",
            AiProvider::Claude => "claude",
            AiProvider::Opencode => "opencode",
            AiProvider::AppleIntelligence => "apple-ai",
        }
    }

    /// 設定ファイルで使用するキー名（状態管理にも使用）
    pub fn config_key(&self) -> &'static str {
        self.command()
    }

    /// 文字列からプロバイダーを解析
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "gemini" => Some(AiProvider::Gemini),
            "codex" => Some(AiProvider::Codex),
            "claude" => Some(AiProvider::Claude),
            "opencode" => Some(AiProvider::Opencode),
            "apple-intelligence" | "apple_intelligence" => Some(AiProvider::AppleIntelligence),
            _ => None,
        }
    }
}

/// フォールバック機能付きのAIサービス
pub struct AiService {
    providers: Vec<AiProvider>,
    language: String,
    models: ModelsConfig,
    cooldown_minutes: u64,
    timeout_seconds: u64,
    debug: bool,
    provider_override: bool,
}

impl AiService {
    /// デフォルトのプロバイダーリストを返す
    fn default_providers() -> Vec<AiProvider> {
        let mut providers = vec![
            AiProvider::Opencode,
            AiProvider::Gemini,
            AiProvider::Codex,
            AiProvider::Claude,
        ];
        if cfg!(all(target_os = "macos", feature = "apple-ai")) {
            providers.push(AiProvider::AppleIntelligence);
        }
        providers
    }

    /// 設定からAiServiceを作成
    pub fn from_config(config: &Config) -> Self {
        let provider_strings: Vec<String> = config.providers.clone();

        // 状態を読み込んで、クールダウン中のプロバイダーを降格
        let reordered_strings = if let Ok(state) = State::load() {
            state.reorder_providers(provider_strings, config.provider_cooldown_minutes)
        } else {
            provider_strings
        };

        let providers: Vec<AiProvider> = reordered_strings
            .iter()
            .filter_map(|s| AiProvider::from_str(s))
            .collect();

        // 有効なプロバイダーがない場合はデフォルトにフォールバック
        let providers = if providers.is_empty() {
            Self::default_providers()
        } else {
            providers
        };

        Self {
            providers,
            language: config.language.clone(),
            models: config.models.clone(),
            cooldown_minutes: config.provider_cooldown_minutes,
            timeout_seconds: config.provider_timeout_seconds,
            debug: false,
            provider_override: false,
        }
    }

    /// デフォルトのフォールバック順序でAiServiceを作成
    pub fn new() -> Self {
        Self {
            providers: Self::default_providers(),
            language: "Japanese".to_string(),
            models: ModelsConfig::default(),
            cooldown_minutes: 60, // デフォルト1時間
            timeout_seconds: 60,  // デフォルト60秒（Config::defaultと同値）
            debug: false,
            provider_override: false,
        }
    }

    /// デバッグモードを設定
    pub fn set_debug(&mut self, debug: bool) {
        self.debug = debug;
    }

    /// プロバイダーを手動指定で上書き（フォールバックなし、失敗記録スキップ）
    pub fn set_provider_override(&mut self, provider: AiProvider) {
        self.providers = vec![provider];
        self.provider_override = true;
    }

    /// デバッグ用にコマンド文字列をフォーマット
    fn format_command_for_debug(
        &self,
        provider: &AiProvider,
        prompt: &str,
        temp_file_path: Option<&std::path::Path>,
    ) -> String {
        let escaped_prompt = prompt.replace('\'', "'\\''");
        match provider {
            AiProvider::Gemini => {
                let model_arg = if self.models.gemini.is_empty() {
                    String::new()
                } else {
                    format!(" -m '{}'", self.models.gemini)
                };
                let debug_arg = if self.debug { " --debug" } else { "" };
                format!("gemini{}{} -p '{}'", model_arg, debug_arg, escaped_prompt)
            }
            AiProvider::Codex => {
                let model_arg = if self.models.codex.is_empty() {
                    String::new()
                } else {
                    format!(" --model '{}'", self.models.codex)
                };
                format!("echo '{}' | codex exec{}", escaped_prompt, model_arg)
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

    /// プロバイダーの失敗を記録
    fn record_provider_failure(&self, provider: &AiProvider) {
        if let Ok(mut state) = State::load() {
            state.record_failure(provider.config_key());
            // 期限切れのエントリをクリーンアップ
            state.cleanup_expired(self.cooldown_minutes);
            // 保存（エラーは無視）
            let _ = state.save();
        }
    }

    /// 言語設定を上書き
    pub fn set_language(&mut self, language: String) {
        self.language = language;
    }

    /// 言語設定を取得
    pub fn language(&self) -> &str {
        &self.language
    }

    /// 少なくとも1つのAI CLIがインストールされていることを確認
    pub fn verify_installation(&self) -> Result<(), AppError> {
        for provider in &self.providers {
            if Self::is_installed(provider) {
                return Ok(());
            }
        }
        Err(AppError::NoAiProviderInstalled)
    }

    /// プロバイダーがインストールされているかチェック
    fn is_installed(provider: &AiProvider) -> bool {
        // Apple Intelligence: apple-ai feature 有効時のみ利用可能（ランタイムで可否判定）
        if matches!(provider, AiProvider::AppleIntelligence) {
            return cfg!(all(target_os = "macos", feature = "apple-ai"));
        }

        // Windows は "where"、Unix は "which" を使用
        let check_cmd = if cfg!(windows) { "where" } else { "which" };
        Command::new(check_cmd)
            .arg(provider.command())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// AI用のプロンプトを構築
    pub fn build_prompt(
        diff: &str,
        recent_commits: &[String],
        language: &str,
        prefix_type: Option<&str>,
        with_body: bool,
        agent_context: Option<&str>,
    ) -> String {
        let format_section = match prefix_type {
            Some("conventional") => CONVENTIONAL_COMMITS_GUIDE.to_string(),
            Some("bracket") => {
                "Use bracket prefix format (e.g., [Add], [Fix], [Update], [Remove], [Refactor]).".to_string()
            }
            Some("colon") => {
                "Use colon prefix format (e.g., Add:, Fix:, Update:, Remove:, Refactor:).".to_string()
            }
            Some("emoji") => {
                "Use emoji prefix format (e.g., ✨ for new feature, 🐛 for bug fix, 📝 for docs, ♻️ for refactor, 🔧 for config).".to_string()
            }
            Some("plain") | Some("none") => {
                "Do NOT use any prefix. Write only the commit message without type prefix.".to_string()
            }
            Some(custom) => {
                format!("Use the following prefix format: {}", custom)
            }
            None => {
                // 自動判定モード: 過去のコミットから推論
                if recent_commits.is_empty() {
                    format!("No recent commits found. {}", CONVENTIONAL_COMMITS_GUIDE)
                } else {
                    format!(
                        "Recent commit messages in this repository:\n{}\n\nAnalyze the recent commit messages above and match their style/format.",
                        recent_commits
                            .iter()
                            .enumerate()
                            .map(|(i, c)| format!("{}. {}", i + 1, c))
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                }
            }
        };

        let body_instructions = if with_body {
            r#"
Structure:
- First line: Subject line (concise summary, ideally under 72 characters)
- Second line: Empty (blank line)
- Third line onwards: Body with bullet points describing key changes

Body Guidelines:
- Use bullet points starting with "- "
- Each bullet point should describe a specific change
- Include 2-5 bullet points based on the scope of changes
- Be specific about what was added, changed, or removed"#
        } else {
            r#"
Rules:
- Write only a single line (no multi-line message)
- Keep it concise (ideally under 72 characters)"#
        };

        let agent_context_section = match agent_context {
            Some(ctx) if !ctx.is_empty() => {
                format!(
                    concat!(
                        "\n<agent-context>\n{}\n</agent-context>\n\n",
                        "IMPORTANT: Use the <agent-context> above as the primary source for understanding ",
                        "the intent and purpose of these changes. The commit message should reflect ",
                        "the high-level goal described in the context, not just describe the raw diff.\n",
                    ),
                    ctx
                )
            }
            _ => String::new(),
        };

        format!(
            r#"Generate a git commit message for the following changes.

{format_section}

Instructions:
- Match the commit message style shown above
- Write the commit message in {language}
{body_instructions}
- Be specific about what changed
- Do NOT end with a period or any punctuation (no ".", "。", etc.)
- Do NOT use past tense or polite/formal endings (no "しました", "ました", "した", "です", etc.)
- Use short, direct noun phrases or imperative form (e.g., "追加", "修正", "変更", NOT "追加しました", "修正した")
- Output ONLY the commit message as plain text
- Do NOT use any markdown formatting (no **, *, `, #, etc.)
- Do NOT include any explanation, reasoning, or thinking process
- Do NOT write phrases like "I will...", "Let me...", "Based on...", "Here is..."
- Respond with the commit message immediately, no preamble
{agent_context_section}
<changes>
{diff}
</changes>"#
        )
    }

    /// フォールバック付きでAI CLIを使用してコミットメッセージを生成
    ///
    /// prefix_type:
    /// - None: 自動判定（過去コミットから推論）
    /// - Some("conventional"): Conventional Commits形式
    /// - Some("none"): プレフィックスなし
    /// - Some(other): カスタム形式
    ///
    /// with_body: true の場合、本文（body）付きのコミットメッセージを生成
    /// silent: true の場合、進捗出力を抑制（サイレントモード）
    /// 返り値: (メッセージ, プロバイダー名)
    pub fn generate_commit_message(
        &self,
        diff: &str,
        recent_commits: &[String],
        prefix_type: Option<&str>,
        with_body: bool,
        silent: bool,
        agent_context: Option<&str>,
    ) -> Result<(String, &'static str), AppError> {
        self.generate_commit_message_internal(
            diff,
            recent_commits,
            prefix_type,
            with_body,
            silent,
            agent_context,
        )
    }

    /// 内部実装: コミットメッセージ生成
    /// 返り値: (メッセージ, プロバイダー名)
    fn generate_commit_message_internal(
        &self,
        diff: &str,
        recent_commits: &[String],
        prefix_type: Option<&str>,
        with_body: bool,
        silent: bool,
        agent_context: Option<&str>,
    ) -> Result<(String, &'static str), AppError> {
        let prompt = Self::build_prompt(
            diff,
            recent_commits,
            &self.language,
            prefix_type,
            with_body,
            agent_context,
        );
        let mut last_error = None;

        for provider in &self.providers {
            if !Self::is_installed(provider) {
                continue;
            }

            if !silent {
                println!("  {} {}...", "Using".dimmed(), provider.name().cyan());
            }

            // Apple Intelligence: fm-rs feature 有効時はネイティブ呼び出し
            #[cfg(all(target_os = "macos", feature = "apple-ai"))]
            let result = if matches!(provider, AiProvider::AppleIntelligence) {
                Self::call_apple_intelligence_native(&prompt, &self.language)
            } else {
                self.call_provider(provider, &prompt)
            };
            #[cfg(not(all(target_os = "macos", feature = "apple-ai")))]
            let result = self.call_provider(provider, &prompt);

            match result {
                Ok(message) => {
                    // --body 未指定時は1行目のみ使用（AIが複数行を返した場合の対策）
                    let message = if !with_body {
                        message.lines().next().unwrap_or("").trim().to_string()
                    } else {
                        message
                    };
                    if message.is_empty() {
                        last_error = Some(AppError::AiProviderError(format!(
                            "{} returned an empty first line",
                            provider.name()
                        )));
                        continue;
                    }
                    return Ok((message, provider.name()));
                }
                Err(e) => {
                    if !silent {
                        eprintln!(
                            "  {} {} failed: {}",
                            "⚠".yellow(),
                            provider.name(),
                            e.to_string().red()
                        );
                    }
                    // 手動指定時は失敗記録をスキップ
                    if !self.provider_override {
                        self.record_provider_failure(provider);
                    }
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or(AppError::NoAiProviderInstalled))
    }

    /// 特定のAIプロバイダーを呼び出し
    fn call_provider(&self, provider: &AiProvider, prompt: &str) -> Result<String, AppError> {
        // opencode は一時ファイル経由でプロンプトを渡す（stdinサポートが不明確なため）
        // TempFile の RAII ガードにより、どのパスで return しても自動クリーンアップされる
        let temp_file = if matches!(provider, AiProvider::Opencode) {
            let temp_dir = std::env::temp_dir();
            let temp_path = temp_dir.join(format!("git-sc-prompt-{}.txt", std::process::id()));
            Some(TempFile::create_with_content(temp_path, prompt.as_bytes())?)
        } else {
            None
        };

        // プロバイダー固有のコマンドを構築
        let (mut cmd, uses_stdin) =
            self.build_provider_command(provider, prompt, temp_file.as_ref())?;

        // デバッグモード: 実行するコマンドを表示
        if self.debug {
            self.print_debug_command(provider, prompt, temp_file.as_ref());
        }

        // プロセスを起動
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AppError::AiProviderError(format!("{} not found", provider.name()))
            } else {
                AppError::AiProviderError(e.to_string())
            }
        })?;

        // stdin にプロンプトを書き込み (codex, claude)
        Self::write_stdin_prompt(&mut child, uses_stdin, prompt)?;

        // stdout/stderr をスレッドで読み取り、タイムアウト付きで完了を待機
        let (exit_status, stdout_str, stderr_str) =
            self.run_process_with_timeout(&mut child, provider)?;

        // 出力を検証してメッセージを返す
        Self::process_provider_output(provider, exit_status, &stdout_str, &stderr_str)
    }

    /// プロバイダー固有の Command を構築する。
    /// 返り値: (Command, stdin を使用するか)
    fn build_provider_command(
        &self,
        provider: &AiProvider,
        prompt: &str,
        temp_file: Option<&TempFile>,
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
            AiProvider::Gemini => {
                if !self.models.gemini.is_empty() {
                    cmd.args(["-m", &self.models.gemini]);
                }
                if self.debug {
                    cmd.arg("--debug");
                }
                cmd.args(["-p", prompt]);
                false
            }
            AiProvider::Codex => {
                cmd.arg("exec");
                if !self.models.codex.is_empty() {
                    cmd.args(["--model", &self.models.codex]);
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
    fn print_debug_command(
        &self,
        provider: &AiProvider,
        prompt: &str,
        temp_file: Option<&TempFile>,
    ) {
        let cmd_str =
            self.format_command_for_debug(provider, prompt, temp_file.map(|tf| tf.path()));
        println!();
        println!("{}", "=== DEBUG: AI Provider Command ===".yellow().bold());
        println!("{}", "─".repeat(50).dimmed());
        println!("{}", cmd_str.cyan());
        // 一時ファイル使用時はファイル情報を表示
        if let Some(tf) = temp_file {
            match fs::metadata(tf.path()) {
                Ok(meta) => println!(
                    "  {} temp_file: {} ({} bytes)",
                    "✓".green(),
                    tf.path().display(),
                    meta.len()
                ),
                Err(e) => println!("  {} temp_file: {} ({})", "✗".red(), tf.path().display(), e),
            }
        }
        println!("{}", "─".repeat(50).dimmed());
        println!();
    }

    /// stdin にプロンプトを書き込む (codex, claude 用)
    fn write_stdin_prompt(
        child: &mut Child,
        uses_stdin: bool,
        prompt: &str,
    ) -> Result<(), AppError> {
        if uses_stdin && let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(prompt.as_bytes())
                .map_err(|e| AppError::AiProviderError(format!("Failed to write prompt: {}", e)))?;
        }
        Ok(())
    }

    /// stdout/stderr をスレッドで読み取り、タイムアウト付きでプロセス完了を待機する。
    /// 返り値: (ExitStatus, stdout, stderr)
    fn run_process_with_timeout(
        &self,
        child: &mut Child,
        provider: &AiProvider,
    ) -> Result<(ExitStatus, String, String), AppError> {
        let is_debug = self.debug;

        if is_debug {
            println!();
            println!(
                "{}",
                "=== DEBUG: AI Provider Output (streaming) ==="
                    .yellow()
                    .bold()
            );
            println!("{}", "─".repeat(50).dimmed());
        }

        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();

        // stdout 読み取りスレッド（デバッグ時はリアルタイム表示）
        let stdout_thread = std::thread::spawn(move || {
            use std::io::BufRead;
            let mut buf = String::new();
            if let Some(pipe) = stdout_pipe {
                let reader = std::io::BufReader::new(pipe);
                for line in reader.lines().map_while(Result::ok) {
                    if is_debug {
                        println!("  {}", line);
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
        let timeout = std::time::Duration::from_secs(self.timeout_seconds);
        let start = std::time::Instant::now();
        let exit_status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    if start.elapsed() > timeout {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(AppError::AiProviderError(format!(
                            "{} timed out after {} seconds",
                            provider.name(),
                            self.timeout_seconds
                        )));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(e) => {
                    return Err(AppError::AiProviderError(format!(
                        "Failed to wait for process: {}",
                        e
                    )));
                }
            }
        };

        let stdout_str = stdout_thread.join().unwrap_or_default();
        let stderr_str = stderr_thread.join().unwrap_or_default();

        if is_debug {
            println!("{}", "─".repeat(50).dimmed());
            println!(
                "  {}: {}",
                "exit code".dimmed(),
                exit_status.to_string().cyan()
            );
            println!();
        }

        Ok((exit_status, stdout_str, stderr_str))
    }

    /// プロバイダーの出力を検証し、クリーンアップ済みのメッセージを返す
    fn process_provider_output(
        provider: &AiProvider,
        exit_status: ExitStatus,
        stdout_str: &str,
        stderr_str: &str,
    ) -> Result<String, AppError> {
        if !exit_status.success() {
            let error_msg = Self::extract_error(stderr_str, provider);
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

    /// Apple Intelligence をネイティブ呼び出し（fm-rs経由）
    #[cfg(all(target_os = "macos", feature = "apple-ai"))]
    fn call_apple_intelligence_native(prompt: &str, language: &str) -> Result<String, AppError> {
        let model = fm_rs::SystemLanguageModel::new().map_err(|e| {
            AppError::AiProviderError(format!("Failed to initialize Apple Intelligence: {}", e))
        })?;

        model.ensure_available().map_err(|_| {
            AppError::AiProviderError(
                "Apple Intelligence is not available (requires macOS 26+, Apple Silicon, Apple Intelligence enabled)".to_string()
            )
        })?;

        let instructions = format!(
            "You are a Git commit message generator. \
            Output ONLY a commit message in {language}. No explanation, no markdown, no code blocks.\n\n\
            CRITICAL FORMAT RULE: The commit message MUST start with a type prefix followed by a COLON and a SPACE.\n\
            Correct: \"feat: add user authentication\"\n\
            Correct: \"fix: resolve null pointer error\"\n\
            WRONG:   \"feat add user authentication\" (missing colon)\n\
            WRONG:   \"Add user authentication\" (missing prefix)\n\n\
            The format is ALWAYS: <type>: <description>\n\n\
            Available types and when to use each:\n{guide}\n\n\
            Examples:\n\
            - New function/struct/feature → feat: <description>\n\
            - Bug fix/error correction → fix: <description>\n\
            - Documentation/comments/README → docs: <description>\n\
            - Formatting/whitespace/import order → style: <description>\n\
            - Code restructuring (no behavior change) → refactor: <description>\n\
            - Performance improvement/caching → perf: <description>\n\
            - Adding/updating tests → test: <description>\n\
            - Dependencies/Cargo.toml/Makefile → build: <description>\n\
            - CI/CD workflow changes → ci: <description>\n\
            - .gitignore/LICENSE/config files → chore: <description>\n\
            - Removing/reverting code → revert: <description>\n\n\
            Style rules:\n\
            - Use short, direct phrases\n\
            - Do NOT end with a period\n\
            - Do NOT use polite or formal sentence endings\n\
            - Keep under 72 characters",
            language = language,
            guide = CONVENTIONAL_COMMITS_GUIDE
        );

        let session = fm_rs::Session::with_instructions(&model, &instructions)
            .map_err(|e| AppError::AiProviderError(format!("Failed to create session: {}", e)))?;

        let options = fm_rs::GenerationOptions::builder().temperature(0.3).build();

        let response = session.respond(prompt, &options).map_err(|e| {
            AppError::AiProviderError(format!("Apple Intelligence generation failed: {}", e))
        })?;

        let message = response.content().trim().to_string();
        let message = Self::clean_message(&message);

        if message.is_empty() {
            return Err(AppError::AiProviderError(
                "Apple Intelligence returned an empty response".to_string(),
            ));
        }

        Ok(message)
    }

    /// stderrからエラーメッセージを抽出
    fn extract_error(stderr: &str, provider: &AiProvider) -> String {
        match provider {
            AiProvider::Gemini => {
                // [API Error: ...] パターンを優先的に探す
                for line in stderr.lines() {
                    if line.starts_with("[API Error:") {
                        return line.to_string();
                    }
                }
                // "critical error occurred" パターンを探す
                // 例: "An unexpected critical error occurred:Error: ..."
                for line in stderr.lines() {
                    let trimmed = line.trim();
                    if trimmed.contains("critical error") || trimmed.contains("Error:") {
                        return trimmed.to_string();
                    }
                }
                "Gemini API request failed".to_string()
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

    /// 生成されたメッセージをクリーンアップ
    fn clean_message(message: &str) -> String {
        let message = message.trim();

        // マークダウンのコードブロックがある場合は削除
        let message = if message.starts_with("```") && message.ends_with("```") {
            let lines: Vec<&str> = message.lines().collect();
            if lines.len() > 2 {
                lines[1..lines.len() - 1].join("\n")
            } else {
                message.to_string()
            }
        } else {
            message.to_string()
        };

        // 先頭と末尾の引用符がある場合は削除
        let message = message.trim_matches('"').trim_matches('\'');

        let message = message.trim().to_string();

        // 件名と本文の間に空行を保証
        Self::ensure_body_separator(&message)
    }

    /// 件名と本文の間に空行があることを保証する
    fn ensure_body_separator(message: &str) -> String {
        let lines: Vec<&str> = message.lines().collect();

        // 1行以下の場合はそのまま返す
        if lines.len() <= 1 {
            return message.to_string();
        }

        // 2行目が空行の場合はそのまま返す
        if lines[1].trim().is_empty() {
            return message.to_string();
        }

        // 2行目が空行でない場合は、件名の後に空行を挿入
        let mut result = String::new();
        result.push_str(lines[0]);
        result.push_str("\n\n");
        result.push_str(&lines[1..].join("\n"));
        result
    }
}

impl Default for AiService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    #[test]
    fn test_ai_provider_name() {
        assert_eq!(AiProvider::Gemini.name(), "Gemini CLI");
        assert_eq!(AiProvider::Codex.name(), "Codex CLI");
        assert_eq!(AiProvider::Claude.name(), "Claude Code");
        assert_eq!(AiProvider::Opencode.name(), "opencode");
        assert_eq!(AiProvider::AppleIntelligence.name(), "Apple Intelligence");
    }

    #[test]
    fn test_ai_provider_command() {
        assert_eq!(AiProvider::Gemini.command(), "gemini");
        assert_eq!(AiProvider::Codex.command(), "codex");
        assert_eq!(AiProvider::Claude.command(), "claude");
        assert_eq!(AiProvider::Opencode.command(), "opencode");
        assert_eq!(AiProvider::AppleIntelligence.command(), "apple-ai");
    }

    #[rstest]
    #[case("gemini", Some(AiProvider::Gemini))]
    #[case("GEMINI", Some(AiProvider::Gemini))]
    #[case("Gemini", Some(AiProvider::Gemini))]
    #[case("codex", Some(AiProvider::Codex))]
    #[case("claude", Some(AiProvider::Claude))]
    #[case("opencode", Some(AiProvider::Opencode))]
    #[case("OPENCODE", Some(AiProvider::Opencode))]
    #[case("apple-intelligence", Some(AiProvider::AppleIntelligence))]
    #[case("apple_intelligence", Some(AiProvider::AppleIntelligence))]
    #[case("APPLE-INTELLIGENCE", Some(AiProvider::AppleIntelligence))]
    #[case("unknown", None)]
    #[case("", None)]
    fn test_ai_provider_from_str(#[case] input: &str, #[case] expected: Option<AiProvider>) {
        let result = AiProvider::from_str(input);
        match (result, expected) {
            (Some(r), Some(e)) => assert_eq!(r.name(), e.name()),
            (None, None) => {}
            _ => panic!("Mismatch for input: {}", input),
        }
    }

    #[test]
    fn test_ai_service_new() {
        let service = AiService::new();
        assert_eq!(service.language, "Japanese");
        let expected_len = if cfg!(all(target_os = "macos", feature = "apple-ai")) {
            5
        } else {
            4
        };
        assert_eq!(service.providers.len(), expected_len);
    }

    #[test]
    fn test_ai_service_set_language() {
        let mut service = AiService::new();
        service.set_language("English".to_string());
        assert_eq!(service.language, "English");
    }

    #[test]
    fn test_set_provider_override() {
        let mut service = AiService::new();
        service.set_provider_override(AiProvider::Claude);
        assert_eq!(service.providers.len(), 1);
        assert!(matches!(service.providers[0], AiProvider::Claude));
        assert!(service.provider_override);
    }

    #[test]
    fn test_set_provider_override_replaces_all() {
        let mut service = AiService::new();
        let original_len = service.providers.len();
        assert!(original_len > 1);
        service.set_provider_override(AiProvider::Gemini);
        assert_eq!(service.providers.len(), 1);
        assert!(matches!(service.providers[0], AiProvider::Gemini));
    }

    #[rstest]
    #[case(Some("conventional"), "Use Conventional Commits format")]
    #[case(Some("bracket"), "Use bracket prefix format")]
    #[case(Some("colon"), "Use colon prefix format")]
    #[case(Some("emoji"), "Use emoji prefix format")]
    #[case(Some("plain"), "Do NOT use any prefix")]
    #[case(Some("none"), "Do NOT use any prefix")]
    fn test_build_prompt_prefix_types(#[case] prefix_type: Option<&str>, #[case] expected: &str) {
        let diff = "test diff";
        let recent_commits: Vec<String> = vec![];
        let prompt =
            AiService::build_prompt(diff, &recent_commits, "Japanese", prefix_type, false, None);
        assert!(
            prompt.contains(expected),
            "Prompt should contain '{}' for prefix_type {:?}",
            expected,
            prefix_type
        );
    }

    #[test]
    fn test_build_prompt_custom_prefix() {
        let diff = "test diff";
        let recent_commits: Vec<String> = vec![];
        let prompt = AiService::build_prompt(
            diff,
            &recent_commits,
            "Japanese",
            Some("JIRA-123: "),
            false,
            None,
        );
        assert!(prompt.contains("Use the following prefix format: JIRA-123:"));
    }

    #[test]
    fn test_build_prompt_auto_mode_empty_commits() {
        let diff = "test diff";
        let recent_commits: Vec<String> = vec![];
        let prompt = AiService::build_prompt(diff, &recent_commits, "Japanese", None, false, None);
        assert!(prompt.contains("No recent commits found"));
        assert!(prompt.contains("Conventional Commits format"));
    }

    #[test]
    fn test_build_prompt_auto_mode_with_commits() {
        let diff = "test diff";
        let recent_commits = vec![
            "feat: add new feature".to_string(),
            "fix: resolve bug".to_string(),
        ];
        let prompt = AiService::build_prompt(diff, &recent_commits, "Japanese", None, false, None);
        assert!(prompt.contains("Recent commit messages in this repository"));
        assert!(prompt.contains("1. feat: add new feature"));
        assert!(prompt.contains("2. fix: resolve bug"));
        assert!(prompt.contains("match their style/format"));
    }

    #[test]
    fn test_build_prompt_contains_diff() {
        let diff = "--- a/file.rs\n+++ b/file.rs\n+new line";
        let recent_commits: Vec<String> = vec![];
        let prompt = AiService::build_prompt(
            diff,
            &recent_commits,
            "English",
            Some("conventional"),
            false,
            None,
        );
        assert!(prompt.contains(diff));
        assert!(prompt.contains("<changes>"));
    }

    #[test]
    fn test_build_prompt_contains_language() {
        let diff = "test diff";
        let recent_commits: Vec<String> = vec![];

        let prompt_ja = AiService::build_prompt(
            diff,
            &recent_commits,
            "Japanese",
            Some("conventional"),
            false,
            None,
        );
        assert!(prompt_ja.contains("Japanese"));

        let prompt_en = AiService::build_prompt(
            diff,
            &recent_commits,
            "English",
            Some("conventional"),
            false,
            None,
        );
        assert!(prompt_en.contains("English"));
    }

    #[test]
    fn test_build_prompt_with_body_true() {
        let diff = "test diff";
        let recent_commits: Vec<String> = vec![];
        let prompt = AiService::build_prompt(
            diff,
            &recent_commits,
            "Japanese",
            Some("conventional"),
            true,
            None,
        );
        // Body モードでは body 関連の指示が含まれる
        assert!(prompt.contains("Body"));
        assert!(prompt.contains("bullet point"));
        assert!(prompt.contains("Subject line"));
        assert!(!prompt.contains("single line"));
    }

    #[test]
    fn test_build_prompt_with_body_false() {
        let diff = "test diff";
        let recent_commits: Vec<String> = vec![];
        let prompt = AiService::build_prompt(
            diff,
            &recent_commits,
            "Japanese",
            Some("conventional"),
            false,
            None,
        );
        // 通常モードでは single line の指示が含まれる
        assert!(prompt.contains("single line"));
        assert!(!prompt.contains("bullet point"));
    }

    #[test]
    fn test_build_prompt_body_with_auto_mode() {
        let diff = "test diff";
        let recent_commits = vec!["feat: previous commit".to_string()];
        let prompt = AiService::build_prompt(diff, &recent_commits, "English", None, true, None);
        // Auto モードでも body 指示が含まれる
        assert!(prompt.contains("Body"));
        assert!(prompt.contains("bullet point"));
    }

    #[test]
    fn test_build_prompt_with_agent_context() {
        let diff = "test diff";
        let recent_commits: Vec<String> = vec![];
        let prompt = AiService::build_prompt(
            diff,
            &recent_commits,
            "Japanese",
            Some("conventional"),
            false,
            Some("Refactored the authentication module to use JWT tokens"),
        );
        assert!(prompt.contains("<agent-context>"));
        assert!(prompt.contains("</agent-context>"));
        assert!(prompt.contains("Refactored the authentication module to use JWT tokens"));
        assert!(prompt.contains("IMPORTANT: Use the <agent-context> above as the primary source"));
        // Agent context should appear before Changes section
        let ctx_pos = prompt.find("<agent-context>").unwrap();
        let changes_pos = prompt.find("<changes>").unwrap();
        assert!(
            ctx_pos < changes_pos,
            "Agent context should appear before Changes section"
        );
    }

    #[test]
    fn test_build_prompt_without_agent_context() {
        let diff = "test diff";
        let recent_commits: Vec<String> = vec![];
        let prompt = AiService::build_prompt(
            diff,
            &recent_commits,
            "Japanese",
            Some("conventional"),
            false,
            None,
        );
        assert!(!prompt.contains("<agent-context>"));
    }

    #[test]
    fn test_build_prompt_with_empty_agent_context() {
        let diff = "test diff";
        let recent_commits: Vec<String> = vec![];
        let prompt = AiService::build_prompt(
            diff,
            &recent_commits,
            "Japanese",
            Some("conventional"),
            false,
            Some(""),
        );
        // Empty agent context should not add the section
        assert!(!prompt.contains("<agent-context>"));
    }

    #[test]
    fn test_clean_message_basic() {
        let message = "feat: add new feature";
        assert_eq!(AiService::clean_message(message), "feat: add new feature");
    }

    #[test]
    fn test_clean_message_trim_whitespace() {
        let message = "  feat: add new feature  \n";
        assert_eq!(AiService::clean_message(message), "feat: add new feature");
    }

    #[test]
    fn test_clean_message_remove_code_block() {
        let message = "```\nfeat: add new feature\n```";
        assert_eq!(AiService::clean_message(message), "feat: add new feature");
    }

    #[test]
    fn test_clean_message_remove_quotes() {
        let message = "\"feat: add new feature\"";
        assert_eq!(AiService::clean_message(message), "feat: add new feature");

        let message = "'feat: add new feature'";
        assert_eq!(AiService::clean_message(message), "feat: add new feature");
    }

    #[test]
    fn test_clean_message_empty() {
        assert_eq!(AiService::clean_message(""), "");
    }

    #[test]
    fn test_clean_message_only_whitespace() {
        assert_eq!(AiService::clean_message("   \n  \n  "), "");
    }

    #[test]
    fn test_clean_message_single_line() {
        assert_eq!(AiService::clean_message("feat: simple"), "feat: simple");
    }

    #[test]
    fn test_clean_message_multiline() {
        let message = "feat: add feature\n\n- detail 1\n- detail 2";
        assert_eq!(AiService::clean_message(message), message);
    }

    #[test]
    fn test_clean_message_nested_quotes() {
        let message = "\"'feat: add feature'\"";
        assert_eq!(AiService::clean_message(message), "feat: add feature");
    }

    #[test]
    fn test_clean_message_partial_fence() {
        // 開始フェンスのみで閉じフェンスがない場合、ensure_body_separatorで空行挿入
        let message = "```\nfeat: add feature";
        assert_eq!(
            AiService::clean_message(message),
            "```\n\nfeat: add feature"
        );
    }

    #[test]
    fn test_clean_message_code_block_two_lines() {
        // 開始と終了のみの2行コードブロック、ensure_body_separatorで空行挿入
        let message = "```\n```";
        assert_eq!(AiService::clean_message(message), "```\n\n```");
    }

    #[test]
    fn test_clean_message_code_block_multiline() {
        let message = "```\nfeat: add feature\n\n- detail 1\n```";
        assert_eq!(
            AiService::clean_message(message),
            "feat: add feature\n\n- detail 1"
        );
    }

    #[test]
    fn test_clean_message_body_with_empty_line() {
        let message = "feat: add feature\n\nBody text here";
        assert_eq!(AiService::clean_message(message), message);
    }

    #[test]
    fn test_clean_message_body_without_empty_line() {
        // 件名と本文の間に空行がない場合、自動挿入される
        let message = "feat: add feature\nBody text here";
        assert_eq!(
            AiService::clean_message(message),
            "feat: add feature\n\nBody text here"
        );
    }

    #[test]
    fn test_clean_message_body_multiple_lines_without_separator() {
        let message = "feat: add feature\n- detail 1\n- detail 2";
        assert_eq!(
            AiService::clean_message(message),
            "feat: add feature\n\n- detail 1\n- detail 2"
        );
    }

    #[test]
    fn test_ensure_body_separator_empty() {
        assert_eq!(AiService::ensure_body_separator(""), "");
    }

    #[test]
    fn test_ensure_body_separator_single_line() {
        assert_eq!(
            AiService::ensure_body_separator("feat: add feature"),
            "feat: add feature"
        );
    }

    #[test]
    fn test_ensure_body_separator_already_has_separator() {
        let message = "feat: add feature\n\nBody";
        assert_eq!(AiService::ensure_body_separator(message), message);
    }

    #[test]
    fn test_ensure_body_separator_missing_separator() {
        let message = "feat: add feature\nBody";
        assert_eq!(
            AiService::ensure_body_separator(message),
            "feat: add feature\n\nBody"
        );
    }

    #[test]
    fn test_ensure_body_separator_three_lines_no_separator() {
        let message = "feat: add feature\n- detail 1\n- detail 2";
        assert_eq!(
            AiService::ensure_body_separator(message),
            "feat: add feature\n\n- detail 1\n- detail 2"
        );
    }

    #[test]
    fn test_ensure_body_separator_whitespace_only_second_line() {
        let message = "feat: add feature\n   \nBody";
        // 空白のみの2行目は空行扱い
        assert_eq!(AiService::ensure_body_separator(message), message);
    }

    #[test]
    fn test_clean_message_code_block_with_language() {
        let message = "```text\nfeat: add new feature\n```";
        assert_eq!(AiService::clean_message(message), "feat: add new feature");
    }

    #[test]
    fn test_extract_error_gemini_api_error() {
        let stderr = "Some warning\n[API Error: Rate limit exceeded]\nMore text";
        let error = AiService::extract_error(stderr, &AiProvider::Gemini);
        assert_eq!(error, "[API Error: Rate limit exceeded]");
    }

    #[test]
    fn test_extract_error_gemini_generic() {
        let stderr = "Some generic error";
        let error = AiService::extract_error(stderr, &AiProvider::Gemini);
        assert_eq!(error, "Gemini API request failed");
    }

    #[test]
    fn test_extract_error_codex() {
        let stderr = "\nError: Something went wrong\nMore details";
        let error = AiService::extract_error(stderr, &AiProvider::Codex);
        assert_eq!(error, "Error: Something went wrong");
    }

    #[test]
    fn test_extract_error_claude() {
        let stderr = "Claude error message";
        let error = AiService::extract_error(stderr, &AiProvider::Claude);
        assert_eq!(error, "Claude error message");
    }

    #[test]
    fn test_extract_error_whitespace_only() {
        let stderr = "   \n  \n  ";
        let error = AiService::extract_error(stderr, &AiProvider::Claude);
        assert_eq!(error, "API request failed");
    }

    #[test]
    fn test_extract_error_gemini_license_error() {
        let stderr = "Warning: something\nAn unexpected critical error occurred:Error: license check failed\nMore info";
        let error = AiService::extract_error(stderr, &AiProvider::Gemini);
        assert!(error.contains("critical error") || error.contains("Error:"));
    }

    #[test]
    fn test_extract_error_gemini_critical_error() {
        let stderr = "An unexpected critical error occurred:Error: something bad";
        let error = AiService::extract_error(stderr, &AiProvider::Gemini);
        assert!(error.contains("critical error"));
    }

    #[test]
    fn test_extract_error_gemini_multiple_api_errors() {
        let stderr = "[API Error: first]\n[API Error: second]";
        let error = AiService::extract_error(stderr, &AiProvider::Gemini);
        // 最初の API Error を返す
        assert_eq!(error, "[API Error: first]");
    }

    #[test]
    fn test_extract_error_codex_auth_error() {
        let stderr =
            "Reading prompt from stdin...\nERROR: Your access token could not be refreshed";
        let error = AiService::extract_error(stderr, &AiProvider::Codex);
        assert!(error.starts_with("ERROR:"));
    }

    #[test]
    fn test_extract_error_codex_error_prefix_priority() {
        let stderr = "error in something\nERROR: specific error message";
        let error = AiService::extract_error(stderr, &AiProvider::Codex);
        // "ERROR:" で始まる行が優先される
        assert_eq!(error, "ERROR: specific error message");
    }

    #[test]
    fn test_extract_error_codex_reconnecting_skipped() {
        let stderr = "Reconnecting to server...\nReading prompt from stdin...\nActual error here";
        let error = AiService::extract_error(stderr, &AiProvider::Codex);
        assert_eq!(error, "Actual error here");
    }

    #[test]
    fn test_extract_error_opencode_empty() {
        let stderr = "";
        let error = AiService::extract_error(stderr, &AiProvider::Opencode);
        assert_eq!(error, "opencode request failed");
    }

    #[test]
    fn test_extract_error_opencode_generic() {
        let stderr = "some log message";
        let error = AiService::extract_error(stderr, &AiProvider::Opencode);
        assert_eq!(error, "some log message");
    }

    #[test]
    fn test_extract_error_opencode_with_error() {
        let stderr = "log\nerror: connection failed\nmore log";
        let error = AiService::extract_error(stderr, &AiProvider::Opencode);
        assert_eq!(error, "error: connection failed");
    }

    #[test]
    fn test_extract_error_opencode_with_failed() {
        let stderr = "some failed operation";
        let error = AiService::extract_error(stderr, &AiProvider::Opencode);
        assert_eq!(error, "some failed operation");
    }

    #[test]
    fn test_extract_error_apple_intelligence_empty() {
        let stderr = "";
        let error = AiService::extract_error(stderr, &AiProvider::AppleIntelligence);
        assert_eq!(error, "Apple Intelligence request failed");
    }

    #[test]
    fn test_extract_error_apple_intelligence_with_error() {
        let stderr = "Info message\nError: model not available\nDetails";
        let error = AiService::extract_error(stderr, &AiProvider::AppleIntelligence);
        assert_eq!(error, "Error: model not available");
    }

    #[test]
    fn test_extract_error_apple_intelligence_generic() {
        let stderr = "some generic info\nno Error: prefix here";
        let error = AiService::extract_error(stderr, &AiProvider::AppleIntelligence);
        assert_eq!(error, "some generic info");
    }

    #[test]
    fn test_extract_error_empty_stderr() {
        let stderr = "";
        // Claude は "API request failed" を返す
        let error = AiService::extract_error(stderr, &AiProvider::Claude);
        assert_eq!(error, "API request failed");
        // Codex は "Codex API request failed" を返す
        let error = AiService::extract_error(stderr, &AiProvider::Codex);
        assert_eq!(error, "Codex API request failed");
    }

    // ============================================================
    // AiService::from_config のテスト
    // ============================================================

    #[test]
    fn test_ai_service_from_config_default() {
        let config = Config::default();
        let service = AiService::from_config(&config);

        assert_eq!(service.language, "Japanese");
        let expected_len = if cfg!(all(target_os = "macos", feature = "apple-ai")) {
            5
        } else {
            4
        };
        assert_eq!(service.providers.len(), expected_len);
        assert_eq!(service.models.gemini, "gemini-2.5-flash-lite");
        assert_eq!(service.models.codex, "gpt-5.1-codex-mini");
        assert_eq!(service.models.claude, "haiku");
        assert_eq!(service.models.opencode, "");
        assert_eq!(service.timeout_seconds, 60);
    }

    #[test]
    fn test_ai_service_from_config_custom_providers() {
        let config = Config {
            providers: vec!["claude".to_string(), "gemini".to_string()],
            ..Default::default()
        };
        let service = AiService::from_config(&config);

        // reorder_providersで順序が変わる可能性があるため、含有のみ検証
        assert_eq!(service.providers.len(), 2);
        let names: Vec<&str> = service.providers.iter().map(|p| p.name()).collect();
        assert!(names.contains(&"Claude Code"));
        assert!(names.contains(&"Gemini CLI"));
    }

    #[test]
    fn test_ai_service_from_config_invalid_providers_fallback() {
        let config = Config {
            providers: vec!["invalid".to_string(), "unknown".to_string()],
            ..Default::default()
        };
        let service = AiService::from_config(&config);

        // 無効なプロバイダーのみの場合はデフォルトにフォールバック
        let expected_len = if cfg!(all(target_os = "macos", feature = "apple-ai")) {
            5
        } else {
            4
        };
        assert_eq!(service.providers.len(), expected_len);
    }

    #[test]
    fn test_ai_service_from_config_custom_language() {
        let config = Config {
            language: "English".to_string(),
            ..Default::default()
        };
        let service = AiService::from_config(&config);

        assert_eq!(service.language, "English");
    }

    #[test]
    fn test_ai_service_from_config_custom_models() {
        let mut config = Config::default();
        config.models.gemini = "pro".to_string();
        config.models.codex = "gpt-4".to_string();
        config.models.claude = "opus".to_string();
        let service = AiService::from_config(&config);

        assert_eq!(service.models.gemini, "pro");
        assert_eq!(service.models.codex, "gpt-4");
        assert_eq!(service.models.claude, "opus");
    }

    // ============================================================
    // AiService::default のテスト
    // ============================================================

    #[test]
    fn test_ai_service_default() {
        let service = AiService::default();

        assert_eq!(service.language, "Japanese");
        let expected_len = if cfg!(all(target_os = "macos", feature = "apple-ai")) {
            5
        } else {
            4
        };
        assert_eq!(service.providers.len(), expected_len);
        assert_eq!(service.providers[0].name(), "opencode");
        assert_eq!(service.providers[1].name(), "Gemini CLI");
        assert_eq!(service.providers[2].name(), "Codex CLI");
        assert_eq!(service.providers[3].name(), "Claude Code");
        if cfg!(all(target_os = "macos", feature = "apple-ai")) {
            assert_eq!(service.providers[4].name(), "Apple Intelligence");
        }
    }

    // ============================================================
    // format_command_for_debug テスト
    // ============================================================

    #[test]
    fn test_format_command_for_debug_gemini() {
        let service = AiService::new();
        let cmd = service.format_command_for_debug(&AiProvider::Gemini, "test prompt", None);
        assert!(cmd.contains("gemini -m"));
        assert!(cmd.contains("-p 'test prompt'"));
    }

    #[test]
    fn test_format_command_for_debug_codex() {
        let service = AiService::new();
        let cmd = service.format_command_for_debug(&AiProvider::Codex, "test prompt", None);
        assert!(cmd.contains("codex exec --model"));
        assert!(cmd.contains("echo 'test prompt'"));
    }

    #[test]
    fn test_format_command_for_debug_claude() {
        let service = AiService::new();
        let cmd = service.format_command_for_debug(&AiProvider::Claude, "test prompt", None);
        assert!(cmd.contains("claude --model"));
        assert!(cmd.contains("-p"));
        assert!(cmd.contains("echo 'test prompt'"));
    }

    #[test]
    fn test_format_command_for_debug_opencode() {
        // デフォルトは空モデルなので -m なし
        let service = AiService::new();
        let temp_path = std::path::Path::new("/tmp/git-sc-prompt-12345.txt");
        let cmd =
            service.format_command_for_debug(&AiProvider::Opencode, "test prompt", Some(temp_path));
        assert!(cmd.contains("opencode run"));
        assert!(!cmd.contains("-m"));
        assert!(cmd.contains("-f '/tmp/git-sc-prompt-12345.txt'"));
    }

    #[test]
    fn test_format_command_for_debug_opencode_with_model() {
        let mut service = AiService::new();
        service.models.opencode = "opencode/some-model".to_string();
        let temp_path = std::path::Path::new("/tmp/git-sc-prompt-12345.txt");
        let cmd =
            service.format_command_for_debug(&AiProvider::Opencode, "test prompt", Some(temp_path));
        assert!(cmd.contains("opencode run"));
        assert!(cmd.contains("-m 'opencode/some-model'"));
        assert!(cmd.contains("-f '/tmp/git-sc-prompt-12345.txt'"));
    }

    #[test]
    fn test_format_command_for_debug_opencode_no_path() {
        let service = AiService::new();
        let cmd = service.format_command_for_debug(&AiProvider::Opencode, "test prompt", None);
        assert!(cmd.contains("opencode run"));
        assert!(cmd.contains("-f '<temp_file>'"));
    }

    #[test]
    fn test_format_command_for_debug_apple_intelligence() {
        let service = AiService::new();
        let cmd =
            service.format_command_for_debug(&AiProvider::AppleIntelligence, "test prompt", None);
        assert!(cmd.contains("apple-ai"));
        assert!(cmd.contains("echo 'test prompt'"));
    }

    #[test]
    fn test_format_command_for_debug_prompt_with_single_quotes() {
        let service = AiService::new();
        let cmd = service.format_command_for_debug(&AiProvider::Gemini, "it's a test", None);
        assert!(cmd.contains("it'\\''s a test"));
    }

    #[test]
    fn test_format_command_for_debug_gemini_empty_model() {
        let mut service = AiService::new();
        service.models.gemini = String::new();
        let cmd = service.format_command_for_debug(&AiProvider::Gemini, "test", None);
        assert!(cmd.contains("gemini -p 'test'"));
        assert!(!cmd.contains("-m"));
    }

    #[test]
    fn test_format_command_for_debug_codex_empty_model() {
        let mut service = AiService::new();
        service.models.codex = String::new();
        let cmd = service.format_command_for_debug(&AiProvider::Codex, "test", None);
        assert!(cmd.contains("codex exec"));
        assert!(!cmd.contains("--model"));
    }

    #[test]
    fn test_format_command_for_debug_claude_empty_model() {
        let mut service = AiService::new();
        service.models.claude = String::new();
        let cmd = service.format_command_for_debug(&AiProvider::Claude, "test", None);
        assert!(cmd.contains("claude"));
        assert!(cmd.contains("-p"));
        assert!(!cmd.contains("--model"));
    }

    #[test]
    fn test_format_command_for_debug_opencode_empty_model() {
        let mut service = AiService::new();
        service.models.opencode = String::new();
        let temp_path = std::path::Path::new("/tmp/test.txt");
        let cmd = service.format_command_for_debug(&AiProvider::Opencode, "test", Some(temp_path));
        assert!(cmd.contains("opencode run"));
        assert!(!cmd.contains("-m"));
        assert!(cmd.contains("-f '/tmp/test.txt'"));
    }

    // ============================================================
    // AiProvider config_key のテスト
    // ============================================================

    #[test]
    fn test_ai_provider_config_key() {
        assert_eq!(AiProvider::Gemini.config_key(), "gemini");
        assert_eq!(AiProvider::Codex.config_key(), "codex");
        assert_eq!(AiProvider::Claude.config_key(), "claude");
        assert_eq!(AiProvider::Opencode.config_key(), "opencode");
        assert_eq!(AiProvider::AppleIntelligence.config_key(), "apple-ai");
    }

    // ============================================================
    // AiService set_debug のテスト
    // ============================================================

    #[test]
    fn test_ai_service_set_debug() {
        let mut service = AiService::new();
        assert!(!service.debug);
        service.set_debug(true);
        assert!(service.debug);
        service.set_debug(false);
        assert!(!service.debug);
    }

    // ============================================================
    // AiService language のテスト
    // ============================================================

    #[test]
    fn test_ai_service_language_getter() {
        let service = AiService::new();
        assert_eq!(service.language(), "Japanese");
    }

    #[test]
    fn test_ai_service_language_after_set() {
        let mut service = AiService::new();
        service.set_language("French".to_string());
        assert_eq!(service.language(), "French");
    }

    // ============================================================
    // AiService from_config with cooldown のテスト
    // ============================================================

    #[test]
    fn test_ai_service_from_config_custom_cooldown() {
        let config = Config {
            provider_cooldown_minutes: 30,
            ..Default::default()
        };
        let service = AiService::from_config(&config);
        assert_eq!(service.cooldown_minutes, 30);
    }

    #[test]
    fn test_ai_service_from_config_zero_cooldown() {
        let config = Config {
            provider_cooldown_minutes: 0,
            ..Default::default()
        };
        let service = AiService::from_config(&config);
        assert_eq!(service.cooldown_minutes, 0);
    }

    // ============================================================
    // Apple Intelligence 統合テスト (cargo test --features apple-ai -- --ignored)
    // ============================================================

    /// Conventional Commits の有効なプレフィックス一覧
    #[cfg(all(target_os = "macos", feature = "apple-ai"))]
    const CONVENTIONAL_PREFIXES: &[&str] = &[
        "feat", "fix", "docs", "style", "refactor", "perf", "test", "build", "ci", "chore",
        "revert",
    ];

    /// 生成メッセージが Conventional Commits 形式かチェック
    #[cfg(all(target_os = "macos", feature = "apple-ai"))]
    fn is_conventional_commit(message: &str) -> bool {
        let first_line = message.lines().next().unwrap_or("");
        CONVENTIONAL_PREFIXES.iter().any(|p| {
            first_line.starts_with(&format!("{}:", p)) || first_line.starts_with(&format!("{}(", p))
        })
    }

    /// Apple Intelligence が利用可能ならプロンプトを送って結果を返す。
    /// 利用不可ならNoneを返す（テストスキップ用）。
    #[cfg(all(target_os = "macos", feature = "apple-ai"))]
    fn try_apple_intelligence(
        diff: &str,
        prefix_type: Option<&str>,
        with_body: bool,
    ) -> Option<Result<String, AppError>> {
        let model = fm_rs::SystemLanguageModel::new().ok()?;
        if model.ensure_available().is_err() {
            return None;
        }
        let prompt = AiService::build_prompt(diff, &[], "English", prefix_type, with_body, None);
        Some(AiService::call_apple_intelligence_native(
            &prompt, "English",
        ))
    }

    /// Apple Intelligence テスト結果を検証して出力するヘルパー。
    /// Conventional Commits 形式でなくても WARN を出すだけでテストは落とさない。
    /// (オンデバイス ~3B モデルの精度限界を許容する)
    #[cfg(all(target_os = "macos", feature = "apple-ai"))]
    fn assert_apple_intelligence_result(
        label: &str,
        result: Option<Result<String, AppError>>,
        check_body_format: bool,
    ) {
        match result {
            None => {
                println!("[SKIP] {} - Apple Intelligence not available", label);
            }
            Some(Ok(msg)) => {
                assert!(!msg.is_empty(), "[{}] Message should not be empty", label);
                if check_body_format {
                    println!("[{}] Generated:\n{}", label, msg);
                    let lines: Vec<&str> = msg.lines().collect();
                    if lines.len() > 1 && !lines[1].trim().is_empty() {
                        println!(
                            "[WARN] [{}] Second line should be empty separator, got: {:?}",
                            label, lines[1]
                        );
                    }
                } else {
                    println!("[{}] Generated: {}", label, msg);
                }
                if is_conventional_commit(&msg) {
                    println!("[OK]   [{}] Conventional Commits format detected", label);
                } else {
                    println!(
                        "[WARN] [{}] Not Conventional Commits format (on-device ~3B model limitation)",
                        label
                    );
                }
            }
            Some(Err(e)) => {
                println!("[FAIL] [{}] Generation failed (acceptable): {}", label, e);
            }
        }
    }

    // ----------------------------------------
    // feat パターン (3 cases)
    // ----------------------------------------

    #[cfg(all(target_os = "macos", feature = "apple-ai"))]
    #[rstest]
    #[case(
        "feat-1-new-function",
        "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -0,0 +1,5 @@\n+pub fn new_feature() {\n+    println!(\"new feature\");\n+}\n"
    )]
    #[case(
        "feat-2-new-struct-and-impl",
        "--- a/src/models/user.rs\n+++ b/src/models/user.rs\n@@ -0,0 +1,18 @@\n+pub struct User {\n+    pub id: u64,\n+    pub name: String,\n+    pub email: String,\n+}\n+\n+impl User {\n+    pub fn new(name: &str, email: &str) -> Self {\n+        Self {\n+            id: 0,\n+            name: name.to_string(),\n+            email: email.to_string(),\n+        }\n+    }\n+\n+    pub fn display_name(&self) -> &str {\n+        &self.name\n+    }\n+}\n"
    )]
    #[case(
        "feat-3-new-cli-flag",
        "--- a/src/cli.rs\n+++ b/src/cli.rs\n@@ -25,6 +25,10 @@\n     #[arg(short, long)]\n     pub verbose: bool,\n \n+    /// Export output as JSON format\n+    #[arg(long)]\n+    pub json: bool,\n+\n     #[arg(short, long)]\n     pub output: Option<String>,\n"
    )]
    #[test]
    #[ignore]
    fn test_apple_intelligence_feat(#[case] label: &str, #[case] diff: &str) {
        assert_apple_intelligence_result(
            label,
            try_apple_intelligence(diff, Some("conventional"), false),
            false,
        );
    }

    // ----------------------------------------
    // fix パターン (3 cases)
    // ----------------------------------------

    #[cfg(all(target_os = "macos", feature = "apple-ai"))]
    #[rstest]
    #[case(
        "fix-1-comparison-operator",
        "--- a/src/app.rs\n+++ b/src/app.rs\n@@ -10,3 +10,3 @@\n-    if count = 0 {\n+    if count == 0 {\n"
    )]
    #[case(
        "fix-2-null-pointer",
        "--- a/src/service.rs\n+++ b/src/service.rs\n@@ -42,4 +42,7 @@\n     pub fn get_user(&self, id: u64) -> Option<&User> {\n-        self.users.get(&id).unwrap()\n+        self.users.get(&id)\n     }\n"
    )]
    #[case(
        "fix-3-off-by-one",
        "--- a/src/pagination.rs\n+++ b/src/pagination.rs\n@@ -15,3 +15,3 @@\n     pub fn total_pages(&self, total_items: usize) -> usize {\n-        total_items / self.page_size\n+        (total_items + self.page_size - 1) / self.page_size\n     }\n"
    )]
    #[test]
    #[ignore]
    fn test_apple_intelligence_fix(#[case] label: &str, #[case] diff: &str) {
        assert_apple_intelligence_result(
            label,
            try_apple_intelligence(diff, Some("conventional"), false),
            false,
        );
    }

    // ----------------------------------------
    // docs パターン (3 cases)
    // ----------------------------------------

    #[cfg(all(target_os = "macos", feature = "apple-ai"))]
    #[rstest]
    #[case(
        "docs-1-readme-install",
        "--- a/README.md\n+++ b/README.md\n@@ -1,2 +1,6 @@\n # Project\n+\n+## Installation\n+\n+```bash\n+cargo install my-tool\n+```\n"
    )]
    #[case(
        "docs-2-rustdoc-comment",
        "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -5,2 +5,8 @@\n+/// Calculates the factorial of a number.\n+///\n+/// # Examples\n+///\n+/// ```\n+/// assert_eq!(factorial(5), 120);\n+/// ```\n pub fn factorial(n: u64) -> u64 {\n"
    )]
    #[case(
        "docs-3-changelog",
        "--- a/CHANGELOG.md\n+++ b/CHANGELOG.md\n@@ -1,3 +1,9 @@\n # Changelog\n \n+## [1.2.0] - 2025-01-15\n+\n+### Added\n+- New export command for JSON output\n+- Support for custom configuration files\n+\n ## [1.1.0] - 2024-12-01\n"
    )]
    #[test]
    #[ignore]
    fn test_apple_intelligence_docs(#[case] label: &str, #[case] diff: &str) {
        assert_apple_intelligence_result(
            label,
            try_apple_intelligence(diff, Some("conventional"), false),
            false,
        );
    }

    // ----------------------------------------
    // style パターン (3 cases)
    // ----------------------------------------

    #[cfg(all(target_os = "macos", feature = "apple-ai"))]
    #[rstest]
    #[case(
        "style-1-formatting",
        "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -5,4 +5,4 @@\n-fn main(){\n-let x=1;\n-let y=2;\n+fn main() {\n+    let x = 1;\n+    let y = 2;\n"
    )]
    #[case(
        "style-2-trailing-whitespace",
        "--- a/src/utils.rs\n+++ b/src/utils.rs\n@@ -1,6 +1,6 @@\n-pub fn trim(s: &str) -> &str {  \n-    s.trim()  \n-}  \n+pub fn trim(s: &str) -> &str {\n+    s.trim()\n+}\n"
    )]
    #[case(
        "style-3-import-sorting",
        "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,5 +1,5 @@\n-use std::io;\n-use std::collections::HashMap;\n-use std::fs;\n+use std::collections::HashMap;\n+use std::fs;\n+use std::io;\n"
    )]
    #[test]
    #[ignore]
    fn test_apple_intelligence_style(#[case] label: &str, #[case] diff: &str) {
        assert_apple_intelligence_result(
            label,
            try_apple_intelligence(diff, Some("conventional"), false),
            false,
        );
    }

    // ----------------------------------------
    // refactor パターン (3 cases)
    // ----------------------------------------

    #[cfg(all(target_os = "macos", feature = "apple-ai"))]
    #[rstest]
    #[case(
        "refactor-1-extract-params",
        "--- a/src/handler.rs\n+++ b/src/handler.rs\n@@ -1,8 +1,6 @@\n-fn process(a: i32, b: i32, c: i32) -> i32 {\n-    let tmp = a + b;\n-    tmp * c\n-}\n+fn process(params: &Params) -> i32 {\n+    (params.a + params.b) * params.c\n+}\n"
    )]
    #[case(
        "refactor-2-extract-method",
        "--- a/src/app.rs\n+++ b/src/app.rs\n@@ -20,12 +20,8 @@\n     pub fn run(&self) {\n-        let config = Config::load();\n-        let validated = config.validate();\n-        if !validated {\n-            eprintln!(\"Invalid config\");\n-            return;\n-        }\n-        self.execute(config);\n+        match self.load_and_validate_config() {\n+            Ok(config) => self.execute(config),\n+            Err(e) => eprintln!(\"Config error: {}\", e),\n+        }\n     }\n"
    )]
    #[case(
        "refactor-3-enum-replace-strings",
        "--- a/src/status.rs\n+++ b/src/status.rs\n@@ -1,10 +1,16 @@\n-pub fn get_status(code: &str) -> &str {\n-    match code {\n-        \"ok\" => \"Success\",\n-        \"err\" => \"Error\",\n-        \"pending\" => \"Pending\",\n-        _ => \"Unknown\",\n-    }\n+pub enum Status {\n+    Ok,\n+    Error,\n+    Pending,\n+}\n+\n+impl Status {\n+    pub fn label(&self) -> &str {\n+        match self {\n+            Status::Ok => \"Success\",\n+            Status::Error => \"Error\",\n+            Status::Pending => \"Pending\",\n+        }\n+    }\n }\n"
    )]
    #[test]
    #[ignore]
    fn test_apple_intelligence_refactor(#[case] label: &str, #[case] diff: &str) {
        assert_apple_intelligence_result(
            label,
            try_apple_intelligence(diff, Some("conventional"), false),
            false,
        );
    }

    // ----------------------------------------
    // perf パターン (3 cases)
    // ----------------------------------------

    #[cfg(all(target_os = "macos", feature = "apple-ai"))]
    #[rstest]
    #[case(
        "perf-1-parallel-iter",
        "--- a/src/search.rs\n+++ b/src/search.rs\n@@ -3,4 +3,5 @@\n-fn search(items: &[Item], query: &str) -> Vec<&Item> {\n-    items.iter().filter(|i| i.name.contains(query)).collect()\n+fn search(items: &[Item], query: &str) -> Vec<&Item> {\n+    let query_lower = query.to_lowercase();\n+    items.par_iter().filter(|i| i.name_lower.contains(&query_lower)).collect()\n"
    )]
    #[case(
        "perf-2-add-caching",
        "--- a/src/db.rs\n+++ b/src/db.rs\n@@ -8,6 +8,12 @@\n pub struct UserRepo {\n     db: Database,\n+    cache: HashMap<u64, User>,\n }\n \n impl UserRepo {\n-    pub fn find(&self, id: u64) -> Option<User> {\n-        self.db.query(\"SELECT * FROM users WHERE id = ?\", &[id])\n+    pub fn find(&mut self, id: u64) -> Option<&User> {\n+        if self.cache.contains_key(&id) {\n+            return self.cache.get(&id);\n+        }\n+        if let Some(user) = self.db.query(\"SELECT * FROM users WHERE id = ?\", &[id]) {\n+            self.cache.insert(id, user);\n+            return self.cache.get(&id);\n+        }\n+        None\n     }\n"
    )]
    #[case(
        "perf-3-reduce-allocations",
        "--- a/src/formatter.rs\n+++ b/src/formatter.rs\n@@ -3,8 +3,8 @@\n pub fn format_items(items: &[Item]) -> String {\n-    let mut parts = Vec::new();\n-    for item in items {\n-        parts.push(format!(\"{}: {}\", item.name, item.value));\n-    }\n-    parts.join(\", \")\n+    let mut buf = String::with_capacity(items.len() * 32);\n+    for (i, item) in items.iter().enumerate() {\n+        if i > 0 { buf.push_str(\", \"); }\n+        buf.push_str(&item.name);\n+        buf.push_str(\": \");\n+        buf.push_str(&item.value.to_string());\n+    }\n+    buf\n }\n"
    )]
    #[test]
    #[ignore]
    fn test_apple_intelligence_perf(#[case] label: &str, #[case] diff: &str) {
        assert_apple_intelligence_result(
            label,
            try_apple_intelligence(diff, Some("conventional"), false),
            false,
        );
    }

    // ----------------------------------------
    // test パターン (3 cases)
    // ----------------------------------------

    #[cfg(all(target_os = "macos", feature = "apple-ai"))]
    #[rstest]
    #[case(
        "test-1-unit-test",
        "--- a/tests/unit_test.rs\n+++ b/tests/unit_test.rs\n@@ -0,0 +1,8 @@\n+#[test]\n+fn test_user_creation() {\n+    let user = User::new(\"test\");\n+    assert_eq!(user.name(), \"test\");\n+}\n"
    )]
    #[case(
        "test-2-add-edge-case",
        "--- a/src/parser.rs\n+++ b/src/parser.rs\n@@ -50,0 +51,18 @@\n+#[cfg(test)]\n+mod tests {\n+    use super::*;\n+\n+    #[test]\n+    fn test_parse_empty_input() {\n+        assert!(parse(\"\").is_err());\n+    }\n+\n+    #[test]\n+    fn test_parse_whitespace_only() {\n+        assert!(parse(\"   \").is_err());\n+    }\n+\n+    #[test]\n+    fn test_parse_unicode() {\n+        assert!(parse(\"こんにちは\").is_ok());\n+    }\n+}\n"
    )]
    #[case(
        "test-3-integration-test",
        "--- a/tests/integration/api_test.rs\n+++ b/tests/integration/api_test.rs\n@@ -0,0 +1,22 @@\n+use assert_cmd::Command;\n+use predicates::prelude::*;\n+\n+#[test]\n+fn test_cli_version_flag() {\n+    Command::cargo_bin(\"my-app\")\n+        .unwrap()\n+        .arg(\"--version\")\n+        .assert()\n+        .success()\n+        .stdout(predicate::str::contains(env!(\"CARGO_PKG_VERSION\")));\n+}\n+\n+#[test]\n+fn test_cli_help_flag() {\n+    Command::cargo_bin(\"my-app\")\n+        .unwrap()\n+        .arg(\"--help\")\n+        .assert()\n+        .success()\n+        .stdout(predicate::str::contains(\"Usage\"));\n+}\n"
    )]
    #[test]
    #[ignore]
    fn test_apple_intelligence_test(#[case] label: &str, #[case] diff: &str) {
        assert_apple_intelligence_result(
            label,
            try_apple_intelligence(diff, Some("conventional"), false),
            false,
        );
    }

    // ----------------------------------------
    // build パターン (3 cases)
    // ----------------------------------------

    #[cfg(all(target_os = "macos", feature = "apple-ai"))]
    #[rstest]
    #[case(
        "build-1-add-dependency",
        "--- a/Cargo.toml\n+++ b/Cargo.toml\n@@ -10,2 +10,3 @@\n serde = \"1.0\"\n+tokio = { version = \"1.0\", features = [\"full\"] }\n"
    )]
    #[case(
        "build-2-update-version",
        "--- a/Cargo.toml\n+++ b/Cargo.toml\n@@ -1,5 +1,5 @@\n [package]\n name = \"my-app\"\n-version = \"1.2.0\"\n+version = \"1.3.0\"\n edition = \"2021\"\n"
    )]
    #[case(
        "build-3-makefile-target",
        "--- a/Makefile\n+++ b/Makefile\n@@ -15,0 +16,6 @@\n+.PHONY: docker\n+docker:\n+\tdocker build -t my-app:latest .\n+\tdocker tag my-app:latest registry.example.com/my-app:latest\n+\tdocker push registry.example.com/my-app:latest\n"
    )]
    #[test]
    #[ignore]
    fn test_apple_intelligence_build(#[case] label: &str, #[case] diff: &str) {
        assert_apple_intelligence_result(
            label,
            try_apple_intelligence(diff, Some("conventional"), false),
            false,
        );
    }

    // ----------------------------------------
    // ci パターン (3 cases)
    // ----------------------------------------

    #[cfg(all(target_os = "macos", feature = "apple-ai"))]
    #[rstest]
    #[case(
        "ci-1-add-cache",
        "--- a/.github/workflows/ci.yml\n+++ b/.github/workflows/ci.yml\n@@ -15,2 +15,6 @@\n     - uses: actions/checkout@v4\n+    - uses: actions/cache@v4\n+      with:\n+        path: |\n+          ~/.cargo/registry\n+          target\n+        key: ${{ runner.os }}-cargo-${{ hashFiles('**/Cargo.lock') }}\n"
    )]
    #[case(
        "ci-2-add-lint-job",
        "--- a/.github/workflows/ci.yml\n+++ b/.github/workflows/ci.yml\n@@ -20,0 +21,12 @@\n+  lint:\n+    runs-on: ubuntu-latest\n+    steps:\n+      - uses: actions/checkout@v4\n+      - uses: dtolnay/rust-toolchain@stable\n+        with:\n+          components: clippy, rustfmt\n+      - run: cargo fmt --all -- --check\n+      - run: cargo clippy -- -D warnings\n"
    )]
    #[case(
        "ci-3-add-release-workflow",
        "--- /dev/null\n+++ b/.github/workflows/release.yml\n@@ -0,0 +1,20 @@\n+name: Release\n+on:\n+  push:\n+    tags:\n+      - 'v*'\n+jobs:\n+  release:\n+    runs-on: ubuntu-latest\n+    steps:\n+      - uses: actions/checkout@v4\n+      - uses: dtolnay/rust-toolchain@stable\n+      - run: cargo build --release\n+      - uses: softprops/action-gh-release@v2\n+        with:\n+          files: target/release/my-app\n"
    )]
    #[test]
    #[ignore]
    fn test_apple_intelligence_ci(#[case] label: &str, #[case] diff: &str) {
        assert_apple_intelligence_result(
            label,
            try_apple_intelligence(diff, Some("conventional"), false),
            false,
        );
    }

    // ----------------------------------------
    // chore パターン (3 cases)
    // ----------------------------------------

    #[cfg(all(target_os = "macos", feature = "apple-ai"))]
    #[rstest]
    #[case(
        "chore-1-gitignore",
        "--- a/.gitignore\n+++ b/.gitignore\n@@ -1,2 +1,5 @@\n /target\n+*.log\n+.env\n+.DS_Store\n+*.swp\n"
    )]
    #[case(
        "chore-2-editorconfig",
        "--- /dev/null\n+++ b/.editorconfig\n@@ -0,0 +1,10 @@\n+root = true\n+\n+[*]\n+indent_style = space\n+indent_size = 4\n+end_of_line = lf\n+charset = utf-8\n+trim_trailing_whitespace = true\n+insert_final_newline = true\n"
    )]
    #[case(
        "chore-3-license-update",
        "--- a/LICENSE\n+++ b/LICENSE\n@@ -1,3 +1,3 @@\n MIT License\n \n-Copyright (c) 2024 Example\n+Copyright (c) 2024-2025 Example\n"
    )]
    #[test]
    #[ignore]
    fn test_apple_intelligence_chore(#[case] label: &str, #[case] diff: &str) {
        assert_apple_intelligence_result(
            label,
            try_apple_intelligence(diff, Some("conventional"), false),
            false,
        );
    }

    // ----------------------------------------
    // revert パターン (3 cases)
    // ----------------------------------------

    #[cfg(all(target_os = "macos", feature = "apple-ai"))]
    #[rstest]
    #[case(
        "revert-1-remove-feature",
        "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -10,8 +10,0 @@\n-pub fn experimental_feature() {\n-    println!(\"This feature caused issues\");\n-}\n-\n-pub fn experimental_helper() {\n-    println!(\"Helper for experimental feature\");\n-}\n"
    )]
    #[case(
        "revert-2-restore-old-logic",
        "--- a/src/auth.rs\n+++ b/src/auth.rs\n@@ -5,6 +5,4 @@\n pub fn verify_token(token: &str) -> bool {\n-    // New JWT verification (broken)\n-    jwt::decode(token)\n-        .map(|claims| claims.exp > now())\n-        .unwrap_or(false)\n+    // Revert to simple token check until JWT is fixed\n+    token.len() == 64 && token.chars().all(|c| c.is_ascii_hexdigit())\n }\n"
    )]
    #[case(
        "revert-3-rollback-dependency",
        "--- a/Cargo.toml\n+++ b/Cargo.toml\n@@ -12,3 +12,3 @@\n [dependencies]\n-serde = \"2.0.0-beta\"\n+serde = \"1.0.228\"\n"
    )]
    #[test]
    #[ignore]
    fn test_apple_intelligence_revert(#[case] label: &str, #[case] diff: &str) {
        assert_apple_intelligence_result(
            label,
            try_apple_intelligence(diff, Some("conventional"), false),
            false,
        );
    }

    // ----------------------------------------
    // body 付きメッセージ生成テスト (3 cases)
    // ----------------------------------------

    #[cfg(all(target_os = "macos", feature = "apple-ai"))]
    #[rstest]
    #[case(
        "body-1-new-struct",
        "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -0,0 +1,15 @@\n+pub struct Config {\n+    pub timeout: u64,\n+    pub retries: u32,\n+}\n+\n+impl Config {\n+    pub fn new() -> Self {\n+        Self { timeout: 30, retries: 3 }\n+    }\n+\n+    pub fn with_timeout(mut self, timeout: u64) -> Self {\n+        self.timeout = timeout;\n+        self\n+    }\n+}\n"
    )]
    #[case(
        "body-2-multiple-fixes",
        "--- a/src/validator.rs\n+++ b/src/validator.rs\n@@ -10,6 +10,8 @@\n pub fn validate_email(email: &str) -> bool {\n-    email.contains(\"@\")\n+    let parts: Vec<&str> = email.split('@').collect();\n+    parts.len() == 2 && !parts[0].is_empty() && parts[1].contains('.')\n }\n \n pub fn validate_age(age: i32) -> bool {\n-    age > 0\n+    age > 0 && age < 150\n }\n"
    )]
    #[case(
        "body-3-large-feature",
        "--- a/src/export.rs\n+++ b/src/export.rs\n@@ -0,0 +1,30 @@\n+use std::fs::File;\n+use std::io::Write;\n+use serde_json;\n+\n+pub enum ExportFormat {\n+    Json,\n+    Csv,\n+    Yaml,\n+}\n+\n+pub fn export(data: &[Record], format: ExportFormat, path: &str) -> Result<(), Box<dyn std::error::Error>> {\n+    let content = match format {\n+        ExportFormat::Json => serde_json::to_string_pretty(data)?,\n+        ExportFormat::Csv => records_to_csv(data),\n+        ExportFormat::Yaml => serde_yaml::to_string(data)?,\n+    };\n+    let mut file = File::create(path)?;\n+    file.write_all(content.as_bytes())?;\n+    Ok(())\n+}\n+\n+fn records_to_csv(data: &[Record]) -> String {\n+    let mut buf = String::from(\"id,name,value\\n\");\n+    for r in data {\n+        buf.push_str(&format!(\"{},{},{}\\n\", r.id, r.name, r.value));\n+    }\n+    buf\n+}\n"
    )]
    #[test]
    #[ignore]
    fn test_apple_intelligence_with_body(#[case] label: &str, #[case] diff: &str) {
        assert_apple_intelligence_result(
            label,
            try_apple_intelligence(diff, Some("conventional"), true),
            true,
        );
    }

    // ----------------------------------------
    // 日本語出力テスト (3 cases)
    // ----------------------------------------

    #[cfg(all(target_os = "macos", feature = "apple-ai"))]
    fn try_apple_intelligence_ja(
        diff: &str,
        prefix_type: Option<&str>,
    ) -> Option<Result<String, AppError>> {
        let model = fm_rs::SystemLanguageModel::new().ok()?;
        if model.ensure_available().is_err() {
            return None;
        }
        let prompt = AiService::build_prompt(diff, &[], "Japanese", prefix_type, false, None);
        Some(AiService::call_apple_intelligence_native(
            &prompt, "Japanese",
        ))
    }

    #[cfg(all(target_os = "macos", feature = "apple-ai"))]
    #[rstest]
    #[case(
        "ja-1-new-function",
        "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -5,0 +6,4 @@\n+fn greet(name: &str) -> String {\n+    format!(\"Hello, {}!\", name)\n+}\n"
    )]
    #[case(
        "ja-2-bugfix",
        "--- a/src/calc.rs\n+++ b/src/calc.rs\n@@ -8,3 +8,3 @@\n pub fn divide(a: f64, b: f64) -> Result<f64, String> {\n-    Ok(a / b)\n+    if b == 0.0 { Err(\"division by zero\".to_string()) } else { Ok(a / b) }\n }\n"
    )]
    #[case(
        "ja-3-add-error-handling",
        "--- a/src/io.rs\n+++ b/src/io.rs\n@@ -3,4 +3,8 @@\n pub fn read_config(path: &str) -> Config {\n-    let content = std::fs::read_to_string(path).unwrap();\n-    toml::from_str(&content).unwrap()\n+    let content = std::fs::read_to_string(path)\n+        .unwrap_or_else(|e| {\n+            eprintln!(\"Failed to read {}: {}\", path, e);\n+            std::process::exit(1);\n+        });\n+    toml::from_str(&content).unwrap_or_else(|e| {\n+        eprintln!(\"Failed to parse {}: {}\", path, e);\n+        std::process::exit(1);\n+    })\n }\n"
    )]
    #[test]
    #[ignore]
    fn test_apple_intelligence_japanese_output(#[case] label: &str, #[case] diff: &str) {
        assert_apple_intelligence_result(
            label,
            try_apple_intelligence_ja(diff, Some("conventional")),
            false,
        );
    }

    // ============================================================
    // extract_error: Codex reconnecting エッジケース
    // ============================================================

    #[test]
    fn test_extract_error_codex_reconnecting_only() {
        let stderr = "Reconnecting to server...\nReconnecting to server...\n";
        let error = AiService::extract_error(stderr, &AiProvider::Codex);
        // "reconnecting" 行はスキップされ、デフォルトメッセージが返る
        assert_eq!(error, "Codex API request failed");
    }

    // ============================================================
    // clean_message: 追加エッジケース
    // ============================================================

    #[test]
    fn test_clean_message_code_block_with_only_whitespace_content() {
        let message = "```\n   \n```";
        let result = AiService::clean_message(message);
        // コードブロック内が空白のみの場合、空文字列になる
        assert!(result.is_empty());
    }

    #[test]
    fn test_clean_message_double_quoted_with_spaces() {
        let message = "  \"feat: add feature\"  ";
        assert_eq!(AiService::clean_message(message), "feat: add feature");
    }

    // ============================================================
    // clean_message: 言語タグ付きコードブロックの複数行
    // ============================================================

    #[test]
    fn test_clean_message_code_block_with_language_multiline() {
        let message = "```commit\nfeat: add auth\n\n- OAuth2 support\n- JWT tokens\n```";
        assert_eq!(
            AiService::clean_message(message),
            "feat: add auth\n\n- OAuth2 support\n- JWT tokens"
        );
    }

    #[test]
    fn test_clean_message_code_block_opening_only_no_content() {
        // 開始フェンスのみ、内容なし
        let message = "```";
        assert_eq!(AiService::clean_message(message), "```");
    }

    // ============================================================
    // process_provider_output: ExitStatus を生成して検証
    // ============================================================

    /// テスト用にコマンド実行で ExitStatus を取得するヘルパー
    fn exit_status(success: bool) -> ExitStatus {
        if success {
            Command::new("true").status().unwrap()
        } else {
            Command::new("false").status().unwrap()
        }
    }

    #[test]
    fn test_process_provider_output_success_with_message() {
        let status = exit_status(true);
        let result =
            AiService::process_provider_output(&AiProvider::Gemini, status, "feat: add X", "");
        assert_eq!(result.unwrap(), "feat: add X");
    }

    #[test]
    fn test_process_provider_output_success_empty_stdout() {
        let status = exit_status(true);
        let result = AiService::process_provider_output(&AiProvider::Gemini, status, "", "");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("empty response"));
    }

    #[test]
    fn test_process_provider_output_success_empty_stdout_with_stderr() {
        let status = exit_status(true);
        let result =
            AiService::process_provider_output(&AiProvider::Gemini, status, "", "some hint");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("stderr"));
    }

    #[test]
    fn test_process_provider_output_failure() {
        let status = exit_status(false);
        let result = AiService::process_provider_output(
            &AiProvider::Gemini,
            status,
            "",
            "something went wrong",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_process_provider_output_stderr_error_for_gemini() {
        // Gemini は stderr に "error:" があればエラー扱い
        let status = exit_status(true);
        let result = AiService::process_provider_output(
            &AiProvider::Gemini,
            status,
            "feat: ok",
            "error: rate limit",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_process_provider_output_stderr_error_ignored_for_codex() {
        // Codex は stderr に "error:" があっても無視
        let status = exit_status(true);
        let result = AiService::process_provider_output(
            &AiProvider::Codex,
            status,
            "feat: add feature",
            "error: this is just a log",
        );
        assert_eq!(result.unwrap(), "feat: add feature");
    }

    #[test]
    fn test_process_provider_output_stderr_error_ignored_for_claude() {
        // Claude は stderr に "error:" があっても無視
        let status = exit_status(true);
        let result = AiService::process_provider_output(
            &AiProvider::Claude,
            status,
            "fix: resolve bug",
            "error: debug log",
        );
        assert_eq!(result.unwrap(), "fix: resolve bug");
    }

    #[test]
    fn test_process_provider_output_stderr_file_not_found() {
        // Gemini で stderr に "file not found" があればエラー
        let status = exit_status(true);
        let result = AiService::process_provider_output(
            &AiProvider::Gemini,
            status,
            "feat: ok",
            "File not found: /path/to/bin",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_process_provider_output_cleans_message() {
        // 出力メッセージがクリーンアップされることを確認
        let status = exit_status(true);
        let result = AiService::process_provider_output(
            &AiProvider::Gemini,
            status,
            "```\nfeat: clean this\n```",
            "",
        );
        assert_eq!(result.unwrap(), "feat: clean this");
    }
}
