use std::fs;
use std::path::Path;

use colored::Colorize;

use super::process::TempFile;
use crate::config::{Config, ModelsConfig, ProviderStep, canonical_provider_key};
use crate::error::AppError;
use crate::state::State;

/// AIプロバイダーの種類
#[derive(Debug, Clone, Copy)]
pub enum AiProvider {
    /// Antigravity CLI (`agy`). 旧 `gemini` CLI の後継 (2026-05-13 公開、旧 CLI は 2026-06-18 終了)。
    Antigravity,
    Codex,
    Claude,
    Opencode,
    /// Grok Build TUI (`grok`). X.AI の Grok を叩く TUI エージェント CLI (cmux 同梱)。
    Grok,
    AppleIntelligence,
}

impl AiProvider {
    pub fn name(&self) -> &'static str {
        match self {
            AiProvider::Antigravity => "Antigravity CLI",
            AiProvider::Codex => "Codex CLI",
            AiProvider::Claude => "Claude Code",
            AiProvider::Opencode => "opencode",
            AiProvider::Grok => "Grok CLI",
            AiProvider::AppleIntelligence => "Apple Intelligence",
        }
    }

    pub(super) fn command(&self) -> &'static str {
        match self {
            AiProvider::Antigravity => "agy",
            AiProvider::Codex => "codex",
            AiProvider::Claude => "claude",
            AiProvider::Opencode => "opencode",
            AiProvider::Grok => "grok",
            AiProvider::AppleIntelligence => "apple-ai",
        }
    }

    /// 設定ファイルで使用するキー名（状態管理にも使用）
    pub fn config_key(&self) -> &'static str {
        match self {
            AiProvider::Antigravity => "antigravity",
            AiProvider::Codex => "codex",
            AiProvider::Claude => "claude",
            AiProvider::Opencode => "opencode",
            AiProvider::Grok => "grok",
            AiProvider::AppleIntelligence => "apple-ai",
        }
    }

    /// 文字列からプロバイダーを解析
    ///
    /// `"gemini"` は旧 CLI 名の後方互換エイリアスとして `Antigravity` にマップする。
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            // "gemini" は 2026-06-18 に廃止される旧 CLI 名。後方互換のため Antigravity にマップする。
            "antigravity" | "agy" | "gemini" => Some(AiProvider::Antigravity),
            "codex" => Some(AiProvider::Codex),
            "claude" => Some(AiProvider::Claude),
            "opencode" => Some(AiProvider::Opencode),
            "grok" => Some(AiProvider::Grok),
            "apple-intelligence" | "apple_intelligence" => Some(AiProvider::AppleIntelligence),
            _ => None,
        }
    }

    /// 入力文字列が旧 `gemini` エイリアスかどうかを返す。debug 警告などに利用する。
    pub fn is_legacy_gemini_alias(s: &str) -> bool {
        s.eq_ignore_ascii_case("gemini")
    }
}

/// フォールバック機能付きのAIサービス
pub struct AiService {
    /// フォールバックチェーン。各ステップは provider/model/command/env を持つ。
    steps: Vec<ProviderStep>,
    language: String,
    pub(super) models: ModelsConfig,
    pub(super) codex_reasoning_effort: String,
    cooldown_minutes: u64,
    pub(super) timeout_seconds: u64,
    pub(super) debug: bool,
    provider_override: bool,
    /// 設定ファイル中に旧 `gemini` エイリアス、または `[models] gemini` の指定が
    /// 残っていた場合に立つフラグ。debug 出力時に注意を促す。
    legacy_gemini_alias_detected: bool,
    /// ai-usage 連携時に fallback chain を絞り込んだ判定ログ(debug 出力用)。
    /// snapshot 取得の可否と、各 step の keep/skip 理由を 1 行ずつ保持する。
    /// `set_debug(true)` 呼び出し時に一度だけ出力してクリアする。
    ai_usage_notes: Vec<String>,
    /// ai-usage フィルタが「実行成功」かつ「全 step を OverThreshold で除外」した場合に
    /// 立つフラグ。true のときは default_steps() へのフォールバックを行わず、
    /// `verify_installation` で `AppError::AiUsageError` として上位に伝える。
    /// snapshot 取得失敗(fail-open)や、そもそも入力 step が 0 件のケースは含まない。
    ai_usage_gate_blocked: bool,
}

impl AiService {
    /// デフォルトのフォールバックチェーンを返す(プロバイダー名のみのステップ)。
    fn default_steps() -> Vec<ProviderStep> {
        let mut steps = vec![
            ProviderStep::from_provider("opencode"),
            ProviderStep::from_provider("grok"),
            ProviderStep::from_provider("antigravity"),
            ProviderStep::from_provider("codex"),
            ProviderStep::from_provider("claude"),
        ];
        if cfg!(all(target_os = "macos", feature = "apple-ai")) {
            steps.push(ProviderStep::from_provider("apple-intelligence"));
        }
        steps
    }

    /// 設定からAiServiceを作成
    pub fn from_config(config: &Config) -> Self {
        let steps: Vec<ProviderStep> = config.providers.clone();

        // providers に旧 "gemini" エイリアスが残っているかをチェック。
        // 互換のため受理するが、debug 出力時に「antigravity に正規化される」旨を伝える。
        // ([models] gemini は読み込み時に antigravity へ昇格済みで、ここでは区別できない)
        let legacy_gemini_alias_detected = steps
            .iter()
            .any(|s| AiProvider::is_legacy_gemini_alias(&s.provider));

        // 状態を読み込んで、クールダウン中のステップを末尾へ降格する
        let reordered = if let Ok(state) = State::load() {
            state.reorder_steps(steps, config.provider_cooldown_minutes)
        } else {
            steps
        };

        // provider が解決できるステップだけを残す(不明な provider 名は除外)。
        let steps: Vec<ProviderStep> = reordered
            .into_iter()
            .filter(|s| AiProvider::from_str(&s.provider).is_some())
            .collect();

        // ai-usage 連携: `[ai_usage] enabled = true` のとき、5h/weekly 残量が閾値超過の
        // step を chain から除外する。snapshot 取得失敗や account 不一致は fail-open
        // (chain は元のまま + debug 用のログを残す)なので、連携が壊れても commit は
        // 従来どおり動く。ただし snapshot 取得成功後、全 step が OverThreshold で
        // 除外された場合は `gate_blocked = true` を立て、`default_steps()` に戻さず、
        // `verify_installation` で明示エラーにする(fallback すると閾値超過の step が
        // 結果的に呼ばれ、ai-usage gate の意味が消えるため)。
        let (steps, ai_usage_notes, ai_usage_gate_blocked) =
            Self::apply_ai_usage_filter(steps, config.ai_usage.as_ref());

        // ai-usage が積極的に空にした場合はフォールバックしない。それ以外(config に
        // provider が無い、全て unknown、fail-open で snapshot 取得失敗など)は
        // 従来どおりデフォルト chain へフォールバックする。
        let steps = if steps.is_empty() && !ai_usage_gate_blocked {
            Self::default_steps()
        } else {
            steps
        };

        Self {
            steps,
            language: config.language.clone(),
            models: config.models.clone(),
            codex_reasoning_effort: config.codex_reasoning_effort.clone(),
            cooldown_minutes: config.provider_cooldown_minutes,
            timeout_seconds: config.provider_timeout_seconds,
            debug: false,
            provider_override: false,
            legacy_gemini_alias_detected,
            ai_usage_notes,
            ai_usage_gate_blocked,
        }
    }

    /// `[ai_usage]` 設定にもとづき step 列をフィルタする。
    ///
    /// 返り値:
    /// - 1: フィルタ後の step 列
    /// - 2: debug 用のメッセージ(空でなければ set_debug(true) 時に eprintln! される)
    /// - 3: `gate_blocked` — 「snapshot 取得成功 & 入力 step は 1 件以上 & 全て
    ///   OverThreshold で除外」のときだけ true。fail-open (snapshot 取得失敗、
    ///   config 未設定、無効) や 入力 step が空のケースは false。
    fn apply_ai_usage_filter(
        steps: Vec<ProviderStep>,
        usage_cfg: Option<&crate::config::AiUsageConfig>,
    ) -> (Vec<ProviderStep>, Vec<String>, bool) {
        let Some(cfg) = usage_cfg else {
            return (steps, Vec::new(), false);
        };
        if !cfg.enabled {
            return (steps, Vec::new(), false);
        }

        let mut notes: Vec<String> = Vec::new();
        let snapshot = match crate::ai_usage::fetch_snapshot(cfg) {
            Ok(s) => s,
            Err(e) => {
                notes.push(format!(
                    "ai-usage snapshot unavailable: {e} (fallback chain unchanged)"
                ));
                return (steps, notes, false);
            }
        };
        Self::apply_ai_usage_filter_with_snapshot(steps, cfg, &snapshot, notes)
    }

    /// snapshot 取得後の評価ロジックを外出しした部分(テストから直接呼ぶ用)。
    fn apply_ai_usage_filter_with_snapshot(
        steps: Vec<ProviderStep>,
        cfg: &crate::config::AiUsageConfig,
        snapshot: &crate::ai_usage::AiUsageSnapshot,
        mut notes: Vec<String>,
    ) -> (Vec<ProviderStep>, Vec<String>, bool) {
        notes.push(format!(
            "ai-usage snapshot loaded ({} accounts, window={:?}, threshold={}%)",
            snapshot.accounts().len(),
            cfg.window,
            cfg.threshold_percent
        ));

        let input_len = steps.len();
        let mut kept = Vec::with_capacity(input_len);
        for step in steps {
            let decision = snapshot.evaluate(&step, cfg.window, cfg.threshold_percent);
            let label = Self::step_debug_label(&step);
            let (verb, extra) = match &decision {
                crate::ai_usage::UsageDecision::Usable { .. } => ("keep", ""),
                crate::ai_usage::UsageDecision::NoAccount { .. } => {
                    ("keep", " (no ai-usage match)")
                }
                crate::ai_usage::UsageDecision::OverThreshold { .. } => ("skip", ""),
            };
            notes.push(format!("{verb} {label}{extra}: {}", decision.reason()));
            if decision.is_usable() {
                kept.push(step);
            }
        }

        let gate_blocked = input_len > 0 && kept.is_empty();
        if gate_blocked {
            notes.push(
                "all configured steps filtered out by ai-usage; aborting instead of falling back \
                 to default chain (gate would otherwise be bypassed)"
                    .to_string(),
            );
        }
        (kept, notes, gate_blocked)
    }

    /// ai-usage debug 表示用の 1 行ラベル(provider + profile/group 明示 + account hint)。
    ///
    /// group まで出すのは、同一 profile の複数 step (Antigravity のモデル系統別プール等) を
    /// debug 出力で見分けられるようにするため。
    fn step_debug_label(step: &ProviderStep) -> String {
        let mut label = step.provider.clone();
        let mut parts: Vec<String> = Vec::new();
        if let Some(profile) = step.ai_usage_profile.as_deref() {
            parts.push(format!("profile={profile}"));
        } else if let Some(hint) = step.account_hint() {
            parts.push(format!("env={hint}"));
        }
        if let Some(group) = step.ai_usage_group.as_deref() {
            parts.push(format!("group={group}"));
        }
        if !parts.is_empty() {
            label.push_str(&format!("({})", parts.join(", ")));
        }
        label
    }

    /// デフォルトのフォールバック順序でAiServiceを作成
    pub fn new() -> Self {
        Self {
            steps: Self::default_steps(),
            language: "Japanese".to_string(),
            models: ModelsConfig::default(),
            codex_reasoning_effort: "low".to_string(),
            cooldown_minutes: 60, // デフォルト1時間
            timeout_seconds: 60,  // デフォルト60秒（Config::defaultと同値）
            debug: false,
            provider_override: false,
            legacy_gemini_alias_detected: false,
            ai_usage_notes: Vec::new(),
            ai_usage_gate_blocked: false,
        }
    }

    /// デバッグモードを設定。
    ///
    /// 旧 `gemini` エイリアスや `[models] gemini` が残っている設定を検出していた場合、
    /// または ai-usage 連携で fallback chain の絞り込み結果があれば、この呼び出しの
    /// タイミングで一度だけ eprintln! で表示する。
    pub fn set_debug(&mut self, debug: bool) {
        self.debug = debug;
        if debug && self.legacy_gemini_alias_detected {
            eprintln!(
                "{}",
                "[git-sc] notice: 'gemini' is a legacy alias for the Antigravity CLI ('agy') and is \
                 normalized to 'antigravity'. Prefer writing 'antigravity' (and [models] antigravity) instead."
                    .yellow()
            );
            // 通知は一度だけで十分
            self.legacy_gemini_alias_detected = false;
        }
        if debug && !self.ai_usage_notes.is_empty() {
            eprintln!("{}", "[git-sc] ai-usage filter:".dimmed());
            for note in &self.ai_usage_notes {
                eprintln!("  {}", note.dimmed());
            }
            self.ai_usage_notes.clear();
        }
    }

    /// プロバイダーを手動指定で上書き（フォールバックなし、失敗記録スキップ）
    ///
    /// 単一ステップ・モデル/env 未指定で上書きする。provider 名は from_str が解決できる
    /// 正規名(例: AppleIntelligence は "apple-intelligence")を使う。
    pub fn set_provider_override(&mut self, provider: AiProvider) {
        let provider_name = canonical_provider_key(provider.config_key());
        self.steps = vec![ProviderStep::from_provider(provider_name)];
        self.provider_override = true;
    }

