//! 開発者向け生成ログ
//!
//! 「どのプロンプトを渡して、どのメッセージが返ってきたか」を実行単位で残し、
//! プロンプト改善の効果測定に使う。既定は無効で、グローバル設定でのみ有効化できる。
//!
//! 設計上の要点:
//!
//! - **1 実行 = 1 ファイル**。git-sc は hook から起動される短命プロセスで、複数の
//!   リポジトリで同時に走る。共有 JSONL への追記は、1 レコードが数十 KB になる
//!   この用途では行の混在を防げない(通常ファイルへの `O_APPEND` は書き込み位置の
//!   競合を防ぐだけで、大きな書き込みが分割されない保証はない)。プロセス間ロックを
//!   足すより、実行ごとにファイルを分ける方が単純で確実。解析側は
//!   `find … -name '*.json' | xargs jq -c .` で JSONL 化できる。
//! - **一時ファイル + rename で公開する**。途中終了しても、書きかけのファイルが
//!   完成品として解析対象に混ざらない。`State::save` と同じ手口。
//! - **fail-open**。ログが書けなくてもコミットは絶対に止めない。
//! - **生応答を clean 前に残す**。`<test>fix: x</test>` と `fix: x` は整形後に
//!   同じ文字列になるため、事故の再現には生応答が要る。

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::fs::{self, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use chrono::Local;
use serde::Serialize;

use crate::config::{Config, DevLogConfig};

/// ログのスキーマ版。フィールドの削除より nullable 化を優先し、増やすときだけ上げる。
const SCHEMA_VERSION: u32 = 1;

/// 1 回の応答から記録する stdout/stderr の上限。これを超えた分は切り詰め、
/// 切り詰めたことをフラグで残す(モデル側の打ち切りと混同しないため)。
const MAX_CAPTURE_BYTES: usize = 256 * 1024;

/// `.cleanup-stamp` の間隔。短命プロセスが毎回全ファイルを走査しないための間引き。
const CLEANUP_INTERVAL_SECS: u64 = 24 * 60 * 60;

/// 一時ファイル名の衝突を避けるためのプロセス内カウンタ
static RUN_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 記録の詳細度
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ContentLevel {
    /// プロンプト全文(= staged diff)は残さず、統計とハッシュだけを残す
    Metadata,
    /// 実際に送ったプロンプトをそのまま残す
    Full,
}

impl ContentLevel {
    fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "full" => Self::Full,
            // 未知の値は安全側(差分を残さない)に倒す
            _ => Self::Metadata,
        }
    }
}

