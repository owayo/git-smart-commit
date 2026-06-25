use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::{ProviderStep, canonical_provider_key};
use crate::error::AppError;

/// プロセス内で一意な一時ファイル接尾辞を生成するためのカウンタ。
/// 並列スレッドが同じナノ秒タイムスタンプを取得した場合でも tmp パスが衝突しないことを保証する。
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// プロバイダーステップの失敗情報
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderFailure {
    /// クールダウン主キー(`ProviderStep::cooldown_key` 由来。name 明示時はそれ、
    /// 未指定時は provider+model+env+command から決定的に導出した値)。
    pub key: String,
    /// 表示・デバッグ用のプロバイダー名(任意)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// 失敗した時刻（UNIXタイムスタンプ、秒）
    pub failed_at: u64,
}

/// アプリケーション状態
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    /// 失敗したステップの一覧(複合キー単位)
    #[serde(default)]
    pub failures: Vec<ProviderFailure>,
}

/// 旧形式の失敗情報(provider名 → {failed_at})。読み込み時の移行にのみ使う。
#[derive(Debug, Deserialize)]
struct LegacyFailure {
    failed_at: u64,
}

/// ディスク上の状態。新形式(`failures`)と旧形式(`provider_failures` HashMap)の
/// 両方を読み取り、`State::load` で新形式へ正規化する。
#[derive(Debug, Default, Deserialize)]
struct StateOnDisk {
    #[serde(default)]
    failures: Vec<ProviderFailure>,
    #[serde(default)]
    provider_failures: HashMap<String, LegacyFailure>,
}

/// 旧 provider名キーが導出すべき新クールダウンキーへ変換する。
/// 「provider のみ(model/env/command なし)」ステップの `cooldown_key` と一致させることで、
/// 移行後も旧 cooldown が同じステップに効き続ける。
fn legacy_key_to_new(provider: &str) -> String {
    ProviderStep::from_provider(provider).cooldown_key()
}

impl State {
    /// 状態ファイルのパスを取得（~/.config/git-sc/.providers-state）
    pub fn state_path() -> Result<PathBuf, AppError> {
        dirs::home_dir()
            .map(|home| home.join(".config").join("git-sc").join(".providers-state"))
            .ok_or_else(|| AppError::ConfigError("Could not find home directory".to_string()))
    }

    /// ファイルから状態を読み込み、存在しない場合はデフォルトを返す。
    ///
    /// 旧形式(provider名単位の HashMap)で保存されたファイルも読み取り、各エントリを
    /// 「provider のみステップ」の複合キーへ移行する。旧 `gemini`/`apple-ai` キーは
    /// `canonical_provider_key` 経由で `antigravity`/`apple-intelligence` のキーへ合流する。
    pub fn load() -> Result<Self, AppError> {
        let path = Self::state_path()?;

        if !path.exists() {
            return Ok(State::default());
        }

        let content = fs::read_to_string(&path)
            .map_err(|e| AppError::ConfigError(format!("Failed to read state: {}", e)))?;

        let disk: StateOnDisk = toml::from_str(&content)
            .map_err(|e| AppError::ConfigError(format!("Failed to parse state: {}", e)))?;

        let mut failures = disk.failures;
        // 旧 HashMap 形式を新キーへ移行する。同じキーが両形式に存在する場合は
        // 新しい failed_at を採用する。
        for (provider, legacy) in disk.provider_failures {
            let key = legacy_key_to_new(&provider);
            if let Some(existing) = failures.iter_mut().find(|f| f.key == key) {
                if legacy.failed_at > existing.failed_at {
                    existing.failed_at = legacy.failed_at;
                }
            } else {
                failures.push(ProviderFailure {
                    key,
                    provider: Some(canonical_provider_key(&provider)),
                    failed_at: legacy.failed_at,
                });
            }
        }

        Ok(State { failures })
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

    /// ステップの失敗を記録する(複合キー単位)。同じキーが既にあれば時刻を更新する。
    pub fn record_failure(&mut self, step: &ProviderStep) {
        let key = step.cooldown_key();
        let now = Self::now();
        if let Some(f) = self.failures.iter_mut().find(|f| f.key == key) {
            f.failed_at = now;
        } else {
            self.failures.push(ProviderFailure {
                key,
                provider: Some(canonical_provider_key(&step.provider)),
                failed_at: now,
            });
        }
    }

    /// 指定ステップが現在クールダウン中か。
    pub fn is_demoted(&self, step: &ProviderStep, cooldown_minutes: u64) -> bool {
        let key = step.cooldown_key();
        let now = Self::now();
        let cooldown_secs = Self::cooldown_secs(cooldown_minutes);
        self.failures
            .iter()
            .any(|f| f.key == key && now.saturating_sub(f.failed_at) < cooldown_secs)
    }

    /// 期限切れの失敗記録をクリーンアップ
    pub fn cleanup_expired(&mut self, cooldown_minutes: u64) {
        let now = Self::now();
        let cooldown_secs = Self::cooldown_secs(cooldown_minutes);

        self.failures
            .retain(|failure| now.saturating_sub(failure.failed_at) < cooldown_secs);
    }

    /// ステップ列を降格状態に基づいて並び替える。降格中のステップは末尾へ移動する。
    /// 並びは安定(同一区分内では元の順序を保つ)。
    pub fn reorder_steps(
        &self,
        steps: Vec<ProviderStep>,
        cooldown_minutes: u64,
    ) -> Vec<ProviderStep> {
        let (normal, demoted): (Vec<_>, Vec<_>) = steps
            .into_iter()
            .partition(|s| !self.is_demoted(s, cooldown_minutes));
        normal.into_iter().chain(demoted).collect()
    }
}

#[cfg(test)]
mod tests {
    // `ProviderStep` と `canonical_provider_key` は親モジュールの
    // `use crate::config::{...}`(ファイル冒頭)を `super::*` 経由で取り込む。
    use super::*;

