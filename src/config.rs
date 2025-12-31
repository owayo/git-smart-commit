use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::AppError;

/// 各プロバイダーのモデル設定
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsConfig {
    pub gemini: String,
    pub codex: String,
    pub claude: String,
}

impl Default for ModelsConfig {
    fn default() -> Self {
        Self {
            gemini: "flash".to_string(),
            codex: "gpt-5.1-codex-mini".to_string(),
            claude: "haiku".to_string(),
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
}

/// デフォルトのクールダウン時間（60分 = 1時間）
fn default_provider_cooldown_minutes() -> u64 {
    60
}

/// デフォルトの言語
fn default_language() -> String {
    "Japanese".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            providers: vec![
                "gemini".to_string(),
                "codex".to_string(),
                "claude".to_string(),
            ],
            language: default_language(),
            models: ModelsConfig::default(),
            prefix_scripts: Vec::new(),
            prefix_rules: Vec::new(),
            provider_cooldown_minutes: default_provider_cooldown_minutes(),
            prefix_type: None,
            auto_push: None,
        }
    }
}

impl Config {
    /// グローバル設定ファイルのパスを取得（~/.git-sc）
    pub fn global_config_path() -> Result<PathBuf, AppError> {
        dirs::home_dir()
            .map(|home| home.join(".git-sc"))
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

    /// グローバル設定を読み込む
    fn load_global() -> Result<Option<Self>, AppError> {
        let path = Self::global_config_path()?;

        if !path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&path)
            .map_err(|e| AppError::ConfigError(format!("Failed to read global config: {}", e)))?;

        match toml::from_str(&content) {
            Ok(config) => Ok(Some(config)),
            Err(e) => {
                eprintln!(
                    "警告: グローバル設定ファイルの構文エラー ({}): {}",
                    path.display(),
                    e
                );
                Ok(None)
            }
        }
    }

    /// プロジェクト設定を読み込む
    fn load_project() -> Result<Option<Self>, AppError> {
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

    /// 2つの設定をマージ（other が優先）
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

        // provider_cooldown_minutes: デフォルトでなければ上書き
        if other.provider_cooldown_minutes != default_provider_cooldown_minutes() {
            self.provider_cooldown_minutes = other.provider_cooldown_minutes;
        }
    }

    /// 階層的に設定を読み込む（グローバル → プロジェクトでマージ）
    pub fn load() -> Result<Self, AppError> {
        // 1. グローバル設定を読み込む
        let mut config = match Self::load_global()? {
            Some(c) => c,
            None => {
                // グローバル設定が存在しない場合はデフォルトを作成
                let config = Config::default();
                config.save()?;
                config
            }
        };

        // 2. プロジェクト設定を読み込んでマージ
        if let Some(project_config) = Self::load_project()? {
            config.merge_with(project_config);
        }

        Ok(config)
    }

