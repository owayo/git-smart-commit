//! Init command module for git-sc
//!
//! Generates configuration file with sample settings.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::error::AppError;

/// Init command for generating configuration file
pub struct InitCommand;

impl InitCommand {
    /// Execute the init command to generate configuration file
    ///
    /// # Arguments
    /// * `force` - If true, overwrite existing file without confirmation
    ///
    /// # Returns
    /// * `Ok(PathBuf)` - Path to the generated file
    /// * `Err(AppError)` - If generation fails
    pub fn execute(force: bool) -> Result<PathBuf, AppError> {
        let config_dir = Config::config_dir().ok_or_else(|| {
            AppError::ConfigError("Unable to determine config directory".to_string())
        })?;

        let config_path = Config::global_config_path()?;

        // Check if file exists
        if config_path.exists() && !force {
            // Ask for confirmation
            if !Self::confirm_overwrite(&config_path)? {
                return Err(AppError::ConfigError("Operation cancelled".to_string()));
            }
        }

        // Create directory if it doesn't exist
        fs::create_dir_all(&config_dir).map_err(|e| {
            AppError::ConfigError(format!(
                "Failed to create directory {}: {}",
                config_dir.display(),
                e
            ))
        })?;

        // Write config file
        let content = Config::default_config_content();
        fs::write(&config_path, content).map_err(|e| {
            AppError::ConfigError(format!(
                "Failed to write config file {}: {}",
                config_path.display(),
                e
            ))
        })?;

        Ok(config_path)
    }

    /// Ask user for confirmation to overwrite existing file
    fn confirm_overwrite(path: &Path) -> Result<bool, AppError> {
        eprint!(
            "Config file already exists at {}. Overwrite? [y/N]: ",
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
        // Should parse as valid TOML
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