    /// provider 名のみの `ProviderStep` を作るヘルパー。
    fn step(p: &str) -> ProviderStep {
        ProviderStep::from_provider(p)
    }

    /// provider 名のみのステップが導出するクールダウンキー。
    fn key_of(p: &str) -> String {
        ProviderStep::from_provider(p).cooldown_key()
    }

    /// 指定キーの失敗が記録されているか。
    fn has_failure(state: &State, p: &str) -> bool {
        let key = key_of(p);
        state.failures.iter().any(|f| f.key == key)
    }

    /// 指定キーの failed_at を取得（存在前提）。
    fn failed_at_of(state: &State, p: &str) -> u64 {
        let key = key_of(p);
        state
            .failures
            .iter()
            .find(|f| f.key == key)
            .unwrap()
            .failed_at
    }

    /// 指定 failed_at で失敗を直接ねじ込むヘルパー（時刻を制御したいテスト用）。
    fn push_failure(state: &mut State, p: &str, failed_at: u64) {
        state.failures.push(ProviderFailure {
            key: key_of(p),
            provider: Some(canonical_provider_key(p)),
            failed_at,
        });
    }

    #[test]
    fn test_state_default() {
        let state = State::default();
        assert!(state.failures.is_empty());
    }

    #[test]
    fn test_record_failure() {
        let mut state = State::default();
        state.record_failure(&step("gemini"));

        assert!(has_failure(&state, "gemini"));
        assert!(failed_at_of(&state, "gemini") > 0);
    }

    #[test]
    fn test_record_failure_case_insensitive() {
        let mut state = State::default();
        state.record_failure(&step("GEMINI"));

        // cooldown_key は provider を小文字化・正規化するので "gemini" でヒットする
        assert!(has_failure(&state, "gemini"));
    }

    #[test]
    fn test_get_demoted_providers_empty() {
        let state = State::default();
        assert!(!state.is_demoted(&step("gemini"), 60));
    }

    #[test]
    fn test_get_demoted_providers_with_recent_failure() {
        let mut state = State::default();
        state.record_failure(&step("gemini"));

        assert!(state.is_demoted(&step("gemini"), 60));
    }

    #[test]
    fn test_get_demoted_providers_expired() {
        let mut state = State::default();
        // 2時間前の失敗を記録
        let two_hours_ago = State::now() - (2 * 60 * 60);
        push_failure(&mut state, "gemini", two_hours_ago);

        // 1時間のクールダウンなので、期限切れ
        assert!(!state.is_demoted(&step("gemini"), 60));
    }

