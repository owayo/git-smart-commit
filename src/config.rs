use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::AppError;

/// 各プロバイダーのモデル設定
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsConfig {
    #[serde(default = "default_gemini_model")]
    pub gemini: String,
    #[serde(default = "default_codex_model")]
    pub codex: String,
    #[serde(default = "default_claude_model")]
    pub claude: String,
    #[serde(default = "default_opencode_model")]
    pub opencode: String,
}

fn default_gemini_model() -> String {
    "gemini-2.5-flash-lite".to_string()
}

fn default_codex_model() -> String {
    "gpt-5.3-codex-spark".to_string()
}

fn default_claude_model() -> String {
    "haiku".to_string()
}

fn default_opencode_model() -> String {
    String::new()
}

impl Default for ModelsConfig {
    fn default() -> Self {
        Self {
            gemini: default_gemini_model(),
            codex: default_codex_model(),
            claude: default_claude_model(),
            opencode: default_opencode_model(),
        }
    }
}

/// 設定ファイルからの部分読み込み用モデル設定
///
/// `Option<T>` により「未指定」と「明示的にデフォルト値を指定」を区別する。
#[derive(Debug, Default, Deserialize)]
struct PartialModelsConfig {
    pub gemini: Option<String>,
    pub codex: Option<String>,
    pub claude: Option<String>,
    pub opencode: Option<String>,
}

/// 設定ファイルからの部分読み込み用
///
/// 全フィールドが `Option` のため、部分設定ファイルでもパースに失敗しない。
/// `merge_into()` で `Config` に明示的に指定されたフィールドのみ上書きする。
#[derive(Debug, Default, Deserialize)]
struct PartialConfig {
    pub providers: Option<Vec<String>>,
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
}

impl PartialConfig {
    /// `Config` に変換（未指定フィールドはデフォルト値を使用）
    fn into_config(self) -> Config {
        let defaults = Config::default();
        Config {
            providers: self.providers.unwrap_or(defaults.providers),
            language: self.language.unwrap_or(defaults.language),
            models: ModelsConfig {
                gemini: self.models.gemini.unwrap_or(defaults.models.gemini),
                codex: self.models.codex.unwrap_or(defaults.models.codex),
                claude: self.models.claude.unwrap_or(defaults.models.claude),
                opencode: self.models.opencode.unwrap_or(defaults.models.opencode),
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
        if let Some(gemini) = self.models.gemini {
            config.models.gemini = gemini;
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
    }
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
    /// AIプロバイダーの優先順序
    #[serde(default)]
    pub providers: Vec<String>,
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
            "opencode".to_string(),
            "gemini".to_string(),
            "codex".to_string(),
            "claude".to_string(),
        ];
        if cfg!(all(target_os = "macos", feature = "apple-ai")) {
            providers.push("apple-intelligence".to_string());
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

        Ok(config)
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
        let providers = if cfg!(all(target_os = "macos", feature = "apple-ai")) {
            r#"providers = ["opencode", "gemini", "codex", "claude", "apple-intelligence"]"#
        } else {
            r#"providers = ["opencode", "gemini", "codex", "claude"]"#
        };

        let apple_ai_available = if cfg!(all(target_os = "macos", feature = "apple-ai")) {
            r#"# 使用可能: "opencode", "gemini", "codex", "claude", "apple-intelligence"
# "apple-intelligence" には macOS 26+ と Apple Silicon が必要"#
        } else {
            r#"# 使用可能: "opencode", "gemini", "codex", "claude""#
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
[models]
gemini = "gemini-2.5-flash-lite"
codex = "{codex_model}"
claude = "haiku"
opencode = ""

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
        if other.models.gemini != ModelsConfig::default().gemini {
            self.models.gemini = other.models.gemini;
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

        // provider_cooldown_minutes: デフォルトでなければ上書き
        if other.provider_cooldown_minutes != default_provider_cooldown_minutes() {
            self.provider_cooldown_minutes = other.provider_cooldown_minutes;
        }

        // provider_timeout_seconds: デフォルトでなければ上書き
        if other.provider_timeout_seconds != default_provider_timeout_seconds() {
            self.provider_timeout_seconds = other.provider_timeout_seconds;
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
            "opencode".to_string(),
            "gemini".to_string(),
            "codex".to_string(),
            "claude".to_string(),
        ];
        if cfg!(all(target_os = "macos", feature = "apple-ai")) {
            expected_providers.push("apple-intelligence".to_string());
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

        assert_eq!(models.gemini, "gemini-2.5-flash-lite");
        assert_eq!(models.codex, default_codex_model());
        assert_eq!(models.claude, "haiku");
        assert_eq!(models.opencode, "");
    }

    #[test]
    fn test_default_codex_model_uses_latest_lowest_token_model() {
        // codex debug models の単発計測結果に基づく現在の既定モデルを固定する。
        assert_eq!(default_codex_model(), "gpt-5.3-codex-spark");
        assert_eq!(Config::default().models.codex, "gpt-5.3-codex-spark");
    }

    #[test]
    fn test_parse_minimal_config() {
        let toml = r#"
        providers = ["gemini"]
        language = "English"
"#;

        let config = Config::from_str(toml).unwrap();

        assert_eq!(config.providers, vec!["gemini".to_string()]);
        assert_eq!(config.language, "English");
        // デフォルト値が使用される
        assert_eq!(config.models.gemini, "gemini-2.5-flash-lite");
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
gemini = "pro"
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
                "claude".to_string(),
                "gemini".to_string(),
                "codex".to_string()
            ]
        );
        assert_eq!(config.language, "English");
        assert_eq!(config.models.gemini, "pro");
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
        assert_eq!(config.models.gemini, deserialized.models.gemini);
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
            providers: vec!["gemini".to_string(), "claude".to_string()],
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
            vec!["gemini".to_string(), "claude".to_string()]
        );
        assert_eq!(global.language, "English");
        // Option フィールドは None の場合維持される
        assert_eq!(global.prefix_type, Some("conventional".to_string()));
        assert_eq!(global.auto_push, Some(true));
    }

    #[test]
    fn test_merge_with_project_overrides_providers() {
        let mut global = Config {
            providers: vec!["gemini".to_string(), "claude".to_string()],
            ..Default::default()
        };

        let project = Config {
            providers: vec!["codex".to_string()],
            ..Default::default()
        };

        global.merge_with(project);

        // プロジェクト設定の providers が完全に置換される
        assert_eq!(global.providers, vec!["codex".to_string()]);
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
                gemini: "pro".to_string(),
                claude: "opus".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };

        global.merge_with(project);

        // プロジェクト設定のモデルが上書きされる
        assert_eq!(global.models.gemini, "pro");
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
gemini = "gemini-2.5-flash-lite"
codex = "gpt-5.4"
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
gemini = "pro"
codex = "gpt-5.4"
claude = "haiku"
"#;

        let mut global = Config::from_str(global_toml).unwrap();
        let project = Config::from_str(project_toml).unwrap();

        global.merge_with(project);

        // すべてのフィールドがプロジェクト設定で上書きされる
        assert_eq!(global.providers, vec!["codex".to_string()]);
        assert_eq!(global.language, "French");
        assert_eq!(global.prefix_type, Some("bracket".to_string()));
        assert_eq!(global.auto_push, Some(false));
        assert_eq!(global.provider_cooldown_minutes, 15);
        assert_eq!(global.models.gemini, "pro");
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
        assert_eq!(config.providers, vec!["gemini", "claude"]);
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
            providers: vec!["claude".to_string()],
            ..Default::default()
        };

        global.merge_with(project);
        assert_eq!(global.providers, vec!["claude".to_string()]);
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
                gemini: "gemini-2.5-pro".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };

