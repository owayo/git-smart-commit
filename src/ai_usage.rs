//! ai-usage --json 連携
//!
//! `[ai_usage] enabled = true` のとき、`AiService::from_config` は起動時に
//! `ai-usage --json` を 1 回だけ叩き、その結果をこのモジュールの
//! `AiUsageSnapshot` に格納する。各 `ProviderStep` はその provider (エイリアス
//! 正規化後) と `ai_usage_profile` を組み合わせて対応 account を検索し、
//! 5 時間枠 / 週次のいずれかで残量が閾値以上なら fallback chain から除外する。
//!
//! 設計方針:
//! - fail-open: ai-usage の実行失敗・JSON パース失敗・timeout は連携無効化として
//!   扱い、既存のフォールバックが動く(commit 処理は継続する)。連携が動くのは
//!   「使用可能な provider を積極的に絞り込む」ためであり、連携が壊れた瞬間に
//!   commit が止まってはならない。
//! - profile 未指定の step は「同一 provider の中で残量が最も少ない account」を
//!   自動採用する(auto-select)。ai-usage 側の複数アカウント (Work / Home 等) を
//!   config で明示せずとも、残量に余裕があるアカウントを自然に使えるようにする。
//!   複数アカウントを明示的に使い分けたい場合は step ごとに `ai_usage_profile`
//!   を指定する。
//! - window: `nearest` は weekly / five_hour のうち **使用率が高い方** を採用する。
//!   安全側判定(閾値超過を早く検出)を優先する。

use std::io::{BufReader, Read};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::config::{AiUsageConfig, AiUsageWindow, ProviderStep, canonical_provider_key};
use crate::error::AppError;

/// ai-usage --json のトップレベル出力(必要なフィールドのみ抜粋)。
#[derive(Debug, Deserialize)]
struct AiUsageOutput {
    #[serde(default)]
    accounts: Vec<AiUsageAccount>,
}

/// 1 アカウント(profile × provider)分の残量情報。
#[derive(Debug, Deserialize, Clone)]
pub struct AiUsageAccount {
    pub profile: String,
    pub provider: String,
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub weekly: Option<UsageWindowData>,
    #[serde(default)]
    pub five_hour: Option<UsageWindowData>,
    #[serde(default)]
    pub error: Option<String>,
}

/// 各枠(weekly / five_hour)の残量。ai-usage 出力の一部フィールドのみ抜粋。
#[derive(Debug, Deserialize, Clone)]
pub struct UsageWindowData {
    #[serde(default)]
    pub used_percent: Option<f64>,
}

/// ai-usage の結果スナップショット。
#[derive(Debug, Clone, Default)]
pub struct AiUsageSnapshot {
    accounts: Vec<AiUsageAccount>,
}

/// step の評価結果。
#[derive(Debug, Clone, PartialEq)]
pub enum UsageDecision {
    /// 使用可能。reason は debug 表示用。
    Usable { reason: String },
    /// 閾値以上のため fallback chain から除外。
    OverThreshold { reason: String },
    /// snapshot に該当 account が無い / ok=false / データ欠損。
    /// 意図せず fallback chain から provider が消える事故を避けるため、
    /// 「不明なら残す (Usable 扱い)」の運用にする(fail-open の一種)。
    /// reason は debug 表示用。
    NoAccount { reason: String },
}

impl UsageDecision {
    /// この decision が「chain に残してよい」判定か。
    pub fn is_usable(&self) -> bool {
        matches!(
            self,
            UsageDecision::Usable { .. } | UsageDecision::NoAccount { .. }
        )
    }
    /// debug 表示用の 1 行理由。
    pub fn reason(&self) -> &str {
        match self {
            UsageDecision::Usable { reason }
            | UsageDecision::OverThreshold { reason }
            | UsageDecision::NoAccount { reason } => reason,
        }
    }
}

impl AiUsageSnapshot {
    /// 手動で snapshot を組み立てる(テスト用)。
    #[cfg(test)]
    pub fn from_accounts(accounts: Vec<AiUsageAccount>) -> Self {
        Self { accounts }
    }