/// プロバイダー 1 回分の呼び出し記録
#[derive(Debug, Clone, Serialize)]
pub struct AttemptRecord {
    /// チェーン内での通し番号(リトライも 1 回として数える)
    pub index: usize,
    pub provider: String,
    pub step_label: String,
    pub model: String,
    /// この step が上書きした環境変数の **名前だけ**。値は資格情報を含みうるので残さない
    pub env_keys: Vec<String>,
    /// 同一 step 内での試行回数(0 = 初回、1 = 引き直し)
    pub retry: u32,
    pub duration_ms: u64,
    /// 整形前の生応答
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_stdout: Option<String>,
    pub raw_stdout_bytes: usize,
    /// ロガーが上限で切り詰めたか。モデル側の打ち切りと区別するために必ず持たせる
    pub raw_stdout_truncated_by_logger: bool,
    /// `content = "full"` のときだけ本文を持つ。`metadata` では、Codex のように
    /// プロンプトを stderr へエコーするプロバイダーから diff が漏れるため落とす
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr_excerpt: Option<String>,
    pub stderr_bytes: usize,
    /// 応答全体を包んでいたタグ名(`<commit>` 以外)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub envelope_tag: Option<String>,
    /// 整形後のメッセージ
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// 品質判定に引っかかった項目。同時に複数成立しうるので配列で持つ
    pub findings: Vec<String>,
    /// この試行の帰結: accepted / retry / fallback / error
    pub decision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 実行 1 回分のログ
#[derive(Debug, Clone, Serialize)]
struct RunRecord {
    schema_version: u32,
    run_id: String,
    started_at: String,
    started_at_unix_ms: u64,
    duration_ms: u64,
    git_sc_version: &'static str,
    content_level: ContentLevel,
    invocation: Invocation,
    repository: Repository,
    input: Input,
    prompt: PromptInfo,
    provider_plan: Vec<String>,
    attempts: Vec<AttemptRecord>,
    result: RunResult,
}

/// 呼び出し方(どのモードで、どのフラグで起動されたか)
#[derive(Debug, Clone, Serialize, Default)]
pub struct Invocation {
    pub mode: String,
    pub quiet: bool,
    pub auto_confirm: bool,
    pub dry_run: bool,
    pub with_body: bool,
    pub stage_all: bool,
    /// AI CLI の hook から呼ばれたか(`CLAW_HOOKS_AGENT_MESSAGE` の有無で判断)
    pub from_agent_hook: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Repository {
    pub path: Option<String>,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Input {
    pub diff_bytes: usize,
    pub diff_lines: usize,
    pub files_changed: usize,
    pub diff_digest: String,
    pub language: String,
    pub prefix_mode: String,
    pub recent_commits: Vec<String>,
    pub agent_context_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct PromptInfo {
    pub bytes: usize,
    pub digest: String,
    /// `content = "full"` のときだけプロンプト全文
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// 実行の結末
///
/// どのモードでの結末かは `invocation.mode` が持つので、status は
/// `committed` / `dry-run` / `declined` / `generated` / `failed` に統一する。
#[derive(Debug, Clone, Serialize, Default)]
pub struct RunResult {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// 実際に使われたメッセージ。プレフィックス適用前の生成結果は
    /// `attempts[].message` に残るので、ここでは適用後だけを持つ
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 実行中に少しずつ埋まっていく run 情報
///
/// 呼び出し方は `App`、プロンプトと入力の特徴は `AiService`、結末は再び `App` と、
/// 情報の出どころが層をまたぐ。各層が知った時点で置いていける入れ物にしておき、
/// 最後に `finish` が 1 つのレコードへまとめる。
#[derive(Debug, Default)]
struct RunContext {
    invocation: Invocation,
    repository: Repository,
    input: Input,
    /// AI へ渡したプロンプト。未設定 = 生成まで至らなかった実行
    prompt: Option<String>,
    provider_plan: Vec<String>,
    result: RunResult,
}

/// 生成ログの収集と書き出し
///
/// `AiService` が試行ごとに `record_attempt` を呼び、`App` が最後に `finish` する。
/// 記録は best-effort で、失敗しても呼び出し側には影響させない。
#[derive(Debug)]
pub struct DevLog {
    dir: PathBuf,
    content_level: ContentLevel,
    retention_days: u64,
    max_total_bytes: u64,
    quiet: bool,
    started: Instant,
    started_at_unix_ms: u64,
    run: RefCell<RunContext>,
    attempts: RefCell<Vec<AttemptRecord>>,
    /// 警告は 1 プロセスにつき 1 回だけ出す
    warned: RefCell<bool>,
}

impl DevLog {
    /// 設定から構築する。無効なら `None`。
    pub fn from_config(config: &Config, quiet: bool) -> Option<Self> {
        let dev_log = config.dev_log.as_ref()?;
        if !dev_log.enabled {
            return None;
        }
        let dir = Self::resolve_dir(dev_log)?;
        let started_at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        Some(Self {
            dir,
            content_level: ContentLevel::parse(&dev_log.content),
            retention_days: dev_log.retention_days,
            max_total_bytes: dev_log.max_total_mb.saturating_mul(1024 * 1024),
            quiet,
            started: Instant::now(),
            started_at_unix_ms,
            run: RefCell::new(RunContext::default()),
            attempts: RefCell::new(Vec::new()),
            warned: RefCell::new(false),
        })
    }

    /// 出力先ディレクトリを決める(既定は `~/.config/git-sc/logs`)
    ///
    /// 既定値は設定ファイルと同じ `Config::config_dir()` から導く。macOS の
    /// `dirs::config_dir()` は `~/Library/Application Support` を返すため、
    /// そちらを使うと設定ファイルとログの置き場所が食い違う。
    fn resolve_dir(dev_log: &DevLogConfig) -> Option<PathBuf> {
        match &dev_log.dir {
            Some(dir) if !dir.trim().is_empty() => {
                Some(PathBuf::from(shellexpand::tilde(dir).into_owned()))
            }
            _ => Config::config_dir().map(|base| base.join("logs")),
        }
    }

    /// 記録の詳細度
    pub fn content_level(&self) -> ContentLevel {
        self.content_level
    }

    /// どのモード・どのフラグで起動されたかを記録する
    pub fn set_invocation(&self, invocation: Invocation) {
        self.run.borrow_mut().invocation = invocation;
    }

    /// 対象リポジトリを記録する
    pub fn set_repository(&self, repository: Repository) {
        self.run.borrow_mut().repository = repository;
    }

    /// 生成の入力(プロンプトと、その素材の特徴)を記録する
    ///
    /// プロンプト全文を残すかは詳細度設定に従う。ここでは常に受け取り、
    /// 書き出し時に `content_level` で振り分ける。
    pub fn set_generation_input(&self, input: Input, prompt: &str, provider_plan: Vec<String>) {
        let mut run = self.run.borrow_mut();
        run.input = input;
        run.prompt = Some(prompt.to_string());
        run.provider_plan = provider_plan;
    }

    /// 実行の結末を記録する
    pub fn set_result(&self, result: RunResult) {
        self.run.borrow_mut().result = result;
    }

    /// 試行を 1 件記録する
    pub fn record_attempt(&self, attempt: AttemptRecord) {
        self.attempts.borrow_mut().push(attempt);
    }

    /// 直前に記録した試行の帰結を上書きする
    ///
    /// 品質判定(打ち切り・連結・タグ残骸)は `call_provider` が返った後に走るため、
    /// 判定結果と最終的な採否は記録後に確定する。
    pub fn update_last_attempt(
        &self,
        message: Option<String>,
        findings: Vec<String>,
        decision: &str,
    ) {
        if let Some(last) = self.attempts.borrow_mut().last_mut() {
            if message.is_some() {
                last.message = message;
            }
            last.findings = findings;
            last.decision = decision.to_string();
        }
    }

    pub fn attempt_count(&self) -> usize {
        self.attempts.borrow().len()
    }

    /// 実行結果を書き出す
    ///
    /// 生成に至らなかった実行(ステージ済みの変更が無い等)は記録しない。改善パイプラインで
    /// 見たいのは「プロンプトと応答の対」であって、起動回数ではないため。
    ///
    /// 書き込みに失敗してもエラーは返さない。ログのために生成やコミットを
    /// 止めるのは本末転倒なので、警告を 1 行出して続行する。
    pub fn finish(&self, error: Option<String>) {
        let run = self.run.borrow();
        let Some(prompt) = run.prompt.as_deref() else {
            return;
        };

        let mut result = run.result.clone();
        if result.status.is_empty() {
            // set_result まで到達しなかった = 途中で失敗した実行
            result.status = "failed".to_string();
        }
        if result.error.is_none() {
            result.error = error;
        }

        let run_id = Self::new_run_id(self.started_at_unix_ms);
        let record = RunRecord {
            schema_version: SCHEMA_VERSION,
            run_id: run_id.clone(),
            started_at: Local::now().to_rfc3339(),
            started_at_unix_ms: self.started_at_unix_ms,
            duration_ms: self.started.elapsed().as_millis() as u64,
            git_sc_version: env!("CARGO_PKG_VERSION"),
            content_level: self.content_level,
            invocation: run.invocation.clone(),
            repository: run.repository.clone(),
            input: run.input.clone(),
            prompt: PromptInfo {
                bytes: prompt.len(),
                digest: digest(prompt),
                content: match self.content_level {
                    ContentLevel::Full => Some(prompt.to_string()),
                    ContentLevel::Metadata => None,
                },
            },
            provider_plan: run.provider_plan.clone(),
            attempts: self.attempts.borrow().clone(),
            result,
        };

        if let Err(e) = self.write_record(&run_id, &record) {
            self.warn(&format!("generation log could not be written: {e}"));
        }
        if let Err(e) = self.cleanup() {
            self.warn(&format!("generation log cleanup failed: {e}"));
        }
    }

    /// 一時ファイルへ書ききってから rename で公開する
    fn write_record(&self, run_id: &str, record: &RunRecord) -> Result<(), String> {
        let day_dir = self.dir.join(Local::now().format("%Y-%m-%d").to_string());
        create_dir_private(&day_dir)?;

        let tmp_path = day_dir.join(format!(".{run_id}.tmp"));
        let final_path = day_dir.join(format!("{run_id}.json"));

        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // プロンプト全文が入りうるので、他ユーザーから読めないようにする
            options.mode(0o600);
        }

        let mut file = options
            .open(&tmp_path)
            .map_err(|e| format!("{}: {e}", tmp_path.display()))?;

        let write_result = serde_json::to_vec_pretty(record)
            .map_err(|e| e.to_string())
            .and_then(|mut bytes| {
                bytes.push(b'\n');
                file.write_all(&bytes).map_err(|e| e.to_string())
            });

        if let Err(e) = write_result {
            drop(file);
            let _ = fs::remove_file(&tmp_path);
            return Err(e);
        }
        drop(file);

        fs::rename(&tmp_path, &final_path).map_err(|e| {
            let _ = fs::remove_file(&tmp_path);
            format!("{}: {e}", final_path.display())
        })
    }

    /// 保持期間と容量上限を超えたログを削除する
    ///
    /// 毎回の全走査を避けるため、`.cleanup-stamp` の mtime が 24 時間以内なら何もしない。
    /// 複数プロセスが同時に走っても、消すのは完成済みファイルだけで、
    /// 「既に無い」は成功扱いなので競合しても壊れない。
    fn cleanup(&self) -> Result<(), String> {
        if !self.dir.exists() {
            return Ok(());
        }
        let stamp = self.dir.join(".cleanup-stamp");
        if let Ok(metadata) = fs::metadata(&stamp)
            && let Ok(modified) = metadata.modified()
            && let Ok(elapsed) = modified.elapsed()
            && elapsed.as_secs() < CLEANUP_INTERVAL_SECS
        {
            return Ok(());
        }

        let mut files = Vec::new();
        collect_log_files(&self.dir, &mut files);

        let cutoff = SystemTime::now()
            .checked_sub(std::time::Duration::from_secs(
                self.retention_days.saturating_mul(24 * 60 * 60),
            ))
            .unwrap_or(SystemTime::UNIX_EPOCH);

        // 1) 保持期間を超えたものを消す
        files.retain(|entry| {
            if entry.modified < cutoff {
                let _ = fs::remove_file(&entry.path);
                false
            } else {
                true
            }
        });

        // 2) まだ容量上限を超えていれば古い順に消す
        let mut total: u64 = files.iter().map(|entry| entry.size).sum();
        if total > self.max_total_bytes {
            files.sort_by_key(|entry| entry.modified);
            for entry in &files {
                if total <= self.max_total_bytes {
                    break;
                }
                if fs::remove_file(&entry.path).is_ok() {
                    total = total.saturating_sub(entry.size);
                }
            }
        }

        // 3) 空になった日付ディレクトリを片付ける
        if let Ok(entries) = fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    let _ = fs::remove_dir(entry.path());
                }
            }
        }

        let _ = fs::write(&stamp, b"");
        Ok(())
    }

    /// 警告は 1 プロセス 1 回まで。`--quiet`(hook 実行)では出さない
    fn warn(&self, message: &str) {
        if self.quiet || *self.warned.borrow() {
            return;
        }
        *self.warned.borrow_mut() = true;
        eprintln!("git-sc: warning: {message}");
    }

    /// 実行 ID。時刻・PID・プロセス内カウンタで一意にする
    fn new_run_id(started_at_unix_ms: u64) -> String {
        let counter = RUN_COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("{started_at_unix_ms}-{}-{counter}", process::id())
    }
}

/// cleanup 対象のファイル
struct LogFile {
    path: PathBuf,
    size: u64,
    modified: SystemTime,
}

/// 日付ディレクトリ配下のログファイルを集める
///
/// 書きかけの `.tmp` も、1 時間以上放置されていれば回収対象にする。
fn collect_log_files(root: &Path, out: &mut Vec<LogFile>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_log_files(&path, out);
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        let name = entry.file_name();
        let name = name.to_string_lossy();

        if name.ends_with(".tmp") {
            // 書きかけの可能性があるので、十分古いものだけ消す
            if modified
                .elapsed()
                .map(|e| e.as_secs() > 60 * 60)
                .unwrap_or(false)
            {
                let _ = fs::remove_file(&path);
            }
            continue;
        }
        if !name.ends_with(".json") {
            continue;
        }
        out.push(LogFile {
            path,
            size: metadata.len(),
            modified,
        });
    }
}

/// ディレクトリを作る。Unix では所有者だけがたどれるようにする
fn create_dir_private(path: &Path) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    fs::create_dir_all(path).map_err(|e| format!("{}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // ログ本体は 0600 だが、ディレクトリ名(リポジトリ名を含みうる)も隠す
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
        if let Some(parent) = path.parent() {
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }
    Ok(())
}

/// 内容の同一性を見るためのダイジェスト
///
/// 暗号学的な強度は不要(ローカルのログ同士を突き合わせるだけ)なので、標準ライブラリの
/// ハッシュで済ませる。アルゴリズムを名前に含めておき、後から差し替えられるようにする。
pub fn digest(content: &str) -> String {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("siphash13:v1:{:016x}", hasher.finish())
}

/// 記録用に長い出力を切り詰める。返り値は (本文, 切り詰めたか)
pub fn capture(content: &str) -> (String, bool) {
    if content.len() <= MAX_CAPTURE_BYTES {
        return (content.to_string(), false);
    }
    // 文字境界に丸めてから切る
    let mut end = MAX_CAPTURE_BYTES;
    while end > 0 && !content.is_char_boundary(end) {
        end -= 1;
    }
    (content[..end].to_string(), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DevLogConfig;

    fn enabled_config(dir: &Path, content: &str) -> Config {
        Config {
            dev_log: Some(DevLogConfig {
                enabled: true,
                dir: Some(dir.to_string_lossy().into_owned()),
                content: content.to_string(),
                retention_days: 14,
                max_total_mb: 500,
            }),
            ..Config::default()
        }
    }

    #[test]
    fn test_disabled_by_default() {
        assert!(DevLog::from_config(&Config::default(), false).is_none());
    }

    #[test]
    fn test_disabled_when_enabled_false() {
        let config = Config {
            dev_log: Some(DevLogConfig::default()),
            ..Config::default()
        };
        assert!(DevLog::from_config(&config, false).is_none());
    }

    #[test]
    fn test_content_level_parse() {
        assert_eq!(ContentLevel::parse("full"), ContentLevel::Full);
        assert_eq!(ContentLevel::parse("FULL"), ContentLevel::Full);
        assert_eq!(ContentLevel::parse("metadata"), ContentLevel::Metadata);
        // 未知の値は差分を残さない側に倒す
        assert_eq!(ContentLevel::parse("everything"), ContentLevel::Metadata);
        assert_eq!(ContentLevel::parse(""), ContentLevel::Metadata);
    }

    /// 生成まで到達した実行を模す(プロンプトを渡してから結末を書く)
    fn finish_with_prompt(log: &DevLog, prompt: &str) {
        log.set_generation_input(Input::default(), prompt, Vec::new());
        log.finish(None);
    }

    #[test]
    fn test_metadata_level_omits_prompt_content() {
        let dir = tempfile::TempDir::new().unwrap();
        let log = DevLog::from_config(&enabled_config(dir.path(), "metadata"), true).unwrap();
        finish_with_prompt(&log, "prompt with the whole diff in it");

        let written = read_single_record(dir.path());
        assert_eq!(written["content_level"], "metadata");
        assert!(
            written["prompt"].get("content").is_none(),
            "metadata では diff を含むプロンプト全文を残さない"
        );
        assert_eq!(written["prompt"]["bytes"], 32);
        assert!(
            written["prompt"]["digest"]
                .as_str()
                .unwrap()
                .starts_with("siphash13:v1:")
        );
    }

    #[test]
    fn test_full_level_records_prompt_content() {
        let dir = tempfile::TempDir::new().unwrap();
        let log = DevLog::from_config(&enabled_config(dir.path(), "full"), true).unwrap();
        finish_with_prompt(&log, "prompt body");

        let written = read_single_record(dir.path());
        assert_eq!(written["prompt"]["content"], "prompt body");
    }

    /// 生成に至らなかった実行(ステージ済みの変更が無い等)はログを残さない
    #[test]
    fn test_run_without_generation_is_not_recorded() {
        let dir = tempfile::TempDir::new().unwrap();
        let log = DevLog::from_config(&enabled_config(dir.path(), "full"), true).unwrap();
        log.set_invocation(Invocation::default());
        log.finish(None);

        let day_dirs: Vec<_> = fs::read_dir(dir.path())
            .map(|entries| entries.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        assert!(
            day_dirs.iter().all(|p| !p.is_dir()),
            "生成していない実行までログを書いている: {day_dirs:?}"
        );
    }

    /// 途中で失敗した実行は status = failed とエラー文言を残す
    #[test]
    fn test_failed_run_records_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let log = DevLog::from_config(&enabled_config(dir.path(), "metadata"), true).unwrap();
        log.set_generation_input(Input::default(), "prompt", Vec::new());
        log.finish(Some("all providers failed".to_string()));

        let written = read_single_record(dir.path());
        assert_eq!(written["result"]["status"], "failed");
        assert_eq!(written["result"]["error"], "all providers failed");
    }

    #[test]
    fn test_records_attempts_in_order() {
        let dir = tempfile::TempDir::new().unwrap();
        let log = DevLog::from_config(&enabled_config(dir.path(), "metadata"), true).unwrap();
        log.record_attempt(sample_attempt(0, "antigravity"));
        log.update_last_attempt(
            Some("test: メッセージ".to_string()),
            vec!["markup".to_string()],
            "fallback",
        );
        log.record_attempt(sample_attempt(1, "codex"));
        log.update_last_attempt(Some("fix: メッセージ".to_string()), Vec::new(), "accepted");

        finish_with_prompt(&log, "prompt");

        let written = read_single_record(dir.path());
        let attempts = written["attempts"].as_array().unwrap();
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0]["provider"], "antigravity");
        assert_eq!(attempts[0]["findings"][0], "markup");
        assert_eq!(attempts[0]["decision"], "fallback");
        assert_eq!(attempts[1]["decision"], "accepted");
        assert_eq!(attempts[1]["message"], "fix: メッセージ");
    }

    #[test]
    fn test_write_leaves_no_temporary_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let log = DevLog::from_config(&enabled_config(dir.path(), "metadata"), true).unwrap();
        finish_with_prompt(&log, "prompt");

        let day_dir = single_day_dir(dir.path());
        let leftovers: Vec<_> = fs::read_dir(&day_dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "一時ファイルが残っている");
    }