    /// ステップの失敗を記録
    fn record_provider_failure(&self, step: &ProviderStep) {
        if let Ok(mut state) = State::load() {
            state.record_failure(step);
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

    /// 少なくとも1つのステップが実行可能であることを確認
    pub fn verify_installation(&self) -> Result<(), AppError> {
        // ai-usage フィルタが全 step を除外したときは、default chain へ戻さない代わりに
        // ここで明示エラーを返す(default に戻すと閾値超過の provider が結局呼ばれて
        // gate の意味が消えるため)。config 未設定や snapshot 取得失敗 (fail-open) では
        // このフラグは立たない。
        if self.ai_usage_gate_blocked {
            return Err(AppError::AiUsageError(
                "設定した全 provider が ai-usage の閾値(threshold_percent)を超えているため、\
                 実行できるアカウントがありません。しばらく待ってから再実行するか、\
                 [ai_usage].threshold_percent を見直してください。詳細は --debug で確認できます。"
                    .to_string(),
            ));
        }
        for step in &self.steps {
            if let Some(provider) = AiProvider::from_str(&step.provider)
                && self.is_step_installed(step, &provider)
            {
                return Ok(());
            }
        }
        Err(AppError::NoAiProviderInstalled)
    }

    /// ステップの実行バイナリがインストールされているかチェック。
    /// command 指定があればその先頭バイナリを、なければ provider 既定コマンドを調べる。
    fn is_step_installed(&self, step: &ProviderStep, provider: &AiProvider) -> bool {
        // Apple Intelligence: apple-ai feature 有効時のみ利用可能（ランタイムで可否判定）
        if matches!(provider, AiProvider::AppleIntelligence) {
            return cfg!(all(target_os = "macos", feature = "apple-ai"));
        }
        let bin = step
            .command
            .as_ref()
            .and_then(|c| c.first())
            .map(|s| s.as_str())
            .unwrap_or_else(|| provider.command());
        Self::is_binary_available(bin, &step.env)
    }

    /// 実行ファイルが PATH 上に存在するか。絶対/相対パスのラッパーも実在判定できる。
    fn is_binary_available(bin: &str, env: &std::collections::BTreeMap<String, String>) -> bool {
        let bin_path = Path::new(bin);
        if bin_path.is_absolute() || Self::has_path_separator(bin) {
            return Self::is_executable_file(bin_path);
        }

        let path_value = env
            .get("PATH")
            .map(std::ffi::OsString::from)
            .or_else(|| std::env::var_os("PATH"));
        let Some(path_value) = path_value else {
            return false;
        };

        std::env::split_paths(&path_value).any(|dir| {
            Self::binary_candidates(&dir, bin, env)
                .iter()
                .any(|candidate| Self::is_executable_file(candidate))
        })
    }

    fn has_path_separator(bin: &str) -> bool {
        bin.contains(std::path::MAIN_SEPARATOR) || (cfg!(windows) && bin.contains('/'))
    }

    fn is_executable_file(path: &Path) -> bool {
        let Ok(meta) = fs::metadata(path) else {
            return false;
        };
        if !meta.is_file() {
            return false;
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            meta.permissions().mode() & 0o111 != 0
        }
        #[cfg(not(unix))]
        {
            true
        }
    }

    fn binary_candidates(
        dir: &Path,
        bin: &str,
        env: &std::collections::BTreeMap<String, String>,
    ) -> Vec<std::path::PathBuf> {
        #[cfg(not(windows))]
        let _ = env;

        #[cfg(windows)]
        {
            let direct = dir.join(bin);
            if Path::new(bin).extension().is_some() {
                return vec![direct];
            }
            let pathext = env
                .get("PATHEXT")
                .map(std::ffi::OsString::from)
                .or_else(|| std::env::var_os("PATHEXT"))
                .map(|v| {
                    v.to_string_lossy()
                        .split(';')
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(|| {
                    vec![
                        ".COM".to_string(),
                        ".EXE".to_string(),
                        ".BAT".to_string(),
                        ".CMD".to_string(),
                    ]
                });
            std::iter::once(direct)
                .chain(
                    pathext
                        .into_iter()
                        .map(|ext| dir.join(format!("{bin}{ext}"))),
                )
                .collect()
        }
        #[cfg(not(windows))]
        {
            vec![dir.join(bin)]
        }
    }

    /// step に対して実際に使うモデルを解決する。
    /// step.model(非空) > [models].<provider> > 空文字列(=各 CLI 既定に委ねる)。
    pub(super) fn resolve_model(&self, provider: &AiProvider, step: &ProviderStep) -> String {
        if let Some(m) = step.model.as_deref().filter(|m| !m.is_empty()) {
            return m.to_string();
        }
        match provider {
            AiProvider::Antigravity => self.models.antigravity.clone(),
            AiProvider::Codex => self.models.codex.clone(),
            AiProvider::Claude => self.models.claude.clone(),
            AiProvider::Opencode => self.models.opencode.clone(),
            AiProvider::Grok => self.models.grok.clone(),
            AiProvider::AppleIntelligence => String::new(),
        }
    }

    /// ログ表示用のステップラベル(プロバイダー名 + モデル + アカウント識別)。
    fn step_label(provider: &AiProvider, step: &ProviderStep) -> String {
        let mut label = provider.name().to_string();
        let model = step.model.as_deref().filter(|m| !m.is_empty());
        let account = step.account_hint();
        let detail = match (model, account) {
            (Some(m), Some(a)) => Some(format!("{m}, {a}")),
            (Some(m), None) => Some(m.to_string()),
            (None, Some(a)) => Some(a),
            (None, None) => None,
        };
        if let Some(detail) = detail {
            label.push_str(&format!(" ({detail})"));
        }
        label
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

        for step in &self.steps {
            // provider が解決できないステップはスキップ(from_config で除外済みだが念のため)。
            let provider = match AiProvider::from_str(&step.provider) {
                Some(p) => p,
                None => continue,
            };
            if !self.is_step_installed(step, &provider) {
                continue;
            }

            let model = self.resolve_model(&provider, step);

            if !silent {
                println!(
                    "  {} {}...",
                    "Using".dimmed(),
                    Self::step_label(&provider, step).cyan()
                );
            }

            // 打ち切り応答(件名が助詞・前置詞の直後で終わる)は同じステップでも確率的に
            // 起きるため、次のステップへ落とす前にこの回数だけ引き直す。
            const TRUNCATION_RETRIES: u32 = 1;
            let mut retries_left = TRUNCATION_RETRIES;

            loop {
                // Apple Intelligence: fm-rs feature 有効時はネイティブ呼び出し
                #[cfg(all(target_os = "macos", feature = "apple-ai"))]
                let result = if matches!(provider, AiProvider::AppleIntelligence) {
                    Self::call_apple_intelligence_native(
                        &prompt,
                        &self.language,
                        prefix_type,
                        !recent_commits.is_empty(),
                    )
                } else {
                    self.call_provider(&provider, step, &model, &prompt, silent)
                };
                #[cfg(not(all(target_os = "macos", feature = "apple-ai")))]
                let result = self.call_provider(&provider, step, &model, &prompt, silent);

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
                            break;
                        }
                        // 生成が途中で打ち切られた件名はコミットに使わない。
                        // AI CLI 自体は成功(exit 0)を返すため、ここで弾かないと
                        // 「... mise 設定を」のような未完成のメッセージが確定してしまう。
                        let subject = message.lines().next().unwrap_or("").to_string();
                        if Self::is_truncated_subject(&subject) {
                            if retries_left > 0 {
                                retries_left -= 1;
                                if !silent {
                                    eprintln!(
                                        "  {} {} returned a truncated message ({}); retrying",
                                        "⚠".yellow(),
                                        Self::step_label(&provider, step),
                                        subject.dimmed()
                                    );
                                }
                                continue;
                            }
                            if !silent {
                                eprintln!(
                                    "  {} {} kept returning a truncated message ({})",
                                    "⚠".yellow(),
                                    Self::step_label(&provider, step),
                                    subject.dimmed()
                                );
                            }
                            last_error = Some(AppError::AiProviderError(format!(
                                "{} returned a truncated message: {}",
                                provider.name(),
                                subject
                            )));
                            break;
                        }
                        return Ok((message, provider.name()));
                    }
                    Err(e) => {
                        if !silent {
                            eprintln!(
                                "  {} {} failed: {}",
                                "⚠".yellow(),
                                Self::step_label(&provider, step),
                                e.to_string().red()
                            );
                        }
                        // 手動指定時は失敗記録をスキップ
                        if !self.provider_override {
                            self.record_provider_failure(step);
                        }
                        last_error = Some(e);
                        break;
                    }
                }
            }
        }

        Err(last_error.unwrap_or(AppError::NoAiProviderInstalled))
    }

    /// 特定のAIプロバイダーを呼び出し
    ///
    /// silent: --generate-for のように stdout を生成メッセージ専用に保つモード。
    /// デバッグ出力(コマンド表示・ストリーミング)を stderr へ逃がす。
    fn call_provider(
        &self,
        provider: &AiProvider,
        step: &ProviderStep,
        model: &str,
        prompt: &str,
        silent: bool,
    ) -> Result<String, AppError> {
        // opencode / grok は一時ファイル経由でプロンプトを渡す。
        // opencode は stdin サポートが不明確なため、grok は `--prompt-file` を使うと
        // `-p <PROMPT>` で長大な diff を渡すより堅牢(引数長制限や cmd.exe 経由の
        // メタ文字問題を回避)。TempFile の RAII ガードにより、どのパスで return しても
        // 自動クリーンアップされる。
        let temp_file = if matches!(provider, AiProvider::Opencode | AiProvider::Grok) {
            Some(TempFile::create_with_content(prompt.as_bytes())?)
        } else {
            None
        };

        // Codex CLI の標準出力は実行トランスクリプトを含むため、
        // 最終応答だけを専用ファイルに書き出してコミットメッセージとして読む。
        let codex_output_file = if matches!(provider, AiProvider::Codex) {
            Some(TempFile::create_with_content(b"")?)
        } else {
            None
        };

        // プロバイダー固有のコマンドを構築
        let (mut cmd, uses_stdin) = self.build_provider_command(
            provider,
            step,
            model,
            prompt,
            temp_file.as_ref(),
            codex_output_file.as_ref(),
        )?;

        // デバッグモード: 実行するコマンドを表示
        if self.debug {
            let debug_file = if matches!(provider, AiProvider::Codex) {
                codex_output_file.as_ref()
            } else {
                temp_file.as_ref()
            };
            self.print_debug_command(provider, step, model, prompt, debug_file, silent);
        }

        // プロセスを起動
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AppError::AiProviderError(format!("{} not found", provider.name()))
            } else {
                AppError::AiProviderError(e.to_string())
            }
        })?;

        // stdout/stderr をスレッドで読み取りつつ、stdin にも別スレッドで書き込み、
        // タイムアウト付きで完了を待機する。
        // stdin 書き込みと stdout/stderr 読み取りを並行させることで、
        // 大きいプロンプト使用時のパイプ双方向デッドロックを防ぐ
        // (詳細は run_process_with_timeout のコメントを参照)。
        let (exit_status, stdout_str, stderr_str) =
            self.run_process_with_timeout(&mut child, provider, uses_stdin, prompt, silent)?;

        let stdout_str = if let Some(output_file) = &codex_output_file {
            fs::read_to_string(output_file.path()).map_err(|e| {
                AppError::AiProviderError(format!("Failed to read Codex output file: {}", e))
            })?
        } else {
            stdout_str
        };

        // 出力を検証してメッセージを返す
        Self::process_provider_output(provider, exit_status, &stdout_str, &stderr_str)
    }
}