    /// snapshot 内の全アカウントを返す(--debug 用の参照)。
    pub fn accounts(&self) -> &[AiUsageAccount] {
        &self.accounts
    }

    /// 与えられた step を評価する。
    pub fn evaluate(
        &self,
        step: &ProviderStep,
        window: AiUsageWindow,
        threshold_percent: f64,
    ) -> UsageDecision {
        let provider_key = canonical_provider_key(&step.provider);

        // 明示 profile 指定: (profile, provider) で一意 lookup。大文字小文字を区別する
        // (ai-usage の `profile` は Chrome の profile 表示名で、そのまま照合する)。
        if let Some(profile) = step.ai_usage_profile.as_deref() {
            let matched: Vec<&AiUsageAccount> = self
                .accounts
                .iter()
                .filter(|a| a.profile == profile)
                .filter(|a| canonical_provider_key(&a.provider) == provider_key)
                .collect();
            let Some(acc) = matched.into_iter().next() else {
                return UsageDecision::NoAccount {
                    reason: format!(
                        "no ai-usage account for (profile={profile}, provider={provider_key})"
                    ),
                };
            };
            if !acc.ok {
                // ok=false は認証失敗など。残量不明として NoAccount 扱い(fail-open)。
                let err = acc.error.as_deref().unwrap_or("ok=false");
                return UsageDecision::NoAccount {
                    reason: format!("account {}/{} not ok: {err}", acc.profile, acc.provider),
                };
            }
            return evaluate_account(acc, window, threshold_percent);
        }

        // profile 未指定: provider 一致の ok=true な account を全部集めて、
        // 最も残量の多い(=used_percent が小さい)ものを採用する。全て閾値超過なら
        // OverThreshold。1 件も無ければ NoAccount(残す = fail-open)。
        let matched: Vec<&AiUsageAccount> = self
            .accounts
            .iter()
            .filter(|a| a.ok)
            .filter(|a| canonical_provider_key(&a.provider) == provider_key)
            .collect();
        if matched.is_empty() {
            return UsageDecision::NoAccount {
                reason: format!("no ok ai-usage account for provider={provider_key}"),
            };
        }

        // 使用率が最も低い(=最も残量が多い) account を選ぶ。
        let mut best: Option<(f64, &AiUsageAccount)> = None;
        for acc in &matched {
            let used = window_used_percent(acc, window);
            best = match best {
                None => Some((used, *acc)),
                Some((prev, _)) if used < prev => Some((used, *acc)),
                other => other,
            };
        }
        let (used, acc) = best.expect("matched non-empty");
        if used >= threshold_percent {
            UsageDecision::OverThreshold {
                reason: format!(
                    "provider={provider_key} best account={} used {:.0}% >= threshold {:.0}%",
                    acc.profile, used, threshold_percent
                ),
            }
        } else {
            UsageDecision::Usable {
                reason: format!(
                    "auto-selected {} (used {:.0}%, threshold {:.0}%)",
                    acc.profile, used, threshold_percent
                ),
            }
        }
    }
}

/// 単一アカウントを window / threshold で評価する。
fn evaluate_account(
    acc: &AiUsageAccount,
    window: AiUsageWindow,
    threshold_percent: f64,
) -> UsageDecision {
    let used = window_used_percent(acc, window);
    if used >= threshold_percent {
        UsageDecision::OverThreshold {
            reason: format!(
                "profile={} provider={} used {:.0}% >= threshold {:.0}%",
                acc.profile, acc.provider, used, threshold_percent
            ),
        }
    } else {
        UsageDecision::Usable {
            reason: format!(
                "profile={} provider={} used {:.0}% (threshold {:.0}%)",
                acc.profile, acc.provider, used, threshold_percent
            ),
        }
    }
}

