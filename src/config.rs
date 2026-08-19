use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::AppError;

/// 各プロバイダーのモデル設定
///
/// 注: Antigravity CLI (`agy`) は v1.0.x で `--model` フラグに対応したため、
/// `antigravity` フィールドの値を agy の `--model` にそのまま渡す。
/// 旧 `gemini` キーは後方互換の入力エイリアスとして受理し、読み込み時に
/// `antigravity` へ昇格する(`PartialModelsConfig` 参照)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsConfig {
    /// Antigravity CLI (`agy`) の `--model` に渡すモデル名。
    /// 表示名(例: "GPT-OSS 120B (Medium)")と slug(例: "gpt-oss-120b-medium")の
    /// どちらでも受理される。`agy models` がどちらを出力するかは agy のバージョンで
    /// 変わる(1.0.x = 表示名 / 1.1.10 = slug)ので、どちらか一方だけが正しいと
    /// 仮定しないこと。未知の名前は agy が非ゼロ終了で明示的に弾くため、
    /// 打ち間違いは既定モデルへの暗黙フォールバックではなくステップ失敗になる。
    /// 空文字列なら `--model` を省略し agy 自身の既定モデルに委ねる。
    #[serde(default = "default_antigravity_model")]
    pub antigravity: String,
    #[serde(default = "default_codex_model")]
    pub codex: String,
    #[serde(default = "default_claude_model")]
    pub claude: String,
    #[serde(default = "default_opencode_model")]
    pub opencode: String,
    /// Grok CLI の `-m` に渡すモデル ID。`grok models` が表示する ID (例: "grok-4.5")
    /// をそのまま指定する。空文字列なら `-m` を省略し grok 自身の既定モデルに委ねる。
    #[serde(default = "default_grok_model")]
    pub grok: String,
}

/// Antigravity CLI (`agy`) のデフォルトモデル。
///
/// agy 1.1.10 で print mode に `--output-format json` が追加され、1 リクエストごとの
/// `usage` (input_tokens 等) が取れるようになったため、Codex と同じ実測比較が可能になった
/// (それ以前は機械可読な使用量出力が無く、公開価格による比較に頼っていた)。
/// 2026-08-04 (JST) に固定プロンプト `Reply ok.` で実測: gpt-oss-120b-medium = 13680 が最小で、
/// 次点 gemini-3.5-flash-medium = 16994 に約 19% の差をつけたため既定として維持する。
/// 詳細な計測値と候補一覧は AGENTS.md の "Default Antigravity (`agy`) model note" を参照。
/// 表示名・slug のどちらでも受理される。空文字列にすれば agy 自身の既定に委ねられる。
fn default_antigravity_model() -> String {
    "GPT-OSS 120B (Medium)".to_string()
}

fn default_codex_model() -> String {
    "gpt-5.4-mini".to_string()
}

fn default_claude_model() -> String {
    "haiku".to_string()
}

fn default_opencode_model() -> String {
    String::new()
}

/// Grok CLI のデフォルトモデル。
///
/// grok CLI 0.2.x は現状 `grok-4.5` のみを提供している (`grok models` で確認)。
/// 空文字列にすれば `-m` を省略し grok 自身の既定モデルに委ねられるため、将来 grok CLI が
/// より安価なモデル (grok-4-fast 等) を追加してもユーザ側で追随できる。既定は
/// CLI 側の変化を追わないよう空文字列 (= 委譲) とする。
fn default_grok_model() -> String {
    String::new()
}

impl Default for ModelsConfig {
    fn default() -> Self {
        Self {
            antigravity: default_antigravity_model(),
            codex: default_codex_model(),
            claude: default_claude_model(),
            opencode: default_opencode_model(),
            grok: default_grok_model(),
        }
    }
}

/// フォールバックチェーンの1ステップ。
///
/// `provider` は「どう CLI を叩くか(引数規約)」、`command` は「何を叩くか(実行バイナリ)」、
/// `env` は「どのアカウント/環境で叩くか(CODEX_HOME 等)」を表し、3 つは直交する軸。
/// 設定では素の文字列(プロバイダー名のみ)とテーブルの両方を受理する(後方互換)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderStep {
    /// 引数規約を決めるプロバイダー種別。"codex"/"antigravity"/"claude"/"opencode"/
    /// "apple-intelligence"、および後方互換エイリアス "gemini"/"agy" を受理する。
    pub provider: String,
    /// このステップで使うモデル名。None/空なら [models].<provider> → 各 CLI 既定。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 実行バイナリと固定引数。先頭=バイナリ(ラッパースクリプトのパス可)、以降=常に渡す追加引数。
    /// None なら provider 既定バイナリ(codex/agy/claude/opencode)を使う。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    /// 起動時に `Command::env()` で明示的に上書きする環境変数(CODEX_HOME 等)。
    /// 値は設定読み込み時に `~` 展開済み。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// クールダウン/ログ用の識別名(任意)。省略時は他フィールドから決定的に導出する。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// ai-usage --json 連携時に (profile, provider) 照合に使う Chrome プロファイル名(任意)。
    /// `[ai_usage] enabled = true` のときのみ意味を持ち、この step の provider と組み合わせて
    /// 該当 account の残量を検索する。None なら「同一 provider の中で残量が最も多い account」を
    /// 自動採用する(auto-select)。cooldown_key には含めない(cooldown は失敗経路の管理、
    /// ai-usage は残量経路の管理で独立して機能させるため)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_usage_profile: Option<String>,
    /// ai-usage --json の `group_label` と照合するモデル系統名(任意)。大文字小文字は無視する。
    ///
    /// 1 アカウントの残量が「モデル系統ごとの別プール」に分かれている provider 用。
    /// 例: Antigravity は同じ profile に対して `group_label = "Gemini"` と
    /// `"Claude&GPT"` の 2 行を返し、両者は独立した週次プールなので、Gemini 側が
    /// 100% でも GPT-OSS 側は使える。これを指定しないと同一 profile の複数行を
    /// 区別できず、片方の枯渇でもう片方まで chain から外れてしまう。
    /// `ai_usage_profile` と同じ理由で cooldown_key には含めない。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_usage_group: Option<String>,
}

impl ProviderStep {
    /// プロバイダー名のみのステップを作る(テスト・`-p` 上書き・後方互換用)。
    pub fn from_provider(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: None,
            command: None,
            env: BTreeMap::new(),
            name: None,
            ai_usage_profile: None,
            ai_usage_group: None,
        }
    }

    /// クールダウン/降格判定の一意キー。
    ///
    /// `name` 明示時はそれを正規化して使う。未指定時は provider(エイリアス正規化)、
    /// model、env、command から決定的に導出する。区切りは値に現れない US(0x1F)。
    /// これにより「同一 provider でも model/アカウント(env)/バイナリが違えば別キー」になり、
    /// 片方がクールダウン中でも他方は生き残る。
    pub fn cooldown_key(&self) -> String {
        if let Some(name) = self.name.as_deref().filter(|s| !s.trim().is_empty()) {
            return name.trim().to_lowercase();
        }
        let provider = canonical_provider_key(&self.provider);
        let model = self
            .model
            .as_deref()
            .filter(|m| !m.is_empty())
            .unwrap_or("");
        // env は BTreeMap なのでキー昇順で順序が決定的(挿入順に依存しない)。
        let env_fp = self
            .env
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("\u{1f}");
        let cmd_fp = self
            .command
            .as_ref()
            .map(|c| c.join("\u{1f}"))
            .unwrap_or_default();
        format!("{provider}\u{1f}{model}\u{1f}{env_fp}\u{1f}{cmd_fp}")
    }

    /// ログ表示用のアカウント識別ヒント。name 明示時はそれ、さもなくば
    /// CODEX_HOME / CLAUDE_CONFIG_DIR 等の値の末尾要素(例: ".codex-work")を返す。
    pub fn account_hint(&self) -> Option<String> {
        if let Some(name) = self.name.as_deref().filter(|s| !s.trim().is_empty()) {
            return Some(name.trim().to_string());
        }
        for key in ["CODEX_HOME", "CLAUDE_CONFIG_DIR"] {
            if let Some(v) = self.env.get(key) {
                let tail = std::path::Path::new(v)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(v.as_str());
                return Some(tail.to_string());
            }
        }
        None
    }
}

impl<'de> Deserialize<'de> for ProviderStep {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{self, MapAccess, Visitor};
        use std::fmt;

        // テーブル形式 { provider = "...", model = "...", ... } 用の内部表現。
        // #[serde(untagged)] を使わないのは、TOML で不正入力時のエラーが
        // "data did not match any variant" に潰れて原因が分からなくなるため。
        // 手書き Visitor なら "missing field `provider`" まで具体的に出せる。
        #[derive(Deserialize)]
        struct Raw {
            provider: String,
            #[serde(default)]
            model: Option<String>,
            #[serde(default)]
            command: Option<Vec<String>>,
            #[serde(default)]
            env: BTreeMap<String, String>,
            #[serde(default)]
            name: Option<String>,
            #[serde(default)]
            ai_usage_profile: Option<String>,
            #[serde(default)]
            ai_usage_group: Option<String>,
        }

        struct StepVisitor;
        impl<'de> Visitor<'de> for StepVisitor {
            type Value = ProviderStep;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str(
                    "a provider name string (e.g. \"codex\") or a table \
                     { provider = \"...\", model = \"...\", command = [...], env = { ... } }",
                )
            }

            fn visit_str<E: de::Error>(self, s: &str) -> Result<Self::Value, E> {
                Ok(ProviderStep::from_provider(s))
            }

            fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<Self::Value, M::Error> {
                let raw = Raw::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(ProviderStep {
                    provider: raw.provider,
                    model: raw.model.filter(|m| !m.is_empty()),
                    command: raw.command.filter(|c| !c.is_empty()),
                    env: raw.env,
                    name: raw.name.filter(|n| !n.trim().is_empty()),
                    ai_usage_profile: raw.ai_usage_profile.filter(|p| !p.trim().is_empty()),
                    ai_usage_group: raw.ai_usage_group.filter(|g| !g.trim().is_empty()),
                })
            }
        }

        deserializer.deserialize_any(StepVisitor)
    }
}

/// 状態ファイルと設定ファイルのプロバイダー名を比較用の正規名にそろえる。
///
/// 後方互換エイリアス("gemini"/"agy" → "antigravity"、"apple-ai"/"apple_intelligence" →
/// "apple-intelligence")を吸収し、クールダウンキーやプロバイダー解決で同一視できるようにする。
pub(crate) fn canonical_provider_key(provider: &str) -> String {
    let lower = provider.to_lowercase();
    match lower.as_str() {
        "agy" | "gemini" | "antigravity" => "antigravity".to_string(),
        "apple-ai" | "apple_intelligence" | "apple-intelligence" => {
            "apple-intelligence".to_string()
        }
        _ => lower,
    }
}

/// 環境変数名が POSIX の `[A-Za-z_][A-Za-z0-9_]*` かを判定する。
fn is_valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// 動的ローダーや任意コード実行に直結する危険な環境変数キーを拒否する。
///
/// 想定脅威: 悪意あるリポジトリが project 側 `.git-sc` に
/// `env = { DYLD_INSERT_LIBRARIES = "/tmp/evil.dylib" }` のようなエントリを仕込み、
/// git-sc 経由で実行される子プロセス(codex/claude/agy)へ共有ライブラリを注入する経路。
/// アカウント切り替え (`CODEX_HOME` / `CLAUDE_CONFIG_DIR` 等) の正当用途は影響を受けない。
fn is_dangerous_env_key(key: &str) -> bool {
    let key = key.to_ascii_uppercase();
    // 大文字比較する: 一部 OS は大文字小文字を区別しないが、Unix dynamic loader はそのまま参照する。
    // ここでは Linux/macOS の動的ローダーキーと、Node/Python/Perl の事前ロード経路をブロックする。
    matches!(
        key.as_str(),
        "LD_PRELOAD"
            | "LD_LIBRARY_PATH"
            | "LD_AUDIT"
            | "DYLD_INSERT_LIBRARIES"
            | "DYLD_LIBRARY_PATH"
            | "DYLD_FALLBACK_LIBRARY_PATH"
            | "DYLD_FRAMEWORK_PATH"
            | "DYLD_FALLBACK_FRAMEWORK_PATH"
            | "DYLD_FORCE_FLAT_NAMESPACE"
            | "DYLD_IMAGE_SUFFIX"
            | "DYLD_PRINT_LIBRARIES"
            | "NODE_OPTIONS"
            | "PYTHONPATH"
            | "PYTHONSTARTUP"
            | "PERL5OPT"
            | "PERL5LIB"
            | "RUBYOPT"
            | "RUBYLIB"
    )
}

