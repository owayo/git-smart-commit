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
#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    /// AIプロバイダーの優先順序
    pub providers: Vec<String>,
    /// コミットメッセージの言語
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            providers: vec![
                "gemini".to_string(),
                "codex".to_string(),
                "claude".to_string(),
            ],
            language: "Japanese".to_string(),
            models: ModelsConfig::default(),
            prefix_scripts: Vec::new(),
            prefix_rules: Vec::new(),
        }
    }
}

impl Config {
    /// 設定ファイルのパスを取得（~/.git-sc）
    pub fn config_path() -> Result<PathBuf, AppError> {
        dirs::home_dir()
            .map(|home| home.join(".git-sc"))
            .ok_or_else(|| AppError::ConfigError("Could not find home directory".to_string()))
    }

    /// ファイルから設定を読み込み、存在しない場合はデフォルトを作成
    pub fn load() -> Result<Self, AppError> {
        let path = Self::config_path()?;

        if !path.exists() {
            // デフォルト設定を作成
            let config = Config::default();
            config.save()?;
            return Ok(config);
        }

        let content = fs::read_to_string(&path)
            .map_err(|e| AppError::ConfigError(format!("Failed to read config: {}", e)))?;

        toml::from_str(&content)
            .map_err(|e| AppError::ConfigError(format!("Failed to parse config: {}", e)))
    }

    /// 設定をファイルに保存
    pub fn save(&self) -> Result<(), AppError> {
        let path = Self::config_path()?;

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
    }

    #[test]
    fn test_parse_config_with_prefix_scripts() {
        let toml = r#"
providers = ["claude"]
language = "Japanese"

[[prefix_scripts]]
url_pattern = "https://github.com/myorg/"
script = "/path/to/script.sh"
"#;

        let config = Config::from_str(toml).unwrap();

        assert_eq!(config.prefix_scripts.len(), 1);
        assert_eq!(
            config.prefix_scripts[0].url_pattern,
            "https://github.com/myorg/"
        );
        assert_eq!(config.prefix_scripts[0].script, "/path/to/script.sh");
    }

    #[test]
    fn test_parse_config_with_prefix_rules() {
        let toml = r#"
providers = ["gemini"]
language = "Japanese"

[[prefix_rules]]
url_pattern = "https://github.com/myorg/"
prefix_type = "conventional"

[[prefix_rules]]
url_pattern = "https://gitlab.com/"
prefix_type = "bracket"
"#;

        let config = Config::from_str(toml).unwrap();

        assert_eq!(config.prefix_rules.len(), 2);
        assert_eq!(
            config.prefix_rules[0].url_pattern,
            "https://github.com/myorg/"
        );
        assert_eq!(config.prefix_rules[0].prefix_type, "conventional");
        assert_eq!(config.prefix_rules[1].url_pattern, "https://gitlab.com/");
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
url_pattern = "https://example.com/"
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
url_pattern = "git@gitlab.example.com"
script = "/opt/scripts/prefix.py"

[[prefix_rules]]
url_pattern = "https://github.com/myorg/"
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
}
