//! Apple Intelligence (Foundation Models) ネイティブ呼び出し
//!
//! fm-rs FFI 経由で on-device モデルを呼び出す。モジュール全体が
//! `ai/mod.rs` 側の cfg (macOS + apple-ai feature) でゲートされる。
//! system instructions は prefix_type からプロンプトと整合する形で動的構築する。

use crate::error::AppError;

use super::prompt::{CONVENTIONAL_COMMITS_GUIDE, CleanedResponse};
use super::service::AiService;

impl AiService {
    /// Apple Intelligence の system instructions を構築する
    ///
    /// Foundation Models では system instructions がプロンプトより優先されるため、
    /// 旧実装のように Conventional Commits を固定で強制すると `prefix_type = "none"` や
    /// `"bracket"`、過去コミット様式への自動追従と矛盾し、prefix 設定が無視される。
    /// 一方、on-device の小型モデルは指示への感度が低く、形式指定をプロンプト側に
    /// 一本化した中立 instructions では出力形式が安定しない(diff の復唱等が起きる)。
    /// そのため、プロンプトの format_section と同じ prefix_type から強制力のある
    /// 形式ルールを動的に構築し、instructions とプロンプトを常に一致させる。
    pub(super) fn build_apple_instructions(
        language: &str,
        prefix_type: Option<&str>,
        has_recent_commits: bool,
    ) -> String {
        // Conventional Commits を強制する形式ルール。
        // 明示指定時と、自動判定で参照すべき直近コミットがない場合
        // (プロンプト側も Conventional ガイドへフォールバックする)に使う。
        let conventional_rule = || {
            format!(
                "CRITICAL FORMAT RULE: The commit message MUST start with a type prefix \
                 followed by a COLON and a SPACE.\n\
                 Correct: \"feat: add user authentication\"\n\
                 Correct: \"fix: resolve null pointer error\"\n\
                 WRONG:   \"feat add user authentication\" (missing colon)\n\
                 WRONG:   \"Add user authentication\" (missing prefix)\n\n\
                 The format is ALWAYS: <type>: <description>\n\n\
                 Available types and when to use each:\n{}",
                CONVENTIONAL_COMMITS_GUIDE
            )
        };

        let format_rule = match prefix_type {
            Some("conventional") => conventional_rule(),
            Some("bracket") => "CRITICAL FORMAT RULE: The commit message MUST start with a \
                 bracket prefix such as [Add], [Fix], [Update], [Remove], [Refactor]."
                .to_string(),
            Some("colon") => "CRITICAL FORMAT RULE: The commit message MUST start with a \
                 prefix such as Add:, Fix:, Update:, Remove:, Refactor:."
                .to_string(),
            Some("emoji") => "CRITICAL FORMAT RULE: The commit message MUST start with an \
                 emoji prefix (e.g., ✨ for new feature, 🐛 for bug fix, 📝 for docs, \
                 ♻️ for refactor, 🔧 for config)."
                .to_string(),
            Some("plain") | Some("none") => {
                "CRITICAL FORMAT RULE: The commit message MUST NOT start with any type \
                 prefix (no \"feat:\", \"fix:\", \"[Add]\", emoji, etc.). \
                 Write only the description itself."
                    .to_string()
            }
            Some(custom) => format!(
                "CRITICAL FORMAT RULE: The commit message MUST use the following prefix \
                 format: {}",
                custom
            ),
            // 自動判定モード: プロンプト内の直近コミット一覧に倣わせる。
            // 注意: ここに具体的な prefix 例("feat:" や "[Add]" 等)を書くと、
            // on-device の小型モデルが直近コミットではなく例の方をオウム返しして
            // しまうため、具体トークンを含まない言い回しで「形式の模倣」だけを強制する。
            // 直近コミットが無い場合はプロンプト側も Conventional ガイドへ
            // フォールバックするため、instructions も conventional ルールで揃える。
            None if has_recent_commits => {
                "CRITICAL FORMAT RULE: The user prompt lists recent commit messages from \
                 this repository. Your commit message MUST imitate their format exactly: \
                 the same kind of prefix (or no prefix if they have none) and the same \
                 overall structure. Reuse the exact prefix words that appear in those \
                 messages — never translate the prefix word into another language. \
                 Do NOT introduce a prefix style that does not appear in those messages."
                    .to_string()
            }
            None => conventional_rule(),
        };

        format!(
            "You are a Git commit message generator. \
            Output ONLY the commit message in {language}. No explanation, no markdown, no code blocks. \
            Never repeat or quote the code changes themselves.\n\n\
            {format_rule}\n\n\
            Style rules:\n\
            - Use short, direct phrases\n\
            - Do NOT end with a period\n\
            - Do NOT use polite or formal sentence endings",
            language = language,
            format_rule = format_rule
        )
    }

    /// Apple Intelligence をネイティブ呼び出し（fm-rs経由）
    pub(super) fn call_apple_intelligence_native(
        prompt: &str,
        language: &str,
        prefix_type: Option<&str>,
        has_recent_commits: bool,
    ) -> Result<CleanedResponse, AppError> {
        let model = fm_rs::SystemLanguageModel::new().map_err(|e| {
            AppError::AiProviderError(format!("Failed to initialize Apple Intelligence: {}", e))
        })?;

        model.ensure_available().map_err(|_| {
            AppError::AiProviderError(
                "Apple Intelligence is not available (requires macOS 26+, Apple Silicon, Apple Intelligence enabled)".to_string()
            )
        })?;

        let instructions =
            Self::build_apple_instructions(language, prefix_type, has_recent_commits);

        let session = fm_rs::Session::with_instructions(&model, &instructions)
            .map_err(|e| AppError::AiProviderError(format!("Failed to create session: {}", e)))?;

        let options = fm_rs::GenerationOptions::builder().temperature(0.3).build();

        let response = session.respond(prompt, &options).map_err(|e| {
            AppError::AiProviderError(format!("Apple Intelligence generation failed: {}", e))
        })?;

        let cleaned = Self::clean_message_detailed(response.content().trim());

        if cleaned.message.is_empty() {
            return Err(AppError::AiProviderError(
                "Apple Intelligence returned an empty response".to_string(),
            ));
        }

        Ok(cleaned)
    }
}
