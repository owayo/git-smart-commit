use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::AppError;

/// プロセス内で一意な一時ファイル接尾辞を生成するためのカウンタ。
/// 並列スレッドが同じナノ秒タイムスタンプを取得した場合でも tmp パスが衝突しないことを保証する。
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

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
    /// 状態ファイルのパスを取得（~/.config/git-sc/.providers-state）
    pub fn state_path() -> Result<PathBuf, AppError> {
        dirs::home_dir()
            .map(|home| home.join(".config").join("git-sc").join(".providers-state"))
            .ok_or_else(|| AppError::ConfigError("Could not find home directory".to_string()))
    }

    /// ファイルから状態を読み込み、存在しない場合はデフォルトを返す
    ///
    /// 旧 `gemini` キーは読み込み時にメモリ上で `antigravity` キーへ合流させる
    /// (ファイル自体は書き換えない)。両方が共存する場合は新しい方の failed_at を採用し、
    /// 短い実装で「移行直後の cooldown 状態」を保ったまま同一プロバイダーとして扱う。
    pub fn load() -> Result<Self, AppError> {
        let path = Self::state_path()?;

        if !path.exists() {
            return Ok(State::default());
        }

        let content = fs::read_to_string(&path)
            .map_err(|e| AppError::ConfigError(format!("Failed to read state: {}", e)))?;

        let mut state: State = toml::from_str(&content)
            .map_err(|e| AppError::ConfigError(format!("Failed to parse state: {}", e)))?;
        state.migrate_legacy_gemini_key();
        Ok(state)
    }

    /// 旧 `gemini` キーを `antigravity` キーへメモリ上で合流させる。
    ///
    /// 2026-05 の Gemini CLI → Antigravity CLI 移行で、旧 git-sc が記録した `gemini`
    /// cooldown が残っていると、`agy` を試したい場面でクールダウン降格されてしまう問題があった。
    /// `from_str("gemini")` は `Antigravity` を返すので、状態側も同様に合流させる。
    pub(crate) fn migrate_legacy_gemini_key(&mut self) {
        if let Some(legacy) = self.provider_failures.remove("gemini") {
            self.provider_failures
                .entry("antigravity".to_string())
                .and_modify(|existing| {
                    if legacy.failed_at > existing.failed_at {
                        existing.failed_at = legacy.failed_at;
                    }
                })
                .or_insert(legacy);
        }
    }

    /// 状態をファイルに保存
    pub fn save(&self) -> Result<(), AppError> {
        let path = Self::state_path()?;
        self.save_to_path(&path)
    }

    /// 任意のパスへ状態を保存（テストおよび `save` の内部実装）
    pub(crate) fn save_to_path(&self, path: &std::path::Path) -> Result<(), AppError> {
        // ディレクトリが存在しない場合は作成
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                AppError::ConfigError(format!("Failed to create state directory: {}", e))
            })?;
        }

        let content = toml::to_string_pretty(self)
            .map_err(|e| AppError::ConfigError(format!("Failed to serialize state: {}", e)))?;

        // 並列実行時に部分書き込みされた状態ファイルを別プロセスが読み取らないように、
        // 一時ファイルへ書き込んでから rename(2) でアトミックに置き換える。
        // 一時ファイル名は PID とナノ秒タイムスタンプに加え、プロセス内単調増加カウンタも
        // 含めることで、同一スレッドの連続呼び出しや複数スレッドで同じタイムスタンプを
        // 取得した場合でも衝突しないようにする。
        let pid = std::process::id();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let counter = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp_path = path.with_extension(format!("tmp.{}.{}.{}", pid, nanos, counter));
        if let Err(e) = fs::write(&tmp_path, &content) {
            // create 成功後の write 失敗(ENOSPC等)でも一時ファイルを残さない。
            // tmp 名は毎回ユニークで再利用されないため、ここで消さないと永久に蓄積する。
            let _ = fs::remove_file(&tmp_path);
            return Err(AppError::ConfigError(format!(
                "Failed to write state: {}",
                e
            )));
        }
        fs::rename(&tmp_path, path).map_err(|e| {
            // rename に失敗した場合は中途半端な一時ファイルを残さないように削除を試みる
            let _ = fs::remove_file(&tmp_path);
            AppError::ConfigError(format!("Failed to commit state file: {}", e))
        })?;

        Ok(())
    }

    /// 現在のUNIXタイムスタンプ（秒）を取得
    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    /// クールダウン分数を秒数へ変換する。
    ///
    /// 設定値は `u64` として読み込まれるため、極端に大きい値でもパニックや
    /// 桁あふれを起こさず「実質無期限」として扱う。
    fn cooldown_secs(cooldown_minutes: u64) -> u64 {
        cooldown_minutes.saturating_mul(60)
    }

    /// 状態ファイルと設定ファイルのプロバイダー名を比較用の正規名にそろえる。
    ///
    /// 設定ファイルには後方互換エイリアスが残ることがあるため、保存済みの失敗キーと
    /// 現在の provider 配列を同じプロバイダーとして扱えるようにする。
    fn canonical_provider_key(provider: &str) -> String {
        let lower = provider.to_lowercase();
        match lower.as_str() {
            "agy" | "gemini" | "antigravity" => "antigravity".to_string(),
            "apple-ai" | "apple_intelligence" | "apple-intelligence" => {
                "apple-intelligence".to_string()
            }
            _ => lower,
        }
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
        let cooldown_secs = Self::cooldown_secs(cooldown_minutes);

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
        let cooldown_secs = Self::cooldown_secs(cooldown_minutes);

        self.provider_failures.retain(|_, failure| {
            let elapsed = now.saturating_sub(failure.failed_at);
            elapsed < cooldown_secs
        });
    }

    /// プロバイダーリストを降格状態に基づいて並び替え
    /// 降格されたプロバイダーは末尾に移動
    pub fn reorder_providers(&self, providers: Vec<String>, cooldown_minutes: u64) -> Vec<String> {
        let demoted = self.get_demoted_providers(cooldown_minutes);
        let demoted: Vec<String> = demoted
            .iter()
            .map(|p| Self::canonical_provider_key(p))
            .collect();

        let mut normal: Vec<String> = providers
            .iter()
            .filter(|p| !demoted.contains(&Self::canonical_provider_key(p)))
            .cloned()
            .collect();

        let mut demoted_providers: Vec<String> = providers
            .iter()
            .filter(|p| demoted.contains(&Self::canonical_provider_key(p)))
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
    fn test_reorder_providers_demotes_antigravity_when_config_uses_gemini_alias() {
        let mut state = State::default();
        state.record_failure("antigravity");

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
    fn test_reorder_providers_demotes_apple_intelligence_from_legacy_state_key() {
        let mut state = State::default();
        state.record_failure("apple-ai");

        let providers = vec![
            "opencode".to_string(),
            "apple-intelligence".to_string(),
            "codex".to_string(),
        ];

        let reordered = state.reorder_providers(providers, 60);
        assert_eq!(
            reordered,
            vec![
                "opencode".to_string(),
                "codex".to_string(),
                "apple-intelligence".to_string(),
            ]
        );
    }

    #[test]
    fn test_reorder_providers_demotes_apple_intelligence_via_config_key() {
        // 本番フロー record_provider_failure → record_failure(provider.config_key()) を再現する。
        // AppleIntelligence の config_key() は "apple-ai" を返す一方、設定や providers 配列では
        // 正規名 "apple-intelligence" を使う。両者は canonical_provider_key が結びつけているため
        // 降格が成立する。上の legacy テストはキーをハードコードしているので config_key() 側の
        // 定義変更を検知できないが、このテストは config_key() の実値を使うことで
        // ai::service と state の結合不変条件を固定する。
        use crate::ai::AiProvider;

        let mut state = State::default();
        state.record_failure(AiProvider::AppleIntelligence.config_key());

        let providers = vec![
            "opencode".to_string(),
            "apple-intelligence".to_string(),
            "codex".to_string(),
        ];

        let reordered = state.reorder_providers(providers, 60);
        assert_eq!(
            reordered,
            vec![
                "opencode".to_string(),
                "codex".to_string(),
                "apple-intelligence".to_string(),
            ]
        );
    }

    #[test]
    fn test_all_provider_config_keys_canonicalize_to_configured_name() {
        // すべての AiProvider について、record_failure に渡される config_key() が
        // canonical_provider_key を通すと「設定 providers 配列で使う正規名」に解決されることを
        // 保証する。これは record_failure(config_key()) で保存したキーと reorder_providers 内の
        // canonical 比較が確実に一致するための不変条件であり、新しいプロバイダー追加時や
        // config_key()/canonical_provider_key() のいずれかを変更した際の取りこぼしを防ぐ。
        use crate::ai::AiProvider;

        let cases = [
            (AiProvider::Antigravity, "antigravity"),
            (AiProvider::Codex, "codex"),
            (AiProvider::Claude, "claude"),
            (AiProvider::Opencode, "opencode"),
            (AiProvider::AppleIntelligence, "apple-intelligence"),
        ];

        for (provider, expected_canonical) in cases {
            let key = provider.config_key();
            assert_eq!(
                State::canonical_provider_key(key).as_str(),
                expected_canonical,
                "provider {:?} の config_key() {:?} が想定の正規キーに解決されません",
                provider,
                key
            );
        }
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

    #[test]
    fn test_record_failure_overwrites_previous() {
        let mut state = State::default();
        state.record_failure("gemini");
        let first_time = state.provider_failures.get("gemini").unwrap().failed_at;

        // 同じプロバイダーに再度失敗を記録
        state.record_failure("gemini");
        let second_time = state.provider_failures.get("gemini").unwrap().failed_at;

        // 2回目のタイムスタンプは1回目以上
        assert!(second_time >= first_time);
        // エントリは1つのまま
        assert_eq!(state.provider_failures.len(), 1);
    }

    #[test]
    fn test_cleanup_expired_keeps_recent() {
        let mut state = State::default();
        state.record_failure("gemini");
        state.record_failure("codex");

        // 両方とも直近の失敗なので、クリーンアップしても残る
        state.cleanup_expired(60);
        assert_eq!(state.provider_failures.len(), 2);
    }

    #[test]
    fn test_cleanup_expired_zero_cooldown() {
        let mut state = State::default();
        state.record_failure("gemini");

        // クールダウン0分の場合、全エントリが期限切れ
        state.cleanup_expired(0);
        assert!(state.provider_failures.is_empty());
    }

    #[test]
    fn test_reorder_providers_empty_providers() {
        let state = State::default();
        let providers: Vec<String> = vec![];
        let reordered = state.reorder_providers(providers, 60);
        assert!(reordered.is_empty());
    }

    #[test]
    fn test_reorder_providers_all_demoted() {
        let mut state = State::default();
        state.record_failure("gemini");
        state.record_failure("codex");
        state.record_failure("claude");

        let providers = vec![
            "gemini".to_string(),
            "codex".to_string(),
            "claude".to_string(),
        ];

        let reordered = state.reorder_providers(providers, 60);
        // 全プロバイダーが降格されても、リスト自体は残る
        assert_eq!(reordered.len(), 3);
    }

    #[test]
    fn test_get_demoted_providers_zero_cooldown() {
        let mut state = State::default();
        state.record_failure("gemini");

        // クールダウン0の場合、全エントリが即座に期限切れ
        let demoted = state.get_demoted_providers(0);
        assert!(demoted.is_empty());
    }

    #[test]
    fn test_state_path_returns_valid_path() {
        let result = State::state_path();
        assert!(result.is_ok());
        let path = result.unwrap();
        assert!(path.to_str().unwrap().contains(".providers-state"));
    }

    #[test]
    fn test_reorder_providers_mixed_case() {
        let mut state = State::default();
        state.record_failure("gemini"); // "gemini" として保存

        let providers = vec![
            "Gemini".to_string(),
            "Claude".to_string(),
            "Codex".to_string(),
        ];

        let reordered = state.reorder_providers(providers, 60);
        assert_eq!(reordered[0], "Claude");
        assert_eq!(reordered[1], "Codex");
        assert_eq!(reordered[2], "Gemini");
    }

    #[test]
    fn test_state_roundtrip_serialization() {
        let mut state = State::default();
        state.record_failure("gemini");
        state.record_failure("claude");

        let serialized = toml::to_string_pretty(&state).unwrap();
        let deserialized: State = toml::from_str(&serialized).unwrap();

        assert_eq!(
            state.provider_failures.len(),
            deserialized.provider_failures.len()
        );
        assert!(deserialized.provider_failures.contains_key("gemini"));
        assert!(deserialized.provider_failures.contains_key("claude"));
    }

    #[test]
    fn test_state_empty_roundtrip() {
        let state = State::default();
        let serialized = toml::to_string_pretty(&state).unwrap();
        let deserialized: State = toml::from_str(&serialized).unwrap();
        assert!(deserialized.provider_failures.is_empty());
    }

    #[test]
    fn test_record_failure_multiple_providers() {
        let mut state = State::default();
        state.record_failure("gemini");
        state.record_failure("claude");
        state.record_failure("codex");

        assert_eq!(state.provider_failures.len(), 3);
        let demoted = state.get_demoted_providers(60);
        assert_eq!(demoted.len(), 3);
    }

    #[test]
    fn test_get_demoted_providers_at_cooldown_boundary() {
        // クールダウン境界値: ちょうど60分前の失敗は期限切れ
        let mut state = State::default();
        let exactly_60_min_ago = State::now() - (60 * 60);
        state.provider_failures.insert(
            "gemini".to_string(),
            ProviderFailure {
                failed_at: exactly_60_min_ago,
            },
        );

        let demoted = state.get_demoted_providers(60);
        // elapsed == cooldown_secs なので期限切れ
        assert!(demoted.is_empty());
    }

    #[test]
    fn test_get_demoted_providers_just_before_boundary() {
        // クールダウン境界値: 59分59秒前の失敗はまだクールダウン中
        let mut state = State::default();
        let just_before = State::now() - (60 * 60 - 1);
        state.provider_failures.insert(
            "gemini".to_string(),
            ProviderFailure {
                failed_at: just_before,
            },
        );

        let demoted = state.get_demoted_providers(60);
        assert!(demoted.contains(&"gemini".to_string()));
    }

    #[test]
    fn test_state_deserialize_from_malformed_toml() {
        // 不正なTOMLからのデシリアライズはエラー
        let result: Result<State, _> = toml::from_str("invalid[[[toml");
        assert!(result.is_err());
    }

    #[test]
    fn test_state_deserialize_empty_toml() {
        // 空のTOMLからデシリアライズするとデフォルト状態
        let state: State = toml::from_str("").unwrap();
        assert!(state.provider_failures.is_empty());
    }

    #[test]
    fn test_cleanup_expired_at_boundary() {
        // クールダウンちょうどの境界でクリーンアップされる
        let mut state = State::default();
        let exactly_at_boundary = State::now() - (30 * 60);
        state.provider_failures.insert(
            "gemini".to_string(),
            ProviderFailure {
                failed_at: exactly_at_boundary,
            },
        );

        // 30分のクールダウンでちょうど30分前 → elapsed == cooldown_secs → 期限切れ
        state.cleanup_expired(30);
        assert!(state.provider_failures.is_empty());
    }

    #[test]
    fn test_reorder_providers_demoted_not_in_list() {
        // 降格されたプロバイダーがリストに含まれない場合、リストは変更なし
        let mut state = State::default();
        state.record_failure("unknown_provider");

        let providers = vec!["gemini".to_string(), "claude".to_string()];

        let reordered = state.reorder_providers(providers.clone(), 60);
        assert_eq!(reordered, providers);
    }

    #[test]
    fn test_state_path_ends_with_expected_filename() {
        let path = State::state_path().unwrap();
        assert!(path.file_name().unwrap().to_str().unwrap() == ".providers-state");
    }

    #[test]
    fn test_provider_failure_serialization() {
        // ProviderFailure 単体のシリアライズ・デシリアライズ
        let failure = ProviderFailure {
            failed_at: 1234567890,
        };
        let serialized = toml::to_string(&failure).unwrap();
        let deserialized: ProviderFailure = toml::from_str(&serialized).unwrap();
        assert_eq!(deserialized.failed_at, 1234567890);
    }

    // ============================================================
    // reorder_providers: 期限切れエントリとの組み合わせ
    // ============================================================

    #[test]
    fn test_reorder_providers_with_expired_and_active() {
        // 1つが期限切れ、1つがアクティブな降格状態
        let mut state = State::default();

        // 2時間前の失敗（期限切れ）
        let two_hours_ago = State::now() - (2 * 60 * 60);
        state.provider_failures.insert(
            "gemini".to_string(),
            ProviderFailure {
                failed_at: two_hours_ago,
            },
        );

        // 直近の失敗（アクティブ）
        state.record_failure("codex");

        let providers = vec![
            "gemini".to_string(),
            "codex".to_string(),
            "claude".to_string(),
        ];

        let reordered = state.reorder_providers(providers, 60);
        // geminiは期限切れなので通常位置、codexは末尾に移動
        assert_eq!(reordered[0], "gemini");
        assert_eq!(reordered[1], "claude");
        assert_eq!(reordered[2], "codex");
    }

    #[test]
    fn test_cleanup_expired_large_cooldown() {
        // 非常に大きなクールダウン値: 全エントリが保持される
        let mut state = State::default();
        let old_failure = State::now() - (24 * 60 * 60); // 24時間前
        state.provider_failures.insert(
            "gemini".to_string(),
            ProviderFailure {
                failed_at: old_failure,
            },
        );

        // 1週間のクールダウン
        state.cleanup_expired(7 * 24 * 60);
        assert!(state.provider_failures.contains_key("gemini"));
    }

    #[test]
    fn test_max_cooldown_does_not_overflow() {
        // 設定値が u64::MAX でも秒変換でパニックや桁あふれを起こさない
        let mut state = State::default();
        state
            .provider_failures
            .insert("gemini".to_string(), ProviderFailure { failed_at: 0 });

        let demoted = state.get_demoted_providers(u64::MAX);
        assert_eq!(demoted, vec!["gemini".to_string()]);

        state.cleanup_expired(u64::MAX);
        assert!(state.provider_failures.contains_key("gemini"));
    }

    #[test]
    fn test_get_demoted_providers_multiple_with_mixed_expiry() {
        // 複数プロバイダーで一部のみ期限切れ
        let mut state = State::default();

        // アクティブ
        state.record_failure("gemini");

        // 期限切れ
        let old = State::now() - (2 * 60 * 60);
        state
            .provider_failures
            .insert("codex".to_string(), ProviderFailure { failed_at: old });

        // アクティブ
        state.record_failure("claude");

        let demoted = state.get_demoted_providers(60);
        assert_eq!(demoted.len(), 2);
        assert!(demoted.contains(&"gemini".to_string()));
        assert!(demoted.contains(&"claude".to_string()));
        assert!(!demoted.contains(&"codex".to_string()));
    }

    #[test]
    fn test_reorder_providers_single_provider() {
        // プロバイダーが1つだけの場合
        let mut state = State::default();
        state.record_failure("gemini");

        let providers = vec!["gemini".to_string()];
        let reordered = state.reorder_providers(providers, 60);
        // 降格されても1つしかないのでそのまま
        assert_eq!(reordered, vec!["gemini".to_string()]);
    }

    #[test]
    fn test_save_to_path_writes_state_atomically() {
        // save_to_path は tempfile + rename によりアトミックに保存できる必要がある
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("state.toml");

        let mut state = State::default();
        state.record_failure("gemini");
        state.save_to_path(&target).unwrap();

        // 書き込み後は対象ファイルだけが残り、一時ファイルは残らない
        assert!(target.exists(), "状態ファイルが作成されていない");
        let tmp_path = target.with_extension("tmp");
        assert!(
            !tmp_path.exists(),
            "一時ファイル {:?} が残存している",
            tmp_path
        );

        // 内容が正しくデシリアライズできることを確認
        let content = fs::read_to_string(&target).unwrap();
        let parsed: State = toml::from_str(&content).unwrap();
        assert!(parsed.provider_failures.contains_key("gemini"));
    }

    #[test]
    fn test_save_to_path_creates_missing_parent_directory() {
        // ディレクトリが存在しない場合でも自動で作成して保存できる
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("nested").join("more").join("state.toml");
        assert!(!nested.parent().unwrap().exists());

        let state = State::default();
        state.save_to_path(&nested).unwrap();

        assert!(nested.exists());
    }

    #[test]
    fn test_save_to_path_overwrites_existing_file() {
        // 既存ファイルがある場合でも rename で正しく置き換わる
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("state.toml");

        // 旧データを先に書き込む
        fs::write(&target, "provider_failures = { old = { failed_at = 0 } }\n").unwrap();

        let mut state = State::default();
        state.record_failure("codex");
        state.save_to_path(&target).unwrap();

        let content = fs::read_to_string(&target).unwrap();
        let parsed: State = toml::from_str(&content).unwrap();
        assert!(parsed.provider_failures.contains_key("codex"));
        assert!(!parsed.provider_failures.contains_key("old"));
    }

    #[test]
    fn test_save_to_path_concurrent_writes_all_succeed() {
        // 同時保存を行ったときに、一時ファイル名衝突によって失敗しないことを確認する。
        // rename(2) は原子的なので最終的に勝った内容が残るが、書き込み自体は全て成功すべき。
        use std::sync::Arc;
        use std::thread;

        let dir = Arc::new(tempfile::tempdir().unwrap());
        let target = Arc::new(dir.path().join("state.toml"));

        let mut handles = Vec::new();
        for i in 0..8 {
            let target = Arc::clone(&target);
            let _dir_alive = Arc::clone(&dir);
            handles.push(thread::spawn(move || {
                let mut state = State::default();
                state.record_failure(&format!("provider-{}", i));
                state.save_to_path(&target)
            }));
        }

        for handle in handles {
            handle.join().expect("スレッドのジョインに失敗").expect(
                "並列 save_to_path はすべて成功すべき（衝突しない一時ファイル名を使う実装が要件）",
            );
        }

        // 最終的に有効な TOML として読み戻せる
        let content = fs::read_to_string(target.as_path()).unwrap();
        let parsed: State = toml::from_str(&content).unwrap();
        // 並列なので最後に勝った1つの provider のみが残る想定。空ではない。
        assert!(!parsed.provider_failures.is_empty());

        // 一時ファイル (target.tmp.* など) が残っていない
        let entries: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|name| name.starts_with("state.tmp."))
            .collect();
        assert!(
            entries.is_empty(),
            "並列保存後に一時ファイルが残存している: {:?}",
            entries
        );
    }

    // ============================================================
    // migrate_legacy_gemini_key のテスト
    // 旧 "gemini" キーを "antigravity" キーへメモリ上で合流させる
    // ============================================================

    #[test]
    fn test_migrate_legacy_gemini_key_renames_when_only_legacy() {
        // 旧 gemini キーのみが存在する場合、antigravity キーへリネームされる
        let mut state = State::default();
        state.record_failure("gemini");
        let original_ts = state.provider_failures["gemini"].failed_at;

        state.migrate_legacy_gemini_key();

        assert!(!state.provider_failures.contains_key("gemini"));
        assert!(state.provider_failures.contains_key("antigravity"));
        assert_eq!(
            state.provider_failures["antigravity"].failed_at,
            original_ts
        );
    }

    #[test]
    fn test_migrate_legacy_gemini_key_keeps_newer_timestamp_when_both_exist() {
        // 両方共存する場合、より新しい failed_at を保持し、gemini キーは消える
        let mut state = State::default();
        let now = State::now();
        state.provider_failures.insert(
            "gemini".to_string(),
            ProviderFailure {
                failed_at: now - 10,
            },
        );
        state.provider_failures.insert(
            "antigravity".to_string(),
            ProviderFailure {
                failed_at: now - 100,
            },
        );

        state.migrate_legacy_gemini_key();

        assert!(!state.provider_failures.contains_key("gemini"));
        // gemini の方が新しいので採用される
        assert_eq!(state.provider_failures["antigravity"].failed_at, now - 10);
    }

    #[test]
    fn test_migrate_legacy_gemini_key_preserves_newer_antigravity() {
        // antigravity の方が新しい場合、antigravity のタイムスタンプを維持
        let mut state = State::default();
        let now = State::now();
        state.provider_failures.insert(
            "gemini".to_string(),
            ProviderFailure {
                failed_at: now - 100,
            },
        );
        state.provider_failures.insert(
            "antigravity".to_string(),
            ProviderFailure {
                failed_at: now - 10,
            },
        );

        state.migrate_legacy_gemini_key();

        assert!(!state.provider_failures.contains_key("gemini"));
        assert_eq!(state.provider_failures["antigravity"].failed_at, now - 10);
    }

    #[test]
    fn test_migrate_legacy_gemini_key_noop_when_no_legacy() {
        // gemini キーがなければ何もしない
        let mut state = State::default();
        state.record_failure("antigravity");
        let original_ts = state.provider_failures["antigravity"].failed_at;

        state.migrate_legacy_gemini_key();

        assert_eq!(state.provider_failures.len(), 1);
        assert_eq!(
            state.provider_failures["antigravity"].failed_at,
            original_ts
        );
    }

    #[test]
    fn test_save_to_path_uses_unique_tmp_path_per_invocation() {
        // 一時ファイル名はプロセス ID とナノ秒タイムスタンプを含むので、
        // 同じ最終パスを共有する複数 save であっても固定パスに衝突しないことを保証する。
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("state.toml");

        // 連続して何度も保存しても成功する（固定 tmp パスを共有するとここで失敗する可能性がある）
        for i in 0..32 {
            let mut state = State::default();
            state.record_failure(&format!("provider-{}", i));
            state.save_to_path(&target).unwrap();
        }

        // 残骸の一時ファイルが存在しないこと
        let stale: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|name| name.starts_with("state.tmp."))
            .collect();
        assert!(stale.is_empty(), "残骸の一時ファイル: {:?}", stale);
    }
}