/// env マップの key を検証し、値を `~` 展開する($VAR 展開や相対パス解決はしない)。
/// 不正なキーや危険なキーがあれば設定エラーにする(沈黙して別アカウントで動く事故を防ぐ)。
fn expand_step_env(
    env: &BTreeMap<String, String>,
    ctx: &str,
) -> Result<BTreeMap<String, String>, AppError> {
    let mut out = BTreeMap::new();
    for (k, v) in env {
        if !is_valid_env_key(k) {
            return Err(AppError::ConfigError(format!(
                "{ctx}: invalid environment variable name: {k:?}"
            )));
        }
        if is_dangerous_env_key(k) {
            return Err(AppError::ConfigError(format!(
                "{ctx}: refusing to set dangerous environment variable {k:?} \
                 (dynamic loader / interpreter pre-load keys are blocked to prevent code injection)"
            )));
        }
        out.insert(k.clone(), shellexpand::tilde(v).to_string());
    }
    Ok(out)
}

/// 設定ファイルからの部分読み込み用モデル設定
///
/// `Option<T>` により「未指定」と「明示的にデフォルト値を指定」を区別する。
#[derive(Debug, Default, Deserialize)]
struct PartialModelsConfig {
    /// Antigravity CLI (`agy`) の `--model` に渡すモデル名(canonical キー)
    pub antigravity: Option<String>,
    /// 旧 Gemini CLI 時代の互換キー(入力専用)。
    /// `antigravity` 未指定時のみ昇格して使う。
    pub gemini: Option<String>,
    pub codex: Option<String>,
    pub claude: Option<String>,
    pub opencode: Option<String>,
    pub grok: Option<String>,
}

/// 設定ファイルからの部分読み込み用
///
/// 全フィールドが `Option` のため、部分設定ファイルでもパースに失敗しない。
/// `merge_into()` で `Config` に明示的に指定されたフィールドのみ上書きする。
#[derive(Debug, Default, Deserialize)]
struct PartialConfig {
    pub providers: Option<Vec<ProviderStep>>,
    pub language: Option<String>,
    #[serde(default)]
    pub models: PartialModelsConfig,
    pub prefix_scripts: Option<Vec<PrefixScriptConfig>>,
    pub prefix_rules: Option<Vec<PrefixRuleConfig>>,
    pub provider_cooldown_minutes: Option<u64>,
    pub prefix_type: Option<String>,
    pub auto_push: Option<bool>,
    pub provider_timeout_seconds: Option<u64>,
    pub nano_buddy: Option<bool>,
    pub codex_reasoning_effort: Option<String>,
    pub ai_usage: Option<AiUsageConfig>,
}

impl PartialConfig {
    /// `Config` に変換（未指定フィールドはデフォルト値を使用）
    fn into_config(self) -> Config {
        let defaults = Config::default();
        Config {
            providers: self.providers.unwrap_or(defaults.providers),
            language: self.language.unwrap_or(defaults.language),
            models: ModelsConfig {
                // 旧 `gemini` キーは `antigravity` 未指定時のみ昇格して採用する。
                antigravity: self
                    .models
                    .antigravity
                    .or(self.models.gemini)
                    .unwrap_or(defaults.models.antigravity),
                codex: self.models.codex.unwrap_or(defaults.models.codex),
                claude: self.models.claude.unwrap_or(defaults.models.claude),
                opencode: self.models.opencode.unwrap_or(defaults.models.opencode),
                grok: self.models.grok.unwrap_or(defaults.models.grok),
            },
            prefix_scripts: self.prefix_scripts.unwrap_or(defaults.prefix_scripts),
            prefix_rules: self.prefix_rules.unwrap_or(defaults.prefix_rules),
            provider_cooldown_minutes: self
                .provider_cooldown_minutes
                .unwrap_or(defaults.provider_cooldown_minutes),
            prefix_type: self.prefix_type.or(defaults.prefix_type),
            auto_push: self.auto_push.or(defaults.auto_push),
            provider_timeout_seconds: self
                .provider_timeout_seconds
                .unwrap_or(defaults.provider_timeout_seconds),
            nano_buddy: self.nano_buddy.unwrap_or(defaults.nano_buddy),
            codex_reasoning_effort: self
                .codex_reasoning_effort
                .unwrap_or(defaults.codex_reasoning_effort),
            ai_usage: self.ai_usage.or(defaults.ai_usage),
        }
    }

    /// 指定されたフィールドのみ `Config` に上書き適用する
    fn merge_into(self, config: &mut Config) {
        if let Some(providers) = self.providers
            && !providers.is_empty()
        {
            config.providers = providers;
        }
        if let Some(language) = self.language {
            config.language = language;
        }
        // 旧 `gemini` キーは `antigravity` 未指定時のみ昇格して採用する。
        if let Some(antigravity) = self.models.antigravity.or(self.models.gemini) {
            config.models.antigravity = antigravity;
        }
        if let Some(codex) = self.models.codex {
            config.models.codex = codex;
        }
        if let Some(claude) = self.models.claude {
            config.models.claude = claude;
        }
        if let Some(opencode) = self.models.opencode {
            config.models.opencode = opencode;
        }
        if let Some(grok) = self.models.grok {
            config.models.grok = grok;
        }
        if let Some(scripts) = self.prefix_scripts
            && !scripts.is_empty()
        {
            config.prefix_scripts = scripts;
        }
        if let Some(rules) = self.prefix_rules
            && !rules.is_empty()
        {
            config.prefix_rules = rules;
        }
        if let Some(minutes) = self.provider_cooldown_minutes {
            config.provider_cooldown_minutes = minutes;
        }
        if let Some(prefix_type) = self.prefix_type {
            config.prefix_type = Some(prefix_type);
        }
        if let Some(auto_push) = self.auto_push {
            config.auto_push = Some(auto_push);
        }
        if let Some(seconds) = self.provider_timeout_seconds {
            config.provider_timeout_seconds = seconds;
        }
        if let Some(nano_buddy) = self.nano_buddy {
            config.nano_buddy = nano_buddy;
        }
        if let Some(effort) = self.codex_reasoning_effort {
            config.codex_reasoning_effort = effort;
        }
        if let Some(ai_usage) = self.ai_usage {
            config.ai_usage = Some(ai_usage);
        }
    }
}

/// ai-usage --json 連携設定
///
/// `enabled = true` かつ `command` の実行に成功したとき、`AiService::from_config` は
/// fallback chain の各 step について残量を評価し、`threshold_percent` を超えている
/// step を chain から除外する。取得に失敗しても既存のフォールバックが動く fail-open。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiUsageConfig {
    /// 連携を有効化する。false なら snapshot 取得も filter も行わない(既定 = false)。
    #[serde(default)]
    pub enabled: bool,
    /// ai-usage を起動するコマンド(既定: `["ai-usage", "--json"]`)。
    #[serde(default = "default_ai_usage_command")]
    pub command: Vec<String>,
    /// この使用率(%)以上の step は chain から除外する(既定: 95)。
    #[serde(default = "default_ai_usage_threshold_percent")]
    pub threshold_percent: f64,
    /// 判定に使う枠(weekly / five_hour / nearest。既定: nearest = 両方のうち最大値)。
    #[serde(default)]
    pub window: AiUsageWindow,
    /// ai-usage --json の実行タイムアウト(秒。既定: 10)。
    #[serde(default = "default_ai_usage_timeout_seconds")]
    pub timeout_seconds: u64,
}

impl Default for AiUsageConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            command: default_ai_usage_command(),
            threshold_percent: default_ai_usage_threshold_percent(),
            window: AiUsageWindow::default(),
            timeout_seconds: default_ai_usage_timeout_seconds(),
        }
    }
}

/// 使用率判定に使う枠。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AiUsageWindow {
    /// 週次のみ。
    Weekly,
    /// 5 時間枠のみ。
    FiveHour,
    /// weekly と five_hour のうち **使用率が高い方**(=安全側)を採用する(既定)。
    #[default]
    Nearest,
}

/// `[ai_usage]` の既定コマンド(`ai-usage --json`)。
fn default_ai_usage_command() -> Vec<String> {
    vec!["ai-usage".to_string(), "--json".to_string()]
}

fn default_ai_usage_threshold_percent() -> f64 {
    95.0
}

fn default_ai_usage_timeout_seconds() -> u64 {
    10
}

/// プレフィックススクリプト設定
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PrefixScriptConfig {
    /// リモートURLにマッチさせる正規表現パターン
    pub url_pattern: String,
    /// 実行するスクリプトのパス
    pub script: String,
}

/// プレフィックスルール設定（URLベース）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PrefixRuleConfig {
    /// リモートURLにマッチさせる正規表現パターン
    pub url_pattern: String,
    /// プレフィックスの種類（conventional, none, etc.）
    pub prefix_type: String,
}

/// アプリケーション設定
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// AIプロバイダーのフォールバックチェーン(各ステップは provider/model/command/env を持つ)
    #[serde(default)]
    pub providers: Vec<ProviderStep>,
    /// コミットメッセージの言語
    #[serde(default = "default_language")]
    pub language: String,
    /// 各プロバイダーのモデル
    #[serde(default)]
    pub models: ModelsConfig,
    /// プレフィックス生成スクリプト設定（オプション）
    #[serde(default)]
    pub prefix_scripts: Vec<PrefixScriptConfig>,
    /// プレフィックスルール設定（URLベース、オプション）
    #[serde(default)]
    pub prefix_rules: Vec<PrefixRuleConfig>,
    /// プロバイダーエラー時のクールダウン時間（分）
    #[serde(default = "default_provider_cooldown_minutes")]
    pub provider_cooldown_minutes: u64,
    /// コミットメッセージの形式（conventional, bracket, colon, emoji, plain）
    #[serde(default)]
    pub prefix_type: Option<String>,
    /// 自動プッシュの有効/無効
    #[serde(default)]
    pub auto_push: Option<bool>,
    /// プロバイダー呼び出しのタイムアウト（秒）
    #[serde(default = "default_provider_timeout_seconds")]
    pub provider_timeout_seconds: u64,
    /// NanoBuddy連携を有効化 (隠しオプション)
    #[serde(default)]
    pub nano_buddy: bool,
    /// Codex の `-c model_reasoning_effort` に渡す値（low/medium/high）
    /// 空文字列の場合は `-c` 指定を省略し codex の既定値を使用
    #[serde(default = "default_codex_reasoning_effort")]
    pub codex_reasoning_effort: String,
    /// ai-usage --json 連携設定(省略時は連携無効)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_usage: Option<AiUsageConfig>,
}

/// デフォルトのクールダウン時間（60分 = 1時間）
fn default_provider_cooldown_minutes() -> u64 {
    60
}

/// デフォルトのプロバイダータイムアウト（60秒）
fn default_provider_timeout_seconds() -> u64 {
    60
}

/// デフォルトの言語
fn default_language() -> String {
    "Japanese".to_string()
}

/// デフォルトの Codex 推論深度
fn default_codex_reasoning_effort() -> String {
    "low".to_string()
}

impl Default for Config {
    fn default() -> Self {
        let mut providers = vec![
            ProviderStep::from_provider("opencode"),
            ProviderStep::from_provider("grok"),
            ProviderStep::from_provider("antigravity"),
            ProviderStep::from_provider("codex"),
            ProviderStep::from_provider("claude"),
        ];
        if cfg!(all(target_os = "macos", feature = "apple-ai")) {
            providers.push(ProviderStep::from_provider("apple-intelligence"));
        }
        Self {
            providers,
            language: default_language(),
            models: ModelsConfig::default(),
            prefix_scripts: Vec::new(),
            prefix_rules: Vec::new(),
            provider_cooldown_minutes: default_provider_cooldown_minutes(),
            prefix_type: None,
            auto_push: None,
            provider_timeout_seconds: default_provider_timeout_seconds(),
            nano_buddy: false,
            codex_reasoning_effort: default_codex_reasoning_effort(),
            ai_usage: None,
        }
    }
}

impl Config {
    /// 設定ディレクトリのパスを取得（~/.config/git-sc）
    pub fn config_dir() -> Option<PathBuf> {
        dirs::home_dir().map(|home| home.join(".config").join("git-sc"))
    }