    #[test]
    fn test_reorder_providers_no_demoted() {
        let state = State::default();
        let steps = vec![step("gemini"), step("codex"), step("claude")];

        let reordered = state.reorder_steps(steps.clone(), 60);
        assert_eq!(reordered, steps);
    }

    #[test]
    fn test_reorder_providers_with_demoted() {
        let mut state = State::default();
        state.record_failure(&step("gemini"));

        let steps = vec![step("gemini"), step("codex"), step("claude")];

        let reordered = state.reorder_steps(steps, 60);
        assert_eq!(
            reordered,
            vec![step("codex"), step("claude"), step("gemini")]
        );
    }

    #[test]
    fn test_reorder_providers_demotes_antigravity_when_config_uses_gemini_alias() {
        let mut state = State::default();
        state.record_failure(&step("antigravity"));

        let steps = vec![step("gemini"), step("codex"), step("claude")];

        // antigravity の失敗が gemini エイリアスのステップを降格させる
        let reordered = state.reorder_steps(steps, 60);
        assert_eq!(
            reordered,
            vec![step("codex"), step("claude"), step("gemini")]
        );
    }

    #[test]
    fn test_reorder_providers_demotes_apple_intelligence_from_legacy_state_key() {
        let mut state = State::default();
        state.record_failure(&step("apple-ai"));

        let steps = vec![step("opencode"), step("apple-intelligence"), step("codex")];

        // 旧 apple-ai キーが apple-intelligence ステップを降格させる
        let reordered = state.reorder_steps(steps, 60);
        assert_eq!(
            reordered,
            vec![step("opencode"), step("codex"), step("apple-intelligence"),]
        );
    }

    #[test]
    fn test_reorder_providers_demotes_apple_intelligence_via_config_key() {
        // 本番フロー record_provider_failure → record_failure(step) を再現する。
        // AppleIntelligence の config_key() は "apple-ai" を返す一方、設定や steps では
        // 正規名 "apple-intelligence" を使う。両者は cooldown_key() 内の canonical 化が
        // 結びつけているため降格が成立する。上の legacy テストはキーを直接ハードコード
        // しているので config_key() 側の定義変更を検知できないが、このテストは config_key()
        // の実値からステップを作ることで ai::service と state の結合不変条件を固定する。
        use crate::ai::AiProvider;

        let mut state = State::default();
        state.record_failure(&step(AiProvider::AppleIntelligence.config_key()));

        let steps = vec![step("opencode"), step("apple-intelligence"), step("codex")];

        let reordered = state.reorder_steps(steps, 60);
        assert_eq!(
            reordered,
            vec![step("opencode"), step("codex"), step("apple-intelligence"),]
        );
    }

    #[test]
    fn test_all_provider_config_keys_canonicalize_to_configured_name() {
        // すべての AiProvider について、record_failure に渡されるステップの config_key() を
        // canonical_provider_key を通すと「設定 steps で使う正規名」に解決されることを保証する。
        // これは record_failure(step) で保存したキー（cooldown_key 内で canonical 化）と
        // reorder_steps 内の is_demoted 比較が確実に一致するための不変条件であり、新しい
        // プロバイダー追加時や config_key()/canonical_provider_key() のいずれかを変更した際の
        // 取りこぼしを防ぐ。
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
                canonical_provider_key(key).as_str(),
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
        state.record_failure(&step("gemini"));
        state.record_failure(&step("codex"));

        let steps = vec![step("gemini"), step("codex"), step("claude")];

        let reordered = state.reorder_steps(steps, 60);
        // claudeが先頭、demotedは元の順序で末尾
        assert_eq!(reordered[0].provider, "claude");
        assert!(reordered.iter().any(|s| s.provider == "gemini"));
        assert!(reordered.iter().any(|s| s.provider == "codex"));
    }

    #[test]
    fn test_cleanup_expired() {
        let mut state = State::default();

        // 現在の失敗
        state.record_failure(&step("gemini"));

        // 2時間前の失敗
        let two_hours_ago = State::now() - (2 * 60 * 60);
        push_failure(&mut state, "codex", two_hours_ago);

        // 1時間のクールダウンでクリーンアップ
        state.cleanup_expired(60);

        assert!(has_failure(&state, "gemini"));
        assert!(!has_failure(&state, "codex"));
    }

