//! git-sc initコマンドモジュール
//!
//! サンプル設定付きの設定ファイルを生成する

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::error::AppError;

/// 設定ファイル生成用のinitコマンド
pub struct InitCommand;

impl InitCommand {
    /// initコマンドを実行して設定ファイルを生成
    ///
    /// # 引数
    /// * `force` - trueの場合、確認なしで既存ファイルを上書き
    ///
    /// # 戻り値
    /// * `Ok(PathBuf)` - 生成されたファイルのパス
    /// * `Err(AppError)` - 生成に失敗した場合
    pub fn execute(force: bool) -> Result<PathBuf, AppError> {
        let config_dir = Config::config_dir()
            .ok_or_else(|| AppError::ConfigError("設定ディレクトリを特定できません".to_string()))?;

        let config_path = Config::global_config_path()?;

        // ファイルが既に存在するかチェック
        if config_path.exists() && !force {
            // 上書き確認
            if !Self::confirm_overwrite(&config_path)? {
                return Err(AppError::ConfigError(
                    "操作がキャンセルされました".to_string(),
                ));
            }
        }

        // ディレクトリが存在しなければ作成
        fs::create_dir_all(&config_dir).map_err(|e| {
            AppError::ConfigError(format!(
                "ディレクトリの作成に失敗しました {}: {}",
                config_dir.display(),
                e
            ))
        })?;

        // 設定ファイルを書き込み
        let content = Config::default_config_content();
        fs::write(&config_path, content).map_err(|e| {
            AppError::ConfigError(format!(
                "設定ファイルの書き込みに失敗しました {}: {}",
                config_path.display(),
                e
            ))
        })?;

        Ok(config_path)
    }

    /// 既存ファイルの上書き確認をユーザーに求める
    fn confirm_overwrite(path: &Path) -> Result<bool, AppError> {
        eprint!(
            "設定ファイルが既に存在します: {}。上書きしますか？ [y/N]: ",
            path.display()
        );
        io::stderr()
            .flush()
            .map_err(|e| AppError::ConfigError(format!("Failed to flush stderr: {}", e)))?;

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .map_err(|e| AppError::ConfigError(format!("Failed to read input: {}", e)))?;

        let input = input.trim().to_lowercase();
        Ok(input == "y" || input == "yes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_content_not_empty() {
        let content = Config::default_config_content();
        assert!(!content.is_empty());
    }

    #[test]
    fn test_default_config_content_has_sections() {
        let content = Config::default_config_content();
        assert!(content.contains("[models]"));
        assert!(content.contains("providers"));
        assert!(content.contains("language"));
    }

    #[test]
    fn test_default_config_content_has_comments() {
        let content = Config::default_config_content();
        assert!(content.contains("# git-sc configuration file"));
        assert!(content.contains("# AI providers"));
        assert!(content.contains("# Prefix scripts"));
    }

    #[test]
    fn test_default_config_is_valid_toml() {
        let content = Config::default_config_content();
        // 有効なTOMLとしてパースできることを検証
        let result: Result<toml::Value, _> = toml::from_str(&content);
        assert!(result.is_ok(), "Config content should be valid TOML");
    }

    #[test]
    fn test_default_config_loads_as_config() {
        let content = Config::default_config_content();
        let result: Result<Config, _> = toml::from_str(&content);
        assert!(
            result.is_ok(),
            "Config content should deserialize to Config"
        );
    }

    #[test]
    fn test_default_config_values_match_defaults() {
        let content = Config::default_config_content();
        let config: Config = toml::from_str(&content).unwrap();
        let defaults = Config::default();

        assert_eq!(config.providers, defaults.providers);
        assert_eq!(config.language, defaults.language);
        assert_eq!(
            config.provider_cooldown_minutes,
            defaults.provider_cooldown_minutes
        );
        assert_eq!(config.models.gemini, defaults.models.gemini);
        assert_eq!(config.models.codex, defaults.models.codex);
        assert_eq!(config.models.claude, defaults.models.claude);
        assert_eq!(config.models.opencode, defaults.models.opencode);
    }

    #[test]
    fn test_default_config_content_has_all_provider_options() {
        let content = Config::default_config_content();
        assert!(content.contains("opencode"));
        assert!(content.contains("gemini"));
        assert!(content.contains("codex"));
        assert!(content.contains("claude"));
    }

    #[test]
    fn test_default_config_content_has_prefix_type_options() {
        let content = Config::default_config_content();
        assert!(content.contains("conventional"));
        assert!(content.contains("bracket"));
        assert!(content.contains("emoji"));
        assert!(content.contains("plain"));
    }
}
