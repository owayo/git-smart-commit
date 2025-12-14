use std::process::Command;

use colored::Colorize;

use crate::config::{Config, ModelsConfig};
use crate::error::AppError;

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
            AiProvider::Gemini => "Gemini",
            AiProvider::Codex => "Codex",
            AiProvider::Claude => "Claude",
        }
    }

    fn command(&self) -> &'static str {
        match self {
            AiProvider::Gemini => "gemini",
            AiProvider::Codex => "codex",
            AiProvider::Claude => "claude",
        }
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
}

impl AiService {
    /// 設定からAiServiceを作成
    pub fn from_config(config: &Config) -> Self {
        let providers: Vec<AiProvider> = config
            .providers
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
        }
    }

    /// デフォルトのフォールバック順序でAiServiceを作成
    pub fn new() -> Self {
        Self {
            providers: vec![AiProvider::Gemini, AiProvider::Codex, AiProvider::Claude],
            language: "Japanese".to_string(),
            models: ModelsConfig::default(),
        }
    }

    /// 言語設定を上書き
    pub fn set_language(&mut self, language: String) {
        self.language = language;
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
        Command::new("which")
            .arg(provider.command())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// AI用のプロンプトを構築
    fn build_prompt(
        diff: &str,
        recent_commits: &[String],
        language: &str,
        prefix_type: Option<&str>,
    ) -> String {
        let format_section = match prefix_type {
            Some("conventional") => {
                "Use Conventional Commits format (e.g., feat:, fix:, docs:, refactor:, test:, chore:).".to_string()
            }
            Some("none") => {
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

        format!(
            r#"Generate a concise git commit message for the following changes.

{format_section}

Instructions:
- If the commits use Conventional Commits (feat:, fix:, etc.), use that format
- If the commits use bracket prefix ([Add], [Fix], etc.), use that format
- If the commits use other prefix styles, match that style
- Write the commit message in {language}

Rules:
- First line: max 50 characters
- Be specific about what changed
- Use imperative mood (Add, Fix, Update, not Added, Fixed, Updated)
- Output ONLY the commit message, no explanation

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
    pub fn generate_commit_message(
        &self,
        diff: &str,
        recent_commits: &[String],
        prefix_type: Option<&str>,
    ) -> Result<String, AppError> {
        let prompt = Self::build_prompt(diff, recent_commits, &self.language, prefix_type);
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
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or(AppError::NoAiProviderInstalled))
    }

    /// 特定のAIプロバイダーを呼び出し
    fn call_provider(&self, provider: &AiProvider, prompt: &str) -> Result<String, AppError> {
        let output = match provider {
            AiProvider::Gemini => Command::new("gemini")
                .args(["-m", &self.models.gemini, prompt])
                .output(),
            AiProvider::Codex => Command::new("codex")
                .args(["exec", "--model", &self.models.codex, prompt])
                .output(),
            AiProvider::Claude => Command::new("claude")
                .args(["--model", &self.models.claude, "-p", prompt])
                .output(),
        };

        let output = output.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AppError::AiProviderError(format!("{} not found", provider.name()))
            } else {
                AppError::AiProviderError(e.to_string())
            }
        })?;

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