/// 指定 window の used_percent を返す。欠損時は 0.0(fail-open 側)。
/// `nearest` は weekly / five_hour の **最大値** (=安全側判定を早める)。
fn window_used_percent(acc: &AiUsageAccount, window: AiUsageWindow) -> f64 {
    let weekly = acc.weekly.as_ref().and_then(|w| w.used_percent);
    let five_hour = acc.five_hour.as_ref().and_then(|w| w.used_percent);
    match window {
        AiUsageWindow::Weekly => weekly.unwrap_or(0.0),
        AiUsageWindow::FiveHour => five_hour.unwrap_or(0.0),
        AiUsageWindow::Nearest => match (weekly, five_hour) {
            (Some(a), Some(b)) => a.max(b),
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => 0.0,
        },
    }
}

/// ai-usage --json を実行してスナップショットを取得する。
///
/// fail-open のため、呼び出し側 (`AiService::from_config`) はエラーを警告として
/// 表示し、連携無効化として扱う(fallback chain は元のまま動く)。
pub fn fetch_snapshot(config: &AiUsageConfig) -> Result<AiUsageSnapshot, AppError> {
    if config.command.is_empty() {
        return Err(AppError::AiUsageError("ai_usage.command is empty".into()));
    }
    let executable = config.command[0].trim();
    if executable.is_empty() {
        return Err(AppError::AiUsageError(
            "ai_usage.command executable must not be empty".into(),
        ));
    }
    let stdout = run_command_with_timeout(&config.command, config.timeout_seconds)?;
    parse_ai_usage_output(&stdout)
}

/// JSON バイト列を snapshot にパースする。
fn parse_ai_usage_output(bytes: &[u8]) -> Result<AiUsageSnapshot, AppError> {
    let parsed: AiUsageOutput = serde_json::from_slice(bytes)
        .map_err(|e| AppError::AiUsageError(format!("failed to parse ai-usage JSON: {e}")))?;
    Ok(AiUsageSnapshot {
        accounts: parsed.accounts,
    })
}