    /// グローバル設定ファイルのパスを取得（~/.config/git-sc/config.toml）
    pub fn global_config_path() -> Result<PathBuf, AppError> {
        Self::config_dir()
            .map(|dir| dir.join("config.toml"))
            .ok_or_else(|| AppError::ConfigError("Could not find home directory".to_string()))
    }

    /// プロジェクト設定ファイルのパスを取得（Git root の .git-sc）
    pub fn project_config_path() -> Result<Option<PathBuf>, AppError> {
        use std::process::Command;

        let output = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .output();

        match output {
            Ok(output) if output.status.success() => {
                let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let path = PathBuf::from(root).join(".git-sc");
                if path.exists() {
                    Ok(Some(path))
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    /// グローバル設定を PartialConfig として読み込む
    fn load_global_partial() -> Result<Option<PartialConfig>, AppError> {
        let path = Self::global_config_path()?;

        if !path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&path)
            .map_err(|e| AppError::ConfigError(format!("Failed to read global config: {}", e)))?;

        match toml::from_str(&content) {
            Ok(config) => Ok(Some(config)),
            Err(e) => {
                // パースエラーの場合は警告を出してデフォルトを使用
                // ファイルは上書きしない
                eprintln!(
                    "警告: グローバル設定ファイルの構文エラー ({}): {}\nデフォルト設定を使用します。",
                    path.display(),
                    e
                );
                Ok(None)
            }
        }
    }

    /// プロジェクト設定を PartialConfig として読み込む
    fn load_project_partial() -> Result<Option<PartialConfig>, AppError> {
        let path = match Self::project_config_path()? {
            Some(p) => p,
            None => return Ok(None),
        };

        let content = fs::read_to_string(&path)
            .map_err(|e| AppError::ConfigError(format!("Failed to read project config: {}", e)))?;

        match toml::from_str(&content) {
            Ok(config) => Ok(Some(config)),
            Err(e) => {
                eprintln!(
                    "警告: プロジェクト設定ファイルの構文エラー ({}):{}\nグローバル設定にフォールバックします。",
                    path.display(),
                    e
                );
                Ok(None)
            }
        }
    }

    /// 階層的に設定を読み込む（グローバル → プロジェクトでマージ）
    ///
    /// PartialConfig（全フィールド Option）で読み込むことで、
    /// 「未指定」と「明示的にデフォルト値を指定」を正しく区別する。
    pub fn load() -> Result<Self, AppError> {
        // 1. グローバル設定を読み込んで Config に変換
        let mut config: Config = match Self::load_global_partial()? {
            Some(partial) => partial.into_config(),
            None => Config::default(),
        };

        // 2. プロジェクト設定を読み込んで、指定されたフィールドのみ上書き
        if let Some(project_partial) = Self::load_project_partial()? {
            project_partial.merge_into(&mut config);
        }

        // 3. 各ステップの env キーを検証し、env 値と command バイナリの `~` を展開する。
        config.finalize_steps()?;

        Ok(config)
    }

    /// 読み込み後の正規化。各ステップの env キーを検証し、env 値と command の
    /// 先頭バイナリの `~` を展開する。env キーが不正な場合はエラーにする
    /// (沈黙してアカウント切替が効かず別アカウントで動く事故を防ぐ)。
    fn finalize_steps(&mut self) -> Result<(), AppError> {
        for (i, step) in self.providers.iter_mut().enumerate() {
            let ctx = format!("providers[{}] (provider={:?})", i, step.provider);
            step.env = expand_step_env(&step.env, &ctx)?;
            if let Some(cmd) = step.command.as_mut()
                && let Some(first) = cmd.first_mut()
            {
                *first = shellexpand::tilde(first).to_string();
            }
        }
        Ok(())
    }

    /// 設定をファイルに保存
    #[allow(dead_code)]
    pub fn save(&self) -> Result<(), AppError> {
        let path = Self::global_config_path()?;

        // ディレクトリが存在しない場合は作成
        if let Some(dir) = Self::config_dir() {
            fs::create_dir_all(&dir).map_err(|e| {
                AppError::ConfigError(format!("Failed to create config directory: {}", e))
            })?;
        }

        let content = toml::to_string_pretty(self)
            .map_err(|e| AppError::ConfigError(format!("Failed to serialize config: {}", e)))?;

        fs::write(&path, content)
            .map_err(|e| AppError::ConfigError(format!("Failed to write config: {}", e)))?;

        Ok(())
    }

    /// init コマンド用のデフォルト設定ファイル内容を生成
    pub fn default_config_content() -> String {
        let codex_model = default_codex_model();
        let antigravity_model = default_antigravity_model();
        let providers = if cfg!(all(target_os = "macos", feature = "apple-ai")) {
            r#"providers = ["opencode", "grok", "antigravity", "codex", "claude", "apple-intelligence"]"#
        } else {
            r#"providers = ["opencode", "grok", "antigravity", "codex", "claude"]"#
        };

        let apple_ai_available = if cfg!(all(target_os = "macos", feature = "apple-ai")) {
            r#"# 使用可能: "opencode", "grok", "antigravity", "codex", "claude", "apple-intelligence"
# "antigravity" は旧 Gemini CLI の後継 (`agy`)。"gemini" と書いても同じプロバイダーとして扱う
# "apple-intelligence" には macOS 26+ と Apple Silicon が必要"#
        } else {
            r#"# 使用可能: "opencode", "grok", "antigravity", "codex", "claude"
# "antigravity" は旧 Gemini CLI の後継 (`agy`)。"gemini" と書いても同じプロバイダーとして扱う"#
        };

        format!(
            r#"# git-sc 設定ファイル
# AI によるスマートコミットメッセージ生成

# AI プロバイダーの優先順
{apple_ai_available}
{providers}

# コミットメッセージの言語
language = "Japanese"

# プロバイダー失敗後のクールダウン時間（分）
# 0 を指定するとクールダウンを無効化
provider_cooldown_minutes = 60

# プロバイダー1回あたりのタイムアウト（秒）
# この秒数を超えたプロバイダーは終了し、次のプロバイダーを試す
provider_timeout_seconds = 60

# コミットメッセージのプレフィックス形式
# 使用可能: "conventional", "bracket", "colon", "emoji", "plain", "none"
# prefix_type = "conventional"

# コミット後に自動 push
# auto_push = false

# Codex に `-c model_reasoning_effort=<value>` として渡す推論深度
# 使用可能: "low", "medium", "high", "xhigh"（空文字列なら省略して codex 既定を使用）
codex_reasoning_effort = "low"

# 各プロバイダーのモデル設定
# Antigravity CLI (`agy`) は `--model` に対応。`agy models` の表示名をそのまま指定する
# (空文字列なら agy 既定モデルに委ねる)。旧 `gemini = "..."` キーは後方互換で
# antigravity に昇格して扱われる。
# Grok CLI は `-m` に対応。`grok models` の ID (例: "grok-4.5") をそのまま指定する
# (空文字列なら grok 既定モデルに委ねる)。
[models]
antigravity = "{antigravity_model}"
codex = "{codex_model}"
claude = "haiku"
opencode = ""
grok = ""

# プレフィックススクリプト（上から順に実行し、最初の一致を使用）
# スクリプト引数: remote_url, branch_name
# 終了コード 0 + 出力あり: 出力をプレフィックスとして使用
# 終了コード 0 + 空出力: プレフィックスなし
# 終了コード 1: AI 生成メッセージをプレフィックスなしで使用
# [[prefix_scripts]]
# url_pattern = "^https://github\\.com/myorg/"
# script = "/path/to/prefix-script.sh"

# プレフィックスルール（URL ベース、最初の一致を使用）
# [[prefix_rules]]
# url_pattern = "github\\.com[:/]myorg/"
# prefix_type = "conventional"

# --- 高度な設定: フォールバックチェーン (provider + model + env/command) ---
# providers の各要素は文字列の代わりにテーブルでも書ける。provider に加えて
# model(このステップのモデル)、command(実行バイナリ/ラッパー差し替え)、
# env(CODEX_HOME 等の環境変数)を指定できる。同じプロバイダを別モデル・別アカウントで
# 複数回並べると、それぞれ独立にフォールバック & クールダウンされる。
# providers = [
#   {{ provider = "codex", model = "gpt-5.4-mini", env = {{ CODEX_HOME = "~/.codex" }} }},
#   {{ provider = "codex", model = "gpt-5.4-mini", env = {{ CODEX_HOME = "~/.codex-work" }} }},
#   {{ provider = "antigravity", model = "Gemini 3.5 Flash (Low)" }},
#   {{ provider = "antigravity", model = "GPT-OSS 120B (Medium)" }},
#   "claude",
# ]
# アカウント切替は env での CODEX_HOME / CLAUDE_CONFIG_DIR 明示を推奨。git-sc が
# 起動時に明示上書きするため、親シェルのアカウント設定に引きずられない。
# ラッパースクリプトを使う場合は command でも可:
#   {{ provider = "codex", command = ["~/path/to/codex-wrapper.sh"] }}

# --- ai-usage 連携 (5h / weekly 残量からエージェントを自動判定) ---
# `ai-usage --json` (https://github.com/owayo/ai-usage) を起動時に 1 回だけ叩き、
# 5 時間枠 / 週次の使用率が閾値(threshold_percent, 既定 95)以上のプロバイダーを
# fallback chain から除外する。取得失敗・アカウント不一致は fail-open で連携無効
# (fallback chain は元のまま) になるため、ai-usage 側が壊れても commit は動く。
# 各 step で `ai_usage_profile = "Work"` を指定すると (profile, provider) で
# 一意に照合される。省略時は「同一 provider の中で残量が最も多い account」を
# 自動採用する。
#
# 1 アカウントの残量がモデル系統ごとに別プールになっている provider (Antigravity は
# `group_label = "Gemini"` と `"Claude&GPT"` の 2 行が返り、別々の週次枠) では
# `ai_usage_group` でどの系統を見るか指定する。指定しないと片方の枯渇でもう片方の
# step まで chain から外れる。大文字小文字は区別しない。
#
# [ai_usage]
# enabled = true                          # false または省略で連携無効
# command = ["ai-usage", "--json"]        # 起動コマンド。ラッパー可
# threshold_percent = 95                  # この使用率以上の step は chain から除外
# window = "nearest"                      # weekly | five_hour | nearest (既定 nearest = 高い方を採用)
# timeout_seconds = 10                    # ai-usage 実行のタイムアウト(秒)
#
# providers = [
#   {{ provider = "codex", ai_usage_profile = "Work",
#      env = {{ CODEX_HOME = "~/.codex" }} }},
#   {{ provider = "codex", ai_usage_profile = "Home",
#      env = {{ CODEX_HOME = "~/.codex-home" }} }},
#   {{ provider = "claude", ai_usage_profile = "Work",
#      env = {{ CLAUDE_CONFIG_DIR = "~/.claude" }} }},
#   {{ provider = "antigravity", model = "GPT-OSS 120B (Medium)",
#      ai_usage_group = "Claude&GPT" }},   # Gemini 系が枯れてもこの系統は使える
#   {{ provider = "antigravity", model = "Gemini 3.5 Flash (Low)",
#      ai_usage_group = "Gemini" }},
#   "opencode",  # profile 未指定は auto-select (最も残量が多い account を採用)
# ]
"#
        )
    }
}

/// テスト用ヘルパー関数
#[cfg(test)]
impl Config {
    /// 文字列から設定を読み込み（テスト用）
    pub fn from_str(content: &str) -> Result<Self, AppError> {
        toml::from_str(content)
            .map_err(|e| AppError::ConfigError(format!("Failed to parse config: {}", e)))
    }

    /// 2つの設定をマージ（テスト用、other が優先）
    ///
    /// 注意: この関数は「デフォルト値と同じ値」を未指定として扱うため、
    /// プロジェクト設定で明示的にデフォルト値を指定したケースでは正しく動作しない。
    /// 実運用では `PartialConfig::merge_into()` を使用する `load()` が正確な動作を保証する。
    pub fn merge_with(&mut self, other: Self) {
        // Vec フィールド: other が空でなければ完全置換
        if !other.providers.is_empty() {
            self.providers = other.providers;
        }
        if !other.prefix_scripts.is_empty() {
            self.prefix_scripts = other.prefix_scripts;
        }
        if !other.prefix_rules.is_empty() {
            self.prefix_rules = other.prefix_rules;
        }

        // String フィールド: other がデフォルトでなければ上書き
        if other.language != default_language() {
            self.language = other.language;
        }

        // Option フィールド: Some で上書き
        if other.prefix_type.is_some() {
            self.prefix_type = other.prefix_type;
        }
        if other.auto_push.is_some() {
            self.auto_push = other.auto_push;
        }

        // nano_buddy: true で上書き
        if other.nano_buddy {
            self.nano_buddy = true;
        }

        // ModelsConfig: 個別フィールドをマージ
        if other.models.antigravity != ModelsConfig::default().antigravity {
            self.models.antigravity = other.models.antigravity;
        }
        if other.models.codex != ModelsConfig::default().codex {
            self.models.codex = other.models.codex;
        }
        if other.models.claude != ModelsConfig::default().claude {
            self.models.claude = other.models.claude;
        }
        if other.models.opencode != ModelsConfig::default().opencode {
            self.models.opencode = other.models.opencode;
        }
        if other.models.grok != ModelsConfig::default().grok {
            self.models.grok = other.models.grok;
        }

        // provider_cooldown_minutes: デフォルトでなければ上書き
        if other.provider_cooldown_minutes != default_provider_cooldown_minutes() {
            self.provider_cooldown_minutes = other.provider_cooldown_minutes;
        }

        // provider_timeout_seconds: デフォルトでなければ上書き
        if other.provider_timeout_seconds != default_provider_timeout_seconds() {
            self.provider_timeout_seconds = other.provider_timeout_seconds;
        }

        // ai_usage: Some で上書き
        if other.ai_usage.is_some() {
            self.ai_usage = other.ai_usage;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    #[test]
    fn test_default_config() {
        let config = Config::default();

        let mut expected_providers = vec![
            ProviderStep::from_provider("opencode"),
            ProviderStep::from_provider("grok"),
            ProviderStep::from_provider("antigravity"),
            ProviderStep::from_provider("codex"),
            ProviderStep::from_provider("claude"),
        ];
        if cfg!(all(target_os = "macos", feature = "apple-ai")) {
            expected_providers.push(ProviderStep::from_provider("apple-intelligence"));
        }
        assert_eq!(config.providers, expected_providers);
        assert_eq!(config.language, "Japanese");
        assert!(config.prefix_scripts.is_empty());
        assert!(config.prefix_rules.is_empty());
        assert_eq!(config.provider_cooldown_minutes, 60);
        assert_eq!(config.provider_timeout_seconds, 60);
        assert_eq!(config.codex_reasoning_effort, "low");
    }

    #[test]
    fn test_codex_reasoning_effort_parses_from_toml() {
        let toml = r#"
codex_reasoning_effort = "high"
"#;
        let config = Config::from_str(toml).unwrap();
        assert_eq!(config.codex_reasoning_effort, "high");
    }

    #[test]
    fn test_codex_reasoning_effort_project_override() {
        let global_toml = r#"
codex_reasoning_effort = "medium"
"#;
        let project_toml = r#"
codex_reasoning_effort = "high"
"#;
        let mut global = Config::from_str(global_toml).unwrap();
        let project_partial: PartialConfig = toml::from_str(project_toml).unwrap();
        project_partial.merge_into(&mut global);
        assert_eq!(global.codex_reasoning_effort, "high");
    }

    #[test]
    fn test_default_models_config() {
        let models = ModelsConfig::default();

        // antigravity のデフォルトは agy 提供モデル中もっとも単価が安い GPT-OSS 120B。
        assert_eq!(models.antigravity, default_antigravity_model());
        assert_eq!(models.codex, default_codex_model());
        assert_eq!(models.claude, "haiku");
        assert_eq!(models.opencode, "");
    }

    #[test]
    fn test_default_codex_model_uses_current_default() {
        // codex debug models で medium reasoning 対応モデルの入力トークンを比較し、
        // 最小の入力トークン数だった現在の既定モデルを固定する。
        assert_eq!(default_codex_model(), "gpt-5.4-mini");
        assert_eq!(Config::default().models.codex, "gpt-5.4-mini");
    }

    #[test]
    fn test_parse_minimal_config() {
        // 旧 "gemini" エイリアスを設定ファイルに書いてもパースは成功し、文字列はそのまま保持される
        // (実行時に from_str() で Antigravity に解決される)。
        let toml = r#"
        providers = ["gemini"]
        language = "English"
"#;

        let config = Config::from_str(toml).unwrap();

        assert_eq!(
            config.providers,
            vec![ProviderStep::from_provider("gemini")]
        );
        assert_eq!(config.language, "English");
        // antigravity モデルは未指定なのでデフォルト値
        assert_eq!(config.models.antigravity, default_antigravity_model());
        assert!(config.prefix_scripts.is_empty());
        assert!(config.prefix_rules.is_empty());
        assert_eq!(config.provider_cooldown_minutes, 60);
    }

    #[test]
    fn test_parse_config_with_custom_cooldown() {
        let toml = r#"
        providers = ["gemini"]
        language = "Japanese"
        provider_cooldown_minutes = 30
"#;

        let config = Config::from_str(toml).unwrap();

        assert_eq!(config.provider_cooldown_minutes, 30);
    }

    #[test]
    fn test_parse_config_with_zero_cooldown() {
        let toml = r#"
        providers = ["gemini"]
        language = "Japanese"
        provider_cooldown_minutes = 0
"#;

        let config = Config::from_str(toml).unwrap();

        // 0に設定するとクールダウン機能を無効化
        assert_eq!(config.provider_cooldown_minutes, 0);
    }

    #[test]
    fn test_parse_config_with_prefix_scripts() {
        let toml = r#"
        providers = ["claude"]
        language = "Japanese"

        [[prefix_scripts]]
        url_pattern = "^https://github\\.com/myorg/"
        script = "/path/to/script.sh"
"#;

        let config = Config::from_str(toml).unwrap();

        assert_eq!(config.prefix_scripts.len(), 1);
        assert_eq!(
            config.prefix_scripts[0].url_pattern,
            "^https://github\\.com/myorg/"
        );
        assert_eq!(config.prefix_scripts[0].script, "/path/to/script.sh");
    }

    #[test]
    fn test_parse_config_with_prefix_rules() {
        let toml = r#"
providers = ["gemini"]
language = "Japanese"

[[prefix_rules]]
url_pattern = "github\\.com[:/]myorg/"
prefix_type = "conventional"

[[prefix_rules]]
url_pattern = "^https://gitlab\\.com/"
prefix_type = "bracket"
"#;

        let config = Config::from_str(toml).unwrap();

        assert_eq!(config.prefix_rules.len(), 2);
        assert_eq!(config.prefix_rules[0].url_pattern, "github\\.com[:/]myorg/");
        assert_eq!(config.prefix_rules[0].prefix_type, "conventional");
        assert_eq!(config.prefix_rules[1].url_pattern, "^https://gitlab\\.com/");
        assert_eq!(config.prefix_rules[1].prefix_type, "bracket");
    }

    #[rstest]
    #[case("conventional")]
    #[case("bracket")]
    #[case("colon")]
    #[case("emoji")]
    #[case("plain")]
    #[case("none")]
    fn test_prefix_type_values(#[case] prefix_type: &str) {
        let toml = format!(
            r#"
providers = ["gemini"]
language = "Japanese"

[[prefix_rules]]
url_pattern = "^https://example\\.com/"
prefix_type = "{}"
"#,
            prefix_type
        );

        let config = Config::from_str(&toml).unwrap();
        assert_eq!(config.prefix_rules[0].prefix_type, prefix_type);
    }

    #[test]
    fn test_parse_full_config() {
        let toml = r#"
providers = ["claude", "gemini", "codex"]
language = "English"

[models]
antigravity = "pro"
codex = "gpt-4"
claude = "opus"

[[prefix_scripts]]
url_pattern = "^git@gitlab\\.example\\.com:"
script = "/opt/scripts/prefix.py"

[[prefix_rules]]
url_pattern = "github\\.com[:/]myorg/"
prefix_type = "conventional"
"#;

        let config = Config::from_str(toml).unwrap();

        assert_eq!(
            config.providers,
            vec![
                ProviderStep::from_provider("claude"),
                ProviderStep::from_provider("gemini"),
                ProviderStep::from_provider("codex"),
            ]
        );
        assert_eq!(config.language, "English");
        assert_eq!(config.models.antigravity, "pro");
        assert_eq!(config.models.codex, "gpt-4");
        assert_eq!(config.models.claude, "opus");
        assert_eq!(config.prefix_scripts.len(), 1);
        assert_eq!(config.prefix_rules.len(), 1);
    }

    #[test]
    fn test_config_serialization() {
        let config = Config::default();
        let serialized = toml::to_string_pretty(&config).unwrap();

        // 再度パースして同じ値になることを確認
        let deserialized: Config = toml::from_str(&serialized).unwrap();

        assert_eq!(config.providers, deserialized.providers);
        assert_eq!(config.language, deserialized.language);
        assert_eq!(config.models.antigravity, deserialized.models.antigravity);
    }

    // ============================================================
    // prefix_type と auto_push のパーステスト
    // ============================================================

    #[test]
    fn test_parse_config_with_prefix_type() {
        let toml = r#"
providers = ["gemini"]
language = "Japanese"
prefix_type = "conventional"
"#;

        let config = Config::from_str(toml).unwrap();

        assert_eq!(config.prefix_type, Some("conventional".to_string()));
    }

    #[test]
    fn test_parse_config_with_auto_push_true() {
        let toml = r#"
providers = ["gemini"]
language = "Japanese"
auto_push = true
"#;

        let config = Config::from_str(toml).unwrap();

        assert_eq!(config.auto_push, Some(true));
    }

    #[test]
    fn test_parse_config_with_auto_push_false() {
        let toml = r#"
providers = ["gemini"]
language = "Japanese"
auto_push = false
"#;

        let config = Config::from_str(toml).unwrap();

        assert_eq!(config.auto_push, Some(false));
    }

    #[test]
    fn test_parse_config_without_prefix_type_and_auto_push() {
        let toml = r#"
providers = ["gemini"]
language = "Japanese"
"#;

        let config = Config::from_str(toml).unwrap();

        assert_eq!(config.prefix_type, None);
        assert_eq!(config.auto_push, None);
    }

    // ============================================================
    // merge_with のテスト
    // ============================================================

    #[test]
    fn test_merge_with_empty_project_config() {
        let mut global = Config {
            providers: vec![
                ProviderStep::from_provider("gemini"),
                ProviderStep::from_provider("claude"),
            ],
            language: "English".to_string(),
            prefix_type: Some("conventional".to_string()),
            auto_push: Some(true),
            ..Default::default()
        };

        // 空の providers を持つプロジェクト設定を作成
        let project = Config {
            providers: Vec::new(),        // 明示的に空にする
            language: default_language(), // デフォルト言語（マージ時に上書きされない）
            ..Default::default()
        };

        global.merge_with(project);

        // プロジェクト設定の providers が空なので、グローバル設定が維持される
        assert_eq!(
            global.providers,
            vec![
                ProviderStep::from_provider("gemini"),
                ProviderStep::from_provider("claude"),
            ]
        );
        assert_eq!(global.language, "English");
        // Option フィールドは None の場合維持される
        assert_eq!(global.prefix_type, Some("conventional".to_string()));
        assert_eq!(global.auto_push, Some(true));
    }

    #[test]
    fn test_merge_with_project_overrides_providers() {
        let mut global = Config {
            providers: vec![
                ProviderStep::from_provider("gemini"),
                ProviderStep::from_provider("claude"),
            ],
            ..Default::default()
        };

        let project = Config {
            providers: vec![ProviderStep::from_provider("codex")],
            ..Default::default()
        };

        global.merge_with(project);

        // プロジェクト設定の providers が完全に置換される
        assert_eq!(global.providers, vec![ProviderStep::from_provider("codex")]);
    }

    #[test]
    fn test_merge_with_project_overrides_language() {
        let mut global = Config {
            language: "English".to_string(),
            ..Default::default()
        };

        let project = Config {
            language: "French".to_string(),
            ..Default::default()
        };

        global.merge_with(project);

        // プロジェクト設定の language が上書きされる
        assert_eq!(global.language, "French");
    }

    #[test]
    fn test_merge_with_project_overrides_prefix_type() {
        let mut global = Config {
            prefix_type: Some("conventional".to_string()),
            ..Default::default()
        };

        let project = Config {
            prefix_type: Some("bracket".to_string()),
            ..Default::default()
        };

        global.merge_with(project);

        // プロジェクト設定の prefix_type が上書きされる
        assert_eq!(global.prefix_type, Some("bracket".to_string()));
    }

    #[test]
    fn test_merge_with_project_overrides_auto_push() {
        let mut global = Config {
            auto_push: Some(true),
            ..Default::default()
        };

        let project = Config {
            auto_push: Some(false),
            ..Default::default()
        };

        global.merge_with(project);

        // プロジェクト設定の auto_push が上書きされる
        assert_eq!(global.auto_push, Some(false));
    }

    #[test]
    fn test_merge_with_project_none_preserves_global() {
        let mut global = Config {
            prefix_type: Some("conventional".to_string()),
            auto_push: Some(true),
            ..Default::default()
        };

        let project = Config::default();
        // project.prefix_type と project.auto_push は None

        global.merge_with(project);

        // グローバル設定が維持される
        assert_eq!(global.prefix_type, Some("conventional".to_string()));
        assert_eq!(global.auto_push, Some(true));
    }

    #[test]
    fn test_merge_with_models_override() {
        let mut global = Config::default();

        let project = Config {
            models: ModelsConfig {
                antigravity: "pro".to_string(),
                claude: "opus".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };

        global.merge_with(project);

        // プロジェクト設定のモデルが上書きされる
        assert_eq!(global.models.antigravity, "pro");
        assert_eq!(global.models.claude, "opus");
        // 変更されていないモデルはデフォルトのまま
        assert_eq!(global.models.codex, default_codex_model());
        assert_eq!(global.models.opencode, "");
    }

    #[test]
    fn test_merge_with_prefix_rules_override() {
        let mut global = Config {
            prefix_rules: vec![PrefixRuleConfig {
                url_pattern: "github.com".to_string(),
                prefix_type: "conventional".to_string(),
            }],
            ..Default::default()
        };

        let project = Config {
            prefix_rules: vec![PrefixRuleConfig {
                url_pattern: "gitlab.com".to_string(),
                prefix_type: "bracket".to_string(),
            }],
            ..Default::default()
        };

        global.merge_with(project);

        // プロジェクト設定の prefix_rules で完全に置換される
        assert_eq!(global.prefix_rules.len(), 1);
        assert_eq!(global.prefix_rules[0].url_pattern, "gitlab.com");
        assert_eq!(global.prefix_rules[0].prefix_type, "bracket");
    }

    #[test]
    fn test_merge_with_cooldown_override() {
        let mut global = Config {
            provider_cooldown_minutes: 60,
            ..Default::default()
        };

        let project = Config {
            provider_cooldown_minutes: 30,
            ..Default::default()
        };

        global.merge_with(project);

        // プロジェクト設定のクールダウンが上書きされる
        assert_eq!(global.provider_cooldown_minutes, 30);
    }

    #[test]
    fn test_merge_with_timeout_override() {
        let mut global = Config {
            provider_timeout_seconds: 60,
            ..Default::default()
        };

        let project = Config {
            provider_timeout_seconds: 10,
            ..Default::default()
        };

        global.merge_with(project);

        // プロジェクト設定のタイムアウトが上書きされる
        assert_eq!(global.provider_timeout_seconds, 10);
    }

    #[test]
    fn test_parse_config_with_timeout() {
        let toml = r#"
providers = ["gemini"]
language = "Japanese"
provider_timeout_seconds = 60
"#;

        let config = Config::from_str(toml).unwrap();

        assert_eq!(config.provider_timeout_seconds, 60);
    }

    #[test]
    fn test_merge_with_full_project_config() {
        let global_toml = r#"
providers = ["gemini", "claude"]
language = "English"
prefix_type = "conventional"
auto_push = true
provider_cooldown_minutes = 60

[models]
antigravity = "gemini-2.5-flash-lite"
codex = "gpt-5.4-mini"
claude = "haiku"
"#;

        // 言語は "French" を使用（"Japanese" はデフォルトなので上書きされない）
        let project_toml = r#"
providers = ["codex"]
language = "French"
prefix_type = "bracket"
auto_push = false
provider_cooldown_minutes = 15

[models]
antigravity = "pro"
codex = "gpt-5.4-mini"
claude = "haiku"
"#;

        let mut global = Config::from_str(global_toml).unwrap();
        let project = Config::from_str(project_toml).unwrap();

        global.merge_with(project);

        // すべてのフィールドがプロジェクト設定で上書きされる
        assert_eq!(global.providers, vec![ProviderStep::from_provider("codex")]);
        assert_eq!(global.language, "French");
        assert_eq!(global.prefix_type, Some("bracket".to_string()));
        assert_eq!(global.auto_push, Some(false));
        assert_eq!(global.provider_cooldown_minutes, 15);
        assert_eq!(global.models.antigravity, "pro");
        // claude は変更されていないのでグローバル設定のまま（両方 haiku）
        assert_eq!(global.models.claude, "haiku");
    }

    // ============================================================
    // nano_buddy のテスト
    // ============================================================

    #[test]
    fn test_config_nano_buddy_default_false() {
        let config = Config::from_str("providers = [\"gemini\"]\nlanguage = \"Japanese\"").unwrap();
        assert!(!config.nano_buddy);
    }

    #[test]
    fn test_config_nano_buddy_enabled() {
        let config = Config::from_str(
            "providers = [\"gemini\"]\nlanguage = \"Japanese\"\nnano_buddy = true",
        )
        .unwrap();
        assert!(config.nano_buddy);
    }

    #[test]
    fn test_merge_with_nano_buddy_true_overrides() {
        let mut global = Config::default();
        assert!(!global.nano_buddy);

        let project = Config {
            nano_buddy: true,
            ..Default::default()
        };

        global.merge_with(project);
        assert!(global.nano_buddy);
    }

    #[test]
    fn test_merge_with_nano_buddy_false_preserves() {
        let mut global = Config {
            nano_buddy: true,
            ..Default::default()
        };

        let project = Config::default(); // nano_buddy = false

        global.merge_with(project);
        // `merge_with()` では `false` は `true` を上書きしない
        assert!(global.nano_buddy);
    }

    #[test]
    fn test_merge_with_timeout_zero_overrides() {
        let mut global = Config {
            provider_timeout_seconds: 60,
            ..Default::default()
        };

        let project = Config {
            provider_timeout_seconds: 0,
            ..Default::default()
        };

        global.merge_with(project);
        // 0はデフォルト(60)と異なるため上書きされる
        assert_eq!(global.provider_timeout_seconds, 0);
    }

    #[test]
    fn test_merge_with_cooldown_zero_overrides() {
        let mut global = Config {
            provider_cooldown_minutes: 60,
            ..Default::default()
        };

        let project = Config {
            provider_cooldown_minutes: 0,
            ..Default::default()
        };

        global.merge_with(project);
        // 0はデフォルト(60)と異なるため上書きされる
        assert_eq!(global.provider_cooldown_minutes, 0);
    }

    #[test]
    fn test_from_str_valid() {
        let toml_str = r#"
providers = ["gemini", "claude"]
language = "ja"
"#;
        let config = Config::from_str(toml_str).unwrap();
        assert_eq!(
            config.providers,
            vec![
                ProviderStep::from_provider("gemini"),
                ProviderStep::from_provider("claude"),
            ]
        );
        assert_eq!(config.language, "ja");
    }

    #[test]
    fn test_from_str_invalid_toml() {
        let result = Config::from_str("invalid[[[toml");
        assert!(result.is_err());
    }

    #[test]
    fn test_from_str_minimal() {
        let config = Config::from_str("").unwrap();
        assert_eq!(config.language, "Japanese");
    }

    #[test]
    fn test_config_dir_returns_valid_path() {
        let dir = Config::config_dir();
        assert!(dir.is_some());
        let dir = dir.unwrap();
        assert!(dir.to_str().unwrap().contains("git-sc"));
    }

    #[test]
    fn test_global_config_path_contains_config_toml() {
        let path = Config::global_config_path();
        assert!(path.is_ok());
        let path = path.unwrap();
        assert!(path.to_str().unwrap().ends_with("config.toml"));
    }

    #[test]
    fn test_merge_with_default_language_not_overridden() {
        // プロジェクトの言語が "Japanese"（デフォルト）の場合、グローバルの言語が維持される
        let mut global = Config {
            language: "English".to_string(),
            ..Default::default()
        };

        let project = Config {
            language: "Japanese".to_string(), // デフォルト値
            ..Default::default()
        };

        global.merge_with(project);
        // "Japanese" はデフォルトなので上書きされない
        assert_eq!(global.language, "English");
    }

    #[test]
    fn test_from_str_unknown_fields_ignored() {
        // 未知のフィールドがあってもエラーにならない
        let toml = r#"
providers = ["gemini"]
language = "Japanese"
unknown_field = "some_value"
"#;
        let result = Config::from_str(toml);
        // serde の deny_unknown_fields が設定されていなければ成功する
        // 設定されている場合はエラーになるのでその動作を確認
        // 実際の動作に合わせてアサーション
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_merge_with_opencode_model_override() {
        let mut global = Config::default();
        assert_eq!(global.models.opencode, "");

        let project = Config {
            models: ModelsConfig {
                opencode: "custom-model".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };

        global.merge_with(project);
        assert_eq!(global.models.opencode, "custom-model");
    }

    // ============================================================
    // merge_with: prefix_scripts の上書きテスト
    // ============================================================

    #[test]
    fn test_merge_with_prefix_scripts_override() {
        let global_script = PrefixScriptConfig {
            url_pattern: "global".to_string(),
            script: "global-script.sh".to_string(),
        };
        let project_script = PrefixScriptConfig {
            url_pattern: "project".to_string(),
            script: "project-script.sh".to_string(),
        };

        let mut global = Config {
            prefix_scripts: vec![global_script],
            ..Default::default()
        };

        let project = Config {
            prefix_scripts: vec![project_script],
            ..Default::default()
        };

        global.merge_with(project);
        assert_eq!(global.prefix_scripts.len(), 1);
        assert_eq!(global.prefix_scripts[0].script, "project-script.sh");
    }

    #[test]
    fn test_merge_with_prefix_scripts_empty_preserves_global() {
        let global_script = PrefixScriptConfig {
            url_pattern: "global".to_string(),
            script: "global-script.sh".to_string(),
        };

        let mut global = Config {
            prefix_scripts: vec![global_script],
            ..Default::default()
        };

        let project = Config::default(); // prefix_scripts = []

        global.merge_with(project);
        // 空のプロジェクト設定はグローバルを保持
        assert_eq!(global.prefix_scripts.len(), 1);
        assert_eq!(global.prefix_scripts[0].script, "global-script.sh");
    }

    #[test]
    fn test_merge_with_nano_buddy_true_to_true() {
        let mut global = Config {
            nano_buddy: true,
            ..Default::default()
        };

        let project = Config {
            nano_buddy: true,
            ..Default::default()
        };

        global.merge_with(project);
        assert!(global.nano_buddy);
    }

    // ============================================================
    // merge_with: 各フィールドのマージ動作テスト
    // ============================================================

    #[test]
    fn test_merge_with_providers_override() {
        let mut global = Config::default();
        let project = Config {
            providers: vec![ProviderStep::from_provider("claude")],
            ..Default::default()
        };

        global.merge_with(project);
        assert_eq!(
            global.providers,
            vec![ProviderStep::from_provider("claude")]
        );
    }

    #[test]
    fn test_merge_with_empty_providers_preserves_global() {
        let mut global = Config::default();
        let original_providers = global.providers.clone();

        let project = Config {
            providers: vec![],
            ..Default::default()
        };

        global.merge_with(project);
        assert_eq!(global.providers, original_providers);
    }

    #[test]
    fn test_merge_with_language_non_default_overrides() {
        let mut global = Config::default();
        let project = Config {
            language: "English".to_string(),
            ..Default::default()
        };

        global.merge_with(project);
        assert_eq!(global.language, "English");
    }

    #[test]
    fn test_merge_with_language_default_preserves_global() {
        let mut global = Config {
            language: "English".to_string(),
            ..Default::default()
        };

        // プロジェクト設定がデフォルト言語（Japanese）の場合、グローバルを保持
        let project = Config::default();
        global.merge_with(project);
        assert_eq!(global.language, "English");
    }

    #[test]
    fn test_merge_with_prefix_type_some_overrides() {
        let mut global = Config::default();
        assert!(global.prefix_type.is_none());

        let project = Config {
            prefix_type: Some("bracket".to_string()),
            ..Default::default()
        };

        global.merge_with(project);
        assert_eq!(global.prefix_type, Some("bracket".to_string()));
    }

    #[test]
    fn test_merge_with_prefix_type_none_preserves_global() {
        let mut global = Config {
            prefix_type: Some("conventional".to_string()),
            ..Default::default()
        };

        let project = Config {
            prefix_type: None,
            ..Default::default()
        };

        global.merge_with(project);
        assert_eq!(global.prefix_type, Some("conventional".to_string()));
    }

    #[test]
    fn test_merge_with_auto_push_some_overrides() {
        let mut global = Config {
            auto_push: Some(false),
            ..Default::default()
        };

        let project = Config {
            auto_push: Some(true),
            ..Default::default()
        };

        global.merge_with(project);
        assert_eq!(global.auto_push, Some(true));
    }

    #[test]
    fn test_merge_with_cooldown_non_default_overrides() {
        let mut global = Config::default();
        assert_eq!(global.provider_cooldown_minutes, 60);

        let project = Config {
            provider_cooldown_minutes: 30,
            ..Default::default()
        };

        global.merge_with(project);
        assert_eq!(global.provider_cooldown_minutes, 30);
    }

    #[test]
    fn test_merge_with_cooldown_default_preserves_global() {
        let mut global = Config {
            provider_cooldown_minutes: 30,
            ..Default::default()
        };

        // デフォルト（60）はグローバルの30を上書きしない
        let project = Config::default();
        global.merge_with(project);
        assert_eq!(global.provider_cooldown_minutes, 30);
    }

    #[test]
    fn test_merge_with_timeout_non_default_overrides() {
        let mut global = Config::default();
        assert_eq!(global.provider_timeout_seconds, 60);

        let project = Config {
            provider_timeout_seconds: 120,
            ..Default::default()
        };

        global.merge_with(project);
        assert_eq!(global.provider_timeout_seconds, 120);
    }

    #[test]
    fn test_merge_with_models_partial_override() {
        let mut global = Config::default();

        let project = Config {
            models: ModelsConfig {
                antigravity: "gemini-2.5-pro".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };

        global.merge_with(project);
        // antigravity のみ上書きされる
        assert_eq!(global.models.antigravity, "gemini-2.5-pro");
        // 他はデフォルトのまま
        assert_eq!(global.models.codex, default_codex_model());
        assert_eq!(global.models.claude, "haiku");
    }

    #[test]
    fn test_merge_with_nano_buddy_false_preserves_global_true() {
        let mut global = Config {
            nano_buddy: true,
            ..Default::default()
        };

        let project = Config {
            nano_buddy: false,
            ..Default::default()
        };

        // nano_buddy は true で上書きのみなので、false では上書きされない
        global.merge_with(project);
        assert!(global.nano_buddy);
    }

    // ============================================================
    // Config::from_str: パースエッジケーステスト
    // ============================================================

    #[test]
    fn test_parse_empty_config_uses_defaults() {
        let config = Config::from_str("").unwrap();
        // 全てデフォルト値
        assert!(config.providers.is_empty());
        assert_eq!(config.language, "Japanese");
        assert_eq!(config.provider_cooldown_minutes, 60);
    }

    #[test]
    fn test_parse_config_with_multiple_prefix_rules() {
        let toml = r#"
            [[prefix_rules]]
            url_pattern = "github\\.com[:/]myorg/"
            prefix_type = "conventional"

            [[prefix_rules]]
            url_pattern = "gitlab\\.com"
            prefix_type = "bracket"
        "#;

        let config = Config::from_str(toml).unwrap();
        assert_eq!(config.prefix_rules.len(), 2);
        assert_eq!(config.prefix_rules[0].prefix_type, "conventional");
        assert_eq!(config.prefix_rules[1].prefix_type, "bracket");
    }

    #[test]
    fn test_parse_config_cooldown_zero_disables() {
        let toml = r#"
            provider_cooldown_minutes = 0
        "#;

        let config = Config::from_str(toml).unwrap();
        assert_eq!(config.provider_cooldown_minutes, 0);
    }

    #[test]
    fn test_parse_invalid_toml_returns_error() {
        let result = Config::from_str("invalid [[ toml");
        assert!(result.is_err());
    }

    // ============================================================
    // PartialConfig / PartialModelsConfig のテスト
    // ============================================================

    /// [models] セクションに gemini のみ指定した場合、
    /// 以前は "missing field codex" エラーが発生していたが、
    /// serde(default) 付与により正しくパースできることを確認する。
    #[test]
    fn test_partial_models_config_only_gemini_parses() {
        let toml = r#"
[models]
gemini = "gemini-2.5-pro"
"#;
        // PartialConfig として直接デシリアライズ
        let partial: PartialConfig =
            toml::from_str(toml).expect("[models] に gemini のみ指定してもパース成功すべき");

        assert_eq!(partial.models.gemini, Some("gemini-2.5-pro".to_string()));
        // 未指定フィールドは None のまま
        assert!(partial.models.codex.is_none());
        assert!(partial.models.claude.is_none());
        assert!(partial.models.opencode.is_none());
    }

    /// プロジェクト設定の language が "Japanese"（デフォルト値と同じ）であっても、
    /// PartialConfig::merge_into() はフィールドが明示的に指定されている場合は上書きする。
    /// グローバルが "English" → プロジェクトが "Japanese" → 最終的に "Japanese" になる。
    #[test]
    fn test_partial_merge_language_default_value_overrides_global() {
        // グローバル設定: language = "English"
        let global_toml = r#"language = "English""#;
        let mut global: Config = toml::from_str(global_toml).unwrap();
        assert_eq!(global.language, "English");

        // プロジェクト設定: language = "Japanese"（デフォルト値と同じだが明示的に指定）
        let project_toml = r#"language = "Japanese""#;
        let project_partial: PartialConfig = toml::from_str(project_toml).unwrap();

        // merge_into でグローバル設定を上書き
        project_partial.merge_into(&mut global);

        // PartialConfig::merge_into() は Some("Japanese") を検出して上書きするため "Japanese" になる
        assert_eq!(global.language, "Japanese");
    }

    /// プロジェクト設定の nano_buddy = false が、
    /// グローバルの nano_buddy = true を正しく上書きすることを確認する。
    #[test]
    fn test_partial_merge_nano_buddy_false_overrides_global_true() {
        // グローバル設定: nano_buddy = true
        let mut global = Config {
            nano_buddy: true,
            ..Default::default()
        };

        // プロジェクト設定: nano_buddy = false を明示的に指定
        let project_toml = r#"nano_buddy = false"#;
        let project_partial: PartialConfig = toml::from_str(project_toml).unwrap();
        assert_eq!(project_partial.nano_buddy, Some(false));

        // merge_into でグローバル設定を上書き
        project_partial.merge_into(&mut global);

        // PartialConfig::merge_into() は Some(false) を検出して false に上書きする
        assert!(!global.nano_buddy);
    }

    /// プロジェクト設定の provider_cooldown_minutes = 60（デフォルト値）が、
    /// グローバルの provider_cooldown_minutes = 5 を正しく上書きすることを確認する。
    #[test]
    fn test_partial_merge_cooldown_default_value_overrides_global_non_default() {
        // グローバル設定: provider_cooldown_minutes = 5（非デフォルト値）
        let mut global = Config {
            provider_cooldown_minutes: 5,
            ..Default::default()
        };

        // プロジェクト設定: provider_cooldown_minutes = 60（デフォルト値と同じだが明示的に指定）
        let project_toml = r#"provider_cooldown_minutes = 60"#;
        let project_partial: PartialConfig = toml::from_str(project_toml).unwrap();
        assert_eq!(project_partial.provider_cooldown_minutes, Some(60));

        // merge_into でグローバル設定を上書き
        project_partial.merge_into(&mut global);

        // PartialConfig::merge_into() は Some(60) を検出してデフォルト値であっても上書きする
        assert_eq!(global.provider_cooldown_minutes, 60);
    }

    // ============================================================
    // PartialConfig::merge_into() の直接テスト
    // ============================================================

    /// 全フィールドが None の PartialConfig を merge_into しても元の Config が保持される
    #[test]
    fn test_partial_merge_into_all_none_preserves_config() {
        let mut config = Config {
            providers: vec![ProviderStep::from_provider("claude")],
            language: "English".to_string(),
            models: ModelsConfig {
                antigravity: "pro".to_string(),
                codex: "gpt-5".to_string(),
                claude: "opus".to_string(),
                opencode: "custom".to_string(),
                grok: "grok-custom".to_string(),
            },
            prefix_scripts: vec![PrefixScriptConfig {
                url_pattern: "github".to_string(),
                script: "test.sh".to_string(),
            }],
            prefix_rules: vec![PrefixRuleConfig {
                url_pattern: "gitlab".to_string(),
                prefix_type: "bracket".to_string(),
            }],
            provider_cooldown_minutes: 30,
            prefix_type: Some("conventional".to_string()),
            auto_push: Some(true),
            provider_timeout_seconds: 120,
            nano_buddy: true,
            codex_reasoning_effort: "high".to_string(),
            ai_usage: None,
        };

        // 空の TOML → 全フィールドが None の PartialConfig
        let partial: PartialConfig = toml::from_str("").unwrap();
        partial.merge_into(&mut config);

        // 全フィールドが元の値のまま保持される
        assert_eq!(
            config.providers,
            vec![ProviderStep::from_provider("claude")]
        );
        assert_eq!(config.language, "English");
        assert_eq!(config.models.antigravity, "pro");
        assert_eq!(config.models.codex, "gpt-5");
        assert_eq!(config.models.claude, "opus");
        assert_eq!(config.models.opencode, "custom");
        assert_eq!(config.prefix_scripts.len(), 1);
        assert_eq!(config.prefix_scripts[0].script, "test.sh");
        assert_eq!(config.prefix_rules.len(), 1);
        assert_eq!(config.prefix_rules[0].prefix_type, "bracket");
        assert_eq!(config.provider_cooldown_minutes, 30);
        assert_eq!(config.prefix_type, Some("conventional".to_string()));
        assert_eq!(config.auto_push, Some(true));
        assert_eq!(config.provider_timeout_seconds, 120);
        assert!(config.nano_buddy);
        assert_eq!(config.codex_reasoning_effort, "high");
    }

    /// 全フィールドが Some の PartialConfig ですべてのフィールドが上書きされる
    #[test]
    fn test_partial_merge_into_all_some_overrides_everything() {
        let mut config = Config::default();

        let toml = r#"
providers = ["codex", "claude"]
language = "English"
provider_cooldown_minutes = 15
prefix_type = "bracket"
auto_push = true
provider_timeout_seconds = 90
nano_buddy = true

[models]
antigravity = "gemini-2.5-pro"
codex = "gpt-5"
claude = "opus"
opencode = "custom-model"

[[prefix_scripts]]
url_pattern = "^https://github\\.com/"
script = "scripts/prefix.sh"

[[prefix_rules]]
url_pattern = "gitlab\\.com"
prefix_type = "conventional"
"#;

        let partial: PartialConfig = toml::from_str(toml).unwrap();
        partial.merge_into(&mut config);

        assert_eq!(
            config.providers,
            vec![
                ProviderStep::from_provider("codex"),
                ProviderStep::from_provider("claude"),
            ]
        );
        assert_eq!(config.language, "English");
        assert_eq!(config.models.antigravity, "gemini-2.5-pro");
        assert_eq!(config.models.codex, "gpt-5");
        assert_eq!(config.models.claude, "opus");
        assert_eq!(config.models.opencode, "custom-model");
        assert_eq!(config.prefix_scripts.len(), 1);
        assert_eq!(config.prefix_scripts[0].script, "scripts/prefix.sh");
        assert_eq!(config.prefix_rules.len(), 1);
        assert_eq!(config.prefix_rules[0].prefix_type, "conventional");
        assert_eq!(config.provider_cooldown_minutes, 15);
        assert_eq!(config.prefix_type, Some("bracket".to_string()));
        assert_eq!(config.auto_push, Some(true));
        assert_eq!(config.provider_timeout_seconds, 90);
        assert!(config.nano_buddy);
    }

    /// providers が空配列の場合、上書きされない（空チェックがある）
    #[test]
    fn test_partial_merge_into_empty_providers_not_overridden() {
        let mut config = Config {
            providers: vec![
                ProviderStep::from_provider("gemini"),
                ProviderStep::from_provider("claude"),
            ],
            ..Default::default()
        };

        let toml = r#"providers = []"#;
        let partial: PartialConfig = toml::from_str(toml).unwrap();
        partial.merge_into(&mut config);

        // providers = [] は空なのでマージされず、元の値が保持される
        assert_eq!(
            config.providers,
            vec![
                ProviderStep::from_provider("gemini"),
                ProviderStep::from_provider("claude"),
            ]
        );
    }

    /// prefix_scripts が空配列の場合、上書きされない（空チェックがある）
    #[test]
    fn test_partial_merge_into_empty_prefix_scripts_not_overridden() {
        let mut config = Config {
            prefix_scripts: vec![PrefixScriptConfig {
                url_pattern: "github".to_string(),
                script: "original.sh".to_string(),
            }],
            ..Default::default()
        };

        // TOML の空配列で PartialConfig を作成
        let toml = r#"prefix_scripts = []"#;
        let partial: PartialConfig = toml::from_str(toml).unwrap();
        partial.merge_into(&mut config);

        // prefix_scripts = [] は空なのでマージされず、元の値が保持される
        assert_eq!(config.prefix_scripts.len(), 1);
        assert_eq!(config.prefix_scripts[0].script, "original.sh");
    }

    /// ModelsConfig の部分マージ: 旧 `gemini` キーは `antigravity` に昇格し、他のモデルは保持される
    #[test]
    fn test_partial_merge_into_legacy_gemini_promotes_to_antigravity() {
        let mut config = Config {
            models: ModelsConfig {
                antigravity: "old-antigravity".to_string(),
                codex: "old-codex".to_string(),
                claude: "old-claude".to_string(),
                opencode: "old-opencode".to_string(),
                grok: "old-grok".to_string(),
            },
            ..Default::default()
        };

        // 旧 git-sc が書いた legacy `gemini` キーのみを含むプロジェクト設定
        let toml = r#"
[models]
gemini = "gemini-2.5-pro"
"#;
        let partial: PartialConfig = toml::from_str(toml).unwrap();
        partial.merge_into(&mut config);

        // legacy `gemini` が antigravity に昇格して上書きされる
        assert_eq!(config.models.antigravity, "gemini-2.5-pro");
        // 他のモデルは元の値が保持される
        assert_eq!(config.models.codex, "old-codex");
        assert_eq!(config.models.claude, "old-claude");
        assert_eq!(config.models.opencode, "old-opencode");
    }

    /// `[models] antigravity` と legacy `gemini` が両方指定された場合、
    /// 明示的な `antigravity` 値が勝つ(不変条件: 新キーは旧エイリアスに優先する)。
    #[test]
    fn test_partial_merge_into_explicit_antigravity_wins_over_legacy_gemini() {
        let mut config = Config {
            models: ModelsConfig {
                antigravity: "old-antigravity".to_string(),
                codex: "old-codex".to_string(),
                claude: "old-claude".to_string(),
                opencode: "old-opencode".to_string(),
                grok: "old-grok".to_string(),
            },
            ..Default::default()
        };

        // 両方のキーが存在するプロジェクト設定
        let toml = r#"
[models]
antigravity = "new-antigravity-explicit"
gemini = "legacy-gemini-value"
"#;
        let partial: PartialConfig = toml::from_str(toml).unwrap();
        partial.merge_into(&mut config);

        // 明示的な antigravity 値が勝ち、legacy gemini は無視される
        assert_eq!(config.models.antigravity, "new-antigravity-explicit");
    }

    /// nano_buddy の true → false 上書き: Some(false) で上書きできることの確認
    #[test]
    fn test_partial_merge_into_nano_buddy_true_to_false() {
        let mut config = Config {
            nano_buddy: true,
            ..Default::default()
        };

        let toml = r#"nano_buddy = false"#;
        let partial: PartialConfig = toml::from_str(toml).unwrap();
        partial.merge_into(&mut config);

        // merge_into は Some(false) を検出して false に上書きする
        // （merge_with とは異なり、PartialConfig は Option で管理するため正確に動作する）
        assert!(!config.nano_buddy);
    }

    /// provider_timeout_seconds の上書き: デフォルト値(60)と異なる値で上書き
    #[test]
    fn test_partial_merge_into_provider_timeout_seconds() {
        let mut config = Config {
            provider_timeout_seconds: 60,
            ..Default::default()
        };

        let toml = r#"provider_timeout_seconds = 180"#;
        let partial: PartialConfig = toml::from_str(toml).unwrap();
        partial.merge_into(&mut config);

        assert_eq!(config.provider_timeout_seconds, 180);
    }

    /// language の上書き: "English" で上書き
    #[test]
    fn test_partial_merge_into_language_override() {
        let mut config = Config {
            language: "Japanese".to_string(),
            ..Default::default()
        };

        let toml = r#"language = "English""#;
        let partial: PartialConfig = toml::from_str(toml).unwrap();
        partial.merge_into(&mut config);

        assert_eq!(config.language, "English");
    }

    // ============================================================
    // default_config_content のテスト
    // ============================================================

    #[test]
    fn test_default_config_content_is_valid_toml() {
        // デフォルト設定内容が有効な TOML として解析できる
        let content = Config::default_config_content();
        let result: Result<PartialConfig, _> = toml::from_str(&content);
        assert!(
            result.is_ok(),
            "default_config_content は有効な TOML であるべき: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_default_config_content_contains_required_fields() {
        // 必須フィールドが含まれている
        let content = Config::default_config_content();
        assert!(content.contains("providers"));
        assert!(content.contains("language"));
        assert!(content.contains("provider_cooldown_minutes"));
        assert!(content.contains("provider_timeout_seconds"));
        assert!(content.contains("[models]"));
    }

    #[test]
    fn test_default_config_content_providers_not_empty() {
        // providers が空でない
        let content = Config::default_config_content();
        let partial: PartialConfig = toml::from_str(&content).unwrap();
        assert!(
            partial.providers.is_some_and(|p| !p.is_empty()),
            "providers は空であってはならない"
        );
    }

    #[test]
    fn test_default_config_content_uses_current_default_models() {
        // init で生成する設定テンプレートがコード上の既定モデルとずれないことを確認する。
        let content = Config::default_config_content();
        let config = Config::from_str(&content).unwrap();

        assert_eq!(config.models.antigravity, default_antigravity_model());
        assert_eq!(config.models.codex, default_codex_model());
        assert!(content.contains(r#"antigravity = "GPT-OSS 120B (Medium)""#));
        assert!(content.contains(r#"codex = "gpt-5.4-mini""#));
        assert_eq!(config.models.claude, default_claude_model());
        assert_eq!(config.models.opencode, default_opencode_model());
    }

    #[test]
    fn test_readme_examples_use_current_codex_default_model() {
        // README の設定例もコード上の既定モデルと同じ値を示す必要がある。
        let expected = format!(r#"codex = "{}""#, default_codex_model());

        assert!(include_str!("../README.md").contains(&expected));
        assert!(include_str!("../README.ja.md").contains(&expected));
    }

    #[test]
    fn test_readme_examples_use_current_antigravity_default_model() {
        // README の設定例も Antigravity の既定モデルと同期させる。
        let expected = format!(r#"antigravity = "{}""#, default_antigravity_model());

        assert!(include_str!("../README.md").contains(&expected));
        assert!(include_str!("../README.ja.md").contains(&expected));
    }

    #[test]
    fn test_agents_notes_use_current_codex_default_model() {
        // 運用メモの Codex 既定モデル説明もコード上の既定値と同期する。
        let expected = format!("The default Codex model is `{}`", default_codex_model());

        assert!(include_str!("../AGENTS.md").contains(&expected));
    }

    #[test]
    fn test_agents_notes_use_current_antigravity_default_model() {
        // 運用メモの Antigravity 既定モデル説明もコード上の既定値と同期する。
        let expected = format!(
            "The default Antigravity model is `{}`",
            default_antigravity_model()
        );

        assert!(include_str!("../AGENTS.md").contains(&expected));
    }

    #[test]
    fn test_default_config_content_has_japanese_comments() {
        // 生成される設定ファイルの説明コメントは日本語で維持する。
        let content = Config::default_config_content();

        assert!(content.contains("# AI プロバイダーの優先順"));
        assert!(content.contains("# 各プロバイダーのモデル設定"));
        assert!(!content.contains("# AI providers priority order"));
        assert!(!content.contains("# Model configuration for each provider"));
    }

    // ============================================================
    // PartialConfig::merge_into: 空コレクションのエッジケース
    // ============================================================

    #[test]
    fn test_partial_merge_empty_providers_does_not_clear_global() {
        // providers = [] はグローバルのprovidersをクリアしない
        let mut config = Config {
            providers: vec![
                ProviderStep::from_provider("gemini"),
                ProviderStep::from_provider("claude"),
            ],
            ..Default::default()
        };

        let toml = r#"providers = []"#;
        let partial: PartialConfig = toml::from_str(toml).unwrap();
        partial.merge_into(&mut config);

        assert_eq!(
            config.providers,
            vec![
                ProviderStep::from_provider("gemini"),
                ProviderStep::from_provider("claude"),
            ]
        );
    }

    #[test]
    fn test_partial_merge_empty_prefix_scripts_does_not_clear_global() {
        // prefix_scripts を持つグローバル設定がプロジェクト設定で上書きされる
        let mut config = Config {
            prefix_scripts: vec![PrefixScriptConfig {
                url_pattern: ".*".to_string(),
                script: "test.sh".to_string(),
            }],
            ..Default::default()
        };

        let toml = "[[prefix_scripts]]\nurl_pattern = \"new\"\nscript = \"new.sh\"";
        let partial: PartialConfig = toml::from_str(toml).unwrap();
        partial.merge_into(&mut config);

        assert_eq!(config.prefix_scripts.len(), 1);
        assert_eq!(config.prefix_scripts[0].script, "new.sh");
    }

    // ============================================================
    // ProviderStep: string-or-table パース
    // ============================================================

    #[test]
    fn test_provider_step_parse_plain_string() {
        let config = Config::from_str(r#"providers = ["codex", "claude"]"#).unwrap();
        assert_eq!(
            config.providers,
            vec![
                ProviderStep::from_provider("codex"),
                ProviderStep::from_provider("claude"),
            ]
        );
    }

    #[test]
    fn test_provider_step_parse_table() {
        let toml = r#"
providers = [
  { provider = "codex", model = "gpt-5.4-mini", env = { CODEX_HOME = "/a" } },
]
"#;
        let config = Config::from_str(toml).unwrap();
        assert_eq!(config.providers.len(), 1);
        let s = &config.providers[0];
        assert_eq!(s.provider, "codex");
        assert_eq!(s.model.as_deref(), Some("gpt-5.4-mini"));
        assert_eq!(s.env.get("CODEX_HOME").map(String::as_str), Some("/a"));
    }

    #[test]
    fn test_provider_step_parse_mixed_chain() {
        // 文字列とテーブルの混在(後方互換 + 新形式)
        let toml = r#"
providers = [
  { provider = "codex", env = { CODEX_HOME = "/a" } },
  "antigravity",
]
"#;
        let config = Config::from_str(toml).unwrap();
        assert_eq!(config.providers.len(), 2);
        assert_eq!(config.providers[0].provider, "codex");
        assert!(config.providers[0].env.contains_key("CODEX_HOME"));
        assert_eq!(
            config.providers[1],
            ProviderStep::from_provider("antigravity")
        );
    }

    #[test]
    fn test_provider_step_parse_command_and_name() {
        let toml = r#"
providers = [
  { provider = "codex", command = ["/path/wrapper.sh", "--flag"], name = "acct1" },
]
"#;
        let config = Config::from_str(toml).unwrap();
        let s = &config.providers[0];
        assert_eq!(
            s.command.as_deref(),
            Some(&["/path/wrapper.sh".to_string(), "--flag".to_string()][..])
        );
        assert_eq!(s.name.as_deref(), Some("acct1"));
    }

    #[test]
    fn test_provider_step_missing_provider_is_clear_error() {
        // #[serde(untagged)] を使わないので、provider 欠落時に具体的なエラーが出る
        let err = Config::from_str(r#"providers = [{ model = "x" }]"#).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("provider"),
            "missing field provider のエラーであるべき: {msg}"
        );
    }

    #[test]
    fn test_provider_step_empty_model_is_none() {
        // model = "" は None 扱い(空文字は「未指定」と同義)
        let config =
            Config::from_str(r#"providers = [{ provider = "codex", model = "" }]"#).unwrap();
        assert_eq!(config.providers[0].model, None);
    }

    #[test]
    fn test_provider_step_parse_ai_usage_profile_and_group() {
        // ai-usage 連携用の 2 フィールド。group は「同一 profile でモデル系統ごとに
        // 残量プールが分かれる provider」(Antigravity) を撃ち分けるために使う。
        let toml = r#"
providers = [
  { provider = "antigravity", model = "GPT-OSS 120B (Medium)",
    ai_usage_profile = "Antigravity", ai_usage_group = "Claude&GPT" },
  { provider = "antigravity", model = "Gemini 3.5 Flash (Low)",
    ai_usage_profile = "Antigravity", ai_usage_group = "Gemini" },
]
"#;
        let config = Config::from_str(toml).unwrap();
        assert_eq!(
            config.providers[0].ai_usage_profile.as_deref(),
            Some("Antigravity")
        );
        assert_eq!(
            config.providers[0].ai_usage_group.as_deref(),
            Some("Claude&GPT")
        );
        assert_eq!(
            config.providers[1].ai_usage_group.as_deref(),
            Some("Gemini")
        );
        // 同一 provider + 同一 profile でも model が違えば cooldown キーは独立
        assert_ne!(
            config.providers[0].cooldown_key(),
            config.providers[1].cooldown_key()
        );
    }

    #[test]
    fn test_provider_step_blank_ai_usage_group_is_none() {
        // 空文字/空白のみは「系統を問わない」= None 扱い
        let config = Config::from_str(
            r#"providers = [{ provider = "antigravity", ai_usage_group = "   " }]"#,
        )
        .unwrap();
        assert_eq!(config.providers[0].ai_usage_group, None);
    }

    #[test]
    fn test_provider_step_ai_usage_group_not_in_cooldown_key() {
        // ai_usage_group は残量ゲート側の関心事。cooldown(失敗ゲート)キーには含めない。
        let plain = ProviderStep::from_provider("antigravity");
        let mut grouped = ProviderStep::from_provider("antigravity");
        grouped.ai_usage_group = Some("Gemini".to_string());
        assert_eq!(plain.cooldown_key(), grouped.cooldown_key());
    }

    // ============================================================
    // ProviderStep::cooldown_key
    // ============================================================

    #[test]
    fn test_cooldown_key_env_order_independent() {
        // BTreeMap なので env の挿入順に依存せず決定的
        let mut a = ProviderStep::from_provider("codex");
        a.env.insert("FOO".to_string(), "1".to_string());
        a.env.insert("CODEX_HOME".to_string(), "/x".to_string());
        let mut b = ProviderStep::from_provider("codex");
        b.env.insert("CODEX_HOME".to_string(), "/x".to_string());
        b.env.insert("FOO".to_string(), "1".to_string());
        assert_eq!(a.cooldown_key(), b.cooldown_key());
    }

    #[test]
    fn test_cooldown_key_distinguishes_axes() {
        let base = ProviderStep::from_provider("codex");
        let mut model = ProviderStep::from_provider("codex");
        model.model = Some("m".to_string());
        let mut env = ProviderStep::from_provider("codex");
        env.env.insert("CODEX_HOME".to_string(), "/x".to_string());
        let mut cmd = ProviderStep::from_provider("codex");
        cmd.command = Some(vec!["w".to_string()]);
        assert_ne!(base.cooldown_key(), model.cooldown_key());
        assert_ne!(base.cooldown_key(), env.cooldown_key());
        assert_ne!(base.cooldown_key(), cmd.cooldown_key());
        assert_ne!(model.cooldown_key(), env.cooldown_key());
    }

    #[test]
    fn test_cooldown_key_canonicalizes_provider_alias() {
        // gemini/agy と antigravity は同じクールダウンキー(後方互換)
        assert_eq!(
            ProviderStep::from_provider("gemini").cooldown_key(),
            ProviderStep::from_provider("antigravity").cooldown_key()
        );
        assert_eq!(
            ProviderStep::from_provider("agy").cooldown_key(),
            ProviderStep::from_provider("antigravity").cooldown_key()
        );
    }

    #[test]
    fn test_cooldown_key_canonicalizes_apple_alias() {
        // apple-ai / apple_intelligence と apple-intelligence は同じクールダウンキー(後方互換)。
        // state.rs 側の key_of でも同一視されるが、ProviderStep 直接の呼び出しでも保証する。
        assert_eq!(
            ProviderStep::from_provider("apple-ai").cooldown_key(),
            ProviderStep::from_provider("apple-intelligence").cooldown_key()
        );
        assert_eq!(
            ProviderStep::from_provider("apple_intelligence").cooldown_key(),
            ProviderStep::from_provider("apple-intelligence").cooldown_key()
        );
    }

    #[test]
    fn test_canonical_provider_key_aliases() {
        // canonical_provider_key 自体が後方互換エイリアスを吸収していることを確認する。
        assert_eq!(canonical_provider_key("antigravity"), "antigravity");
        assert_eq!(canonical_provider_key("gemini"), "antigravity");
        assert_eq!(canonical_provider_key("agy"), "antigravity");
        assert_eq!(canonical_provider_key("AGY"), "antigravity"); // 大文字小文字を問わない
        assert_eq!(
            canonical_provider_key("apple-intelligence"),
            "apple-intelligence"
        );
        assert_eq!(canonical_provider_key("apple-ai"), "apple-intelligence");
        assert_eq!(
            canonical_provider_key("apple_intelligence"),
            "apple-intelligence"
        );
        // 関係ないプロバイダーはそのまま小文字化のみ
        assert_eq!(canonical_provider_key("CODEX"), "codex");
        assert_eq!(canonical_provider_key("claude"), "claude");
        assert_eq!(canonical_provider_key("opencode"), "opencode");
    }

    #[test]
    fn test_cooldown_key_name_takes_precedence() {
        let mut a = ProviderStep::from_provider("codex");
        a.name = Some("MyAcct".to_string());
        assert_eq!(a.cooldown_key(), "myacct");
    }

    // ============================================================
    // ProviderStep::account_hint
    // ============================================================

    #[test]
    fn test_account_hint_from_env() {
        let mut s = ProviderStep::from_provider("codex");
        s.env
            .insert("CODEX_HOME".to_string(), "/home/u/.codex-work".to_string());
        assert_eq!(s.account_hint().as_deref(), Some(".codex-work"));
    }

    #[test]
    fn test_account_hint_from_name() {
        let mut s = ProviderStep::from_provider("codex");
        s.name = Some("acct1".to_string());
        assert_eq!(s.account_hint().as_deref(), Some("acct1"));
    }

    #[test]
    fn test_account_hint_none_when_no_env_or_name() {
        assert_eq!(ProviderStep::from_provider("codex").account_hint(), None);
    }

    // ============================================================
    // env キー検証 / ~ 展開
    // ============================================================

    #[test]
    fn test_is_valid_env_key_accepts_posix() {
        assert!(is_valid_env_key("CODEX_HOME"));
        assert!(is_valid_env_key("_X"));
        assert!(is_valid_env_key("A1"));
    }

    #[test]
    fn test_is_valid_env_key_rejects_invalid() {
        assert!(!is_valid_env_key("1BAD"));
        assert!(!is_valid_env_key("A-B"));
        assert!(!is_valid_env_key(""));
        assert!(!is_valid_env_key("A B"));
    }

    #[test]
    fn test_is_valid_env_key_rejects_non_ascii() {
        // is_ascii_alphabetic は ASCII 範囲外の Unicode 文字を拒否する。
        // 環境変数キーは POSIX 仕様で ASCII の英数字・_ のみ許容されるので非 ASCII は受け付けてはならない。
        assert!(!is_valid_env_key("日本語"));
        assert!(!is_valid_env_key("CODEX_日本"));
        assert!(!is_valid_env_key("Ä"));
    }

    #[test]
    fn test_is_valid_env_key_single_char_allowed() {
        // 単独の英字 / _ は POSIX で有効な env キー。
        // chars.all() は空イテレータで true を返すため正しく通る。
        assert!(is_valid_env_key("A"));
        assert!(is_valid_env_key("_"));
        assert!(is_valid_env_key("z"));
    }

    #[test]
    fn test_expand_step_env_expands_tilde() {
        let mut env = BTreeMap::new();
        env.insert("CODEX_HOME".to_string(), "~/.codex".to_string());
        let out = expand_step_env(&env, "test").unwrap();
        let v = out.get("CODEX_HOME").unwrap();
        assert!(v.starts_with('/'), "~ は絶対パスに展開されるべき: {v}");
        assert!(!v.contains('~'));
    }

    #[test]
    fn test_expand_step_env_rejects_invalid_key() {
        let mut env = BTreeMap::new();
        env.insert("BAD KEY".to_string(), "x".to_string());
        let err = expand_step_env(&env, "test").unwrap_err();
        assert!(
            err.to_string()
                .contains("invalid environment variable name"),
            "不正な env キーは設定エラーになるべき"
        );
    }

    #[test]
    fn test_expand_step_env_rejects_dynamic_loader_keys() {
        // 動的ローダー注入経路(LD_PRELOAD/DYLD_INSERT_LIBRARIES 等)は
        // 悪意あるリポジトリの project `.git-sc` から仕込まれた場合に
        // 子プロセス(codex/claude/agy)で任意コード実行に至る。
        // POSIX 識別子としては妥当なキーでも、これらは拒否する必要がある。
        for key in [
            "LD_PRELOAD",
            "LD_LIBRARY_PATH",
            "LD_AUDIT",
            "DYLD_INSERT_LIBRARIES",
            "DYLD_LIBRARY_PATH",
            "DYLD_FALLBACK_LIBRARY_PATH",
            "DYLD_FRAMEWORK_PATH",
            "DYLD_FALLBACK_FRAMEWORK_PATH",
            "DYLD_FORCE_FLAT_NAMESPACE",
            "DYLD_IMAGE_SUFFIX",
            "DYLD_PRINT_LIBRARIES",
            "NODE_OPTIONS",
            "PYTHONPATH",
            "PYTHONSTARTUP",
            "PERL5OPT",
            "PERL5LIB",
            "RUBYOPT",
            "RUBYLIB",
        ] {
            let mut env = BTreeMap::new();
            env.insert(key.to_string(), "/tmp/evil".to_string());
            let err = expand_step_env(&env, "test").unwrap_err().to_string();
            assert!(
                err.contains("dangerous environment variable"),
                "{key} は危険な env キーとして拒否すべき (実エラー: {err})"
            );
        }
    }

    #[test]
    fn test_expand_step_env_rejects_dangerous_keys_case_insensitively() {
        // Windows の環境変数名は大小文字を区別しないため、小文字や混在ケースでも
        // NODE_OPTIONS 等の事前ロード経路を拒否しないと denylist を回避できる。
        for key in ["node_options", "PyThOnPaTh", "dyld_insert_libraries"] {
            let mut env = BTreeMap::new();
            env.insert(key.to_string(), "/tmp/evil".to_string());
            let err = expand_step_env(&env, "test").unwrap_err().to_string();
            assert!(
                err.contains("dangerous environment variable"),
                "{key} は大小文字に関係なく危険な env キーとして拒否すべき (実エラー: {err})"
            );
        }
    }

    #[test]
    fn test_expand_step_env_allows_legitimate_account_keys() {
        // 正当なアカウント切り替え系のキーは引き続き許容される必要がある。
        let mut env = BTreeMap::new();
        env.insert("CODEX_HOME".to_string(), "~/.codex".to_string());
        env.insert("CLAUDE_CONFIG_DIR".to_string(), "~/.claude".to_string());
        env.insert("HOME".to_string(), "~/work-home".to_string());
        env.insert("PATH".to_string(), "/opt/bin:/usr/bin".to_string());
        let out = expand_step_env(&env, "test").unwrap();
        assert_eq!(out.len(), 4);
        assert!(out.contains_key("CODEX_HOME"));
        assert!(out.contains_key("CLAUDE_CONFIG_DIR"));
        assert!(out.contains_key("HOME"));
        assert!(out.contains_key("PATH"));
    }
}