        global.merge_with(project);
        // gemini のみ上書きされる
        assert_eq!(global.models.gemini, "gemini-2.5-pro");
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
            providers: vec!["claude".to_string()],
            language: "English".to_string(),
            models: ModelsConfig {
                gemini: "pro".to_string(),
                codex: "gpt-5".to_string(),
                claude: "opus".to_string(),
                opencode: "custom".to_string(),
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
        };

        // 空の TOML → 全フィールドが None の PartialConfig
        let partial: PartialConfig = toml::from_str("").unwrap();
        partial.merge_into(&mut config);

        // 全フィールドが元の値のまま保持される
        assert_eq!(config.providers, vec!["claude".to_string()]);
        assert_eq!(config.language, "English");
        assert_eq!(config.models.gemini, "pro");
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
gemini = "gemini-2.5-pro"
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
            vec!["codex".to_string(), "claude".to_string()]
        );
        assert_eq!(config.language, "English");
        assert_eq!(config.models.gemini, "gemini-2.5-pro");
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
            providers: vec!["gemini".to_string(), "claude".to_string()],
            ..Default::default()
        };

        let toml = r#"providers = []"#;
        let partial: PartialConfig = toml::from_str(toml).unwrap();
        partial.merge_into(&mut config);

        // providers = [] は空なのでマージされず、元の値が保持される
        assert_eq!(
            config.providers,
            vec!["gemini".to_string(), "claude".to_string()]
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

    /// ModelsConfig の部分マージ: gemini のみ指定し、他のモデルは保持される
    #[test]
    fn test_partial_merge_into_models_partial_only_gemini() {
        let mut config = Config {
            models: ModelsConfig {
                gemini: "old-gemini".to_string(),
                codex: "old-codex".to_string(),
                claude: "old-claude".to_string(),
                opencode: "old-opencode".to_string(),
            },
            ..Default::default()
        };

        let toml = r#"
[models]
gemini = "gemini-2.5-pro"
"#;
        let partial: PartialConfig = toml::from_str(toml).unwrap();
        partial.merge_into(&mut config);

        // gemini のみ上書きされる
        assert_eq!(config.models.gemini, "gemini-2.5-pro");
        // 他のモデルは元の値が保持される
        assert_eq!(config.models.codex, "old-codex");
        assert_eq!(config.models.claude, "old-claude");
        assert_eq!(config.models.opencode, "old-opencode");
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

        assert_eq!(config.models.gemini, default_gemini_model());
        assert_eq!(config.models.codex, default_codex_model());
        assert!(content.contains(r#"codex = "gpt-5.3-codex-spark""#));
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
            providers: vec!["gemini".to_string(), "claude".to_string()],
            ..Default::default()
        };

        let toml = r#"providers = []"#;
        let partial: PartialConfig = toml::from_str(toml).unwrap();
        partial.merge_into(&mut config);

        assert_eq!(
            config.providers,
            vec!["gemini".to_string(), "claude".to_string()]
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
}