    #[test]
    fn test_state_serialization() {
        let mut state = State::default();
        state.record_failure(&step("gemini"));

        let serialized = toml::to_string_pretty(&state).unwrap();
        let deserialized: State = toml::from_str(&serialized).unwrap();

        assert!(has_failure(&deserialized, "gemini"));
    }

    #[test]
    fn test_record_failure_overwrites_previous() {
        let mut state = State::default();
        state.record_failure(&step("gemini"));
        let first_time = failed_at_of(&state, "gemini");

        // 同じプロバイダーに再度失敗を記録
        state.record_failure(&step("gemini"));
        let second_time = failed_at_of(&state, "gemini");

        // 2回目のタイムスタンプは1回目以上
        assert!(second_time >= first_time);
        // エントリは1つのまま
        assert_eq!(state.failures.len(), 1);
    }

    #[test]
    fn test_cleanup_expired_keeps_recent() {
        let mut state = State::default();
        state.record_failure(&step("gemini"));
        state.record_failure(&step("codex"));

        // 両方とも直近の失敗なので、クリーンアップしても残る
        state.cleanup_expired(60);
        assert_eq!(state.failures.len(), 2);
    }

    #[test]
    fn test_cleanup_expired_zero_cooldown() {
        let mut state = State::default();
        state.record_failure(&step("gemini"));

        // クールダウン0分の場合、全エントリが期限切れ
        state.cleanup_expired(0);
        assert!(state.failures.is_empty());
    }

    #[test]
    fn test_reorder_providers_empty_providers() {
        let state = State::default();
        let steps: Vec<ProviderStep> = vec![];
        let reordered = state.reorder_steps(steps, 60);
        assert!(reordered.is_empty());
    }

    #[test]
    fn test_reorder_providers_all_demoted() {
        let mut state = State::default();
        state.record_failure(&step("gemini"));
        state.record_failure(&step("codex"));
        state.record_failure(&step("claude"));

        let steps = vec![step("gemini"), step("codex"), step("claude")];

        let reordered = state.reorder_steps(steps, 60);
        // 全プロバイダーが降格されても、リスト自体は残る
        assert_eq!(reordered.len(), 3);
    }

    #[test]
    fn test_get_demoted_providers_zero_cooldown() {
        let mut state = State::default();
        state.record_failure(&step("gemini"));

        // クールダウン0の場合、全エントリが即座に期限切れ
        assert!(!state.is_demoted(&step("gemini"), 0));
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
        state.record_failure(&step("gemini")); // "gemini" として保存

        let steps = vec![step("Gemini"), step("Claude"), step("Codex")];

        // 大文字の "Gemini" でも cooldown_key の小文字化で gemini の失敗と一致し降格される
        let reordered = state.reorder_steps(steps, 60);
        assert_eq!(reordered[0].provider, "Claude");
        assert_eq!(reordered[1].provider, "Codex");
        assert_eq!(reordered[2].provider, "Gemini");
    }

    #[test]
    fn test_state_roundtrip_serialization() {
        let mut state = State::default();
        state.record_failure(&step("gemini"));
        state.record_failure(&step("claude"));

        let serialized = toml::to_string_pretty(&state).unwrap();
        let deserialized: State = toml::from_str(&serialized).unwrap();

        assert_eq!(state.failures.len(), deserialized.failures.len());
        assert!(has_failure(&deserialized, "gemini"));
        assert!(has_failure(&deserialized, "claude"));
    }

    #[test]
    fn test_state_empty_roundtrip() {
        let state = State::default();
        let serialized = toml::to_string_pretty(&state).unwrap();
        let deserialized: State = toml::from_str(&serialized).unwrap();
        assert!(deserialized.failures.is_empty());
    }

    #[test]
    fn test_record_failure_multiple_providers() {
        let mut state = State::default();
        state.record_failure(&step("gemini"));
        state.record_failure(&step("claude"));
        state.record_failure(&step("codex"));

        assert_eq!(state.failures.len(), 3);
        // 3つとも直近の失敗なので全て降格中
        assert!(state.is_demoted(&step("gemini"), 60));
        assert!(state.is_demoted(&step("claude"), 60));
        assert!(state.is_demoted(&step("codex"), 60));
    }

