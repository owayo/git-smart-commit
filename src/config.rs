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