    /// 設定をファイルに保存
    pub fn save(&self) -> Result<(), AppError> {
        let path = Self::global_config_path()?;

        let content = toml::to_string_pretty(self)
            .map_err(|e| AppError::ConfigError(format!("Failed to serialize config: {}", e)))?;

        fs::write(&path, content)
            .map_err(|e| AppError::ConfigError(format!("Failed to write config: {}", e)))?;

        Ok(())
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    #[test]
    fn test_default_config() {
        let config = Config::default();

        assert_eq!(
            config.providers,
            vec![
                "gemini".to_string(),
                "codex".to_string(),
                "claude".to_string()
            ]
        );
        assert_eq!(config.language, "Japanese");
        assert!(config.prefix_scripts.is_empty());
        assert!(config.prefix_rules.is_empty());
        assert_eq!(config.provider_cooldown_minutes, 60);
    }

    #[test]
    fn test_default_models_config() {
        let models = ModelsConfig::default();

        assert_eq!(models.gemini, "flash");
        assert_eq!(models.codex, "gpt-5.1-codex-mini");
        assert_eq!(models.claude, "haiku");
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
        assert_eq!(config.models.gemini, "flash");
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
        let mut global = Config::default();
        global.providers = vec!["gemini".to_string(), "claude".to_string()];
        global.language = "English".to_string();
        global.prefix_type = Some("conventional".to_string());
        global.auto_push = Some(true);

        // 空の providers を持つプロジェクト設定を作成
        let mut project = Config::default();
        project.providers = Vec::new(); // 明示的に空にする
        project.language = default_language(); // デフォルト言語（マージ時に上書きされない）

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
        let mut global = Config::default();
        global.providers = vec!["gemini".to_string(), "claude".to_string()];

        let mut project = Config::default();
        project.providers = vec!["codex".to_string()];

        global.merge_with(project);

        // プロジェクト設定の providers が完全に置換される
        assert_eq!(global.providers, vec!["codex".to_string()]);
    }

    #[test]
    fn test_merge_with_project_overrides_language() {
        let mut global = Config::default();
        global.language = "English".to_string();

        let mut project = Config::default();
        project.language = "French".to_string();

        global.merge_with(project);

        // プロジェクト設定の language が上書きされる
        assert_eq!(global.language, "French");
    }

    #[test]
    fn test_merge_with_project_overrides_prefix_type() {
        let mut global = Config::default();
        global.prefix_type = Some("conventional".to_string());

        let mut project = Config::default();
        project.prefix_type = Some("bracket".to_string());

        global.merge_with(project);

        // プロジェクト設定の prefix_type が上書きされる
        assert_eq!(global.prefix_type, Some("bracket".to_string()));
    }

    #[test]
    fn test_merge_with_project_overrides_auto_push() {
        let mut global = Config::default();
        global.auto_push = Some(true);

        let mut project = Config::default();
        project.auto_push = Some(false);

        global.merge_with(project);

        // プロジェクト設定の auto_push が上書きされる
        assert_eq!(global.auto_push, Some(false));
    }

    #[test]
    fn test_merge_with_project_none_preserves_global() {
        let mut global = Config::default();
        global.prefix_type = Some("conventional".to_string());
        global.auto_push = Some(true);

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

        let mut project = Config::default();
        project.models.gemini = "pro".to_string();
        project.models.claude = "opus".to_string();

        global.merge_with(project);

        // プロジェクト設定のモデルが上書きされる
        assert_eq!(global.models.gemini, "pro");
        assert_eq!(global.models.claude, "opus");
        // 変更されていないモデルはデフォルトのまま
        assert_eq!(global.models.codex, "gpt-5.1-codex-mini");
    }

    #[test]
    fn test_merge_with_prefix_rules_override() {
        let mut global = Config::default();
        global.prefix_rules = vec![PrefixRuleConfig {
            url_pattern: "github.com".to_string(),
            prefix_type: "conventional".to_string(),
        }];

        let mut project = Config::default();
        project.prefix_rules = vec![PrefixRuleConfig {
            url_pattern: "gitlab.com".to_string(),
            prefix_type: "bracket".to_string(),
        }];

        global.merge_with(project);

        // プロジェクト設定の prefix_rules で完全に置換される
        assert_eq!(global.prefix_rules.len(), 1);
        assert_eq!(global.prefix_rules[0].url_pattern, "gitlab.com");
        assert_eq!(global.prefix_rules[0].prefix_type, "bracket");
    }

    #[test]
    fn test_merge_with_cooldown_override() {
        let mut global = Config::default();
        global.provider_cooldown_minutes = 60;

        let mut project = Config::default();
        project.provider_cooldown_minutes = 30;

        global.merge_with(project);

        // プロジェクト設定のクールダウンが上書きされる
        assert_eq!(global.provider_cooldown_minutes, 30);
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
gemini = "flash"
codex = "gpt-5.1-codex-mini"
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
codex = "gpt-5.1-codex-mini"
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
}
