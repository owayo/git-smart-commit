//! Apple Intelligence (Foundation Models) ネイティブ呼び出し
//!
//! fm-rs FFI 経由で on-device モデルを呼び出す。モジュール全体が
//! `ai/mod.rs` 側の cfg (macOS + apple-ai feature) でゲートされる。
//! system instructions は prefix_type からプロンプトと整合する形で動的構築する。

use std::time::Duration;

use crate::error::AppError;

use super::prompt::{CONVENTIONAL_COMMITS_GUIDE, CleanedResponse};
use super::service::AiService;

/// `context_size()` が読めなかったときに使う値。
///
/// macOS 27 / Foundation Models 27 の on-device モデルの実測値であり、
/// framework 側も pre-27 ランタイムにはこの値を back-deploy する。
const FALLBACK_CONTEXT_TOKENS: u64 = 4096;

/// 応答のために空けておくトークン数(件名のみ)。
///
/// 実測の出力は件名のみで 17〜25 トークン。`max_response_tokens` は上限であって
/// 目標ではなく、きつく締めると壊れた出力を招くと fm-rs 側も警告しているので、
/// 実測の 4 倍前後を取る。
const RESPONSE_TOKENS_SUBJECT: u64 = 96;

/// 応答のために空けておくトークン数(本文つき)。実測は本文込みで 100 前後。
const RESPONSE_TOKENS_WITH_BODY: u64 = 256;

/// チャットテンプレートぶんの上乗せ見積もり。
///
/// `token_usage_for` は渡した文字列そのものしか測れないが、実際の
/// `input_tokens` には framework がセッションを組み立てるときのテンプレートが
/// 加算される。実測では instructions + prompt の合計に対して 54 トークン多く、
/// 余裕を見て倍以上を確保する。
const TEMPLATE_OVERHEAD_TOKENS: u64 = 128;

/// diff を縮めて測り直す回数。
///
/// トークン数と文字数の比は diff の中身(日本語コメントの量、記号の密度)で
/// 変わるため 1 回の比例計算では決まらない。実測ではどの diff も 2 回以内に
/// 収まる。
const COMPACTION_ATTEMPTS: usize = 4;

/// Apple Intelligence 呼び出しの入力一式。
///
/// 共通プロンプトに加えてその素材も持つのは、コンテキスト上限に収まらないときに
/// diff を縮めてプロンプトを組み直すため。他プロバイダーはコンテキストが桁違いに
/// 大きいので、この組み直しは Apple のステップに閉じている。
pub(super) struct AppleRequest<'a> {
    pub(super) prompt: &'a str,
    pub(super) diff: &'a str,
    pub(super) recent_commits: &'a [String],
    pub(super) language: &'a str,
    pub(super) prefix_type: Option<&'a str>,
    pub(super) with_body: bool,
    pub(super) agent_context: Option<&'a str>,
    pub(super) timeout: Duration,
}

/// 呼び出し結果。生成ログ用の計測値を添える。
pub(super) struct AppleOutcome {
    pub(super) cleaned: CleanedResponse,
    pub(super) usage: AppleUsage,
}

