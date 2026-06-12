//! AI プロンプト構築とメッセージ整形
//!
//! プロンプトの形式契約(prefix 種別・body 構造・agent context)と、
//! AI 応答のクリーンアップ(コードフェンス除去・件名/本文の空行保証)を担当する。

use super::service::AiService;

/// Conventional Commits プレフィックスの詳細説明
pub(super) const CONVENTIONAL_COMMITS_GUIDE: &str = "\
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

impl AiService {
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

    /// 生成されたメッセージをクリーンアップ
    pub(super) fn clean_message(message: &str) -> String {
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
    pub(super) fn ensure_body_separator(message: &str) -> String {
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