    #[test]
    fn test_get_demoted_providers_at_cooldown_boundary() {
        // クールダウン境界値: ちょうど60分前の失敗は期限切れ
        let mut state = State::default();
        let exactly_60_min_ago = State::now() - (60 * 60);
        push_failure(&mut state, "gemini", exactly_60_min_ago);

        // elapsed == cooldown_secs なので期限切れ
        assert!(!state.is_demoted(&step("gemini"), 60));
    }

    #[test]
    fn test_get_demoted_providers_just_before_boundary() {
        // クールダウン境界値: 59分59秒前の失敗はまだクールダウン中
        let mut state = State::default();
        let just_before = State::now() - (60 * 60 - 1);
        push_failure(&mut state, "gemini", just_before);

        assert!(state.is_demoted(&step("gemini"), 60));
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
        assert!(state.failures.is_empty());
    }

    #[test]
    fn test_cleanup_expired_at_boundary() {
        // クールダウンちょうどの境界でクリーンアップされる
        let mut state = State::default();
        let exactly_at_boundary = State::now() - (30 * 60);
        push_failure(&mut state, "gemini", exactly_at_boundary);

        // 30分のクールダウンでちょうど30分前 → elapsed == cooldown_secs → 期限切れ
        state.cleanup_expired(30);
        assert!(state.failures.is_empty());
    }

    #[test]
    fn test_reorder_providers_demoted_not_in_list() {
        // 降格されたプロバイダーがリストに含まれない場合、リストは変更なし
        let mut state = State::default();
        state.record_failure(&step("unknown_provider"));

        let steps = vec![step("gemini"), step("claude")];

        let reordered = state.reorder_steps(steps.clone(), 60);
        assert_eq!(reordered, steps);
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
            key: key_of("gemini"),
            provider: Some(canonical_provider_key("gemini")),
            failed_at: 1234567890,
        };
        let serialized = toml::to_string(&failure).unwrap();
        let deserialized: ProviderFailure = toml::from_str(&serialized).unwrap();
        assert_eq!(deserialized.failed_at, 1234567890);
        assert_eq!(deserialized.key, key_of("gemini"));
        assert_eq!(deserialized.provider.as_deref(), Some("antigravity"));
    }

    // ============================================================
    // reorder_steps: 期限切れエントリとの組み合わせ
    // ============================================================

    #[test]
    fn test_reorder_providers_with_expired_and_active() {
        // 1つが期限切れ、1つがアクティブな降格状態
        let mut state = State::default();

        // 2時間前の失敗（期限切れ）
        let two_hours_ago = State::now() - (2 * 60 * 60);
        push_failure(&mut state, "gemini", two_hours_ago);

        // 直近の失敗（アクティブ）
        state.record_failure(&step("codex"));

        let steps = vec![step("gemini"), step("codex"), step("claude")];

        let reordered = state.reorder_steps(steps, 60);
        // geminiは期限切れなので通常位置、codexは末尾に移動
        assert_eq!(reordered[0].provider, "gemini");
        assert_eq!(reordered[1].provider, "claude");
        assert_eq!(reordered[2].provider, "codex");
    }

    #[test]
    fn test_cleanup_expired_large_cooldown() {
        // 非常に大きなクールダウン値: 全エントリが保持される
        let mut state = State::default();
        let old_failure = State::now() - (24 * 60 * 60); // 24時間前
        push_failure(&mut state, "gemini", old_failure);

        // 1週間のクールダウン
        state.cleanup_expired(7 * 24 * 60);
        assert!(has_failure(&state, "gemini"));
    }

    #[test]
    fn test_max_cooldown_does_not_overflow() {
        // 設定値が u64::MAX でも秒変換でパニックや桁あふれを起こさない
        let mut state = State::default();
        push_failure(&mut state, "gemini", 0);

        assert!(state.is_demoted(&step("gemini"), u64::MAX));

        state.cleanup_expired(u64::MAX);
        assert!(has_failure(&state, "gemini"));
    }

    #[test]
    fn test_get_demoted_providers_multiple_with_mixed_expiry() {
        // 複数プロバイダーで一部のみ期限切れ
        let mut state = State::default();

        // アクティブ
        state.record_failure(&step("gemini"));

        // 期限切れ
        let old = State::now() - (2 * 60 * 60);
        push_failure(&mut state, "codex", old);

        // アクティブ
        state.record_failure(&step("claude"));

        assert!(state.is_demoted(&step("gemini"), 60));
        assert!(state.is_demoted(&step("claude"), 60));
        assert!(!state.is_demoted(&step("codex"), 60));
    }