    #[cfg(unix)]
    #[test]
    fn test_written_file_is_not_readable_by_group_or_others() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::TempDir::new().unwrap();
        let log = DevLog::from_config(&enabled_config(dir.path(), "full"), true).unwrap();
        finish_with_prompt(&log, "diff の中身がそのまま入る");

        let path = single_log_path(dir.path());
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "group/other から読める: {:o}", mode);
    }

    #[test]
    fn test_cleanup_removes_files_older_than_retention() {
        let dir = tempfile::TempDir::new().unwrap();
        let config = Config {
            dev_log: Some(DevLogConfig {
                enabled: true,
                dir: Some(dir.path().to_string_lossy().into_owned()),
                content: "metadata".to_string(),
                retention_days: 1,
                max_total_mb: 500,
            }),
            ..Config::default()
        };
        let old_dir = dir.path().join("2020-01-01");
        fs::create_dir_all(&old_dir).unwrap();
        let old_file = old_dir.join("old.json");
        fs::write(&old_file, "{}").unwrap();
        set_modified_days_ago(&old_file, 3);

        let log = DevLog::from_config(&config, true).unwrap();
        finish_with_prompt(&log, "prompt");

        assert!(!old_file.exists(), "保持期間を超えたログが残っている");
    }

    #[test]
    fn test_cleanup_is_skipped_while_stamp_is_fresh() {
        let dir = tempfile::TempDir::new().unwrap();
        let config = Config {
            dev_log: Some(DevLogConfig {
                enabled: true,
                dir: Some(dir.path().to_string_lossy().into_owned()),
                content: "metadata".to_string(),
                retention_days: 1,
                max_total_mb: 500,
            }),
            ..Config::default()
        };
        fs::create_dir_all(dir.path()).unwrap();
        fs::write(dir.path().join(".cleanup-stamp"), b"").unwrap();

        let old_dir = dir.path().join("2020-01-01");
        fs::create_dir_all(&old_dir).unwrap();
        let old_file = old_dir.join("old.json");
        fs::write(&old_file, "{}").unwrap();
        set_modified_days_ago(&old_file, 3);

        let log = DevLog::from_config(&config, true).unwrap();
        finish_with_prompt(&log, "prompt");

        assert!(
            old_file.exists(),
            "スタンプが新しい間は走査ごと省略されるはず"
        );
    }

    #[test]
    fn test_capture_truncates_long_output_at_char_boundary() {
        let long = "あ".repeat(MAX_CAPTURE_BYTES);
        let (captured, truncated) = capture(&long);
        assert!(truncated);
        assert!(captured.len() <= MAX_CAPTURE_BYTES);
        // 文字境界で切れているので、そのまま UTF-8 として扱える
        assert!(captured.chars().all(|c| c == 'あ'));

        let (short, truncated) = capture("短い");
        assert!(!truncated);
        assert_eq!(short, "短い");
    }

    #[test]
    fn test_digest_is_stable_and_distinguishes_content() {
        assert_eq!(digest("same"), digest("same"));
        assert_ne!(digest("a"), digest("b"));
    }

    fn sample_attempt(index: usize, provider: &str) -> AttemptRecord {
        AttemptRecord {
            index,
            provider: provider.to_string(),
            step_label: provider.to_string(),
            model: String::new(),
            env_keys: Vec::new(),
            retry: 0,
            duration_ms: 1,
            raw_stdout: Some("raw".to_string()),
            raw_stdout_bytes: 3,
            raw_stdout_truncated_by_logger: false,
            stderr_excerpt: None,
            stderr_bytes: 0,
            envelope_tag: None,
            message: None,
            findings: Vec::new(),
            decision: "pending".to_string(),
            error: None,
        }
    }

    fn single_day_dir(root: &Path) -> PathBuf {
        fs::read_dir(root)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|p| p.is_dir())
            .expect("日付ディレクトリが作られていない")
    }

    fn single_log_path(root: &Path) -> PathBuf {
        fs::read_dir(single_day_dir(root))
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|ext| ext == "json"))
            .expect("ログファイルが作られていない")
    }

    fn read_single_record(root: &Path) -> serde_json::Value {
        let content = fs::read_to_string(single_log_path(root)).unwrap();
        serde_json::from_str(&content).unwrap()
    }

    fn set_modified_days_ago(path: &Path, days: u64) {
        let target = SystemTime::now() - std::time::Duration::from_secs(days * 24 * 60 * 60);
        let file = fs::File::options().write(true).open(path).unwrap();
        file.set_modified(target).unwrap();
    }
}
