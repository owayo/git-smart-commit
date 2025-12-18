use std::io::Write;
use std::process::{Command, Stdio};

use colored::Colorize;

use crate::config::{Config, ModelsConfig};
use crate::error::AppError;
use crate::state::State;

/// AIプロバイダーの種類
#[derive(Debug, Clone, Copy)]
pub enum AiProvider {
    Gemini,
    Codex,
    Claude,
}

impl AiProvider {
    fn name(&self) -> &'static str {
        match self {
            AiProvider::Gemini => "Gemini CLI",
            AiProvider::Codex => "Codex CLI",
            AiProvider::Claude => "Claude Code",
        }
    }

    fn command(&self) -> &'static str {
        match self {
            AiProvider::Gemini => "gemini",
            AiProvider::Codex => "codex",
            AiProvider::Claude => "claude",
        }
    }

    /// 設定ファイルで使用するキー名（状態管理にも使用）
    pub fn config_key(&self) -> &'static str {
        self.command()
    }

    /// 文字列からプロバイダーを解析
    fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "gemini" => Some(AiProvider::Gemini),
            "codex" => Some(AiProvider::Codex),
            "claude" => Some(AiProvider::Claude),
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
}

impl AiService {
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
            vec![AiProvider::Gemini, AiProvider::Codex, AiProvider::Claude]
        } else {
            providers
        };

        Self {
            providers,
            language: config.language.clone(),
            models: config.models.clone(),
            cooldown_minutes: config.provider_cooldown_minutes,
        }
    }

    /// デフォルトのフォールバック順序でAiServiceを作成
    pub fn new() -> Self {
        Self {
            providers: vec![AiProvider::Gemini, AiProvider::Codex, AiProvider::Claude],
            language: "Japanese".to_string(),
            models: ModelsConfig::default(),
            cooldown_minutes: 60, // デフォルト1時間
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
        // Windows uses "where", Unix uses "which"
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
    ) -> String {
        let format_section = match prefix_type {
            Some("conventional") => {
                "Use Conventional Commits format (e.g., feat:, fix:, docs:, refactor:, test:, chore:).".to_string()
            }
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
                    "No recent commits found. Use Conventional Commits format (e.g., feat:, fix:, docs:, refactor:, test:, chore:).".to_string()
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

        format!(
            r#"Generate a git commit message for the following changes.

{format_section}

Instructions:
- Match the commit message style shown above
- Write the commit message in {language}
{body_instructions}
- Be specific about what changed
- Output ONLY the commit message as plain text
- Do NOT use any markdown formatting (no **, *, `, #, etc.)
- Do NOT include any explanation, reasoning, or thinking process
- Do NOT write phrases like "I will...", "Let me...", "Based on...", "Here is..."
- Respond with the commit message immediately, no preamble

Changes:
```diff
{diff}
```"#
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
    pub fn generate_commit_message(
        &self,
        diff: &str,
        recent_commits: &[String],
        prefix_type: Option<&str>,
        with_body: bool,
    ) -> Result<String, AppError> {
        let prompt =
            Self::build_prompt(diff, recent_commits, &self.language, prefix_type, with_body);
        let mut last_error = None;

        for provider in &self.providers {
            if !Self::is_installed(provider) {
                continue;
            }

            println!("  {} {}...", "Using".dimmed(), provider.name().cyan());

            match self.call_provider(provider, &prompt) {
                Ok(message) => return Ok(message),
                Err(e) => {
                    eprintln!(
                        "  {} {} failed: {}",
                        "⚠".yellow(),
                        provider.name(),
                        e.to_string().red()
                    );
                    // 失敗を記録して次回の優先度を下げる
                    self.record_provider_failure(provider);
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or(AppError::NoAiProviderInstalled))
    }

    /// 特定のAIプロバイダーを呼び出し
    fn call_provider(&self, provider: &AiProvider, prompt: &str) -> Result<String, AppError> {
        // Build command with stdin support to avoid command line length limits on Windows
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", provider.command()]);
            c
        } else {
            Command::new(provider.command())
        };

        // Add provider-specific arguments (without the prompt)
        match provider {
            AiProvider::Gemini => {
                cmd.args(["-m", &self.models.gemini]);
            }
            AiProvider::Codex => {
                cmd.args(["exec", "--model", &self.models.codex]);
            }
            AiProvider::Claude => {
                cmd.args(["--model", &self.models.claude, "-p"]);
            }
        };

        // Pass prompt via stdin to avoid OS error 206 (filename too long) on Windows
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AppError::AiProviderError(format!("{} not found", provider.name()))
            } else {
                AppError::AiProviderError(e.to_string())
            }
        })?;

        // Write prompt to stdin
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(prompt.as_bytes())
                .map_err(|e| AppError::AiProviderError(format!("Failed to write prompt: {}", e)))?;
        }

        let output = child
            .wait_with_output()
            .map_err(|e| AppError::AiProviderError(format!("Failed to wait for process: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let error_msg = Self::extract_error(&stderr, provider);
            return Err(AppError::AiProviderError(error_msg));
        }

        let message = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let message = Self::clean_message(&message);

        if message.is_empty() {
            return Err(AppError::AiProviderError(format!(
                "{} returned an empty response",
                provider.name()
            )));
        }

        Ok(message)
    }

    /// stderrからエラーメッセージを抽出
    fn extract_error(stderr: &str, provider: &AiProvider) -> String {
        match provider {
            AiProvider::Gemini => {
                // [API Error: ...] パターンを探す
                for line in stderr.lines() {
                    if line.starts_with("[API Error:") {
                        return line.to_string();
                    }
                }
                "Gemini API request failed".to_string()
            }
            AiProvider::Codex | AiProvider::Claude => {
                // 最初の非空行またはジェネリックメッセージを返す
                stderr
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("API request failed")
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

        message.trim().to_string()
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
    }

    #[test]
    fn test_ai_provider_command() {
        assert_eq!(AiProvider::Gemini.command(), "gemini");
        assert_eq!(AiProvider::Codex.command(), "codex");
        assert_eq!(AiProvider::Claude.command(), "claude");
    }

    #[rstest]
    #[case("gemini", Some(AiProvider::Gemini))]
    #[case("GEMINI", Some(AiProvider::Gemini))]
    #[case("Gemini", Some(AiProvider::Gemini))]
    #[case("codex", Some(AiProvider::Codex))]
    #[case("claude", Some(AiProvider::Claude))]
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
        assert_eq!(service.providers.len(), 3);
    }

    #[test]
    fn test_ai_service_set_language() {
        let mut service = AiService::new();
        service.set_language("English".to_string());
        assert_eq!(service.language, "English");
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
        let prompt = AiService::build_prompt(diff, &recent_commits, "Japanese", prefix_type, false);
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
        let prompt =
            AiService::build_prompt(diff, &recent_commits, "Japanese", Some("JIRA-123: "), false);
        assert!(prompt.contains("Use the following prefix format: JIRA-123:"));
    }

    #[test]
    fn test_build_prompt_auto_mode_empty_commits() {
        let diff = "test diff";
        let recent_commits: Vec<String> = vec![];
        let prompt = AiService::build_prompt(diff, &recent_commits, "Japanese", None, false);
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
        let prompt = AiService::build_prompt(diff, &recent_commits, "Japanese", None, false);
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
        );
        assert!(prompt.contains(diff));
        assert!(prompt.contains("```diff"));
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
        );
        assert!(prompt_ja.contains("Japanese"));

        let prompt_en = AiService::build_prompt(
            diff,
            &recent_commits,
            "English",
            Some("conventional"),
            false,
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
        );
        // 通常モードでは single line の指示が含まれる
        assert!(prompt.contains("single line"));
        assert!(!prompt.contains("bullet point"));
    }

    #[test]
    fn test_build_prompt_body_with_auto_mode() {
        let diff = "test diff";
        let recent_commits = vec!["feat: previous commit".to_string()];
        let prompt = AiService::build_prompt(diff, &recent_commits, "English", None, true);
        // Auto モードでも body 指示が含まれる
        assert!(prompt.contains("Body"));
        assert!(prompt.contains("bullet point"));
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
    fn test_extract_error_empty_stderr() {
        let stderr = "";
        let error = AiService::extract_error(stderr, &AiProvider::Codex);
        assert_eq!(error, "API request failed");
    }

    // ============================================================
    // AiService::from_config のテスト
    // ============================================================

    #[test]
    fn test_ai_service_from_config_default() {
        let config = Config::default();
        let service = AiService::from_config(&config);

        assert_eq!(service.language, "Japanese");
        assert_eq!(service.providers.len(), 3);
        assert_eq!(service.models.gemini, "flash");
        assert_eq!(service.models.codex, "gpt-5.1-codex-mini");
        assert_eq!(service.models.claude, "haiku");
    }

    #[test]
    fn test_ai_service_from_config_custom_providers() {
        let mut config = Config::default();
        config.providers = vec!["claude".to_string(), "gemini".to_string()];
        let service = AiService::from_config(&config);

        assert_eq!(service.providers.len(), 2);
        assert_eq!(service.providers[0].name(), "Claude Code");
        assert_eq!(service.providers[1].name(), "Gemini CLI");
    }

    #[test]
    fn test_ai_service_from_config_invalid_providers_fallback() {
        let mut config = Config::default();
        config.providers = vec!["invalid".to_string(), "unknown".to_string()];
        let service = AiService::from_config(&config);

        // 無効なプロバイダーのみの場合はデフォルトにフォールバック
        assert_eq!(service.providers.len(), 3);
    }

    #[test]
    fn test_ai_service_from_config_custom_language() {
        let mut config = Config::default();
        config.language = "English".to_string();
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
        assert_eq!(service.providers.len(), 3);
        assert_eq!(service.providers[0].name(), "Gemini CLI");
        assert_eq!(service.providers[1].name(), "Codex CLI");
        assert_eq!(service.providers[2].name(), "Claude Code");
    }

    // ============================================================
    // clean_message 追加テスト
    // ============================================================

    #[test]
    fn test_clean_message_nested_quotes() {
        let message = "\"'feat: message'\"";
        // 外側の引用符のみ削除される
        let result = AiService::clean_message(message);
        assert!(result.contains("feat: message"));
    }

    #[test]
    fn test_clean_message_empty() {
        let message = "";
        assert_eq!(AiService::clean_message(message), "");
    }

    #[test]
    fn test_clean_message_only_whitespace() {
        let message = "   \n\t  ";
        assert_eq!(AiService::clean_message(message), "");
    }

    #[test]
    fn test_clean_message_multiline() {
        let message = "feat: add feature\n\nThis is a longer description.";
        assert_eq!(
            AiService::clean_message(message),
            "feat: add feature\n\nThis is a longer description."
        );
    }

    #[test]
    fn test_clean_message_code_block_multiline() {
        let message = "```\nfeat: add feature\n\nDescription here\n```";
        let result = AiService::clean_message(message);
        assert!(result.contains("feat: add feature"));
        assert!(result.contains("Description here"));
    }

    // ============================================================
    // extract_error 追加テスト
    // ============================================================

    #[test]
    fn test_extract_error_whitespace_only() {
        let stderr = "   \n\t  ";
        let error = AiService::extract_error(stderr, &AiProvider::Claude);
        assert_eq!(error, "API request failed");
    }

    #[test]
    fn test_extract_error_gemini_multiple_api_errors() {
        // 最初のAPI Errorを返す
        let stderr = "[API Error: First error]\n[API Error: Second error]";
        let error = AiService::extract_error(stderr, &AiProvider::Gemini);
        assert_eq!(error, "[API Error: First error]");
    }
}
