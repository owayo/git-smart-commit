//! AI プロンプト構築とメッセージ整形
//!
//! プロンプトの形式契約(prefix 種別・body 構造・agent context)と、
//! AI 応答のクリーンアップ(コードフェンス除去・件名/本文の空行保証)を担当する。

use super::service::AiService;

/// Conventional Commits の標準 type
pub(super) const CONVENTIONAL_TYPES: [&str; 11] = [
    "feat", "fix", "docs", "style", "refactor", "perf", "test", "build", "ci", "chore", "revert",
];

/// `clean_message_detailed` の結果
///
/// `envelope_tag` は応答全体を包んでいたタグ名のうち `commit` 以外のもの。
/// プロンプトは `<commit>` で囲むよう指示しているが、モデルがタグ名をコミット種別と
/// 取り違えて `<test>…</test>` を返すことが実際にあり(実測 1 件)、この名前は
/// 「モデルが意図した形式」のヒントとして上位層が使う。本文自体には手を加えない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CleanedResponse {
    pub(super) message: String,
    pub(super) envelope_tag: Option<String>,
}

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
- The subject line must contain exactly ONE commit message with ONE prefix. Never combine several messages on it
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
- Write exactly ONE commit message with ONE prefix. Never combine several messages on the line
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
- Output the commit message wrapped in <commit></commit> tags
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

    /// 生成されたメッセージをクリーンアップ(テストからの呼び出し用)
    ///
    /// 本体は `clean_message_detailed` に一本化されているが、応答を包んでいたタグ名を
    /// 見ない既存テストが多いため、メッセージだけを返す薄いラッパを残している。
    #[cfg(test)]
    pub(super) fn clean_message(message: &str) -> String {
        Self::clean_message_detailed(message).message
    }

    /// クリーンアップ結果に、応答全体を包んでいたタグ名を添えて返す
    pub(super) fn clean_message_detailed(message: &str) -> CleanedResponse {
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

        // 応答全体が 1 組の対称タグで包まれていれば、タグ名を問わずまず 1 段剥がす。
        // 全体ラップに該当しない場合だけ、`<commit>` 専用の寛容な取り出しを行う。
        // この順序は入れ替えられない: 先に「どこかにある `<commit>`」を拾うと、
        // `<wrapper>説明 <commit>本文</commit></wrapper>` のような応答から
        // 内側だけを抜き出して外側の説明を見落とす。
        let (message, envelope_tag) = match Self::split_full_tag_envelope(&message) {
            Some((name, body)) => {
                // `<commit>` は指示どおりの形なのでヒントにはしない
                let tag = (name != "commit").then(|| name.to_string());
                (body.to_string(), tag)
            }
            None => (Self::strip_commit_tags(&message), None),
        };

        // 先頭と末尾の引用符がある場合は削除
        let message = message.trim_matches('"').trim_matches('\'');

        let message = message.trim().to_string();

        // 件名と本文の間に空行を保証
        CleanedResponse {
            message: Self::ensure_body_separator(&message),
            envelope_tag,
        }
    }

    /// 応答全体が属性なしの同名タグ 1 組で包まれていれば、その名前と中身を返す
    ///
    /// プロンプトの `<commit>` 指示に従わず、コミット種別をタグ名にして
    /// `<test>OAuthスコープの狭さとdirectory.readonlyを検証</test>` を返すモデルが
    /// 実際にあり、そのまま山括弧ごとコミットされた(2026-09-02)。
    ///
    /// 剥がす条件は「応答全体を包む 1 組」「開始・終了のタグ名が完全一致」「属性なし」
    /// 「1 段だけ」に限る。途中にあるタグや片側だけのタグまで救うと、
    /// `fix: レビュー結果の<details>内チェックボックス誤検出修正` のような正常な件名と
    /// 区別できなくなる(ローカル 132 リポジトリ・8315 コミットの実測で、山括弧を含む
    /// 件名 6 件のうち 5 件は先頭がタグではない通常の件名だった。全体ラップに限れば
    /// 誤検出 0 件)。
    fn split_full_tag_envelope(message: &str) -> Option<(&str, &str)> {
        let message = message.trim();

        let after_open = message.strip_prefix('<')?;
        let name_end = after_open.find('>')?;
        let name = &after_open[..name_end];

        // 属性なしの ASCII タグ名だけを受け付ける(`<commit foo="bar">` は対象外)
        let mut chars = name.chars();
        let first = chars.next()?;
        if !first.is_ascii_alphabetic()
            || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return None;
        }

        // `<` + name + `>` の次から本文が始まる。タグ名は ASCII なので境界は安全
        let body_start = name_end + 2;
        let close_start = message.len().checked_sub(name.len() + 3)?;
        if close_start < body_start {
            return None;
        }

        // 本文がマルチバイトで終わる場合、末尾から数えた位置が文字境界とは限らない。
        // `get` は境界でなければ None を返すのでスライスの panic を避けられる
        let close = message.get(close_start..)?;
        if close.strip_prefix("</")?.strip_suffix('>')? != name {
            return None;
        }

        let body = message.get(body_start..close_start)?;
        if body.trim().is_empty() {
            return None;
        }

        Some((name, body))
    }

    /// `<commit>...</commit>` で囲まれた本文を取り出す
    ///
    /// プロンプトはコミットメッセージをこのタグで囲むよう指示している(応答の打ち切り対策。
    /// 終端が明示されると生成が安定する)。ただし指示に従わないモデルもあるため、
    /// 片側しか無い場合・まったく無い場合もそのまま通す寛容な実装にする。
    pub(super) fn strip_commit_tags(message: &str) -> String {
        const OPEN: &str = "<commit>";
        const CLOSE: &str = "</commit>";

        let body = match (message.find(OPEN), message.rfind(CLOSE)) {
            // 通常形。開始タグの後ろから閉じタグの手前まで
            (Some(start), Some(end)) if start + OPEN.len() <= end => {
                &message[start + OPEN.len()..end]
            }
            // 閉じタグが開始タグより前にある異常な並びは加工しない
            (Some(_), Some(_)) => message,
            // 閉じタグが完全な形で無い。応答が閉じタグの途中で終わることが実際にあり
            // (実測: `<commit>ci: CIワークフローを追加</`)、そのままだと `</` が
            // コミットメッセージ末尾に残るため、閉じタグの断片を落としてから採用する。
            // 本文自体が途中で切れていれば is_truncated_subject が弾く
            (Some(start), None) => Self::trim_partial_close_tag(&message[start + OPEN.len()..]),
            // 開始タグだけ欠けている
            (None, Some(end)) => &message[..end],
            (None, None) => message,
        };

        body.trim().to_string()
    }

    /// 末尾に残った `</commit>` の書きかけを取り除く
    ///
    /// 応答が閉じタグの生成途中で終わると `…追加</` `…追加</commi` のような断片が残る。
    /// 2 文字以上一致した場合だけ落とす(`<` 一文字は本文の一部でありうるため)。
    fn trim_partial_close_tag(body: &str) -> &str {
        const CLOSE: &str = "</commit>";
        let trimmed = body.trim_end();
        for len in (2..CLOSE.len()).rev() {
            if trimmed.ends_with(&CLOSE[..len]) {
                return &trimmed[..trimmed.len() - len];
            }
        }
        body
    }

    /// 件名に複数のコミットメッセージが連結されているかを判定する
    ///
    /// 「1 行で書け」という制約は守りつつ、複数の変更をまとめて表現しようとして
    /// `ci: CI設定追加 docs: README更新 build: mise.toml追加` のように 1 行へ
    /// 複数のメッセージを詰め込むことがある。コミットの件名としては壊れているので
    /// 打ち切りと同じく引き直す。
    ///
    /// 判定は Conventional Commits の標準 type が 2 つ以上現れた場合だけに限る。
    /// 任意の `語:` を数える緩い条件は、`docs(memory): … trust but verify: …` のような
    /// 正常な件名を巻き込む(実在するコミット履歴 9139 件で 5 件の誤検出)。標準 type に
    /// 絞ると同じ履歴で誤検出は 0 件だった。
    pub(super) fn is_concatenated_subject(subject: &str) -> bool {
        subject
            .split_whitespace()
            .filter(|token| Self::is_conventional_type_token(token))
            .count()
            >= 2
    }

    /// 語が `type:` / `type(scope):` / `type!:` の形をした標準 type かを判定する
    ///
    /// 末尾がコロンであることを要求するので、`http://…` のような文中の URL や
    /// `観点 B:` のような注釈は該当しない。
    fn is_conventional_type_token(token: &str) -> bool {
        let Some(head) = token.strip_suffix(':') else {
            return false;
        };
        let head = head.strip_suffix('!').unwrap_or(head);
        let base = match (head.find('('), head.ends_with(')')) {
            (Some(paren), true) => &head[..paren],
            _ => head,
        };
        CONVENTIONAL_TYPES.contains(&base.to_ascii_lowercase().as_str())
    }

    /// 件名が既に Conventional Commits の type プレフィックスを持つかを判定する
    pub(super) fn has_conventional_prefix(message: &str) -> bool {
        message
            .lines()
            .next()
            .and_then(|subject| subject.split_whitespace().next())
            .is_some_and(Self::is_conventional_type_token)
    }

    /// 応答を包んでいたタグ名が標準 type だったときに、その prefix を件名へ復元する
    ///
    /// `<test>OAuthスコープの狭さとdirectory.readonlyを検証</test>` は、モデルが
    /// `test:` を書くつもりでタグ名にしてしまった形と読める。タグを剥がしただけでは
    /// プレフィックスが失われるので、次をすべて満たすときに限り復元する:
    ///
    /// - タグ名が標準 type と小文字で完全一致する(`<TEST>` は復元しない。
    ///   大文字はモデルが type を書いた証拠として弱い)
    /// - 件名にまだ type プレフィックスが無い(`<test>fix: …</test>` は二重にしない)
    /// - 呼び出し側が「Conventional Commits を期待している」と判断している
    ///   (この関数はその判断をしない。`prefix_type = plain` の設定を上書きしないため)
    ///
    /// 復元は件名行だけに行い、本文の行はそのまま残す。
    pub(super) fn restore_conventional_prefix(message: &str, tag: &str) -> Option<String> {
        if !CONVENTIONAL_TYPES.contains(&tag) {
            return None;
        }
        if Self::has_conventional_prefix(message) {
            return None;
        }
        let mut lines = message.lines();
        let subject = lines.next()?;
        if subject.trim().is_empty() {
            return None;
        }
        let restored = format!("{tag}: {subject}");
        let rest: Vec<&str> = lines.collect();
        if rest.is_empty() {
            Some(restored)
        } else {
            Some(format!("{restored}\n{}", rest.join("\n")))
        }
    }

    /// 件名にタグの残骸が残っているかを判定する
    ///
    /// `split_full_tag_envelope` が剥がせるのは「属性なしの同名タグが全体を包む」形だけで、
    /// 開始と終了でタグ名が違う・属性が付く・片側しか無い応答は素通しになる。そのまま
    /// コミットすると山括弧が件名に残るので、打ち切りや連結と同じく引き直しの対象にする。
    ///
    /// 判定は件名の先頭が開きタグか、末尾が `</name>` の場合だけ。ローカル 132 リポジトリ・
    /// 8315 コミットの実測では、この条件に該当する正常な件名は 1 件も無かった
    /// (`fix: レビュー結果の<details>内…` のように途中に山括弧を持つ件名は該当しない)。
    ///
    /// 先頭側はタグ名の形を問わない。`<commit foo="bar">…</commit>` のような属性付きの
    /// 応答は `strip_commit_tags` の片側救済で閉じタグだけが落ち、`<commit foo="bar">…`
    /// という半端な形で残る。タグ名が妥当な識別子かどうかで絞ると、この形を見逃す。
    pub(super) fn has_leftover_markup(subject: &str) -> bool {
        let subject = subject.trim();

        let starts_with_tag = subject
            .strip_prefix('<')
            .is_some_and(|rest| rest.contains('>'));

        let ends_with_tag = subject
            .strip_suffix('>')
            .and_then(|rest| rest.rfind("</").map(|at| &rest[at + 2..]))
            .is_some_and(Self::is_bare_tag_name);

        starts_with_tag || ends_with_tag
    }

    /// 属性を持たない ASCII のタグ名かを判定する
    fn is_bare_tag_name(name: &str) -> bool {
        let mut chars = name.chars();
        chars
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic())
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    }

    /// 件名が文の途中で打ち切られているかを判定する
    ///
    /// AI CLI が exit 0(Antigravity なら status=SUCCESS)を返しながら、応答本文だけが
    /// 助詞や前置詞の直後で終わることがある。空でも異常終了でもないため既存の検証を
    /// すべて通過してしまい、「... mise 設定を」のような未完成の件名がそのまま
    /// コミットされる。実測(2026-08-27, agy 1.1.21 + GPT-OSS 120B (Medium), 同一
    /// プロンプト 12 回)では 4 回が助詞「を」の直後で終了した。
    ///
    /// 打ち切りは確率的に起きるため、検出したら同じステップを引き直すか、次のステップへ
    /// フォールバックする。誤検出は「正常な件名が捨てられ、全ステップで同じ判定が続けば
    /// コミット自体ができなくなる」という重い失敗につながるので、判定は実在するコミット
    /// 履歴で誤検出が出なかった条件だけに絞る(下記の実測を参照)。
    pub(super) fn is_truncated_subject(subject: &str) -> bool {
        let subject = subject.trim();
        // 空文字は呼び出し側が別のエラーとして扱うため、ここでは判定しない
        if subject.is_empty() {
            return false;
        }

        // 日本語: 格助詞・読点で終わる(接続の途中で切れている)。
        // 「に」「へ」「は」「も」「で」「や」は *入れない*: 「再生速度をスライダーで調整可能に」
        // 「バージョンを最新へ」のような体言止めが正常な件名として多用されるため
        // (実測で 20 件以上が該当し、含めると全部を打ち切りと誤判定する)。
        // 活用語尾「る」「し」「て」も通常の文末に現れるため対象にしない。
        const TRAILING_PARTICLES: [char; 6] = ['を', 'と', 'が', 'の', '、', '，'];
        if let Some(last) = subject.chars().last()
            && TRAILING_PARTICLES.contains(&last)
        {
            return true;
        }

        // 英語の判定は件名全体が ASCII のときだけ行う。日本語の件名に含まれる単独の
        // 英字(例: 「... で塞ぐ (#57 案 A)」)を冠詞と読み違えるのを避けるため。
        // 同じ理由で冠詞 "a"/"an" は語彙から外す。
        if subject.is_ascii() {
            // 前置詞・接続詞で終わる(後続の名詞句が欠けている)
            const TRAILING_WORDS: [&str; 14] = [
                "to", "for", "and", "or", "with", "in", "on", "at", "of", "the", "from", "by",
                "into", "via",
            ];
            if let Some(last_word) = subject.split_whitespace().next_back() {
                let normalized = last_word
                    .trim_matches(|c: char| !c.is_alphanumeric())
                    .to_lowercase();
                if TRAILING_WORDS.contains(&normalized.as_str()) {
                    return true;
                }
            }

            // 開いた括弧が閉じていない(例: scope が "feat(mise" で切れている)。
            // 全角括弧は日本語の注釈で入れ子・非対称に使われるため対象にしない。
            if subject.matches('(').count() > subject.matches(')').count() {
                return true;
            }
        }

        false
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