impl Default for AiService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::process::{Command, ExitStatus, Stdio};

    use super::*;
    use crate::ai_usage::{AiUsageAccount, AiUsageSnapshot, UsageWindowData};
    use crate::config::AiUsageConfig;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    #[test]
    fn test_ai_provider_name() {
        assert_eq!(AiProvider::Antigravity.name(), "Antigravity CLI");
        assert_eq!(AiProvider::Codex.name(), "Codex CLI");
        assert_eq!(AiProvider::Claude.name(), "Claude Code");
        assert_eq!(AiProvider::Opencode.name(), "opencode");
        assert_eq!(AiProvider::Grok.name(), "Grok CLI");
        assert_eq!(AiProvider::AppleIntelligence.name(), "Apple Intelligence");
    }

    #[test]
    fn test_ai_provider_command() {
        assert_eq!(AiProvider::Antigravity.command(), "agy");
        assert_eq!(AiProvider::Codex.command(), "codex");
        assert_eq!(AiProvider::Claude.command(), "claude");
        assert_eq!(AiProvider::Opencode.command(), "opencode");
        assert_eq!(AiProvider::Grok.command(), "grok");
        assert_eq!(AiProvider::AppleIntelligence.command(), "apple-ai");
    }

    #[test]
    fn test_ai_provider_config_key() {
        // state ファイルや設定ファイル上のキーは command と独立に定義する。
        // 旧 "gemini" キーは load 時にメモリ上で "antigravity" に合流させるため、
        // 公開キーは "antigravity" のみとする。
        assert_eq!(AiProvider::Antigravity.config_key(), "antigravity");
        assert_eq!(AiProvider::Codex.config_key(), "codex");
        assert_eq!(AiProvider::Claude.config_key(), "claude");
        assert_eq!(AiProvider::Opencode.config_key(), "opencode");
        assert_eq!(AiProvider::Grok.config_key(), "grok");
        assert_eq!(AiProvider::AppleIntelligence.config_key(), "apple-ai");
    }

    #[rstest]
    #[case("antigravity", Some(AiProvider::Antigravity))]
    #[case("ANTIGRAVITY", Some(AiProvider::Antigravity))]
    #[case("Antigravity", Some(AiProvider::Antigravity))]
    #[case("agy", Some(AiProvider::Antigravity))]
    #[case("AGY", Some(AiProvider::Antigravity))]
    // 後方互換: 旧 Gemini CLI 名は Antigravity にマップする
    #[case("gemini", Some(AiProvider::Antigravity))]
    #[case("GEMINI", Some(AiProvider::Antigravity))]
    #[case("Gemini", Some(AiProvider::Antigravity))]
    #[case("codex", Some(AiProvider::Codex))]
    #[case("claude", Some(AiProvider::Claude))]
    #[case("opencode", Some(AiProvider::Opencode))]
    #[case("OPENCODE", Some(AiProvider::Opencode))]
    #[case("grok", Some(AiProvider::Grok))]
    #[case("GROK", Some(AiProvider::Grok))]
    #[case("Grok", Some(AiProvider::Grok))]
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

    #[rstest]
    #[case("gemini", true)]
    #[case("GEMINI", true)]
    #[case("Gemini", true)]
    #[case("antigravity", false)]
    #[case("agy", false)]
    #[case("", false)]
    #[case("codex", false)]
    fn test_is_legacy_gemini_alias(#[case] input: &str, #[case] expected: bool) {
        assert_eq!(AiProvider::is_legacy_gemini_alias(input), expected);
    }

    #[test]
    fn test_check_arg_size_limit_accepts_normal_prompt() {
        // 通常運用範囲のプロンプトサイズは受理される
        let prompt = "x".repeat(10_000);
        assert!(AiService::check_arg_size_limit(&prompt).is_ok());
    }

    #[test]
    fn test_check_arg_size_limit_rejects_oversized_prompt() {
        // 512 KiB を超えるプロンプトは明確なエラーで拒否される
        let prompt = "x".repeat(512 * 1024 + 1);
        let result = AiService::check_arg_size_limit(&prompt);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("Antigravity CLI") && msg.contains("byte limit"),
            "error message should mention Antigravity CLI and byte limit: {}",
            msg
        );
    }

    #[test]
    fn test_ai_service_new() {
        let service = AiService::new();
        assert_eq!(service.language, "Japanese");
        let expected_len = if cfg!(all(target_os = "macos", feature = "apple-ai")) {
            6
        } else {
            5
        };
        assert_eq!(service.steps.len(), expected_len);
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
        assert_eq!(service.steps.len(), 1);
        assert_eq!(service.steps[0].provider, "claude");
        assert!(service.provider_override);
    }

    #[test]
    fn test_set_provider_override_replaces_all() {
        let mut service = AiService::new();
        let original_len = service.steps.len();
        assert!(original_len > 1);
        service.set_provider_override(AiProvider::Antigravity);
        assert_eq!(service.steps.len(), 1);
        assert_eq!(service.steps[0].provider, "antigravity");
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
        // エージェントコンテキストは変更内容セクションより前に配置する。
        let ctx_pos = prompt.find("<agent-context>").unwrap();
        let changes_pos = prompt.find("<changes>").unwrap();
        assert!(
            ctx_pos < changes_pos,
            "エージェントコンテキストは変更内容セクションより前に配置されるべき"
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
        // 空のエージェントコンテキストではセクションを追加しない。
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

    /// 打ち切り検出。真陽性は agy 1.1.21 + GPT-OSS 120B (Medium) が exit 0 で返した応答と、
    /// 実際のコミット履歴に残っていた打ち切りコミットから採取。偽陽性ケースは同じ履歴
    /// (132 リポジトリ / 9136 コミット)で「正常なのに打ち切りと誤判定した」ものを回帰として
    /// 固定する。誤検出は正常な件名を捨ててコミット不能に至らせるため、ここが最も重要。
    #[rstest]
    // 日本語: 格助詞の直後で切れている(agy の実測応答)
    #[case("ci: GitHub Actions CIワークフローとmise設定を", true)]
    #[case("ci: GitHub Actions workflowとmise設定ファイルを", true)]
    #[case("refactor: state.rs のリトライ処理と", true)]
    #[case("docs: READMEの", true)]
    #[case("fix: 設定ファイルの読み込みが", true)]
    // 日本語: 実際のコミット履歴に残っていた打ち切り
    #[case("perf: 行コンテキスト取得を O(1) に最適化し LineIndex を", true)]
    #[case("feat: 異常状態フラグで赤ベル表示を", true)]
    #[case("ci: get-latest-tag-review と review ジョブに自動リトライ設定を", true)]
    #[case(
        "fix: Dockerfile に pnpm-workspace.yaml をコピーして CI ビルド失敗を",
        true
    )]
    // 日本語: 読点で切れている
    #[case("feat: mise 対応を追加、", true)]
    // 日本語: 正常な件名(体言止め・活用語尾)は打ち切り扱いしない
    #[case("ci: GitHub Actions CIワークフローとmise設定を追加", false)]
    #[case("feat: mise でツールバージョンを固定する", false)]
    #[case("fix: 競合状態を修正", false)]
    #[case("chore: 依存を更新", false)]
    // 日本語: 「〜に」「〜へ」で終わる体言止めは正常。実際のコミット履歴からの回帰ケースで、
    // これらを打ち切り扱いすると通常のコミットが弾かれてしまう
    #[case("feat: 再生速度をスライダーで調整可能に", false)]
    #[case("build: cc と biome のバージョンを最新へ", false)]
    #[case("fix: Gitステータス取得失敗でfail-closedに", false)]
    #[case("fix: X プロバイダ名を twitter から x に", false)]
    #[case("style: 日付横バッジ間隔を gap-3 に", false)]
    #[case("docs: READMEにmise手順を追記", false)]
    #[case("fix: 推奨の剣を名前の後ろへ、丸を SVG に", false)]
    // 日本語の件名に含まれる単独の英字を冠詞と読み違えない(実際のコミット履歴からの回帰ケース)
    #[case(
        "fix(lint): 構文ゲートの残り 2 件を「1 件目だけ出す」で塞ぐ (#57 案 A)",
        false
    )]
    // 全角括弧は日本語の注釈で非対称に使われるため打ち切り扱いしない
    #[case("docs: 調査結果（端末3912 の PC 側オーディオデバイス不安定", false)]
    // 英語: 前置詞・接続詞の直後で切れている
    #[case("feat: add mise support for", true)]
    #[case("fix: resolve race condition in the", true)]
    #[case("refactor: split provider command and", true)]
    // 英語: 正常な件名
    #[case("feat: add mise support for tool versions", false)]
    #[case("fix: resolve race condition in state file", false)]
    // Conventional Commits の scope が閉じずに切れている
    #[case("feat(mise", true)]
    #[case("feat(mise): ツールバージョン固定を追加", false)]
    // 空文字は別のエラーとして扱うため、ここでは打ち切り扱いしない
    #[case("", false)]
    #[case("   ", false)]
    fn test_is_truncated_subject(#[case] subject: &str, #[case] expected: bool) {
        assert_eq!(
            AiService::is_truncated_subject(subject),
            expected,
            "subject: {subject:?}"
        );
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
        let error = AiService::extract_error(stderr, &AiProvider::Antigravity);
        assert_eq!(error, "[API Error: Rate limit exceeded]");
    }

    #[test]
    fn test_extract_error_antigravity_generic() {
        // `[API Error:` でも `critical error` でも `Error:` でもないシンプルなメッセージは
        // 最初の非空行をそのまま返す (旧 Gemini 実装の固定ラベルから方針変更し、具体情報を優先する)。
        let stderr = "Some generic error";
        let error = AiService::extract_error(stderr, &AiProvider::Antigravity);
        assert_eq!(error, "Some generic error");
    }

    #[test]
    fn test_extract_error_antigravity_empty_falls_back() {
        // stderr が完全に空の場合のみ固定の "Antigravity CLI request failed" にフォールバック
        let stderr = "";
        let error = AiService::extract_error(stderr, &AiProvider::Antigravity);
        assert_eq!(error, "Antigravity CLI request failed");
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
        let error = AiService::extract_error(stderr, &AiProvider::Antigravity);
        assert!(error.contains("critical error") || error.contains("Error:"));
    }

    #[test]
    fn test_extract_error_gemini_critical_error() {
        let stderr = "An unexpected critical error occurred:Error: something bad";
        let error = AiService::extract_error(stderr, &AiProvider::Antigravity);
        assert!(error.contains("critical error"));
    }

    #[test]
    fn test_extract_error_gemini_multiple_api_errors() {
        let stderr = "[API Error: first]\n[API Error: second]";
        let error = AiService::extract_error(stderr, &AiProvider::Antigravity);
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

    /// "reconnecting" と "error" が同じ行に同居する場合は再接続ログとして扱い、
    /// その後ろにある本物のエラー行を採用する。
    /// 例: `Reconnecting after error: connection reset` のような再接続ログを
    /// API エラー本体と誤検出しないための不変条件を固定する。
    #[test]
    fn test_extract_error_codex_skips_reconnecting_line_even_when_it_contains_error_keyword() {
        let stderr = "Reconnecting after error: connection reset\nERROR: rate limit exceeded";
        let error = AiService::extract_error(stderr, &AiProvider::Codex);
        // 先頭行に "error" は含むが "reconnecting" を含むためスキップされ、
        // 後続の "ERROR:" 行が選ばれる。
        assert_eq!(error, "ERROR: rate limit exceeded");
    }

    /// `Reconnecting` のような再接続ログだけが流れて、本物のエラー行が無いケースでも
    /// "Codex API request failed" のジェネリックフォールバックに落ちることを保証する。
    /// `lines().rev().find(...)` で `Reconnecting`/`Reading prompt` が除外されるため、
    /// 全ての行が除外対象だと唯一 unwrap_or 経路に到達する。
    #[test]
    fn test_extract_error_codex_only_reconnecting_lines_falls_back_to_generic() {
        let stderr = "Reconnecting to server...\nReading prompt from stdin...\n";
        let error = AiService::extract_error(stderr, &AiProvider::Codex);
        assert_eq!(error, "Codex API request failed");
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
            6
        } else {
            5
        };
        assert_eq!(service.steps.len(), expected_len);
        // antigravity のデフォルトは GPT-OSS 120B (Medium)
        assert_eq!(
            service.models.antigravity,
            Config::default().models.antigravity
        );
        assert_eq!(service.models.codex, Config::default().models.codex);
        assert_eq!(service.models.claude, "haiku");
        assert_eq!(service.models.opencode, "");
        assert_eq!(service.timeout_seconds, 60);
    }

    #[test]
    fn test_ai_service_from_config_custom_providers() {
        // 設定ファイルに旧 "gemini" 文字列を書いた場合、from_str で Antigravity にマップされる。
        let config = Config {
            providers: vec![
                ProviderStep::from_provider("claude"),
                ProviderStep::from_provider("gemini"),
            ],
            ..Default::default()
        };
        let service = AiService::from_config(&config);

        // reorder_stepsで順序が変わる可能性があるため、含有のみ検証
        assert_eq!(service.steps.len(), 2);
        let providers: Vec<&str> = service.steps.iter().map(|s| s.provider.as_str()).collect();
        assert!(providers.contains(&"claude"));
        // "gemini" は from_str では Antigravity 扱いだが、ProviderStep.provider 文字列は
        // 入力のまま保持される (正規化はキー比較側で行う) ため "gemini" のまま残る。
        assert!(providers.contains(&"gemini"));
    }

    #[test]
    fn test_ai_service_from_config_detects_legacy_gemini_alias_in_providers() {
        // providers に "gemini" を含む場合、legacy エイリアス検出フラグが立つ。
        let config = Config {
            providers: vec![
                ProviderStep::from_provider("gemini"),
                ProviderStep::from_provider("claude"),
            ],
            ..Default::default()
        };
        let service = AiService::from_config(&config);
        assert!(
            service.legacy_gemini_alias_detected,
            "providers に 'gemini' があれば legacy alias 検出フラグが立つべき"
        );
    }

    #[test]
    fn test_ai_service_from_config_no_legacy_alias_for_clean_config() {
        // デフォルト設定 (providers に 'gemini' エイリアスなし) では legacy 検出されない。
        let config = Config::default();
        let service = AiService::from_config(&config);
        assert!(!service.legacy_gemini_alias_detected);
    }

    #[test]
    fn test_ai_service_from_config_invalid_providers_fallback() {
        let config = Config {
            providers: vec![
                ProviderStep::from_provider("invalid"),
                ProviderStep::from_provider("unknown"),
            ],
            ..Default::default()
        };
        let service = AiService::from_config(&config);

        // 無効なプロバイダーのみの場合はデフォルトにフォールバック
        let expected_len = if cfg!(all(target_os = "macos", feature = "apple-ai")) {
            6
        } else {
            5
        };
        assert_eq!(service.steps.len(), expected_len);
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
        config.models.antigravity = "pro".to_string();
        config.models.codex = "gpt-4".to_string();
        config.models.claude = "opus".to_string();
        let service = AiService::from_config(&config);

        assert_eq!(service.models.antigravity, "pro");
        assert_eq!(service.models.codex, "gpt-4");
        assert_eq!(service.models.claude, "opus");
    }

    #[test]
    fn test_ai_service_from_config_codex_reasoning_effort() {
        let config = Config {
            codex_reasoning_effort: "high".to_string(),
            ..Config::default()
        };
        let service = AiService::from_config(&config);

        assert_eq!(service.codex_reasoning_effort, "high");
    }

    #[test]
    fn test_ai_service_from_config_default_reasoning_effort_is_low() {
        let config = Config::default();
        let service = AiService::from_config(&config);

        assert_eq!(service.codex_reasoning_effort, "low");
    }

    // ============================================================
    // AiService::default のテスト
    // ============================================================

    #[test]
    fn test_ai_service_default() {
        let service = AiService::default();

        assert_eq!(service.language, "Japanese");
        let expected_len = if cfg!(all(target_os = "macos", feature = "apple-ai")) {
            6
        } else {
            5
        };
        assert_eq!(service.steps.len(), expected_len);
        assert_eq!(service.steps[0].provider, "opencode");
        assert_eq!(service.steps[1].provider, "grok");
        assert_eq!(service.steps[2].provider, "antigravity");
        assert_eq!(service.steps[3].provider, "codex");
        assert_eq!(service.steps[4].provider, "claude");
        if cfg!(all(target_os = "macos", feature = "apple-ai")) {
            assert_eq!(service.steps[5].provider, "apple-intelligence");
        }
    }

    // ============================================================
    // format_command_for_debug テスト
    // ============================================================

    #[test]
    fn test_format_command_for_debug_antigravity() {
        // Antigravity CLI (`agy`) はモデルや debug フラグを持たないので、`agy -p 'PROMPT'` のみ表示される。
        let service = AiService::new();
        let cmd = service.format_command_for_debug(
            &AiProvider::Antigravity,
            &service.models.antigravity.clone(),
            &ProviderStep::from_provider("antigravity"),
            "test prompt",
            None,
        );
        assert!(
            cmd.starts_with("agy "),
            "expected `agy` invocation, got: {}",
            cmd
        );
        assert!(cmd.contains("-p 'test prompt'"));
        // AiService::new() のデフォルトモデルが --model として付与される
        assert!(cmd.contains("--model 'GPT-OSS 120B (Medium)'"));
        assert!(!cmd.contains("--debug"));
    }

    #[test]
    fn test_format_command_for_debug_codex() {
        let service = AiService::new();
        let cmd = service.format_command_for_debug(
            &AiProvider::Codex,
            &service.models.codex.clone(),
            &ProviderStep::from_provider("codex"),
            "test prompt",
            None,
        );
        assert!(cmd.contains("codex --disable hooks -c model_reasoning_effort='low' exec"));
        assert!(cmd.contains("-o '<output_file>'"));
        assert!(cmd.contains("echo 'test prompt'"));
    }

    #[test]
    fn test_format_command_for_debug_claude() {
        let service = AiService::new();
        let cmd = service.format_command_for_debug(
            &AiProvider::Claude,
            &service.models.claude.clone(),
            &ProviderStep::from_provider("claude"),
            "test prompt",
            None,
        );
        assert!(cmd.contains("claude --model"));
        assert!(cmd.contains("-p"));
        assert!(cmd.contains("echo 'test prompt'"));
    }

    #[test]
    fn test_format_command_for_debug_opencode() {
        // デフォルトは空モデルなので -m なし
        let service = AiService::new();
        let temp_path = std::path::Path::new("/tmp/git-sc-prompt-12345.txt");
        let cmd = service.format_command_for_debug(
            &AiProvider::Opencode,
            &service.models.opencode.clone(),
            &ProviderStep::from_provider("opencode"),
            "test prompt",
            Some(temp_path),
        );
        assert!(cmd.contains("opencode run"));
        assert!(!cmd.contains("-m"));
        assert!(cmd.contains("-f '/tmp/git-sc-prompt-12345.txt'"));
    }

    #[test]
    fn test_format_command_for_debug_opencode_with_model() {
        let mut service = AiService::new();
        service.models.opencode = "opencode/some-model".to_string();
        let temp_path = std::path::Path::new("/tmp/git-sc-prompt-12345.txt");
        let cmd = service.format_command_for_debug(
            &AiProvider::Opencode,
            "opencode/some-model",
            &ProviderStep::from_provider("opencode"),
            "test prompt",
            Some(temp_path),
        );
        assert!(cmd.contains("opencode run"));
        assert!(cmd.contains("-m 'opencode/some-model'"));
        assert!(cmd.contains("-f '/tmp/git-sc-prompt-12345.txt'"));
    }

    #[test]
    fn test_format_command_for_debug_opencode_no_path() {
        let service = AiService::new();
        let cmd = service.format_command_for_debug(
            &AiProvider::Opencode,
            &service.models.opencode.clone(),
            &ProviderStep::from_provider("opencode"),
            "test prompt",
            None,
        );
        assert!(cmd.contains("opencode run"));
        assert!(cmd.contains("-f '<temp_file>'"));
    }

    #[test]
    fn test_format_command_for_debug_grok() {
        // grok はデフォルトで空モデルなので -m なし。副作用抑制フラグ一式が付与される。
        let service = AiService::new();
        let temp_path = std::path::Path::new("/tmp/git-sc-prompt-grok.txt");
        let cmd = service.format_command_for_debug(
            &AiProvider::Grok,
            &service.models.grok.clone(),
            &ProviderStep::from_provider("grok"),
            "test prompt",
            Some(temp_path),
        );
        assert!(cmd.starts_with("grok "), "grok から始まるべき: {}", cmd);
        assert!(cmd.contains("--output-format plain"));
        assert!(cmd.contains("--sandbox read-only"));
        assert!(cmd.contains("--no-plan"));
        assert!(cmd.contains("--no-memory"));
        assert!(cmd.contains("--disable-web-search"));
        assert!(cmd.contains("--max-turns 1"));
        assert!(cmd.contains("--verbatim"));
        assert!(cmd.contains("--prompt-file '/tmp/git-sc-prompt-grok.txt'"));
        assert!(!cmd.contains(" -m "), "モデル空なら -m は付けない: {}", cmd);
    }

    #[test]
    fn test_format_command_for_debug_grok_with_model() {
        let mut service = AiService::new();
        service.models.grok = "grok-4.5".to_string();
        let temp_path = std::path::Path::new("/tmp/git-sc-prompt-grok.txt");
        let cmd = service.format_command_for_debug(
            &AiProvider::Grok,
            "grok-4.5",
            &ProviderStep::from_provider("grok"),
            "test prompt",
            Some(temp_path),
        );
        assert!(cmd.contains("-m 'grok-4.5'"));
        assert!(cmd.contains("--prompt-file '/tmp/git-sc-prompt-grok.txt'"));
    }

    #[test]
    fn test_format_command_for_debug_grok_no_path() {
        let service = AiService::new();
        let cmd = service.format_command_for_debug(
            &AiProvider::Grok,
            &service.models.grok.clone(),
            &ProviderStep::from_provider("grok"),
            "test prompt",
            None,
        );
        assert!(cmd.contains("--prompt-file '<temp_file>'"));
    }

    #[test]
    fn test_format_command_for_debug_apple_intelligence() {
        let service = AiService::new();
        let cmd = service.format_command_for_debug(
            &AiProvider::AppleIntelligence,
            "",
            &ProviderStep::from_provider("apple-intelligence"),
            "test prompt",
            None,
        );
        assert!(cmd.contains("apple-ai"));
        assert!(cmd.contains("echo 'test prompt'"));
    }

    #[test]
    fn test_format_command_for_debug_prompt_with_single_quotes() {
        let service = AiService::new();
        let cmd = service.format_command_for_debug(
            &AiProvider::Antigravity,
            &service.models.antigravity.clone(),
            &ProviderStep::from_provider("antigravity"),
            "it's a test",
            None,
        );
        assert!(cmd.contains("it'\\''s a test"));
    }

    #[test]
    fn test_format_command_for_debug_antigravity_with_model() {
        // antigravity モデルを指定すると、デバッグ表示に `--model` が現れる。
        let mut service = AiService::new();
        service.models.antigravity = "GPT-OSS 120B (Medium)".to_string();
        let cmd = service.format_command_for_debug(
            &AiProvider::Antigravity,
            "GPT-OSS 120B (Medium)",
            &ProviderStep::from_provider("antigravity"),
            "test",
            None,
        );
        assert!(cmd.contains("agy --model 'GPT-OSS 120B (Medium)' -p 'test'"));
    }

    #[test]
    fn test_format_command_for_debug_codex_empty_model() {
        let mut service = AiService::new();
        service.models.codex = String::new();
        let cmd = service.format_command_for_debug(
            &AiProvider::Codex,
            "",
            &ProviderStep::from_provider("codex"),
            "test",
            None,
        );
        assert!(cmd.contains("codex --disable hooks -c model_reasoning_effort='low' exec"));
        assert!(!cmd.contains("--model"));
    }

    #[test]
    fn test_format_command_for_debug_codex_always_disables_hooks() {
        let service = AiService::new();
        let cmd = service.format_command_for_debug(
            &AiProvider::Codex,
            &service.models.codex.clone(),
            &ProviderStep::from_provider("codex"),
            "test",
            None,
        );
        assert!(
            cmd.contains("--disable hooks"),
            "Codex 呼び出しでは常に --disable hooks が付くべき: {}",
            cmd
        );
    }

    #[test]
    fn test_format_command_for_debug_codex_custom_reasoning_effort() {
        let mut service = AiService::new();
        service.codex_reasoning_effort = "high".to_string();
        let cmd = service.format_command_for_debug(
            &AiProvider::Codex,
            &service.models.codex.clone(),
            &ProviderStep::from_provider("codex"),
            "test",
            None,
        );
        assert!(cmd.contains("-c model_reasoning_effort='high'"));
        assert!(!cmd.contains("model_reasoning_effort='low'"));
    }

    #[test]
    fn test_format_command_for_debug_codex_empty_reasoning_effort_omits_flag() {
        let mut service = AiService::new();
        service.codex_reasoning_effort = String::new();
        let cmd = service.format_command_for_debug(
            &AiProvider::Codex,
            &service.models.codex.clone(),
            &ProviderStep::from_provider("codex"),
            "test",
            None,
        );
        assert!(cmd.contains("codex --disable hooks exec"));
        assert!(!cmd.contains("model_reasoning_effort"));
    }

    /// Grok: 一時ファイルとフラグ群が正しく組み立てられ、stdin を使わないこと
    #[test]
    fn test_build_provider_command_grok_uses_prompt_file() {
        let service = AiService::new();
        let prompt_file = TempFile::create_with_content(b"test").unwrap();

        let (cmd, uses_stdin) = service
            .build_provider_command(
                &AiProvider::Grok,
                &ProviderStep::from_provider("grok"),
                "",
                "test",
                Some(&prompt_file),
                None,
            )
            .unwrap();
        let debug = format!("{:?}", cmd);

        assert!(!uses_stdin, "grok は stdin を使わない (--prompt-file 経由)");
        // 副作用抑制フラグが全て含まれる
        assert!(debug.contains("\"--output-format\""));
        assert!(debug.contains("\"plain\""));
        assert!(debug.contains("\"--sandbox\""));
        assert!(debug.contains("\"read-only\""));
        assert!(debug.contains("\"--no-plan\""));
        assert!(debug.contains("\"--no-memory\""));
        assert!(debug.contains("\"--disable-web-search\""));
        assert!(debug.contains("\"--max-turns\""));
        assert!(debug.contains("\"--verbatim\""));
        assert!(debug.contains("\"--prompt-file\""));
        assert!(debug.contains(&prompt_file.path().to_string_lossy().to_string()));
    }

    /// Grok: モデル指定があれば `-m <model>` を付与すること
    #[test]
    fn test_build_provider_command_grok_passes_model() {
        let mut service = AiService::new();
        service.models.grok = "grok-4.5".to_string();
        let prompt_file = TempFile::create_with_content(b"test").unwrap();

        let (cmd, _) = service
            .build_provider_command(
                &AiProvider::Grok,
                &ProviderStep::from_provider("grok"),
                "grok-4.5",
                "test",
                Some(&prompt_file),
                None,
            )
            .unwrap();
        let debug = format!("{:?}", cmd);

        assert!(debug.contains("\"-m\""));
        assert!(debug.contains("\"grok-4.5\""));
    }

    /// Grok: モデル未指定(空)なら `-m` を付けないこと
    #[test]
    fn test_build_provider_command_grok_no_model_when_empty() {
        let mut service = AiService::new();
        service.models.grok = String::new();
        let prompt_file = TempFile::create_with_content(b"test").unwrap();

        let (cmd, _) = service
            .build_provider_command(
                &AiProvider::Grok,
                &ProviderStep::from_provider("grok"),
                "",
                "test",
                Some(&prompt_file),
                None,
            )
            .unwrap();
        let debug = format!("{:?}", cmd);

        assert!(!debug.contains("\"-m\""));
    }

    #[test]
    fn test_build_provider_command_codex_uses_output_file() {
        let service = AiService::new();
        let output_file = TempFile::create_with_content(b"").unwrap();

        let (cmd, uses_stdin) = service
            .build_provider_command(
                &AiProvider::Codex,
                &ProviderStep::from_provider("codex"),
                "",
                "test",
                None,
                Some(&output_file),
            )
            .unwrap();
        let debug = format!("{:?}", cmd);

        assert!(uses_stdin);
        assert!(debug.contains("\"-o\""));
        assert!(debug.contains(&output_file.path().to_string_lossy().to_string()));
    }

    /// Windows では全プロバイダーを `cmd /C` 経由で起動するが、cmd.exe は Rust 標準の
    /// 引数クォートを解釈しないため、diff を含むプロンプトを `-p` 引数で安全に渡せない。
    /// Antigravity はコマンド構築段階で明示エラーになり、フォールバックへ進むこと。
    #[cfg(windows)]
    #[test]
    fn test_build_provider_command_rejects_antigravity_on_windows() {
        let service = AiService::new();

        let result = service.build_provider_command(
            &AiProvider::Antigravity,
            &ProviderStep::from_provider("antigravity"),
            "",
            "feat: test",
            None,
            None,
        );

        match result {
            Err(AppError::AiProviderError(msg)) => {
                assert!(msg.contains("Windows"), "unexpected message: {}", msg);
            }
            other => panic!("expected AiProviderError, got {:?}", other.map(|_| ())),
        }
    }

    /// Antigravity (Unix系) はプロンプトを `-p` 引数で渡し、stdin を使わないこと
    #[cfg(not(windows))]
    #[test]
    fn test_build_provider_command_antigravity_passes_prompt_as_arg() {
        let service = AiService::new();

        let (cmd, uses_stdin) = service
            .build_provider_command(
                &AiProvider::Antigravity,
                &ProviderStep::from_provider("antigravity"),
                "",
                "feat: test",
                None,
                None,
            )
            .unwrap();
        let debug = format!("{:?}", cmd);

        assert!(!uses_stdin);
        assert!(debug.contains("\"-p\""));
        assert!(debug.contains("feat: test"));
    }

    /// Antigravity: モデル指定があれば `--model` を付与すること
    #[cfg(not(windows))]
    #[test]
    fn test_build_provider_command_antigravity_passes_model() {
        let mut service = AiService::new();
        service.models.antigravity = "GPT-OSS 120B (Medium)".to_string();

        let (cmd, _) = service
            .build_provider_command(
                &AiProvider::Antigravity,
                &ProviderStep::from_provider("antigravity"),
                "GPT-OSS 120B (Medium)",
                "feat: test",
                None,
                None,
            )
            .unwrap();
        let debug = format!("{:?}", cmd);

        assert!(debug.contains("\"--model\""));
        assert!(debug.contains("GPT-OSS 120B (Medium)"));
    }

    /// Antigravity: モデル未指定(空)なら `--model` を付けないこと
    #[cfg(not(windows))]
    #[test]
    fn test_build_provider_command_antigravity_no_model_when_empty() {
        let mut service = AiService::new();
        service.models.antigravity = String::new();

        let (cmd, _) = service
            .build_provider_command(
                &AiProvider::Antigravity,
                &ProviderStep::from_provider("antigravity"),
                "",
                "feat: test",
                None,
                None,
            )
            .unwrap();
        let debug = format!("{:?}", cmd);

        assert!(!debug.contains("--model"));
    }

    /// Apple Intelligence の system instructions が prefix_type に連動すること。
    /// 旧実装は Conventional Commits を固定強制しており、prefix_type 設定
    /// (none/bracket/emoji等)や過去コミット様式への自動追従と矛盾していた。
    #[cfg(all(target_os = "macos", feature = "apple-ai"))]
    #[test]
    fn test_apple_instructions_follow_prefix_type() {
        // conventional: 従来どおり type prefix を強制
        let conventional =
            AiService::build_apple_instructions("Japanese", Some("conventional"), false);
        assert!(conventional.contains("Japanese"));
        assert!(conventional.contains("MUST start with a type prefix"));
        assert!(conventional.contains("feat:"));

        // none/plain: prefix の禁止を明示(固定強制の矛盾が解消されていること)
        let none = AiService::build_apple_instructions("Japanese", Some("none"), false);
        assert!(none.contains("MUST NOT start with any type prefix"));
        let plain = AiService::build_apple_instructions("Japanese", Some("plain"), false);
        assert!(plain.contains("MUST NOT start with any type prefix"));

        // bracket: 角括弧形式を強制
        let bracket = AiService::build_apple_instructions("Japanese", Some("bracket"), false);
        assert!(bracket.contains("[Add]"));
        assert!(!bracket.contains("MUST start with a type prefix"));

        // カスタム prefix はそのまま指示に含める
        let custom = AiService::build_apple_instructions("Japanese", Some("MYPROJ-"), false);
        assert!(custom.contains("MYPROJ-"));

        // 自動判定(直近コミットあり): 模倣を強制しつつ、具体的な prefix 例を
        // 含めない(小型モデルが例をオウム返しするのを防ぐ)
        let auto = AiService::build_apple_instructions("Japanese", None, true);
        assert!(auto.contains("MUST imitate their format"));
        assert!(!auto.contains("feat:"));
        assert!(!auto.contains("[Add]"));

        // 自動判定(直近コミットなし): プロンプト側のフォールバックと同じく
        // conventional ルールで揃える
        let auto_empty = AiService::build_apple_instructions("Japanese", None, false);
        assert!(auto_empty.contains("MUST start with a type prefix"));
    }

    #[test]
    fn test_format_command_for_debug_claude_empty_model() {
        let mut service = AiService::new();
        service.models.claude = String::new();
        let cmd = service.format_command_for_debug(
            &AiProvider::Claude,
            "",
            &ProviderStep::from_provider("claude"),
            "test",
            None,
        );
        assert!(cmd.contains("claude"));
        assert!(cmd.contains("-p"));
        assert!(!cmd.contains("--model"));
    }

    #[test]
    fn test_format_command_for_debug_opencode_empty_model() {
        let mut service = AiService::new();
        service.models.opencode = String::new();
        let temp_path = std::path::Path::new("/tmp/test.txt");
        let cmd = service.format_command_for_debug(
            &AiProvider::Opencode,
            "",
            &ProviderStep::from_provider("opencode"),
            "test",
            Some(temp_path),
        );
        assert!(cmd.contains("opencode run"));
        assert!(!cmd.contains("-m"));
        assert!(cmd.contains("-f '/tmp/test.txt'"));
    }

    // ============================================================
    // (AiProvider config_key のテストは Antigravity への移行に伴い
    //  ファイル冒頭の test_ai_provider_config_key で再定義済み)
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
            &prompt,
            "English",
            prefix_type,
            false,
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
            &prompt,
            "Japanese",
            prefix_type,
            false,
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

    #[cfg(unix)]
    #[test]
    fn test_run_process_with_timeout_captures_stdout_and_stderr() {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("printf 'stdout-line\\n'; printf 'stderr-line\\n' >&2")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut service = AiService::new();
        service.timeout_seconds = 5;

        let (status, stdout, stderr) = service
            .run_process_with_timeout(&mut child, &AiProvider::Codex, false, "", false)
            .unwrap();

        assert!(status.success());
        assert_eq!(stdout, "stdout-line\n");
        assert_eq!(stderr, "stderr-line\n");
    }

    #[cfg(unix)]
    #[test]
    fn test_run_process_with_timeout_kills_timed_out_process() {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("sleep 2")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut service = AiService::new();
        service.timeout_seconds = 0;

        let result =
            service.run_process_with_timeout(&mut child, &AiProvider::Codex, false, "", false);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("timed out"));
        // タイムアウト経路でも wait 済みで、子プロセスが残らないことを確認する。
        assert!(child.try_wait().unwrap().is_some());
    }

    /// 大きいプロンプトを stdin に書き込みつつ、子プロセスが stdin を読み切る前に
    /// 大量の stdout を出力するケースでもデッドロックしないことを検証する。
    /// stdin 書き込みと stdout 読み取りが並行していない実装(旧実装)では、双方の
    /// パイプバッファが満杯になって相互にブロックし、タイムアウトしてしまう。
    #[cfg(unix)]
    #[test]
    fn test_run_process_with_timeout_large_stdin_no_deadlock() {
        // 子プロセス: 先に約 1MiB を stdout へ出力してから stdin を読み捨てる。
        // 先に大量出力することで「子が stdin 消費前に stdout でブロックする」状況を作る。
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("yes ABCDEFGHIJ | head -n 100000; cat >/dev/null")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut service = AiService::new();
        service.timeout_seconds = 30;

        // パイプバッファ(macOS/Linux で概ね 16〜64KiB)を確実に超えるプロンプト
        let large_prompt = "x".repeat(1_000_000);

        let (status, stdout, _stderr) = service
            .run_process_with_timeout(&mut child, &AiProvider::Codex, true, &large_prompt, false)
            .unwrap();

        // デッドロックせず子は正常終了し、stdout も全量読み取れている
        assert!(status.success());
        assert_eq!(stdout.len(), 100_000 * 11); // "ABCDEFGHIJ\n" = 11 バイト/行
    }

    /// uses_stdin=true で子プロセスが stdin を読まないまま動き続ける場合でも、
    /// (stdin 書き込みスレッドがブロックしても)メインループのタイムアウトで kill され、
    /// デッドロックせずタイムアウトエラーになることを検証する。
    #[cfg(unix)]
    #[test]
    fn test_run_process_with_timeout_blocked_stdin_still_times_out() {
        // 子: stdin を読まず、出力もせず、長時間 sleep する
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("sleep 30")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut service = AiService::new();
        service.timeout_seconds = 1;

        // パイプバッファを超える大きさ。子が読まないため write_all はブロックするが、
        // メインの try_wait ループがタイムアウトを検出して kill するためデッドロックしない。
        let large_prompt = "x".repeat(1_000_000);

        let result = service.run_process_with_timeout(
            &mut child,
            &AiProvider::Codex,
            true,
            &large_prompt,
            false,
        );

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("timed out"));
        // タイムアウト経路でも wait 済みで、子プロセスが残らないことを確認する。
        assert!(child.try_wait().unwrap().is_some());
    }

    /// uses_stdin=true で子プロセスが stdin を読まずに正常終了(exit 0)した場合、
    /// プロンプトの書き込みが BrokenPipe で失敗するため、不完全なプロンプトでの結果を
    /// 成功扱いせず provider エラーになることを検証する。
    #[cfg(unix)]
    #[test]
    fn test_run_process_with_timeout_stdin_write_failure_errors_on_success_exit() {
        // 子: stdin を一切読まず即座に exit 0
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("exit 0")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut service = AiService::new();
        service.timeout_seconds = 5;

        // パイプバッファを超える大きさ。子が読まずに終了するため write_all は BrokenPipe になる。
        let large_prompt = "x".repeat(1_000_000);

        let result = service.run_process_with_timeout(
            &mut child,
            &AiProvider::Codex,
            true,
            &large_prompt,
            false,
        );

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Failed to write prompt"),
            "stdin 書き込み失敗かつ exit 0 のときは provider エラーになるべき"
        );
    }

    #[test]
    fn test_process_provider_output_success_with_message() {
        let status = exit_status(true);
        let result =
            AiService::process_provider_output(&AiProvider::Antigravity, status, "feat: add X", "");
        assert_eq!(result.unwrap(), "feat: add X");
    }

    #[test]
    fn test_process_provider_output_success_empty_stdout() {
        let status = exit_status(true);
        let result = AiService::process_provider_output(&AiProvider::Antigravity, status, "", "");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("empty response"));
    }

    #[test]
    fn test_process_provider_output_success_empty_stdout_with_stderr() {
        let status = exit_status(true);
        let result =
            AiService::process_provider_output(&AiProvider::Antigravity, status, "", "some hint");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("stderr"));
    }

    #[test]
    fn test_process_provider_output_failure() {
        let status = exit_status(false);
        let result = AiService::process_provider_output(
            &AiProvider::Antigravity,
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
            &AiProvider::Antigravity,
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
            &AiProvider::Antigravity,
            status,
            "feat: ok",
            "File not found: /path/to/bin",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_process_provider_output_claude_error_in_stdout() {
        // Claude Code はエラーメッセージを stdout に出力するため、
        // exit code 非0 + stderr 空 + stdout にエラーがある場合は stdout からエラーを取得
        let status = exit_status(false);
        let result = AiService::process_provider_output(
            &AiProvider::Claude,
            status,
            "There's an issue with the selected model (haiku). It may not exist or you may not have access to it.",
            "",
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("issue with the selected model"),
            "Claude の stdout エラーが取得されるべき: {err}"
        );
    }

    #[test]
    fn test_process_provider_output_claude_error_prefers_stderr() {
        // stderr にもエラーがある場合は stderr を優先
        let status = exit_status(false);
        let result = AiService::process_provider_output(
            &AiProvider::Claude,
            status,
            "stdout error message",
            "stderr error message",
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("stderr error message"),
            "stderr が空でない場合は stderr を優先: {err}"
        );
    }

    #[test]
    fn test_process_provider_output_claude_error_empty_both() {
        // stdout も stderr も空の場合はフォールバック
        let status = exit_status(false);
        let result = AiService::process_provider_output(&AiProvider::Claude, status, "", "");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("API request failed"),
            "両方空の場合はフォールバック: {err}"
        );
    }

    #[test]
    fn test_process_provider_output_cleans_message() {
        // 出力メッセージがクリーンアップされることを確認
        let status = exit_status(true);
        let result = AiService::process_provider_output(
            &AiProvider::Antigravity,
            status,
            "```\nfeat: clean this\n```",
            "",
        );
        assert_eq!(result.unwrap(), "feat: clean this");
    }

    // ============================================================
    // process_provider_output 追加テスト
    // ============================================================

    #[test]
    fn test_process_provider_output_exit0_empty_stdout_empty_stderr() {
        // exit code 0 + 空stdout + 空stderr → 空レスポンスエラー
        let status = exit_status(true);
        let result = AiService::process_provider_output(&AiProvider::Claude, status, "", "");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("empty response"),
            "空stdout+空stderrでは 'empty response' エラーになるべき: {}",
            err
        );
        // stderrヒントが含まれないことも確認
        assert!(
            !err.contains("stderr"),
            "stderrが空の場合はstderrヒントが含まれないべき: {}",
            err
        );
    }

    #[test]
    fn test_process_provider_output_exit0_empty_stdout_with_stderr_hint() {
        // exit code 0 + 空stdout + stderr あり → stderrヒント付きエラー
        let status = exit_status(true);
        let result = AiService::process_provider_output(
            &AiProvider::Codex,
            status,
            "",
            "warning: model is overloaded",
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("empty response"),
            "空stdoutでは 'empty response' エラーになるべき: {}",
            err
        );
        assert!(
            err.contains("stderr"),
            "stderrがある場合はヒントが含まれるべき: {}",
            err
        );
        assert!(
            err.contains("model is overloaded"),
            "stderrの内容がヒントに含まれるべき: {}",
            err
        );
    }

    #[test]
    fn test_process_provider_output_codex_stderr_error_keyword_skipped() {
        // Codex: exit code 0 + stdout あり + stderr に "error:" → stderrは無視されて正常
        let status = exit_status(true);
        let result = AiService::process_provider_output(
            &AiProvider::Codex,
            status,
            "feat: implement auth",
            "error: some debug info from codex",
        );
        assert!(
            result.is_ok(),
            "Codex では stderr の 'error:' は無視されるべき"
        );
        assert_eq!(result.unwrap(), "feat: implement auth");
    }

    #[test]
    fn test_process_provider_output_claude_stderr_error_keyword_skipped() {
        // Claude: exit code 0 + stdout あり + stderr に "error:" → stderrは無視されて正常
        let status = exit_status(true);
        let result = AiService::process_provider_output(
            &AiProvider::Claude,
            status,
            "fix: correct null check",
            "error: prompt echo from claude",
        );
        assert!(
            result.is_ok(),
            "Claude では stderr の 'error:' は無視されるべき"
        );
        assert_eq!(result.unwrap(), "fix: correct null check");
    }

    #[test]
    fn test_process_provider_output_opencode_stderr_file_not_found_error() {
        // 非Codex/Claude (opencode): exit code 0 + stdout あり + stderr に "file not found" → エラー
        let status = exit_status(true);
        let result = AiService::process_provider_output(
            &AiProvider::Opencode,
            status,
            "feat: ok",
            "file not found: /usr/local/bin/opencode",
        );
        assert!(
            result.is_err(),
            "非Codex/Claude では stderr の 'file not found' はエラーになるべき"
        );
    }

    #[test]
    fn test_process_provider_output_apple_intelligence_stderr_error_keyword() {
        // 非Codex/Claude (AppleIntelligence): exit code 0 + stdout あり + stderr に "error:" → エラー
        let status = exit_status(true);
        let result = AiService::process_provider_output(
            &AiProvider::AppleIntelligence,
            status,
            "feat: add feature",
            "error: model initialization failed",
        );
        assert!(
            result.is_err(),
            "非Codex/Claude では stderr の 'error:' はエラーになるべき"
        );
    }

    // ============================================================
    // clean_message エッジケース追加テスト
    // ============================================================

    #[test]
    fn test_clean_message_nested_code_block() {
        // ネストされたコードブロック: 外側の ``` のみが除去される
        let message = "```\n```inner```\n```";
        let result = AiService::clean_message(message);
        assert_eq!(result, "```inner```");
    }

    #[test]
    fn test_clean_message_multiple_consecutive_code_blocks() {
        // 複数の連続するコードブロック: 外側だけが ``` で囲まれていないため、そのまま
        let message = "```\nfirst block\n```\n```\nsecond block\n```";
        // 先頭が ``` で末尾も ``` だが、中間にも ``` があるので外側として処理される
        let result = AiService::clean_message(message);
        // starts_with("```") && ends_with("```") なので外側が除去され、
        // 中間の内容が残る。ensure_body_separatorで件名と本文の間に空行挿入。
        assert_eq!(result, "first block\n\n```\n```\nsecond block");
    }

    #[test]
    fn test_clean_message_inline_backtick_code() {
        // バッククォートのインラインコード: コードブロックではないのでそのまま維持
        let message = "`feat: add`";
        let result = AiService::clean_message(message);
        // starts_with("```") ではないのでコードブロック除去は行われない
        // 引用符の trim_matches も ` はマッチしない
        assert_eq!(result, "`feat: add`");
    }

    // ============================================================
    // ensure_body_separator: 追加エッジケース
    // ============================================================

    #[test]
    fn test_ensure_body_separator_tab_only_second_line() {
        // 2行目がタブのみの場合は空行として扱われる
        let message = "feat: add feature\n\t\nBody text";
        let result = AiService::ensure_body_separator(message);
        // タブのみの行は trim() で空になるため、空行として扱われる
        assert_eq!(result, "feat: add feature\n\t\nBody text");
    }

    #[test]
    fn test_ensure_body_separator_multiple_body_lines() {
        // 複数行の本文で2行目が非空の場合、空行が挿入される
        let message = "feat: add feature\nline 2\nline 3\nline 4";
        let result = AiService::ensure_body_separator(message);
        assert_eq!(result, "feat: add feature\n\nline 2\nline 3\nline 4");
    }

    // ============================================================
    // clean_message: 追加エッジケース
    // ============================================================

    #[test]
    fn test_clean_message_single_quotes_wrapping() {
        // シングルクォートで囲まれたメッセージ
        let message = "'feat: add new feature'";
        let result = AiService::clean_message(message);
        assert_eq!(result, "feat: add new feature");
    }

    #[test]
    fn test_clean_message_double_quotes_wrapping() {
        // ダブルクォートで囲まれたメッセージ
        let message = "\"fix: resolve crash\"";
        let result = AiService::clean_message(message);
        assert_eq!(result, "fix: resolve crash");
    }

    #[test]
    fn test_clean_message_code_block_with_trailing_whitespace() {
        // コードブロック前後に空白がある場合
        let message = "  ```\nfeat: add feature\n```  ";
        let result = AiService::clean_message(message);
        assert_eq!(result, "feat: add feature");
    }

    #[test]
    fn test_clean_message_preserves_body_separator() {
        // 件名と本文の間の空行が保持される
        let message = "feat: add feature\n\nDetailed description here";
        let result = AiService::clean_message(message);
        assert_eq!(result, "feat: add feature\n\nDetailed description here");
    }

    // ============================================================
    // build_prompt: squash + body の組み合わせテスト
    // ============================================================

    #[test]
    fn test_build_prompt_squash_with_body() {
        // squash用のプロンプトはprefix_type="conventional"、commitsなし
        let diff = "diff content";
        let prompt =
            AiService::build_prompt(diff, &[], "Japanese", Some("conventional"), true, None);
        assert!(prompt.contains("diff content"));
        assert!(prompt.contains("Japanese"));
        // Conventional Commits ガイドが含まれる
        assert!(prompt.contains("feat:"));
        assert!(prompt.contains("fix:"));
    }

    #[test]
    fn test_build_prompt_with_agent_context_and_body() {
        // agent_contextとbodyの両方が有効な場合
        let diff = "diff content";
        let agent_context = "Implementing user authentication feature";
        let prompt = AiService::build_prompt(
            diff,
            &["feat: previous commit".to_string()],
            "English",
            None,
            true,
            Some(agent_context),
        );
        assert!(prompt.contains(agent_context));
        assert!(prompt.contains("diff content"));
    }

    // ============================================================
    // format_command_for_debug: 特殊文字を含むプロンプト
    // ============================================================

    #[test]
    fn test_format_command_for_debug_prompt_with_newlines() {
        // 改行を含むプロンプトがエスケープされる
        let service = AiService::new();
        let prompt = "line 1\nline 2\nline 3";
        let result = service.format_command_for_debug(
            &AiProvider::Antigravity,
            &service.models.antigravity.clone(),
            &ProviderStep::from_provider("antigravity"),
            prompt,
            None,
        );
        assert!(result.contains("line 1\nline 2"));
    }

    #[test]
    fn test_format_command_for_debug_apple_intelligence_special_chars() {
        // Apple Intelligenceプロバイダーでの特殊文字処理
        let service = AiService::new();
        let prompt = "feat: add 'quotes' and \"doubles\"";
        let result = service.format_command_for_debug(
            &AiProvider::AppleIntelligence,
            "",
            &ProviderStep::from_provider("apple-intelligence"),
            prompt,
            None,
        );
        assert!(result.starts_with("echo '"));
        assert!(result.contains("apple-ai"));
    }

    // ============================================================
    // clean_message: コードブロック内に引用符がある場合
    // ============================================================

    #[test]
    fn test_clean_message_code_block_with_inner_quotes() {
        // コードブロック除去後にさらに引用符が残る場合
        let msg = "```\n\"feat: add feature\"\n```";
        assert_eq!(AiService::clean_message(msg), "feat: add feature");
    }

    #[test]
    fn test_clean_message_backtick_only_opening() {
        // 開始バッククォートのみで閉じがない場合はそのまま
        let msg = "```\nfeat: add feature";
        // starts_with("```") は true だが ends_with("```") は false
        let result = AiService::clean_message(msg);
        assert!(result.contains("feat: add feature"));
    }

    // ============================================================
    // clean_message: 連続コードブロック・複合エッジケース
    // ============================================================

    #[test]
    fn test_clean_message_consecutive_code_blocks() {
        // 複数のコードブロックがある場合、最外層のみ除去される
        let msg = "```\nfeat: inner\n```\nextra text\n```\nfix: another\n```";
        let result = AiService::clean_message(msg);
        // 最初の ``` と最後の ``` が除去され、中間が残る
        assert!(result.contains("feat: inner"));
    }

    #[test]
    fn test_clean_message_code_block_two_lines_only() {
        // コードブロックが2行のみ（開始 + 終了）の場合、コンテンツとして扱われる
        // ensure_body_separator が2行目を非空と判定し空行を挿入する
        let msg = "```\n```";
        let result = AiService::clean_message(msg);
        assert_eq!(result, "```\n\n```");
    }

    #[test]
    fn test_clean_message_nested_quotes_in_code_block() {
        // コードブロック内にクォートがネストされている場合
        let msg = "```\n\"feat: 'quoted' message\"\n```";
        let result = AiService::clean_message(msg);
        assert_eq!(result, "feat: 'quoted' message");
    }

    // ============================================================
    // ensure_body_separator: エッジケース
    // ============================================================

    #[test]
    fn test_ensure_body_separator_already_has_blank_line() {
        // 空行が既にある場合はそのまま
        let msg = "feat: title\n\n- body";
        let result = AiService::ensure_body_separator(msg);
        assert_eq!(result, msg);
    }

    #[test]
    fn test_ensure_body_separator_no_blank_line() {
        // 空行がない場合は挿入される
        let msg = "feat: title\n- body";
        let result = AiService::ensure_body_separator(msg);
        assert_eq!(result, "feat: title\n\n- body");
    }

    #[test]
    fn test_ensure_body_separator_one_line_unchanged() {
        // 1行の場合はそのまま
        let msg = "feat: title";
        let result = AiService::ensure_body_separator(msg);
        assert_eq!(result, msg);
    }

    #[test]
    fn test_ensure_body_separator_spaces_only_second_line_treated_as_blank() {
        // 2行目が空白のみの場合は空行とみなされそのまま
        let msg = "feat: title\n   \n- body";
        let result = AiService::ensure_body_separator(msg);
        assert_eq!(result, msg);
    }

    // ============================================================
    // process_provider_output: 空白stderrとGemini以外のstderrチェック
    // ============================================================

    #[test]
    fn test_process_provider_output_gemini_stderr_whitespace_only() {
        // Geminiでstderrが空白のみの場合、エラー扱いにならない
        use std::os::unix::process::ExitStatusExt;
        let status = ExitStatus::from_raw(0);
        let result = AiService::process_provider_output(
            &AiProvider::Antigravity,
            status,
            "feat: add feature\n",
            "   \n  ",
        );
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "feat: add feature");
    }

    #[test]
    fn test_process_provider_output_gemini_stderr_with_error_keyword() {
        // Gemini（Codex/Claude以外）でstderrに "error:" が含まれる場合はエラー
        use std::os::unix::process::ExitStatusExt;
        let status = ExitStatus::from_raw(0);
        let result = AiService::process_provider_output(
            &AiProvider::Antigravity,
            status,
            "feat: add feature\n",
            "error: something went wrong",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_process_provider_output_codex_stderr_with_error_ignored() {
        // Codexではstderrのerror検出をスキップ（誤検出防止）
        use std::os::unix::process::ExitStatusExt;
        let status = ExitStatus::from_raw(0);
        let result = AiService::process_provider_output(
            &AiProvider::Codex,
            status,
            "feat: add feature\n",
            "error: this is just a log line",
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_process_provider_output_stdout_becomes_empty_after_clean() {
        // clean_message後に空になるケース
        use std::os::unix::process::ExitStatusExt;
        let status = ExitStatus::from_raw(0);
        let result =
            AiService::process_provider_output(&AiProvider::Antigravity, status, "  \"\"  \n", "");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty response"));
    }

    // ============================================================
    // extract_error: プロバイダー別の複合エラーパターン
    // ============================================================

    #[test]
    fn test_extract_error_gemini_first_of_multiple_api_errors() {
        // 複数のAPIエラーがある場合、最初のものが返される
        let stderr = "[API Error: rate limit]\n[API Error: quota exceeded]";
        let result = AiService::extract_error(stderr, &AiProvider::Antigravity);
        assert_eq!(result, "[API Error: rate limit]");
    }

    #[test]
    fn test_extract_error_codex_uppercase_error_over_lowercase() {
        // "ERROR:" が "error" より優先される
        let stderr =
            "Reading prompt from stdin...\nerror: minor issue\nERROR: Your access token expired";
        let result = AiService::extract_error(stderr, &AiProvider::Codex);
        assert_eq!(result, "ERROR: Your access token expired");
    }

    #[test]
    fn test_extract_error_opencode_failed_keyword() {
        // opencode: "failed" キーワードの検出
        let stderr = "Starting...\nConnection failed to server\nDone";
        let result = AiService::extract_error(stderr, &AiProvider::Opencode);
        assert_eq!(result, "Connection failed to server");
    }

    #[test]
    fn test_extract_error_apple_intelligence_error_prefix() {
        // Apple Intelligence: "Error:" プレフィックスの検出
        let stderr = "Initializing model...\nError: Model not available\nCleanup done";
        let result = AiService::extract_error(stderr, &AiProvider::AppleIntelligence);
        assert_eq!(result, "Error: Model not available");
    }

    #[test]
    fn test_extract_error_apple_intelligence_no_error_prefix() {
        // Apple Intelligence: "Error:" がない場合は最初の非空行
        let stderr = "\n  \nSome generic message\nAnother line";
        let result = AiService::extract_error(stderr, &AiProvider::AppleIntelligence);
        assert_eq!(result, "Some generic message");
    }

    // ============================================================
    // build_prompt: 特殊文字・大量コミットのテスト
    // ============================================================

    #[test]
    fn test_build_prompt_with_special_chars_in_diff() {
        // diff内に特殊文字が含まれる場合でもプロンプトが壊れない
        let diff = "diff --git a/file.rs b/file.rs\n+let s = \"hello\\nworld\";";
        let prompt =
            AiService::build_prompt(diff, &[], "Japanese", Some("conventional"), false, None);
        assert!(prompt.contains(diff));
        assert!(prompt.contains("<changes>"));
        assert!(prompt.contains("</changes>"));
    }

    #[test]
    fn test_build_prompt_with_many_recent_commits() {
        // 大量のコミットが渡された場合の番号付け
        let commits: Vec<String> = (1..=20).map(|i| format!("commit {}", i)).collect();
        let prompt = AiService::build_prompt("diff", &commits, "Japanese", None, false, None);
        assert!(prompt.contains("1. commit 1"));
        assert!(prompt.contains("20. commit 20"));
    }

    #[test]
    fn test_build_prompt_agent_context_empty_string() {
        // agent_contextが空文字列の場合、agent-contextセクションは含まれない
        let prompt = AiService::build_prompt("diff", &[], "Japanese", None, false, Some(""));
        assert!(!prompt.contains("<agent-context>"));
    }

    #[test]
    fn test_build_prompt_custom_prefix_type() {
        // カスタムprefix_typeが正しく反映される
        let prompt = AiService::build_prompt(
            "diff",
            &[],
            "Japanese",
            Some("my-custom-format"),
            false,
            None,
        );
        assert!(prompt.contains("Use the following prefix format: my-custom-format"));
    }

    // ============================================================
    // clean_message のテスト
    // ============================================================

    #[test]
    fn test_clean_message_plain_text() {
        // 通常のメッセージはそのまま返る
        assert_eq!(
            AiService::clean_message("feat: add feature"),
            "feat: add feature"
        );
    }

    #[test]
    fn test_clean_message_markdown_code_block() {
        // マークダウンのコードブロックが除去される
        let msg = "```\nfeat: add feature\n```";
        assert_eq!(AiService::clean_message(msg), "feat: add feature");
    }

    #[test]
    fn test_clean_message_markdown_code_block_with_lang() {
        // 言語指定付きコードブロック
        let msg = "```text\nfix: resolve bug\n```";
        assert_eq!(AiService::clean_message(msg), "fix: resolve bug");
    }

    #[test]
    fn test_clean_message_surrounding_quotes() {
        // 前後の引用符が除去される
        assert_eq!(AiService::clean_message("\"feat: add\""), "feat: add");
        assert_eq!(AiService::clean_message("'fix: bug'"), "fix: bug");
    }

    #[test]
    fn test_clean_message_whitespace_trim() {
        // 前後の空白がトリムされる
        assert_eq!(AiService::clean_message("  feat: add  "), "feat: add");
    }

    #[test]
    fn test_clean_message_empty_and_whitespace() {
        assert_eq!(AiService::clean_message(""), "");
        assert_eq!(AiService::clean_message("   "), "");
    }

    #[test]
    fn test_clean_message_code_block_only_backticks() {
        // バッククォートだけの場合（2行）→ コードブロック抽出不可
        // ensure_body_separator により2行目の前に空行が挿入される
        let msg = "```\n```";
        let result = AiService::clean_message(msg);
        assert_eq!(result, "```\n\n```");
    }

    #[test]
    fn test_clean_message_multiline_with_body() {
        // 複数行メッセージ（件名 + 本文）
        let msg = "feat: add feature\ndetail line";
        let result = AiService::clean_message(msg);
        // 2行目が空行でないので空行が挿入される
        assert_eq!(result, "feat: add feature\n\ndetail line");
    }

    #[test]
    fn test_clean_message_multiline_with_separator() {
        // 既に空行セパレータがある場合はそのまま
        let msg = "feat: add feature\n\n- detail 1\n- detail 2";
        assert_eq!(AiService::clean_message(msg), msg);
    }

    // ============================================================
    // ensure_body_separator のテスト
    // ============================================================

    #[test]
    fn test_ensure_body_separator_single_line_short() {
        assert_eq!(AiService::ensure_body_separator("feat: add"), "feat: add");
    }

    #[test]
    fn test_ensure_body_separator_already_separated() {
        let msg = "title\n\nbody";
        assert_eq!(AiService::ensure_body_separator(msg), msg);
    }

    #[test]
    fn test_ensure_body_separator_no_separator() {
        let msg = "title\nbody";
        assert_eq!(AiService::ensure_body_separator(msg), "title\n\nbody");
    }

    #[test]
    fn test_ensure_body_separator_multiple_body_lines_simple() {
        let msg = "title\nline1\nline2\nline3";
        assert_eq!(
            AiService::ensure_body_separator(msg),
            "title\n\nline1\nline2\nline3"
        );
    }

    // ============================================================
    // extract_error のテスト
    // ============================================================

    #[test]
    fn test_extract_error_gemini_api_error_lowercase() {
        let stderr = "Some info\n[API Error: rate limit exceeded]\nMore info";
        assert_eq!(
            AiService::extract_error(stderr, &AiProvider::Antigravity),
            "[API Error: rate limit exceeded]"
        );
    }

    #[test]
    fn test_extract_error_gemini_critical_error_broke() {
        let stderr = "An unexpected critical error occurred:Error: something broke";
        let result = AiService::extract_error(stderr, &AiProvider::Antigravity);
        assert!(result.contains("critical error"));
    }

    #[test]
    fn test_extract_error_antigravity_no_match_returns_first_line() {
        // 既知パターンに合致しない場合は最初の非空行を返す
        let stderr = "some random output";
        assert_eq!(
            AiService::extract_error(stderr, &AiProvider::Antigravity),
            "some random output"
        );
    }

    #[test]
    fn test_extract_error_codex_error_line() {
        let stderr = "Reading prompt...\nERROR: Your access token expired\nReconnecting...";
        assert_eq!(
            AiService::extract_error(stderr, &AiProvider::Codex),
            "ERROR: Your access token expired"
        );
    }

    #[test]
    fn test_extract_error_codex_lowercase_error() {
        let stderr = "Reading prompt...\nSomething error happened\n";
        assert_eq!(
            AiService::extract_error(stderr, &AiProvider::Codex),
            "Something error happened"
        );
    }

    #[test]
    fn test_extract_error_codex_fallback_last_line() {
        let stderr = "Reading prompt...\nReconnecting...\nUnknown issue";
        assert_eq!(
            AiService::extract_error(stderr, &AiProvider::Codex),
            "Unknown issue"
        );
    }

    #[test]
    fn test_extract_error_claude_first_line() {
        let stderr = "Connection refused\nRetrying...";
        assert_eq!(
            AiService::extract_error(stderr, &AiProvider::Claude),
            "Connection refused"
        );
    }

    #[test]
    fn test_extract_error_claude_empty() {
        assert_eq!(
            AiService::extract_error("", &AiProvider::Claude),
            "API request failed"
        );
    }

    #[test]
    fn test_extract_error_opencode_error_keyword() {
        let stderr = "info: starting\nerror: model not found\n";
        assert_eq!(
            AiService::extract_error(stderr, &AiProvider::Opencode),
            "error: model not found"
        );
    }

    #[test]
    fn test_extract_error_opencode_failed_keyword_timeout() {
        let stderr = "Request failed due to timeout\n";
        assert_eq!(
            AiService::extract_error(stderr, &AiProvider::Opencode),
            "Request failed due to timeout"
        );
    }

    #[test]
    fn test_extract_error_apple_intelligence() {
        let stderr = "Info: loading model\nError: model unavailable\n";
        assert_eq!(
            AiService::extract_error(stderr, &AiProvider::AppleIntelligence),
            "Error: model unavailable"
        );
    }

    // ============================================================
    // process_provider_output のテスト
    // ============================================================

    #[test]
    fn test_process_provider_output_success() {
        use std::process::Command;
        // 正常終了のExitStatusを取得
        let status = Command::new("true").status().unwrap();
        let result =
            AiService::process_provider_output(&AiProvider::Antigravity, status, "feat: add\n", "");
        assert_eq!(result.unwrap(), "feat: add");
    }

    #[test]
    fn test_process_provider_output_empty_stdout() {
        use std::process::Command;
        let status = Command::new("true").status().unwrap();
        let result = AiService::process_provider_output(&AiProvider::Antigravity, status, "", "");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty response"));
    }

    #[test]
    fn test_process_provider_output_failed_exit() {
        use std::process::Command;
        let status = Command::new("false").status().unwrap();
        let result = AiService::process_provider_output(
            &AiProvider::Antigravity,
            status,
            "",
            "some error output",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_process_provider_output_stderr_error_for_gemini_via_command() {
        use std::process::Command;
        let status = Command::new("true").status().unwrap();
        // Gemini はstderrに "error:" があるとエラー扱い
        let result = AiService::process_provider_output(
            &AiProvider::Antigravity,
            status,
            "feat: add",
            "error: something",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_process_provider_output_stderr_ignored_for_codex() {
        use std::process::Command;
        let status = Command::new("true").status().unwrap();
        // Codex はstderrに "error:" があっても無視
        let result = AiService::process_provider_output(
            &AiProvider::Codex,
            status,
            "feat: add\n",
            "error: some debug info",
        );
        assert_eq!(result.unwrap(), "feat: add");
    }

    #[test]
    fn test_process_provider_output_stderr_ignored_for_claude() {
        use std::process::Command;
        let status = Command::new("true").status().unwrap();
        // Claude もstderrのエラーチェックをスキップ
        let result = AiService::process_provider_output(
            &AiProvider::Claude,
            status,
            "fix: resolve bug\n",
            "error: debug output",
        );
        assert_eq!(result.unwrap(), "fix: resolve bug");
    }

    // ============================================================
    // format_command_for_debug のテスト
    // ============================================================

    #[test]
    fn test_format_command_antigravity_basic() {
        // antigravity モデルが空なら、デバッグ表示には `-p PROMPT` だけが現れる
        // (`--model`/`--debug` は付かない)。
        let service = AiService {
            steps: vec![ProviderStep::from_provider("antigravity")],
            language: "Japanese".to_string(),
            models: ModelsConfig {
                // モデル未指定(空)なら agy 既定に委ねるため `--model` を付けない
                antigravity: String::new(),
                ..Default::default()
            },
            codex_reasoning_effort: "low".to_string(),
            cooldown_minutes: 60,
            timeout_seconds: 60,
            debug: false,
            provider_override: false,
            legacy_gemini_alias_detected: false,
            ai_usage_notes: Vec::new(),
            ai_usage_gate_blocked: false,
        };
        let cmd = service.format_command_for_debug(
            &AiProvider::Antigravity,
            "",
            &ProviderStep::from_provider("antigravity"),
            "test prompt",
            None,
        );
        assert!(
            cmd.starts_with("agy "),
            "expected `agy` invocation, got: {}",
            cmd
        );
        assert!(cmd.contains("-p 'test prompt'"));
        // モデル未指定なので --model は付かない
        assert!(!cmd.contains("--model"));
        // agy には --debug フラグがない
        assert!(!cmd.contains("--debug"));
    }

    #[test]
    fn test_format_command_antigravity_with_model() {
        // antigravity モデルを指定すると、デバッグ表示に `--model` が現れる。
        let service = AiService {
            steps: vec![ProviderStep::from_provider("antigravity")],
            language: "Japanese".to_string(),
            models: ModelsConfig {
                antigravity: "GPT-OSS 120B (Medium)".to_string(),
                ..Default::default()
            },
            codex_reasoning_effort: "low".to_string(),
            cooldown_minutes: 60,
            timeout_seconds: 60,
            debug: true,
            provider_override: false,
            legacy_gemini_alias_detected: false,
            ai_usage_notes: Vec::new(),
            ai_usage_gate_blocked: false,
        };
        let cmd = service.format_command_for_debug(
            &AiProvider::Antigravity,
            "GPT-OSS 120B (Medium)",
            &ProviderStep::from_provider("antigravity"),
            "prompt",
            None,
        );
        assert!(cmd.contains("--model 'GPT-OSS 120B (Medium)'"));
        assert!(cmd.contains("-p 'prompt'"));
        // agy には --debug フラグがない
        assert!(!cmd.contains("--debug"));
    }

    #[test]
    fn test_format_command_codex_always_disables_hooks() {
        // stop_hook_active = false でも常に --disable hooks が付く
        let service = AiService {
            steps: vec![ProviderStep::from_provider("codex")],
            language: "Japanese".to_string(),
            models: ModelsConfig::default(),
            codex_reasoning_effort: "low".to_string(),
            cooldown_minutes: 60,
            timeout_seconds: 60,
            debug: false,
            provider_override: false,
            legacy_gemini_alias_detected: false,
            ai_usage_notes: Vec::new(),
            ai_usage_gate_blocked: false,
        };
        let cmd = service.format_command_for_debug(
            &AiProvider::Codex,
            &service.models.codex.clone(),
            &ProviderStep::from_provider("codex"),
            "prompt",
            None,
        );
        assert!(cmd.contains("--disable hooks"));
    }

    #[test]
    fn test_format_command_claude() {
        let service = AiService {
            steps: vec![ProviderStep::from_provider("claude")],
            language: "Japanese".to_string(),
            models: ModelsConfig {
                claude: "haiku".to_string(),
                ..Default::default()
            },
            codex_reasoning_effort: "low".to_string(),
            cooldown_minutes: 60,
            timeout_seconds: 60,
            debug: false,
            provider_override: false,
            legacy_gemini_alias_detected: false,
            ai_usage_notes: Vec::new(),
            ai_usage_gate_blocked: false,
        };
        let cmd = service.format_command_for_debug(
            &AiProvider::Claude,
            "haiku",
            &ProviderStep::from_provider("claude"),
            "prompt",
            None,
        );
        assert!(cmd.contains("claude"));
        assert!(cmd.contains("--model 'haiku'"));
        assert!(cmd.contains("-p"));
    }

    #[test]
    fn test_format_command_prompt_with_single_quotes() {
        // シングルクォートを含むプロンプトのエスケープ
        let service = AiService::new();
        let cmd = service.format_command_for_debug(
            &AiProvider::Antigravity,
            &service.models.antigravity.clone(),
            &ProviderStep::from_provider("antigravity"),
            "it's a test",
            None,
        );
        assert!(cmd.contains("it'\\''s a test"));
    }

    // ============================================================
    // AiService::from_config のテスト
    // ============================================================

    #[test]
    fn test_from_config_default() {
        let config = Config::default();
        let service = AiService::from_config(&config);
        assert_eq!(service.language, "Japanese");
        assert!(!service.steps.is_empty());
    }

    #[test]
    fn test_from_config_custom_language() {
        let config = Config {
            language: "English".to_string(),
            ..Default::default()
        };
        let service = AiService::from_config(&config);
        assert_eq!(service.language, "English");
    }

    #[test]
    fn test_from_config_empty_providers_fallback() {
        // 空のプロバイダーリストではデフォルトにフォールバック
        let config = Config {
            providers: vec![],
            ..Default::default()
        };
        let service = AiService::from_config(&config);
        assert!(!service.steps.is_empty());
    }

    #[test]
    fn test_from_config_invalid_providers_fallback() {
        // 無効なプロバイダー名のみの場合もデフォルトにフォールバック
        let config = Config {
            providers: vec![
                ProviderStep::from_provider("invalid1"),
                ProviderStep::from_provider("invalid2"),
            ],
            ..Default::default()
        };
        let service = AiService::from_config(&config);
        assert!(!service.steps.is_empty());
    }

    #[test]
    fn test_from_config_custom_timeout() {
        let config = Config {
            provider_timeout_seconds: 120,
            ..Default::default()
        };
        let service = AiService::from_config(&config);
        assert_eq!(service.timeout_seconds, 120);
    }

    #[test]
    fn test_from_config_custom_cooldown() {
        let config = Config {
            provider_cooldown_minutes: 30,
            ..Default::default()
        };
        let service = AiService::from_config(&config);
        assert_eq!(service.cooldown_minutes, 30);
    }

    // ============================================================
    // TempFile: 基本動作テスト
    // ============================================================

    #[test]
    fn test_temp_file_content_written() {
        // 書き込んだ内容が正しく保存される
        let content = b"test prompt content";
        let tmp = TempFile::create_with_content(content).unwrap();
        let read_back = std::fs::read(tmp.path()).unwrap();
        assert_eq!(read_back, content);
    }

    #[test]
    fn test_temp_file_unique_paths() {
        // 複数のファイルが異なるパスを持つ
        let tmp1 = TempFile::create_with_content(b"a").unwrap();
        let tmp2 = TempFile::create_with_content(b"b").unwrap();
        assert_ne!(tmp1.path(), tmp2.path());
    }

    #[test]
    fn test_temp_file_drop_cleanup() {
        // Drop後にファイルが削除される
        let path = {
            let tmp = TempFile::create_with_content(b"temp").unwrap();
            let p = tmp.path().to_path_buf();
            assert!(p.exists());
            p
        };
        assert!(!path.exists());
    }

    #[test]
    fn test_temp_file_multibyte_content() {
        // マルチバイト文字を含むコンテンツが正しく保存される
        let content = "日本語プロンプト 🚀".as_bytes();
        let tmp = TempFile::create_with_content(content).unwrap();
        let read_back = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(read_back, "日本語プロンプト 🚀");
    }

    #[test]
    fn test_temp_file_empty_content() {
        // 空コンテンツでもファイルは正常に作成される
        let tmp = TempFile::create_with_content(b"").unwrap();
        let read_back = std::fs::read(tmp.path()).unwrap();
        assert!(read_back.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn test_temp_file_is_not_readable_by_group_or_others() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempFile::create_with_content(b"secret diff").unwrap();
        let mode = std::fs::metadata(tmp.path()).unwrap().permissions().mode();

        assert_eq!(mode & 0o077, 0);
    }

    // ============================================================
    // extract_error: 追加エッジケーステスト
    // ============================================================

    #[test]
    fn test_extract_error_codex_reading_prompt_skipped() {
        // "Reading prompt" で始まる行はスキップされる
        let stderr = "Reading prompt from stdin\n";
        let result = AiService::extract_error(stderr, &AiProvider::Codex);
        assert_eq!(result, "Codex API request failed");
    }

    #[test]
    fn test_extract_error_claude_whitespace_only() {
        let result = AiService::extract_error("   \n   \n   ", &AiProvider::Claude);
        assert_eq!(result, "API request failed");
    }

    #[test]
    fn test_extract_error_opencode_empty_stderr() {
        let result = AiService::extract_error("", &AiProvider::Opencode);
        assert_eq!(result, "opencode request failed");
    }

    #[test]
    fn test_extract_error_apple_intelligence_non_error_line() {
        let stderr = "just some info output";
        let result = AiService::extract_error(stderr, &AiProvider::AppleIntelligence);
        assert_eq!(result, "just some info output");
    }

    // ============================================================
    // clean_message: クォート組み合わせの挙動を文書化するテスト
    // ============================================================

    #[test]
    fn test_clean_message_outer_single_inner_double_quotes() {
        // 外側 single 内側 double のクォートは外側だけが除去される。
        // trim_matches('"') で `'` の両端は変化せず、続く trim_matches('\'') で
        // 外側 `'` が除去された結果、内側の `"text"` がそのまま残ることを保証する。
        let message = "'\"feat: add feature\"'";
        assert_eq!(AiService::clean_message(message), "\"feat: add feature\"");
    }

    #[test]
    fn test_clean_message_double_outer_quotes_removed_once() {
        // 連続する複数の同種クォートは trim_matches によって一括で除去される。
        let message = "\"\"feat: scope\"\"";
        assert_eq!(AiService::clean_message(message), "feat: scope");
    }

    // ============================================================
    // extract_error: 追加エッジケース
    // ============================================================

    #[test]
    fn test_extract_error_opencode_failed_in_unrelated_text() {
        // Opencode は "failed" を含む最初の行をエラーとみなすため、
        // 通常情報のテキストでも "failed" を含むと拾われる挙動を文書化する。
        let stderr = "info: previous run failed retry succeeded";
        let result = AiService::extract_error(stderr, &AiProvider::Opencode);
        assert_eq!(result, "info: previous run failed retry succeeded");
    }

    #[test]
    fn test_extract_error_codex_uppercase_priority_over_lowercase() {
        // ERROR: で始まる行が先に抽出され、後続の "error" を含む行は無視される。
        let stderr = "informational line\nERROR: top priority error\nlowercase error info";
        let result = AiService::extract_error(stderr, &AiProvider::Codex);
        assert_eq!(result, "ERROR: top priority error");
    }

    // ============================================================
    // env 明示上書き / command バイナリ差し替え
    // (token-burn の env 継承バグ対策の中核: step.env が Command に明示適用される)
    // ============================================================

    #[cfg(not(windows))]
    #[test]
    fn test_build_provider_command_applies_env_override() {
        // step.env は Command::env() で明示設定され、spawn 時に親環境を上書きする。
        let service = AiService::new();
        let mut step = ProviderStep::from_provider("codex");
        step.env
            .insert("CODEX_HOME".to_string(), "/custom/codex".to_string());
        let output_file = TempFile::create_with_content(b"").unwrap();
        let (cmd, _) = service
            .build_provider_command(
                &AiProvider::Codex,
                &step,
                "",
                "prompt",
                None,
                Some(&output_file),
            )
            .unwrap();
        let has_env = cmd.get_envs().any(|(k, v)| {
            k == std::ffi::OsStr::new("CODEX_HOME")
                && v == Some(std::ffi::OsStr::new("/custom/codex"))
        });
        assert!(
            has_env,
            "CODEX_HOME が Command に明示設定されているべき(親環境への暗黙依存を避ける)"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn test_build_provider_command_uses_custom_binary() {
        // command 指定時は provider 既定バイナリではなく指定バイナリを起動する。
        let service = AiService::new();
        let mut step = ProviderStep::from_provider("codex");
        step.command = Some(vec!["/my/wrapper.sh".to_string()]);
        let output_file = TempFile::create_with_content(b"").unwrap();
        let (cmd, _) = service
            .build_provider_command(
                &AiProvider::Codex,
                &step,
                "",
                "prompt",
                None,
                Some(&output_file),
            )
            .unwrap();
        assert_eq!(cmd.get_program().to_string_lossy(), "/my/wrapper.sh");
    }

    #[cfg(not(windows))]
    #[test]
    fn test_build_provider_command_default_binary_when_no_command() {
        // command 未指定なら provider 既定バイナリ(codex)。
        let service = AiService::new();
        let step = ProviderStep::from_provider("codex");
        let output_file = TempFile::create_with_content(b"").unwrap();
        let (cmd, _) = service
            .build_provider_command(
                &AiProvider::Codex,
                &step,
                "",
                "prompt",
                None,
                Some(&output_file),
            )
            .unwrap();
        assert_eq!(cmd.get_program().to_string_lossy(), "codex");
    }

    #[cfg(not(windows))]
    #[test]
    fn test_is_step_installed_respects_path_env_override() {
        // 実行時と同じ env override で探索しないと、PATH で指定したラッパーを見落とす。
        let service = AiService::new();
        let dir = tempfile::tempdir().unwrap();
        let bin_path = dir.path().join("git-sc-test-provider");
        std::fs::write(&bin_path, "#!/bin/sh\nexit 0\n").unwrap();

        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&bin_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&bin_path, perms).unwrap();

        let mut step = ProviderStep::from_provider("codex");
        step.command = Some(vec!["git-sc-test-provider".to_string()]);
        step.env.insert(
            "PATH".to_string(),
            dir.path().to_string_lossy().into_owned(),
        );

        assert!(service.is_step_installed(&step, &AiProvider::Codex));
    }

    #[cfg(not(windows))]
    #[test]
    fn test_is_binary_available_absolute_path() {
        use std::collections::BTreeMap;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let bin_path = dir.path().join("wrapper.sh");
        std::fs::write(&bin_path, "#!/bin/sh\nexit 0\n").unwrap();
        let mut perms = std::fs::metadata(&bin_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&bin_path, perms).unwrap();

        let env = BTreeMap::new();
        // 絶対パスの実行可能ファイルは PATH に依存せず検出できる
        let bin = bin_path.to_string_lossy();
        assert!(AiService::is_binary_available(&bin, &env));

        // 存在しない絶対パスは検出されない
        let missing = dir.path().join("does-not-exist");
        let missing = missing.to_string_lossy();
        assert!(!AiService::is_binary_available(&missing, &env));
    }

    #[cfg(not(windows))]
    #[test]
    fn test_is_binary_available_rejects_non_executable_file() {
        use std::collections::BTreeMap;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let bin_path = dir.path().join("not-exec");
        std::fs::write(&bin_path, "data").unwrap();
        // 実行ビットの無い(0o644)ファイルはバイナリとして扱わない
        let mut perms = std::fs::metadata(&bin_path).unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&bin_path, perms).unwrap();

        let env = BTreeMap::new();
        let bin = bin_path.to_string_lossy();
        assert!(!AiService::is_binary_available(&bin, &env));
    }

    #[cfg(not(windows))]
    #[test]
    fn test_is_binary_available_missing_from_path() {
        use std::collections::BTreeMap;

        // PATH を空ディレクトリのみへ制限すれば、存在しないバイナリ名は見つからない
        let dir = tempfile::tempdir().unwrap();
        let mut env = BTreeMap::new();
        env.insert(
            "PATH".to_string(),
            dir.path().to_string_lossy().into_owned(),
        );
        assert!(!AiService::is_binary_available(
            "git-sc-definitely-missing-binary",
            &env
        ));
    }

    #[test]
    fn test_windows_cmd_arg_has_metachar_flags_dangerous_tokens() {
        // cmd.exe メタ文字を含むトークンは危険と判定する(Windows の cmd /C 注入対策)
        for bad in [
            "x&calc", "a|b", "a>f", "a<f", "a^b", "%PATH%", "!x!", "a\"b", "a\rb", "a\nb",
        ] {
            assert!(
                AiService::windows_cmd_arg_has_metachar(bad),
                "メタ文字を含む {bad:?} は危険と判定すべき"
            );
        }
    }

    #[test]
    fn test_windows_cmd_arg_has_metachar_allows_legitimate_tokens() {
        // 通常のモデル名・パス(空白・括弧・ハイフン・コロン・バックスラッシュ)は安全と判定する
        for ok in [
            "gpt-5.4-mini",
            "GPT-OSS 120B (Medium)",
            "Gemini 3.5 Flash (Low)",
            "claude",
            r"C:\tools\codex.cmd",
            "provider:model-name",
            "",
        ] {
            assert!(
                !AiService::windows_cmd_arg_has_metachar(ok),
                "正規トークン {ok:?} は安全と判定すべき"
            );
        }
    }

    #[test]
    fn test_resolve_model_prefers_step_then_config() {
        // step.model(非空) > [models].<provider> の優先順位
        let service = AiService::new();
        let mut step = ProviderStep::from_provider("codex");
        step.model = Some("step-model".to_string());
        assert_eq!(
            service.resolve_model(&AiProvider::Codex, &step),
            "step-model"
        );

        // step.model 未指定なら [models].codex(=デフォルト)を使う
        let empty = ProviderStep::from_provider("codex");
        assert_eq!(
            service.resolve_model(&AiProvider::Codex, &empty),
            service.models.codex
        );
    }

    #[test]
    fn test_step_label_includes_model_and_account() {
        // ログラベルに model とアカウント(env 由来)が出る
        let mut step = ProviderStep::from_provider("codex");
        step.model = Some("gpt-5.4-mini".to_string());
        step.env
            .insert("CODEX_HOME".to_string(), "/home/u/.codex-work".to_string());
        let label = AiService::step_label(&AiProvider::Codex, &step);
        assert!(label.contains("gpt-5.4-mini"), "label: {label}");
        assert!(label.contains(".codex-work"), "label: {label}");
    }

    #[test]
    fn test_step_debug_label_distinguishes_ai_usage_group() {
        // 同一 profile の 2 step (Antigravity のモデル系統別プール) を debug 出力で
        // 見分けられること。group が出ないと 2 行が同一ラベルになり判定を追えない。
        let mut gemini = ProviderStep::from_provider("antigravity");
        gemini.ai_usage_profile = Some("Antigravity".to_string());
        gemini.ai_usage_group = Some("Gemini".to_string());
        let label = AiService::step_debug_label(&gemini);
        assert!(label.contains("profile=Antigravity"), "label: {label}");
        assert!(label.contains("group=Gemini"), "label: {label}");

        // group 未指定なら従来どおり profile / env のみ
        let mut plain = ProviderStep::from_provider("codex");
        plain
            .env
            .insert("CODEX_HOME".to_string(), "/home/u/.codex-work".to_string());
        let label = AiService::step_debug_label(&plain);
        assert_eq!(label, "codex(env=.codex-work)");
    }

    // ============================================================
    // ai-usage 全 step 除外時の gate 挙動テスト
    // ============================================================

    /// used_percent を weekly に載せた擬似アカウントを作る。
    fn make_over_threshold_account(profile: &str, provider: &str, used: f64) -> AiUsageAccount {
        AiUsageAccount {
            profile: profile.to_string(),
            provider: provider.to_string(),
            group_label: None,
            ok: true,
            weekly: Some(UsageWindowData {
                used_percent: Some(used),
            }),
            five_hour: Some(UsageWindowData {
                used_percent: Some(used),
            }),
            error: None,
        }
    }

    #[test]
    fn test_filter_all_over_threshold_sets_gate_blocked() {
        // snapshot 上、全 provider が閾値超過。gate_blocked = true が立つ。
        let steps = vec![
            ProviderStep::from_provider("codex"),
            ProviderStep::from_provider("claude"),
        ];
        let snapshot = AiUsageSnapshot::from_accounts(vec![
            make_over_threshold_account("Work", "codex", 99.0),
            make_over_threshold_account("Work", "claude", 99.0),
        ]);
        let cfg = AiUsageConfig {
            enabled: true,
            threshold_percent: 95.0,
            ..AiUsageConfig::default()
        };
        let (kept, notes, gate_blocked) =
            AiService::apply_ai_usage_filter_with_snapshot(steps, &cfg, &snapshot, Vec::new());
        assert!(kept.is_empty(), "全 step が除外されるはず");
        assert!(gate_blocked, "gate_blocked が立つはず");
        assert!(
            notes.iter().any(|n| n.contains("aborting")),
            "abort 通知が notes に含まれるはず: {notes:?}"
        );
    }

    #[test]
    fn test_filter_partial_over_threshold_keeps_survivors() {
        // 一部だけ閾値超過。残った step があれば gate_blocked は立たない。
        let steps = vec![
            ProviderStep::from_provider("codex"),
            ProviderStep::from_provider("claude"),
        ];
        let snapshot = AiUsageSnapshot::from_accounts(vec![
            make_over_threshold_account("Work", "codex", 99.0),
            make_over_threshold_account("Work", "claude", 10.0),
        ]);
        let cfg = AiUsageConfig {
            enabled: true,
            threshold_percent: 95.0,
            ..AiUsageConfig::default()
        };
        let (kept, _notes, gate_blocked) =
            AiService::apply_ai_usage_filter_with_snapshot(steps, &cfg, &snapshot, Vec::new());
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].provider, "claude");
        assert!(
            !gate_blocked,
            "残った step があるなら gate_blocked は false"
        );
    }

    #[test]
    fn test_filter_empty_input_does_not_set_gate_blocked() {
        // 入力 step が 0 件 → ai-usage は何も除外していないので gate_blocked は false。
        // (from_config はここでデフォルト chain にフォールバックできるようにする)
        let snapshot = AiUsageSnapshot::from_accounts(vec![]);
        let cfg = AiUsageConfig {
            enabled: true,
            ..AiUsageConfig::default()
        };
        let (kept, _notes, gate_blocked) =
            AiService::apply_ai_usage_filter_with_snapshot(Vec::new(), &cfg, &snapshot, Vec::new());
        assert!(kept.is_empty());
        assert!(!gate_blocked, "入力が空 → gate_blocked は立たない");
    }

    #[test]
    fn test_verify_installation_returns_ai_usage_error_when_gate_blocked() {
        // gate_blocked = true の AiService は verify_installation で AiUsageError を返す。
        let mut service = AiService::new();
        service.ai_usage_gate_blocked = true;
        service.steps = Vec::new();
        let err = service
            .verify_installation()
            .expect_err("gate_blocked のとき Err を返すはず");
        assert!(
            matches!(err, AppError::AiUsageError(_)),
            "AiUsageError を返すはず: {err:?}"
        );
    }

    #[test]
    fn test_from_config_all_over_threshold_does_not_fallback_to_default_chain() {
        // ai-usage フィルタが全 step を除外したときに default_steps へ戻さないことを
        // apply_ai_usage_filter_with_snapshot 経由で疑似的に検証する。
        // (from_config は外部プロセスを叩くため直接テストしない)
        let steps = vec![
            ProviderStep::from_provider("codex"),
            ProviderStep::from_provider("claude"),
        ];
        let snapshot = AiUsageSnapshot::from_accounts(vec![
            make_over_threshold_account("Work", "codex", 100.0),
            make_over_threshold_account("Work", "claude", 100.0),
        ]);
        let cfg = AiUsageConfig {
            enabled: true,
            threshold_percent: 95.0,
            ..AiUsageConfig::default()
        };
        let (kept, _notes, gate_blocked) =
            AiService::apply_ai_usage_filter_with_snapshot(steps, &cfg, &snapshot, Vec::new());

        // from_config のフォールバック判定を再現する
        let final_steps = if kept.is_empty() && !gate_blocked {
            AiService::default_steps()
        } else {
            kept
        };
        assert!(
            gate_blocked,
            "全 OverThreshold なので gate_blocked が立つはず"
        );
        assert!(
            final_steps.is_empty(),
            "gate_blocked=true のときフォールバックせず空のまま(default_steps に戻さない)"
        );
    }
}
