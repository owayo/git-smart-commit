use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::AppError;

/// プロバイダーの失敗情報
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderFailure {
    /// 失敗した時刻（UNIXタイムスタンプ、秒）
    pub failed_at: u64,
}

/// アプリケーション状態
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    /// プロバイダーごとの失敗情報
    #[serde(default)]
    pub provider_failures: HashMap<String, ProviderFailure>,
}

impl State {
    /// 状態ファイルのパスを取得（~/.git-sc-state）
    pub fn state_path() -> Result<PathBuf, AppError> {
        dirs::home_dir()
            .map(|home| home.join(".git-sc-state"))
            .ok_or_else(|| AppError::ConfigError("Could not find home directory".to_string()))
    }

    /// ファイルから状態を読み込み、存在しない場合はデフォルトを返す
    pub fn load() -> Result<Self, AppError> {
        let path = Self::state_path()?;

        if !path.exists() {
            return Ok(State::default());
        }

        let content = fs::read_to_string(&path)
            .map_err(|e| AppError::ConfigError(format!("Failed to read state: {}", e)))?;

        toml::from_str(&content)
            .map_err(|e| AppError::ConfigError(format!("Failed to parse state: {}", e)))
    }

    /// 状態をファイルに保存
    pub fn save(&self) -> Result<(), AppError> {
        let path = Self::state_path()?;

        let content = toml::to_string_pretty(self)
            .map_err(|e| AppError::ConfigError(format!("Failed to serialize state: {}", e)))?;

        fs::write(&path, content)
            .map_err(|e| AppError::ConfigError(format!("Failed to write state: {}", e)))?;

        Ok(())
    }

    /// 現在のUNIXタイムスタンプ（秒）を取得
    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    /// プロバイダーの失敗を記録
    pub fn record_failure(&mut self, provider: &str) {
        self.provider_failures.insert(
            provider.to_lowercase(),
            ProviderFailure {
                failed_at: Self::now(),
            },
        );
    }

    /// クールダウン中のプロバイダーのリストを取得
    pub fn get_demoted_providers(&self, cooldown_minutes: u64) -> Vec<String> {
        let now = Self::now();
        let cooldown_secs = cooldown_minutes * 60;

        self.provider_failures
            .iter()
            .filter(|(_, failure)| {
                let elapsed = now.saturating_sub(failure.failed_at);
                elapsed < cooldown_secs
            })
            .map(|(provider, _)| provider.clone())
            .collect()
    }

    /// 期限切れの失敗記録をクリーンアップ
    pub fn cleanup_expired(&mut self, cooldown_minutes: u64) {
        let now = Self::now();
        let cooldown_secs = cooldown_minutes * 60;

        self.provider_failures.retain(|_, failure| {
            let elapsed = now.saturating_sub(failure.failed_at);
            elapsed < cooldown_secs
        });
    }

    /// プロバイダーリストを降格状態に基づいて並び替え
    /// 降格されたプロバイダーは末尾に移動
    pub fn reorder_providers(&self, providers: Vec<String>, cooldown_minutes: u64) -> Vec<String> {
        let demoted = self.get_demoted_providers(cooldown_minutes);

        let mut normal: Vec<String> = providers
            .iter()
            .filter(|p| !demoted.contains(&p.to_lowercase()))
            .cloned()
            .collect();

        let mut demoted_providers: Vec<String> = providers
            .iter()
            .filter(|p| demoted.contains(&p.to_lowercase()))
            .cloned()
            .collect();

        normal.append(&mut demoted_providers);
        normal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_default() {
        let state = State::default();
        assert!(state.provider_failures.is_empty());
    }

    #[test]
    fn test_record_failure() {
        let mut state = State::default();
        state.record_failure("gemini");

        assert!(state.provider_failures.contains_key("gemini"));
        assert!(state.provider_failures.get("gemini").unwrap().failed_at > 0);
    }

    #[test]
    fn test_record_failure_case_insensitive() {
        let mut state = State::default();
        state.record_failure("GEMINI");

        assert!(state.provider_failures.contains_key("gemini"));
    }

    #[test]
    fn test_get_demoted_providers_empty() {
        let state = State::default();
        let demoted = state.get_demoted_providers(60);
        assert!(demoted.is_empty());
    }

    #[test]
    fn test_get_demoted_providers_with_recent_failure() {
        let mut state = State::default();
        state.record_failure("gemini");

        let demoted = state.get_demoted_providers(60);
        assert!(demoted.contains(&"gemini".to_string()));
    }

    #[test]
    fn test_get_demoted_providers_expired() {
        let mut state = State::default();
        // 2時間前の失敗を記録
        let two_hours_ago = State::now() - (2 * 60 * 60);
        state.provider_failures.insert(
            "gemini".to_string(),
            ProviderFailure {
                failed_at: two_hours_ago,
            },
        );

        // 1時間のクールダウンなので、期限切れ
        let demoted = state.get_demoted_providers(60);
        assert!(demoted.is_empty());
    }

    #[test]
    fn test_reorder_providers_no_demoted() {
        let state = State::default();
        let providers = vec![
            "gemini".to_string(),
            "codex".to_string(),
            "claude".to_string(),
        ];

        let reordered = state.reorder_providers(providers.clone(), 60);
        assert_eq!(reordered, providers);
    }

    #[test]
    fn test_reorder_providers_with_demoted() {
        let mut state = State::default();
        state.record_failure("gemini");

        let providers = vec![
            "gemini".to_string(),
            "codex".to_string(),
            "claude".to_string(),
        ];

        let reordered = state.reorder_providers(providers, 60);
        assert_eq!(
            reordered,
            vec![
                "codex".to_string(),
                "claude".to_string(),
                "gemini".to_string(),
            ]
        );
    }

    #[test]
    fn test_reorder_providers_multiple_demoted() {
        let mut state = State::default();
        state.record_failure("gemini");
        state.record_failure("codex");

        let providers = vec![
            "gemini".to_string(),
            "codex".to_string(),
            "claude".to_string(),
        ];

        let reordered = state.reorder_providers(providers, 60);
        // claudeが先頭、demotedは元の順序で末尾
        assert_eq!(reordered[0], "claude".to_string());
        assert!(reordered.contains(&"gemini".to_string()));
        assert!(reordered.contains(&"codex".to_string()));
    }

    #[test]
    fn test_cleanup_expired() {
        let mut state = State::default();

        // 現在の失敗
        state.record_failure("gemini");

        // 2時間前の失敗
        let two_hours_ago = State::now() - (2 * 60 * 60);
        state.provider_failures.insert(
            "codex".to_string(),
            ProviderFailure {
                failed_at: two_hours_ago,
            },
        );

        // 1時間のクールダウンでクリーンアップ
        state.cleanup_expired(60);

        assert!(state.provider_failures.contains_key("gemini"));
        assert!(!state.provider_failures.contains_key("codex"));
    }

    #[test]
    fn test_state_serialization() {
        let mut state = State::default();
        state.record_failure("gemini");

        let serialized = toml::to_string_pretty(&state).unwrap();
        let deserialized: State = toml::from_str(&serialized).unwrap();

        assert!(deserialized.provider_failures.contains_key("gemini"));
    }
}