/// 1 回の生成の実測値。Foundation Models 27 でのみ `input_tokens` /
/// `output_tokens` が埋まる(pre-27 では framework が報告しない)。
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct AppleUsage {
    pub(super) context_tokens: u64,
    /// `instructions + プロンプト` の実測トークン数 (チャットテンプレート分は含まない)
    pub(super) measured_input_tokens: u64,
    pub(super) response_limit_tokens: u64,
    pub(super) input_tokens: Option<u64>,
    pub(super) output_tokens: Option<u64>,
    pub(super) compacted: bool,
}

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
                "CRITICAL FORMAT RULE: The commit message MUST start with exactly ONE type \
                 prefix taken from the list below, followed by a COLON and a SPACE, then the \
                 description.\n\
                 The format is ALWAYS: <type>: <description>\n\
                 The prefix appears exactly once. Never write a second prefix inside the \
                 description. Never omit the colon.\n\n\
                 Available types and when to use each:\n{}",
                Self::apple_conventional_type_list()
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
            - Write exactly ONE commit message. Never combine two messages, and never write two prefixes in a row\n\
            - Use short, direct phrases\n\
            - Do NOT end with a period\n\
            - Do NOT use polite or formal sentence endings",
            language = language,
            format_rule = format_rule
        )
    }

    /// Conventional Commits の type 一覧を、コロンを含まない表記で返す
    ///
    /// 共有の `CONVENTIONAL_COMMITS_GUIDE` は `- docs: documentation only changes`
    /// のように「type + コロン」の形で並んでおり、on-device の ~3B モデルはこれを
    /// そのままコピーして `fix: docs: README を更新` のような二重プレフィックスを
    /// 書く。実測 (2026-09-17): 縮約が必要な大きい diff 4 件すべてで発生した。
    /// `- docs = ...` に置き換えると 4 件中 0 件まで下がる。共有ガイドを parse して
    /// 作るので、type が追加されても追従する。
    fn apple_conventional_type_list() -> String {
        CONVENTIONAL_COMMITS_GUIDE
            .lines()
            .filter_map(|line| {
                let entry = line.strip_prefix("- ")?;
                let (name, description) = entry.split_once(": ")?;
                Some(format!("- {} = {}", name, description))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 応答のために空けておくトークン数
    pub(super) fn apple_response_reserve(with_body: bool) -> u64 {
        if with_body {
            RESPONSE_TOKENS_WITH_BODY
        } else {
            RESPONSE_TOKENS_SUBJECT
        }
    }

    /// diff をファイル一覧つきで指定文字数まで縮める
    ///
    /// 頭から切るだけにしないのは、コミットメッセージが最も必要とする情報が
    /// 「どのファイルが変わったか」だからで、本文を削ってもこの一覧は残す。
    /// 判定は `diff --git` / `diff --cc` / `diff --combined` のブロック開始行で、
    /// マージコミットの combined diff も拾える。
    pub(super) fn compact_diff_for_apple(diff: &str, target_chars: usize) -> String {
        const NOTICE: &str = "... (diff truncated to fit the on-device context window; the file list above is complete)";

        let headers: Vec<&str> = diff
            .lines()
            .filter(|line| {
                line.starts_with("diff --git ")
                    || line.starts_with("diff --cc ")
                    || line.starts_with("diff --combined ")
            })
            .collect();

        // ファイル一覧に割り当てるのは予算の半分まで。変更ファイルが数百ある
        // コミットで一覧が予算を食い尽くし、本文が消えるのを避ける
        let header_budget = target_chars / 2;
        let mut header_section = String::new();
        let mut listed = 0usize;
        for header in &headers {
            if header_section.chars().count() + header.chars().count() + 1 > header_budget {
                break;
            }
            header_section.push_str(header);
            header_section.push('\n');
            listed += 1;
        }
        let omitted = headers.len().saturating_sub(listed);
        if omitted > 0 {
            header_section.push_str(&format!("... ({} more changed files)\n", omitted));
        }

        let body_budget = target_chars.saturating_sub(header_section.chars().count());
        let body = Self::take_chars_to_line_boundary(diff, body_budget);

        if header_section.is_empty() {
            format!("{}\n\n{}", body, NOTICE)
        } else {
            format!("{}\n{}\n\n{}", header_section, body, NOTICE)
        }
    }

    /// 先頭から最大 `max_chars` 文字を、最後の完全な行までで切り出す
    fn take_chars_to_line_boundary(text: &str, max_chars: usize) -> &str {
        if max_chars == 0 {
            return "";
        }
        let cutoff = match text.char_indices().nth(max_chars) {
            Some((idx, _)) => idx,
            None => return text,
        };
        let head = &text[..cutoff];
        match head.rfind('\n') {
            Some(pos) => &head[..pos],
            None => head,
        }
    }

    /// プロンプトをコンテキスト上限に収める
    ///
    /// 収まっていればそのまま返す。収まらない場合は diff を縮めて組み直す。
    /// Apple はチェーンの最後尾なので、「入らないので何も返さない」より
    /// 「ファイル一覧と主要な変更から生成する」ほうが実用的だと判断している。
    /// それでも収まらなければ `AiProviderInputError` を返し、プロバイダーを
    /// クールダウンさせずに次のステップへ渡す。
    fn fit_prompt_to_context(
        model: &fm_rs::SystemLanguageModel,
        req: &AppleRequest<'_>,
        instructions: &str,
        input_budget: u64,
    ) -> Result<(String, u64, bool), AppError> {
        // `token_usage_for` は 1 回 180ms かかる (実測、macOS 27 / M 系)。生成本体が
        // 2〜6 秒なので無視はできないが、instructions とプロンプトを別々に測ると
        // それだけで 2 回ぶん払うことになる。連結して 1 回で測る: 実際に session へ
        // 渡るのも「instructions + プロンプト」の組なので、境界のトークン化が
        // 数トークンずれるだけで、判定に必要な精度は変わらない
        let measure = |prompt: &str| -> Result<u64, AppError> {
            model
                .token_usage_for(&format!("{}\n{}", instructions, prompt))
                .map(|usage| usage.token_count as u64)
                .map_err(|e| {
                    AppError::AiProviderError(format!(
                        "Apple Intelligence: トークン数を計測できませんでした: {}",
                        e
                    ))
                })
        };

        let measured = measure(req.prompt)?;
        if measured <= input_budget {
            return Ok((req.prompt.to_string(), measured, false));
        }

        let rebuild = |diff: &str| {
            Self::build_prompt(
                diff,
                req.recent_commits,
                req.language,
                req.prefix_type,
                req.with_body,
                req.agent_context,
            )
        };

        // diff 以外(指示文・直近コミット・agent context)は縮まないので、
        // 比例計算は diff ぶんのトークンだけを対象にする
        let skeleton_tokens = measure(&rebuild(""))?;
        if input_budget <= skeleton_tokens {
            return Err(AppError::AiProviderInputError(format!(
                "Apple Intelligence: diff を除いた指示文だけでコンテキスト上限を超えます \
                 (指示文 {} トークン / 使用可能 {} トークン)",
                skeleton_tokens, input_budget
            )));
        }
        let diff_budget = input_budget - skeleton_tokens;

        let mut diff_tokens = measured.saturating_sub(skeleton_tokens).max(1);
        let mut diff_chars = req.diff.chars().count();
        let mut last_tokens = measured;

        for _ in 0..COMPACTION_ATTEMPTS {
            // 0.9 は行境界への切り戻しとファイル一覧の再付与でぶれるぶんの安全率
            let ratio = (diff_budget as f64 / diff_tokens as f64) * 0.9;
            let next_chars = ((diff_chars as f64) * ratio) as usize;
            if next_chars == 0 {
                break;
            }
            let compacted = Self::compact_diff_for_apple(req.diff, next_chars);
            let prompt = rebuild(&compacted);
            last_tokens = measure(&prompt)?;
            if last_tokens <= input_budget {
                return Ok((prompt, last_tokens, true));
            }
            diff_chars = next_chars;
            diff_tokens = last_tokens.saturating_sub(skeleton_tokens).max(1);
        }

        Err(AppError::AiProviderInputError(format!(
            "Apple Intelligence: プロンプトがコンテキスト上限に収まりません \
             (必要 {} トークン / 使用可能 {} トークン)",
            last_tokens, input_budget
        )))
    }

    /// fm-rs のエラーを、クールダウンに入れるものと入れないものに振り分ける
    ///
    /// プロンプトの内容で決まる失敗(コンテキスト超過・ガードレール・拒否・
    /// 非対応言語)は、同じプロバイダーでも次の diff なら成功するので
    /// `AiProviderInputError` にしてクールダウンを避ける。モデルが今使えない
    /// 類の失敗(アセット未取得・レート制限・タイムアウト)は従来どおり
    /// `AiProviderError` としてクールダウン対象にする。
    fn map_apple_generation_error(error: fm_rs::Error) -> AppError {
        use fm_rs::Error;

        match error {
            Error::ContextSizeExceeded(detail) => AppError::AiProviderInputError(format!(
                "Apple Intelligence: プロンプトがコンテキスト上限を超えました ({})",
                detail
            )),
            Error::GuardrailViolation(detail) => AppError::AiProviderInputError(format!(
                "Apple Intelligence: 安全ガードレールにより拒否されました ({})",
                detail
            )),
            Error::Refusal(detail) => AppError::AiProviderInputError(format!(
                "Apple Intelligence: モデルが応答を拒否しました ({})",
                detail
            )),
            Error::UnsupportedLanguageOrLocale(detail) => AppError::AiProviderInputError(format!(
                "Apple Intelligence: この言語/ロケールには対応していません ({})",
                detail
            )),
            Error::UnsupportedGenerationGuide(detail) => AppError::AiProviderInputError(format!(
                "Apple Intelligence: この生成ガイドには対応していません ({})",
                detail
            )),
            Error::UnsupportedTranscriptContent(detail) => AppError::AiProviderInputError(format!(
                "Apple Intelligence: この入力内容には対応していません ({})",
                detail
            )),
            Error::InvalidInput(detail) => AppError::AiProviderInputError(format!(
                "Apple Intelligence: 入力が不正です ({})",
                detail
            )),
            other => AppError::AiProviderError(format!(
                "Apple Intelligence generation failed: {}",
                other
            )),
        }
    }

    /// Apple Intelligence のランタイム可用性を確認する
    ///
    /// feature の有無(コンパイル時)ではなく、この Mac で実際にモデルが使えるか
    /// を見る。Apple Intelligence 未設定・非対応ハード・アセット未取得はここで
    /// 分かる。
    pub(super) fn apple_runtime_available() -> Result<(), AppError> {
        let model = fm_rs::SystemLanguageModel::new().map_err(|e| {
            AppError::AiProviderError(format!("Failed to initialize Apple Intelligence: {}", e))
        })?;
        model.ensure_available().map_err(|e| {
            AppError::AiProviderError(format!("Apple Intelligence is not available: {}", e))
        })
    }

    /// Apple Intelligence をネイティブ呼び出し（fm-rs経由）
    pub(super) fn call_apple_intelligence_native(
        req: &AppleRequest<'_>,
    ) -> Result<AppleOutcome, AppError> {
        let model = fm_rs::SystemLanguageModel::new().map_err(|e| {
            AppError::AiProviderError(format!("Failed to initialize Apple Intelligence: {}", e))
        })?;

        model.ensure_available().map_err(|_| {
            AppError::AiProviderError(
                "Apple Intelligence is not available (requires macOS 26+, Apple Silicon, Apple Intelligence enabled)".to_string()
            )
        })?;

        let instructions = Self::build_apple_instructions(
            req.language,
            req.prefix_type,
            !req.recent_commits.is_empty(),
        );

        // on-device モデルのコンテキストは実測で 4096 トークンしかなく、git-sc が
        // 全プロバイダー共通で許している 10000 文字の diff はこれを超えうる
        // (実リポジトリの 120 コミットで 7〜8%)。渡す前に収める
        let context_tokens = model.context_size().unwrap_or(FALLBACK_CONTEXT_TOKENS);
        let response_limit = Self::apple_response_reserve(req.with_body);
        let input_budget = context_tokens.saturating_sub(response_limit + TEMPLATE_OVERHEAD_TOKENS);

        let (prompt, measured_input_tokens, compacted) =
            Self::fit_prompt_to_context(&model, req, &instructions, input_budget)?;

        let session = fm_rs::Session::with_instructions(&model, &instructions)
            .map_err(|e| AppError::AiProviderError(format!("Failed to create session: {}", e)))?;

        // ツールは 1 つも登録していないが、tool calling を明示的に禁じておく。
        // Foundation Models 27 の既定は "allowed" で、将来 built-in system tools が
        // 既定で入るようになってもコミットメッセージ生成が寄り道しないようにする
        let options = fm_rs::GenerationOptions::builder()
            .temperature(0.3)
            .max_response_tokens(response_limit as u32)
            .tool_calling_mode(fm_rs::ToolCallingMode::Disallowed)
            .build();

        // `provider_timeout_seconds = 0` はサブプロセス側では「即打ち切り」になるが、
        // fm-rs の `respond_with_timeout` は 0 を「無制限」と解釈する。そのまま渡すと
        // 同じ設定値がプロバイダーによって正反対の意味になり、無制限側に倒れた
        // ネイティブ呼び出しだけが永久にブロックしうるので、ここで意味を揃える
        if req.timeout.is_zero() {
            return Err(AppError::AiProviderError(
                "Apple Intelligence timed out after 0 seconds".to_string(),
            ));
        }

        // 他プロバイダーはサブプロセス側でタイムアウトが効くが、ネイティブ呼び出しは
        // 自分で渡さないと無制限に待つ
        let response = session
            .respond_with_timeout(&prompt, &options, req.timeout)
            .map_err(Self::map_apple_generation_error)?;

        let session_usage = response.usage();
        let cleaned = Self::clean_message_detailed(response.content().trim());

        if cleaned.message.is_empty() {
            return Err(AppError::AiProviderError(
                "Apple Intelligence returned an empty response".to_string(),
            ));
        }

        Ok(AppleOutcome {
            cleaned,
            usage: AppleUsage {
                context_tokens,
                measured_input_tokens,
                response_limit_tokens: response_limit,
                input_tokens: session_usage.map(|usage| usage.input_tokens),
                output_tokens: session_usage.map(|usage| usage.output_tokens),
                compacted,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_diff(files: usize) -> String {
        (0..files)
            .map(|i| {
                format!(
                    "diff --git a/src/module_{i}.rs b/src/module_{i}.rs\n\
                     --- a/src/module_{i}.rs\n\
                     +++ b/src/module_{i}.rs\n\
                     @@ -1,3 +1,4 @@\n\
                     -    let old_value_{i} = compute(&input);\n\
                     +    let new_value_{i} = compute(&input)?;\n"
                )
            })
            .collect()
    }

    /// 本文を削ってもファイル一覧は残すこと
    ///
    /// コミットメッセージが最も必要とするのは「どのファイルが変わったか」なので、
    /// 頭から切り落とすだけでは最後のファイルが丸ごと見えなくなる。
    #[test]
    fn test_compact_diff_keeps_every_file_header() {
        let diff = sample_diff(5);
        let compacted = AiService::compact_diff_for_apple(&diff, 500);

        for i in 0..5 {
            assert!(
                compacted.contains(&format!("a/src/module_{i}.rs")),
                "縮約後もファイル {i} のヘッダが残るはず:\n{compacted}"
            );
        }
        assert!(
            compacted.chars().count() < diff.chars().count(),
            "縮約されていない"
        );
    }

    /// マージコミットの combined diff (`diff --cc`) もファイル一覧に拾うこと
    ///
    /// 通常の `diff --git` だけを見ていると、マージコミットでは一覧が空になる。
    #[test]
    fn test_compact_diff_lists_combined_diff_headers() {
        let diff = "diff --cc src/merged.rs\n\
                    @@@ -1,2 -1,2 +1,2 @@@\n\
                    - old from ours\n\
                    - old from theirs\n\
                    ++resolved value\n"
            .repeat(4);
        let compacted = AiService::compact_diff_for_apple(&diff, 120);

        assert!(
            compacted.contains("diff --cc src/merged.rs"),
            "combined diff のヘッダを拾えていない:\n{compacted}"
        );
    }

    /// ファイル数が多いときは一覧側を打ち切り、本文の枠を残すこと
    ///
    /// 一覧に上限を設けないと、変更ファイルが数百あるコミットで一覧が予算を
    /// 食い尽くし、diff 本文が 1 行も残らない。
    #[test]
    fn test_compact_diff_caps_the_file_list() {
        let diff = sample_diff(60);
        let target = 600;
        let compacted = AiService::compact_diff_for_apple(&diff, target);

        assert!(
            compacted.contains("more changed files"),
            "一覧が打ち切られたことを示す注記がない:\n{compacted}"
        );
        assert!(
            compacted.contains("@@ -1,3 +1,4 @@"),
            "一覧に食われて diff 本文が残っていない:\n{compacted}"
        );
    }

    /// 予算 0 でもパニックしないこと
    #[test]
    fn test_compact_diff_handles_zero_budget() {
        let compacted = AiService::compact_diff_for_apple(&sample_diff(3), 0);
        assert!(compacted.contains("diff truncated"));
    }

    /// マルチバイト文字の途中で切って UTF-8 境界パニックを起こさないこと
    #[test]
    fn test_compact_diff_handles_multibyte() {
        let diff = "diff --git a/doc.md b/doc.md\n".to_string() + &"+日本語の行です\n".repeat(50);
        let compacted = AiService::compact_diff_for_apple(&diff, 100);
        assert!(compacted.contains("a/doc.md"));
    }

    /// プロンプト内容で決まる失敗はクールダウンに入れないこと
    ///
    /// コンテキスト上限超過は実運用で 1 割弱のコミットで起きる。これを通常の
    /// 失敗として扱うと、大きい 1 コミットのせいで Apple Intelligence が 1 時間
    /// 止まり、そのあとの小さな diff まで使えなくなる。
    #[test]
    fn test_prompt_bound_errors_are_not_cooldown_worthy() {
        let prompt_bound = [
            fm_rs::Error::ContextSizeExceeded("context size 4096, request 5183".to_string()),
            fm_rs::Error::GuardrailViolation("blocked".to_string()),
            fm_rs::Error::Refusal("declined".to_string()),
            fm_rs::Error::UnsupportedLanguageOrLocale("unsupported".to_string()),
        ];
        for error in prompt_bound {
            let label = error.to_string();
            let mapped = AiService::map_apple_generation_error(error);
            assert!(
                matches!(mapped, AppError::AiProviderInputError(_)),
                "{label} は AiProviderInputError になるはず"
            );
            assert!(
                !AiService::should_record_failure(&mapped),
                "{label} でクールダウンに入れてはいけない"
            );
        }
    }

    /// モデル側の状態による失敗は従来どおりクールダウンに入れること
    #[test]
    fn test_provider_state_errors_are_cooldown_worthy() {
        let provider_bound = [
            fm_rs::Error::RateLimited("slow down".to_string()),
            fm_rs::Error::AssetsUnavailable("not downloaded".to_string()),
            fm_rs::Error::Timeout("timed out".to_string()),
            fm_rs::Error::ModelNotReady,
        ];
        for error in provider_bound {
            let label = error.to_string();
            let mapped = AiService::map_apple_generation_error(error);
            assert!(
                matches!(mapped, AppError::AiProviderError(_)),
                "{label} は AiProviderError になるはず"
            );
            assert!(
                AiService::should_record_failure(&mapped),
                "{label} はクールダウンに入れる"
            );
        }
    }

    /// type 一覧からコロン付きの表記を排除していること
    ///
    /// `- docs: documentation only changes` の形が instructions に残っていると、
    /// on-device の小型モデルがそれを出力例として写し取り、`fix: docs: ...` の
    /// ような二重プレフィックスを書く。共有ガイドを parse して作るので、type が
    /// 追加されたときもこのテストが追従を保証する。
    #[test]
    fn test_type_list_has_no_colon_form() {
        let list = AiService::apple_conventional_type_list();

        for name in crate::ai::prompt::CONVENTIONAL_TYPES {
            assert!(
                list.contains(&format!("- {} = ", name)),
                "type `{}` が一覧から落ちている:\n{}",
                name,
                list
            );
            assert!(
                !list.contains(&format!("- {}: ", name)),
                "type `{}` がコロン付きで残っている:\n{}",
                name,
                list
            );
        }
        // 説明文は保持する (type の選択精度がここに依存する)
        assert!(list.contains("bug fix"));
    }

    /// 応答用に確保するトークン数は本文の有無で変えること
    #[test]
    fn test_response_reserve_depends_on_body() {
        assert!(
            AiService::apple_response_reserve(true) > AiService::apple_response_reserve(false),
            "本文つきのほうが多くの応答トークンを要る"
        );
    }
}