/// ai-usage をタイムアウト付きで実行し、stdout の raw バイト列を返す。
///
/// stdout / stderr は別スレッドで並行読み取りする。ai-usage の JSON はしばしば
/// パイプバッファ(数十 KB)を超えるので、`try_wait` だけでは子プロセスが書き込み
/// でブロックしたまま親が待ち続けるデッドロックが起きる。並行 reader がドレインし
/// 続けることでこれを防ぐ。
fn run_command_with_timeout(command: &[String], timeout_secs: u64) -> Result<Vec<u8>, AppError> {
    let mut child = Command::new(&command[0])
        .args(&command[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AppError::AiUsageError(format!("failed to spawn ai-usage: {e}")))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::AiUsageError("ai-usage stdout unavailable".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AppError::AiUsageError("ai-usage stderr unavailable".into()))?;

    let stdout_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = BufReader::new(stdout).read_to_end(&mut buf);
        buf
    });
    let stderr_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = BufReader::new(stderr).read_to_end(&mut buf);
        buf
    });

    let timeout = Duration::from_secs(timeout_secs);
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    // reader スレッドは EOF で自然終了する。ここでは join せずに
                    // 落として構わない(戻り値を捨てるだけなので detach でも
                    // メモリリークにはならない)。
                    let _ = stdout_handle.join();
                    let _ = stderr_handle.join();
                    return Err(AppError::AiUsageError(format!(
                        "ai-usage timed out after {timeout_secs}s"
                    )));
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_handle.join();
                let _ = stderr_handle.join();
                return Err(AppError::AiUsageError(format!("ai-usage wait failed: {e}")));
            }
        }
    };

    let stdout_buf = stdout_handle.join().unwrap_or_default();
    let stderr_buf = stderr_handle.join().unwrap_or_default();

    if !status.success() {
        return Err(AppError::AiUsageError(format!(
            "ai-usage exited with {}: {}",
            status,
            String::from_utf8_lossy(&stderr_buf).trim()
        )));
    }
    Ok(stdout_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window_data(used_percent: Option<f64>) -> UsageWindowData {
        UsageWindowData { used_percent }
    }

    fn account(
        profile: &str,
        provider: &str,
        ok: bool,
        weekly: Option<f64>,
        five_hour: Option<f64>,
    ) -> AiUsageAccount {
        AiUsageAccount {
            profile: profile.to_string(),
            provider: provider.to_string(),
            ok,
            weekly: Some(window_data(weekly)),
            five_hour: Some(window_data(five_hour)),
            error: None,
        }
    }

    fn step_with_profile(provider: &str, profile: Option<&str>) -> ProviderStep {
        let mut step = ProviderStep::from_provider(provider);
        step.ai_usage_profile = profile.map(String::from);
        step
    }

    #[test]
    fn parse_ai_usage_output_ok() {
        let json = br#"{"accounts":[{"profile":"Work","provider":"claude","ok":true,"weekly":{"used_percent":72.0},"five_hour":{"used_percent":5.0}}]}"#;
        let snapshot = parse_ai_usage_output(json).unwrap();
        assert_eq!(snapshot.accounts.len(), 1);
        assert_eq!(snapshot.accounts[0].profile, "Work");
        assert_eq!(snapshot.accounts[0].provider, "claude");
        assert!(snapshot.accounts[0].ok);
        assert_eq!(
            snapshot.accounts[0]
                .weekly
                .as_ref()
                .and_then(|w| w.used_percent),
            Some(72.0)
        );
    }

    #[test]
    fn parse_ai_usage_output_ignores_unknown_fields() {
        // ai-usage 出力に未知フィールド(email, plan, group_label 等)があっても壊れない。
        let json = br#"{"generated_at":"2026-07-01T00:00:00Z","accounts":[{"profile":"Work","provider":"codex","ok":true,"plan":"team","email":"user@example.com","weekly":{"used_percent":10.0,"resets_at":"..."}}]}"#;
        let snapshot = parse_ai_usage_output(json).unwrap();
        assert_eq!(snapshot.accounts.len(), 1);
    }

    #[test]
    fn parse_ai_usage_output_rejects_invalid_json() {
        let result = parse_ai_usage_output(b"{ not json");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("failed to parse"));
    }

    #[test]
    fn evaluate_explicit_profile_over_threshold_weekly() {
        // profile 明示 + weekly 判定で 95% 閾値超過 → OverThreshold。
        let snap = AiUsageSnapshot::from_accounts(vec![account(
            "Work",
            "claude",
            true,
            Some(96.0),
            Some(1.0),
        )]);
        let step = step_with_profile("claude", Some("Work"));
        let decision = snap.evaluate(&step, AiUsageWindow::Weekly, 95.0);
        assert!(matches!(decision, UsageDecision::OverThreshold { .. }));
        assert!(!decision.is_usable());
    }

    #[test]
    fn evaluate_explicit_profile_usable_five_hour() {
        // profile 明示 + five_hour 判定で余裕あり → Usable。
        let snap = AiUsageSnapshot::from_accounts(vec![account(
            "Work",
            "codex",
            true,
            Some(90.0),
            Some(20.0),
        )]);
        let step = step_with_profile("codex", Some("Work"));
        let decision = snap.evaluate(&step, AiUsageWindow::FiveHour, 95.0);
        assert!(matches!(decision, UsageDecision::Usable { .. }));
        assert!(decision.is_usable());
    }

    #[test]
    fn evaluate_nearest_picks_higher_used_percent() {
        // nearest は weekly / five_hour のうち **最大値** を採用する(安全側)。
        // weekly=72, five_hour=1 → nearest=72 → 閾値 60 に対して超過。
        let snap = AiUsageSnapshot::from_accounts(vec![account(
            "Work",
            "claude",
            true,
            Some(72.0),
            Some(1.0),
        )]);
        let step = step_with_profile("claude", Some("Work"));
        let decision = snap.evaluate(&step, AiUsageWindow::Nearest, 60.0);
        assert!(matches!(decision, UsageDecision::OverThreshold { .. }));
    }

    #[test]
    fn evaluate_missing_profile_returns_no_account() {
        // 明示された profile が snapshot に無い → NoAccount(fail-open で残す)。
        let snap = AiUsageSnapshot::from_accounts(vec![account(
            "Home",
            "claude",
            true,
            Some(1.0),
            Some(1.0),
        )]);
        let step = step_with_profile("claude", Some("Work"));
        let decision = snap.evaluate(&step, AiUsageWindow::Nearest, 95.0);
        assert!(matches!(decision, UsageDecision::NoAccount { .. }));
        assert!(decision.is_usable(), "NoAccount は chain から除外しない");
    }

    #[test]
    fn evaluate_profile_ok_false_returns_no_account_fail_open() {
        // ok=false の account は「残量不明」として NoAccount 扱い。fail-open。
        let snap = AiUsageSnapshot::from_accounts(vec![AiUsageAccount {
            profile: "Work".into(),
            provider: "claude".into(),
            ok: false,
            weekly: None,
            five_hour: None,
            error: Some("cloudflare challenge".into()),
        }]);
        let step = step_with_profile("claude", Some("Work"));
        let decision = snap.evaluate(&step, AiUsageWindow::Nearest, 95.0);
        assert!(matches!(decision, UsageDecision::NoAccount { .. }));
        assert!(decision.reason().contains("cloudflare challenge"));
    }

    #[test]
    fn evaluate_auto_select_picks_lowest_used_provider() {
        // profile 未指定: 同一 provider の中で used_percent が最も低い account を採用。
        let snap = AiUsageSnapshot::from_accounts(vec![
            account("Work", "claude", true, Some(90.0), Some(90.0)),
            account("Home", "claude", true, Some(10.0), Some(1.0)),
        ]);
        let step = step_with_profile("claude", None);
        let decision = snap.evaluate(&step, AiUsageWindow::Nearest, 95.0);
        assert!(matches!(decision, UsageDecision::Usable { .. }));
        assert!(decision.reason().contains("Home"));
    }

    #[test]
    fn evaluate_auto_select_all_over_threshold() {
        // profile 未指定で全 account が閾値超過 → OverThreshold。
        let snap = AiUsageSnapshot::from_accounts(vec![
            account("Work", "claude", true, Some(99.0), Some(99.0)),
            account("Home", "claude", true, Some(96.0), Some(97.0)),
        ]);
        let step = step_with_profile("claude", None);
        let decision = snap.evaluate(&step, AiUsageWindow::Nearest, 95.0);
        assert!(matches!(decision, UsageDecision::OverThreshold { .. }));
        // 最も残量が多い(=Home の weekly 96, five 97, 高い方 97)が採用され、
        // それでも 95 以上なので OverThreshold。
        assert!(decision.reason().contains("Home"));
    }

    #[test]
    fn evaluate_auto_select_no_matching_provider() {
        // 該当 provider の account が snapshot に無い → NoAccount(fail-open)。
        let snap = AiUsageSnapshot::from_accounts(vec![account(
            "Work",
            "codex",
            true,
            Some(1.0),
            Some(1.0),
        )]);
        let step = step_with_profile("claude", None);
        let decision = snap.evaluate(&step, AiUsageWindow::Nearest, 95.0);
        assert!(matches!(decision, UsageDecision::NoAccount { .. }));
        assert!(decision.is_usable());
    }

    #[test]
    fn evaluate_uses_canonical_provider_for_gemini_alias() {
        // step.provider = "gemini" (旧エイリアス) と snapshot の "antigravity" が
        // canonical 経由でマッチする。
        let snap = AiUsageSnapshot::from_accounts(vec![account(
            "Work",
            "antigravity",
            true,
            Some(0.0),
            Some(0.0),
        )]);
        let step = step_with_profile("gemini", None);
        let decision = snap.evaluate(&step, AiUsageWindow::Nearest, 95.0);
        assert!(
            matches!(decision, UsageDecision::Usable { .. }),
            "gemini エイリアスは antigravity と同一 provider として扱われるべき: {decision:?}"
        );
    }

    #[test]
    fn evaluate_window_missing_data_treated_as_zero() {
        // window の used_percent が欠損している場合は 0.0 として扱う(fail-open 側)。
        let snap = AiUsageSnapshot::from_accounts(vec![AiUsageAccount {
            profile: "Work".into(),
            provider: "claude".into(),
            ok: true,
            weekly: None,
            five_hour: None,
            error: None,
        }]);
        let step = step_with_profile("claude", Some("Work"));
        let decision = snap.evaluate(&step, AiUsageWindow::Nearest, 95.0);
        assert!(matches!(decision, UsageDecision::Usable { .. }));
    }

    #[test]
    fn fetch_snapshot_rejects_empty_command() {
        let cfg = AiUsageConfig {
            enabled: true,
            command: vec![],
            threshold_percent: 95.0,
            window: AiUsageWindow::Nearest,
            timeout_seconds: 5,
        };
        let err = fetch_snapshot(&cfg).unwrap_err();
        assert!(err.to_string().contains("command is empty"));
    }

    #[test]
    fn fetch_snapshot_rejects_whitespace_executable() {
        let cfg = AiUsageConfig {
            enabled: true,
            command: vec!["   ".into()],
            threshold_percent: 95.0,
            window: AiUsageWindow::Nearest,
            timeout_seconds: 5,
        };
        let err = fetch_snapshot(&cfg).unwrap_err();
        assert!(err.to_string().contains("executable must not be empty"));
    }

    #[test]
    fn fetch_snapshot_returns_error_on_nonexistent_command() {
        // 存在しないバイナリは fail-open のため呼び出し側で警告される。
        let cfg = AiUsageConfig {
            enabled: true,
            command: vec!["/nonexistent/ai-usage-does-not-exist".into()],
            threshold_percent: 95.0,
            window: AiUsageWindow::Nearest,
            timeout_seconds: 5,
        };
        let err = fetch_snapshot(&cfg).unwrap_err();
        assert!(err.to_string().contains("failed to spawn"));
    }

    #[test]
    fn fetch_snapshot_returns_error_on_nonzero_exit() {
        // 標準の false は exit 1 を返す。stderr を含んだエラーメッセージが返る。
        let cfg = AiUsageConfig {
            enabled: true,
            command: vec!["false".into()],
            threshold_percent: 95.0,
            window: AiUsageWindow::Nearest,
            timeout_seconds: 5,
        };
        let err = fetch_snapshot(&cfg).unwrap_err();
        assert!(err.to_string().contains("exited with"));
    }

    #[test]
    fn fetch_snapshot_parses_valid_json_from_command() {
        // printf などで固定 JSON を吐かせる。POSIX の /bin/sh -c を使う。
        let cfg = AiUsageConfig {
            enabled: true,
            command: vec![
                "sh".into(),
                "-c".into(),
                r#"printf '{"accounts":[{"profile":"Work","provider":"claude","ok":true,"weekly":{"used_percent":5.0},"five_hour":{"used_percent":1.0}}]}'"#.into(),
            ],
            threshold_percent: 95.0,
            window: AiUsageWindow::Nearest,
            timeout_seconds: 5,
        };
        let snap = fetch_snapshot(&cfg).unwrap();
        assert_eq!(snap.accounts().len(), 1);
        assert_eq!(snap.accounts()[0].profile, "Work");
    }

    #[test]
    fn fetch_snapshot_reports_timeout() {
        // sleep 60 は 60 秒待つ。timeout=1 で強制終了して timeout エラー。
        let cfg = AiUsageConfig {
            enabled: true,
            command: vec!["sh".into(), "-c".into(), "sleep 60".into()],
            threshold_percent: 95.0,
            window: AiUsageWindow::Nearest,
            timeout_seconds: 1,
        };
        let err = fetch_snapshot(&cfg).unwrap_err();
        assert!(
            err.to_string().contains("timed out"),
            "expected timeout error, got: {}",
            err
        );
    }
}