    #[test]
    fn test_reorder_providers_single_provider() {
        // プロバイダーが1つだけの場合
        let mut state = State::default();
        state.record_failure(&step("gemini"));

        let steps = vec![step("gemini")];
        let reordered = state.reorder_steps(steps, 60);
        // 降格されても1つしかないのでそのまま
        assert_eq!(reordered, vec![step("gemini")]);
    }

    #[test]
    fn test_save_to_path_writes_state_atomically() {
        // save_to_path は tempfile + rename によりアトミックに保存できる必要がある
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("state.toml");

        let mut state = State::default();
        state.record_failure(&step("gemini"));
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
        assert!(has_failure(&parsed, "gemini"));
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

        // 旧データ（旧 HashMap 形式）を先に書き込む。load() ではなく save_to_path の
        // 上書き挙動を確認するためのダミーなので、内容自体は読まれない。
        fs::write(&target, "provider_failures = { old = { failed_at = 0 } }\n").unwrap();

        let mut state = State::default();
        state.record_failure(&step("codex"));
        state.save_to_path(&target).unwrap();

        let content = fs::read_to_string(&target).unwrap();
        let parsed: State = toml::from_str(&content).unwrap();
        assert!(has_failure(&parsed, "codex"));
        // 新形式 State は failures(Vec) のみを読むので、旧 provider_failures の "old" は載らない
        assert!(!has_failure(&parsed, "old"));
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
                state.record_failure(&step(&format!("provider-{}", i)));
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
        assert!(!parsed.failures.is_empty());

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
    // 旧 gemini エイリアスの合流テスト
    // 旧 "gemini" 系キーが "antigravity" の cooldown_key に合流することを確認する。
    // 旧 migrate_legacy_gemini_key 関数は廃止され、両形式の解釈は load() の責務に
    // 移ったため、ここでは cooldown_key の合流（同一視）という不変条件のみを固定する。
    // ============================================================

    #[test]
    fn test_legacy_gemini_alias_merges_into_antigravity() {
        // 旧 provider 名 "gemini" / "agy" は antigravity と同じ cooldown_key を導出する。
        // これにより旧 gemini の失敗記録が antigravity ステップへ確実に効く。
        assert_eq!(key_of("gemini"), key_of("antigravity"));
        assert_eq!(key_of("agy"), key_of("antigravity"));
    }

    #[test]
    fn test_legacy_gemini_failure_demotes_antigravity_step() {
        // record_failure(step("gemini")) で記録した失敗が antigravity ステップを降格させる。
        // 旧 migrate_legacy_gemini_key が担っていた「gemini → antigravity 合流」の意図を、
        // 降格判定レベルで保証する。
        let mut state = State::default();
        state.record_failure(&step("gemini"));

        assert!(state.is_demoted(&step("antigravity"), 60));
        assert!(state.is_demoted(&step("agy"), 60));
        // 逆方向（antigravity 記録 → gemini ステップ降格）も成立する
        let mut state2 = State::default();
        state2.record_failure(&step("antigravity"));
        assert!(state2.is_demoted(&step("gemini"), 60));
    }

    #[test]
    fn test_legacy_apple_aliases_merge_into_apple_intelligence() {
        // 旧 apple-ai / apple_intelligence キーが apple-intelligence の cooldown_key に合流する。
        assert_eq!(key_of("apple-ai"), key_of("apple-intelligence"));
        assert_eq!(key_of("apple_intelligence"), key_of("apple-intelligence"));
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
            state.record_failure(&step(&format!("provider-{}", i)));
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

    // ============================================================
    // 複合キー: アカウント/モデル/コマンド別の独立クールダウン
    // (このリファクタの中核要件: 同一プロバイダでも env/model/command が違えば独立に降格)
    // ============================================================

    fn step_with_env(provider: &str, key: &str, val: &str) -> ProviderStep {
        let mut s = ProviderStep::from_provider(provider);
        s.env.insert(key.to_string(), val.to_string());
        s
    }

    fn step_with_model(provider: &str, model: &str) -> ProviderStep {
        let mut s = ProviderStep::from_provider(provider);
        s.model = Some(model.to_string());
        s
    }

    #[test]
    fn test_same_provider_different_account_demoted_independently() {
        // codex(CODEX_HOME=A) が失敗しても codex(CODEX_HOME=B) は生きる
        let mut state = State::default();
        let acct_a = step_with_env("codex", "CODEX_HOME", "/home/u/.codex");
        let acct_b = step_with_env("codex", "CODEX_HOME", "/home/u/.codex-work");
        state.record_failure(&acct_a);
        assert!(
            state.is_demoted(&acct_a, 60),
            "失敗したアカウントは降格される"
        );
        assert!(
            !state.is_demoted(&acct_b, 60),
            "別アカウント(別 env)は独立クォータなので降格されない"
        );
    }

    #[test]
    fn test_same_provider_different_model_demoted_independently() {
        // antigravity(Gemini系) が失敗しても antigravity(GPT-OSS系) は生きる
        let mut state = State::default();
        let gemini = step_with_model("antigravity", "Gemini 3.5 Flash (Low)");
        let gptoss = step_with_model("antigravity", "GPT-OSS 120B (Medium)");
        state.record_failure(&gemini);
        assert!(state.is_demoted(&gemini, 60));
        assert!(
            !state.is_demoted(&gptoss, 60),
            "別モデルは独立クォータなので降格されない"
        );
    }

    #[test]
    fn test_reorder_steps_demotes_only_failed_account() {
        let mut state = State::default();
        let acct_a = step_with_env("codex", "CODEX_HOME", "/home/u/.codex");
        let acct_b = step_with_env("codex", "CODEX_HOME", "/home/u/.codex-work");
        state.record_failure(&acct_a);
        let chain = vec![acct_a.clone(), acct_b.clone(), step("antigravity")];
        let reordered = state.reorder_steps(chain, 60);
        // 失敗した acct_a だけが末尾へ。acct_b と antigravity は元の順序を保つ。
        assert_eq!(reordered[0], acct_b);
        assert_eq!(reordered[1].provider, "antigravity");
        assert_eq!(reordered[2], acct_a);
    }

    #[test]
    fn test_name_distinguishes_cooldown_key() {
        // 同一 provider でも name が違えば独立に降格
        let mut a = ProviderStep::from_provider("codex");
        a.name = Some("acct1".to_string());
        let mut b = ProviderStep::from_provider("codex");
        b.name = Some("acct2".to_string());
        let mut state = State::default();
        state.record_failure(&a);
        assert!(state.is_demoted(&a, 60));
        assert!(!state.is_demoted(&b, 60));
    }

    #[test]
    fn test_command_distinguishes_cooldown_key() {
        // 同一 provider でも command(ラッパー)が違えば独立
        let mut a = ProviderStep::from_provider("codex");
        a.command = Some(vec!["/wrapper-a.sh".to_string()]);
        let mut b = ProviderStep::from_provider("codex");
        b.command = Some(vec!["/wrapper-b.sh".to_string()]);
        let mut state = State::default();
        state.record_failure(&a);
        assert!(state.is_demoted(&a, 60));
        assert!(!state.is_demoted(&b, 60));
    }

    // ============================================================
    // 旧 HashMap 形式 (`provider_failures`) → 新 Vec 形式 (`failures`) への移行
    // AGENTS.md に記載の不変条件:
    //   "State::load マイグレーションが旧 provider 名キーを正規 cooldown_key に
    //    変換し、gemini/apple-ai 系の legacy 値は antigravity/apple-intelligence に合流する"
    // ============================================================

    /// `legacy_key_to_new` が旧 provider 名を新 cooldown_key に変換する。
    /// 旧 `gemini` / `agy` → `antigravity` の cooldown_key、
    /// 旧 `apple-ai` / `apple_intelligence` → `apple-intelligence` の cooldown_key に合流する。
    #[test]
    fn test_legacy_key_to_new_canonicalizes_provider_aliases() {
        // antigravity 系
        assert_eq!(
            legacy_key_to_new("gemini"),
            ProviderStep::from_provider("antigravity").cooldown_key()
        );
        assert_eq!(
            legacy_key_to_new("agy"),
            ProviderStep::from_provider("antigravity").cooldown_key()
        );
        // apple 系
        assert_eq!(
            legacy_key_to_new("apple-ai"),
            ProviderStep::from_provider("apple-intelligence").cooldown_key()
        );
        assert_eq!(
            legacy_key_to_new("apple_intelligence"),
            ProviderStep::from_provider("apple-intelligence").cooldown_key()
        );
        // 通常 provider
        assert_eq!(
            legacy_key_to_new("codex"),
            ProviderStep::from_provider("codex").cooldown_key()
        );
    }

    /// 旧 HashMap 形式 (`provider_failures`) のみを含む TOML を `StateOnDisk` として
    /// パースできる。新形式の `failures` Vec とは独立してデシリアライズされる。
    #[test]
    fn test_state_on_disk_parses_legacy_provider_failures() {
        let toml_content = r#"
[provider_failures.gemini]
failed_at = 1700000000

[provider_failures.codex]
failed_at = 1700000001
"#;
        let disk: StateOnDisk = toml::from_str(toml_content).unwrap();
        assert_eq!(disk.provider_failures.len(), 2);
        assert!(disk.failures.is_empty());
        assert_eq!(disk.provider_failures["gemini"].failed_at, 1700000000);
        assert_eq!(disk.provider_failures["codex"].failed_at, 1700000001);
    }

    /// 旧 HashMap 形式と新 Vec 形式が両方含まれる TOML をパースし、
    /// 両方が独立してデシリアライズされる。
    #[test]
    fn test_state_on_disk_parses_mixed_legacy_and_new_format() {
        let toml_content = r#"
[[failures]]
key = "codex"
failed_at = 1800000000

[provider_failures.gemini]
failed_at = 1700000000
"#;
        let disk: StateOnDisk = toml::from_str(toml_content).unwrap();
        // 新形式の failures Vec は1件
        assert_eq!(disk.failures.len(), 1);
        assert_eq!(disk.failures[0].failed_at, 1800000000);
        // 旧形式の HashMap も1件残っている (load 内で移行される)
        assert_eq!(disk.provider_failures.len(), 1);
        assert_eq!(disk.provider_failures["gemini"].failed_at, 1700000000);
    }

    /// 旧 HashMap 形式の TOML から `State::load` 経由でロードした際、
    /// 旧 provider 名キーが新 cooldown_key に変換され、新 Vec 形式の `failures` に
    /// 移行されることを確認する。HOME 環境変数を操作するため、init.rs 等の
    /// HOME 操作テストと共有する `crate::test_support::lock_env()` で直列化する。
    #[test]
    fn test_load_migrates_legacy_provider_failures_hashmap_to_failures_vec() {
        // HOME を書き換えるため、他ファイル(init.rs 等)の HOME 操作テストと
        // 同一のプロセス共有ロックで直列化する。各ファイルがローカルに Mutex を
        // 持つと排他が効かず、HOME の取り違えで間欠的に失敗する。
        let _lock = crate::test_support::lock_env();

        let temp = tempfile::tempdir().unwrap();
        let original_home = std::env::var_os("HOME");
        unsafe {
            std::env::set_var("HOME", temp.path());
        }

        // 旧 HashMap 形式の状態ファイルを直接書き込む
        let path = State::state_path().unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"
[provider_failures.gemini]
failed_at = 1700000000

[provider_failures.codex]
failed_at = 1700000001

[provider_failures.apple-ai]
failed_at = 1700000002
"#,
        )
        .unwrap();

        let state = State::load().unwrap();

        // 旧 HashMap が新 Vec 形式の failures に移行される
        assert_eq!(state.failures.len(), 3);
        // 旧 `gemini` キーは `antigravity` の cooldown_key に合流する
        let antigravity_key = ProviderStep::from_provider("antigravity").cooldown_key();
        assert!(state.failures.iter().any(|f| f.key == antigravity_key));
        // 旧 `apple-ai` キーは `apple-intelligence` の cooldown_key に合流する
        let apple_key = ProviderStep::from_provider("apple-intelligence").cooldown_key();
        assert!(state.failures.iter().any(|f| f.key == apple_key));
        // `codex` はそのまま codex の cooldown_key として残る
        let codex_key = ProviderStep::from_provider("codex").cooldown_key();
        assert!(state.failures.iter().any(|f| f.key == codex_key));

        // HOME 環境変数を元に戻す
        unsafe {
            match original_home {
                Some(prev) => std::env::set_var("HOME", prev),
                None => std::env::remove_var("HOME"),
            }
        }
    }
}
