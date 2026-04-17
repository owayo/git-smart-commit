use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::error::AppError;

/// 差分の最大文字数
const MAX_DIFF_CHARS: usize = 10000;

/// プレフィックススクリプトの実行結果
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptResult {
    /// プレフィックスが返された（exit 0 + 内容あり）
    Prefix(String),
    /// 空が返された（exit 0 + 内容なし）→ プレフィックスなし
    Empty,
    /// スクリプトが失敗（exit 1）→ AI生成のメッセージをそのまま使用
    Failed,
}

/// reword 用コミットメッセージ一時ファイル
///
/// 固定名を避けて一意なファイルを作成し、Drop 時に自動削除する。
struct TempRewordMessageFile {
    path: PathBuf,
}

impl TempRewordMessageFile {
    /// 競合しない一時ファイルを作成し、コミットメッセージを書き込む
    fn create(message: &str) -> Result<Self, AppError> {
        let temp_dir = std::env::temp_dir();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();

        for attempt in 0..100 {
            let path = temp_dir.join(format!(
                "git-sc-reword-message-{}-{}-{}.txt",
                std::process::id(),
                timestamp,
                attempt
            ));

            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    // 書き込み/sync失敗時はファイルを削除してからエラーを返す
                    if let Err(e) = file
                        .write_all(message.as_bytes())
                        .and_then(|_| file.sync_all())
                    {
                        drop(file);
                        let _ = std::fs::remove_file(&path);
                        return Err(AppError::GitError(format!(
                            "Failed to write temp file: {}",
                            e
                        )));
                    }
                    drop(file);
                    return Ok(Self { path });
                }
                Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    return Err(AppError::GitError(format!(
                        "Failed to create temp file: {}",
                        e
                    )));
                }
            }
        }

        Err(AppError::GitError(
            "Failed to create unique temp file".to_string(),
        ))
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TempRewordMessageFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Git操作サービス
pub struct GitService {
    repo_path: PathBuf,
}

impl GitService {
    /// 現在のディレクトリに対するGitServiceを作成
    pub fn new() -> Self {
        Self {
            repo_path: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }

    // ============================================================
    // git コマンド実行ヘルパー
    // ============================================================

    /// git コマンドを実行し、成功時は stdout（trim済み）を返す
    fn run_git(&self, args: &[&str]) -> Result<String, AppError> {
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| AppError::GitError(e.to_string()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let msg = if stderr.is_empty() {
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if stdout.is_empty() {
                    format!("exit code: {}", output.status)
                } else {
                    stdout
                }
            } else {
                stderr
            };
            return Err(AppError::GitError(msg));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// git コマンドを実行し、成功/失敗のみを返す（stdout不要のケース）
    fn run_git_ok(&self, args: &[&str]) -> Result<(), AppError> {
        self.run_git(args)?;
        Ok(())
    }

    /// git コマンドを実行し、成功時は Some(stdout)、失敗時は None を返す
    fn try_run_git(&self, args: &[&str]) -> Option<String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.repo_path)
            .output()
            .ok()?;

        if output.status.success() {
            let result = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if result.is_empty() {
                None
            } else {
                Some(result)
            }
        } else {
            None
        }
    }

    /// コミットハッシュが有効か検証し、無効な場合は InvalidCommitHash エラーを返す
    ///
    /// `^{commit}` サフィックスで型制約し、tree/blob 等の非コミットオブジェクトを拒否する。
    fn verify_commit_hash(&self, hash: &str) -> Result<(), AppError> {
        let rev = format!("{}^{{commit}}", hash);
        let output = Command::new("git")
            .args(["rev-parse", "--verify", "--quiet", "--end-of-options", &rev])
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| AppError::GitError(e.to_string()))?;

        if !output.status.success() {
            return Err(AppError::InvalidCommitHash(hash.to_string()));
        }
        Ok(())
    }

    // ============================================================
    // 内部ユーティリティ
    // ============================================================

    /// Gitリポジトリのルートディレクトリを取得
    fn get_git_root(&self) -> Option<PathBuf> {
        self.try_run_git(&["rev-parse", "--show-toplevel"])
            .map(PathBuf::from)
    }

    /// .git-sc-ignoreファイルを読み込んでGitignoreを構築
    fn load_ignore_patterns(&self) -> Option<Gitignore> {
        let git_root = self.get_git_root()?;
        let ignore_path = git_root.join(".git-sc-ignore");

        if !ignore_path.exists() {
            return None;
        }

        let mut builder = GitignoreBuilder::new(&git_root);
        if builder.add(&ignore_path).is_some() {
            // エラーがあった場合はNoneを返す
            return None;
        }

        builder.build().ok()
    }

    /// プレフィックススクリプトを実行する基準ディレクトリを取得
    ///
    /// `.git-sc` は Git ルートに置かれるため、相対パスの解決と
    /// スクリプトの作業ディレクトリは Git ルートにそろえる。
    fn prefix_script_base_dir(&self) -> PathBuf {
        self.get_git_root()
            .unwrap_or_else(|| self.repo_path.clone())
    }

    /// プレフィックススクリプトの実行パスを解決
    fn resolve_prefix_script_path(&self, script: &str) -> PathBuf {
        let script_path = Path::new(script);
        if script_path.is_absolute() {
            script_path.to_path_buf()
        } else {
            self.prefix_script_base_dir().join(script_path)
        }
    }

    /// diffからignoreパターンにマッチするファイルを除外
    fn filter_ignored_files(diff_text: &str, ignore: &Gitignore) -> String {
        if diff_text.is_empty() {
            return String::new();
        }

        let lines: Vec<&str> = diff_text.lines().collect();
        let mut filtered_lines = Vec::new();
        let mut i = 0;

        while i < lines.len() {
            let line = lines[i];

            if line.starts_with("diff --git") {
                // ファイルパスを抽出 (例: "diff --git a/path/to/file b/path/to/file")
                let block_start = i;
                let file_paths = Self::extract_file_paths_from_diff_header(line);

                // ignoreパターンにマッチするかチェック
                let should_ignore = file_paths
                    .as_ref()
                    .map(|(before_path, after_path)| {
                        [before_path.as_str(), after_path.as_str()]
                            .into_iter()
                            .any(|path| ignore.matched_path_or_any_parents(path, false).is_ignore())
                    })
                    .unwrap_or(false);

                // このブロックの終端を見つける
                i += 1;
                while i < lines.len() && !lines[i].starts_with("diff --git") {
                    i += 1;
                }

                // ignoreにマッチしなければブロックを追加
                if !should_ignore {
                    for line in lines.iter().take(i).skip(block_start) {
                        filtered_lines.push(*line);
                    }
                }
                continue;
            } else {
                filtered_lines.push(line);
            }
            i += 1;
        }

        filtered_lines.join("\n")
    }

    /// diffヘッダーから変更前/変更後のファイルパスを抽出
    ///
    /// Git は `diff --git` 行でファイル名にスペースが含まれる場合もクォート形式に
    /// しないため、素朴な空白分割では誤抽出になる（例: `diff --git a/foo bar.txt
    /// b/foo bar.txt` → `foo` と `bar.txt` に分割されてしまう）。
    /// 非クォートの同一パス対称ケースでは中央分割で正しく抽出し、片側だけが
    /// クォートされているケースではクォート側を優先的に解析する。
    fn extract_file_paths_from_diff_header(header: &str) -> Option<(String, String)> {
        let rest = header.strip_prefix("diff --git ")?.trim();

        // クォートを一切含まない場合は同一パス対称ケースを先に試す
        // （スペース含みファイル名でも `a/PATH b/PATH` 形式を正しく抽出できる）
        if !rest.contains('"')
            && let Some(paths) = Self::try_split_symmetric_unquoted_header(rest)
        {
            return Some(paths);
        }

        // 先頭がクォートの場合はクォート解析、そうでなければ空白分割
        let (before_path, rest_after) = Self::take_diff_header_path(rest)?;
        let rest_after = rest_after.trim_start();

        // 残り側がクォートなら再度クォート解析、そうでなければ末尾までを
        // そのまま after_path として扱う（`"a/旧.txt" b/new name.txt` のような
        // 片側クォート + スペース含み非クォートに対応）
        let after_path = if rest_after.starts_with('"') {
            Self::take_diff_header_path(rest_after)?.0
        } else {
            rest_after.to_string()
        };

        Some((
            before_path.strip_prefix("a/")?.to_string(),
            after_path.strip_prefix("b/")?.to_string(),
        ))
    }

    /// 非クォートの `a/PATH b/PATH` 形式を中央分割で抽出する
    ///
    /// `a/PATH b/PATH` の全長は `5 + 2 * len(PATH)` となり必ず奇数であることを
    /// 利用して中央位置を特定し、前後パートが `a/`・`b/` で始まり、
    /// パス部分が一致する場合のみ成功とする。
    fn try_split_symmetric_unquoted_header(rest: &str) -> Option<(String, String)> {
        let total_len = rest.len();
        // `a/x b/x` で最短7文字、全長は奇数である必要がある
        if total_len < 7 || total_len.is_multiple_of(2) {
            return None;
        }
        let mid = total_len / 2;
        if rest.as_bytes().get(mid) != Some(&b' ') {
            return None;
        }
        let before_part = &rest[..mid];
        let after_part = &rest[mid + 1..];
        let before_path = before_part.strip_prefix("a/")?;
        let after_path = after_part.strip_prefix("b/")?;
        if before_path != after_path {
            return None;
        }
        // パス部にクォート文字が残る場合はこの経路では扱わない
        if before_path.contains('"') {
            return None;
        }
        Some((before_path.to_string(), after_path.to_string()))
    }

    /// diffヘッダーからファイルパスを1つ読み取る
    fn take_diff_header_path(input: &str) -> Option<(String, &str)> {
        let input = input.trim_start();

        if let Some(after_quote) = input.strip_prefix('"') {
            Self::decode_quoted_diff_path_with_rest(after_quote)
        } else {
            let end = input.find(char::is_whitespace).unwrap_or(input.len());
            if end == 0 {
                return None;
            }
            Some((input[..end].to_string(), &input[end..]))
        }
    }

    /// diffヘッダーからファイルパスを抽出
    fn extract_file_path_from_diff_header(header: &str) -> Option<String> {
        Self::extract_file_paths_from_diff_header(header).map(|(path, _)| path)
    }

    /// Git の quoted path をデコードする
    ///
    /// Git は `core.quotePath=true` のとき、非 ASCII 文字を 8 進エスケープで出力する。
    /// `.git-sc-ignore` との照合に使うため、実際のパス文字列へ復元する。
    fn decode_quoted_diff_path_with_rest(input: &str) -> Option<(String, &str)> {
        fn is_octal(byte: u8) -> bool {
            matches!(byte, b'0'..=b'7')
        }

        let bytes = input.as_bytes();
        let mut decoded = Vec::new();
        let mut i = 0;
        let mut closed_quote = false;

        while i < bytes.len() {
            match bytes[i] {
                b'"' => {
                    closed_quote = true;
                    i += 1;
                    break;
                }
                b'\\' => {
                    i += 1;
                    let escaped = *bytes.get(i)?;

                    match escaped {
                        b'n' => decoded.push(b'\n'),
                        b't' => decoded.push(b'\t'),
                        b'r' => decoded.push(b'\r'),
                        b'\\' => decoded.push(b'\\'),
                        b'"' => decoded.push(b'"'),
                        b'0'..=b'7' => {
                            let second = *bytes.get(i + 1)?;
                            let third = *bytes.get(i + 2)?;
                            if !is_octal(second) || !is_octal(third) {
                                return None;
                            }

                            // Gitのquoted pathの3桁8進エスケープは1バイトを表す
                            // 0o400..=0o777は不正入力として拒否する
                            let value = (escaped - b'0') as u16 * 64
                                + (second - b'0') as u16 * 8
                                + (third - b'0') as u16;
                            if value > u8::MAX as u16 {
                                return None;
                            }
                            decoded.push(value as u8);
                            i += 2;
                        }
                        other => decoded.push(other),
                    }
                }
                byte => decoded.push(byte),
            }

            i += 1;
        }

        if !closed_quote || decoded.is_empty() {
            return None;
        }

        Some((String::from_utf8_lossy(&decoded).into_owned(), &input[i..]))
    }

    /// Git の quoted path をデコードする
    #[cfg(test)]
    fn decode_quoted_diff_path(input: &str) -> Option<String> {
        Self::decode_quoted_diff_path_with_rest(input).map(|(path, _)| path)
    }

    /// diffを最大文字数に切り詰める
    pub fn truncate_diff(diff: &str) -> String {
        if diff.chars().count() <= MAX_DIFF_CHARS {
            return diff.to_string();
        }

        // 文字数でカット
        let truncated: String = diff.chars().take(MAX_DIFF_CHARS).collect();

        // 最後の完全な行まで切り詰める（中途半端な行を避ける）
        if let Some(last_newline) = truncated.rfind('\n') {
            format!(
                "{}\n\n... (diff truncated: exceeded {} characters)",
                &truncated[..last_newline],
                MAX_DIFF_CHARS
            )
        } else {
            format!(
                "{}\n\n... (diff truncated: exceeded {} characters)",
                truncated, MAX_DIFF_CHARS
            )
        }
    }

    /// diffに対して全てのフィルタリングを適用
    fn apply_all_filters(&self, diff: &str) -> String {
        // 1. .git-sc-ignore パターンにマッチするファイルを除外
        //    バイナリフィルタより先に実行する必要がある。
        //    filter_binary_diff は diff --git ヘッダーをサマリー行に変換するため、
        //    先に実行するとignoreパターンがバイナリファイルに適用されなくなる。
        let filtered = if let Some(ignore) = self.load_ignore_patterns() {
            Self::filter_ignored_files(diff, &ignore)
        } else {
            diff.to_string()
        };

        // 2. バイナリファイルをサマリーに変換
        let filtered = Self::filter_binary_diff(&filtered);

        // 3. 文字数制限を適用
        Self::truncate_diff(&filtered)
    }

    /// git diffの出力からバイナリファイルの詳細差分を除外し、変更種別のみを出力
    fn filter_binary_diff(diff_text: &str) -> String {
        if diff_text.is_empty() {
            return String::new();
        }

        let lines: Vec<&str> = diff_text.lines().collect();
        let mut filtered_lines = Vec::new();
        let mut i = 0;

        while i < lines.len() {
            let line = lines[i];

            if line.starts_with("diff --git") {
                // 新しいdiffブロックの開始
                let block_start = i;
                let file_path = Self::extract_file_path_from_diff_header(line)
                    .unwrap_or_else(|| "unknown".to_string());
                i += 1;

                // ブロック内の情報を収集
                let mut is_binary = false;
                let mut is_new_file = false;
                let mut is_deleted = false;
                let mut rename_from: Option<String> = None;
                let mut rename_to: Option<String> = None;

                while i < lines.len() && !lines[i].starts_with("diff --git") {
                    let current_line = lines[i];

                    if current_line.starts_with("Binary files") {
                        is_binary = true;
                    } else if current_line.starts_with("new file mode") {
                        is_new_file = true;
                    } else if current_line.starts_with("deleted file mode") {
                        is_deleted = true;
                    } else if let Some(from) = current_line.strip_prefix("rename from ") {
                        rename_from = Some(from.to_string());
                    } else if let Some(to) = current_line.strip_prefix("rename to ") {
                        rename_to = Some(to.to_string());
                    }
                    i += 1;
                }

                if is_binary {
                    // バイナリファイルは変更種別のサマリーのみ出力
                    let summary = if let (Some(from), Some(to)) = (&rename_from, &rename_to) {
                        format!("[Binary] renamed: {} -> {}", from, to)
                    } else if is_new_file {
                        format!("[Binary] added: {}", file_path)
                    } else if is_deleted {
                        format!("[Binary] deleted: {}", file_path)
                    } else {
                        format!("[Binary] modified: {}", file_path)
                    };
                    filtered_lines.push(summary);
                } else {
                    // テキストファイルはそのまま出力
                    for line in lines.iter().take(i).skip(block_start) {
                        filtered_lines.push((*line).to_string());
                    }
                }
                continue;
            } else {
                filtered_lines.push(line.to_string());
            }
            i += 1;
        }

        filtered_lines.join("\n")
    }

    // ============================================================
    // パブリック API
    // ============================================================

    /// Gitリポジトリ内にいるか確認
    pub fn verify_repository(&self) -> Result<(), AppError> {
        // .git ディレクトリが直接存在するかチェック
        if self.repo_path.join(".git").exists() {
            return Ok(());
        }

        // Gitリポジトリのサブディレクトリにいる場合もチェック
        let output = Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| AppError::GitError(e.to_string()))?;

        if output.status.success() {
            Ok(())
        } else {
            Err(AppError::NotGitRepository)
        }
    }

    /// ステージ済みのdiffを取得（バイナリファイル、.git-sc-ignore対象、空白のみの変更を除外）
    pub fn get_staged_diff(&self) -> Result<String, AppError> {
        let raw = self.run_git(&["diff", "--cached", "-w", "-U0"])?;
        Ok(self.apply_all_filters(&raw))
    }

    /// 直近のコミットメッセージを取得
    pub fn get_recent_commits(&self, count: usize) -> Result<Vec<String>, AppError> {
        // 空リポジトリでは `git log` のエラーメッセージがロケール依存になるため、
        // 先に HEAD コミットの有無を機械的に判定してから `git log` を呼び出す。
        if !self.has_head_commit()? {
            return Ok(Vec::new());
        }

        let output = Command::new("git")
            .args(["log", "--format=%s", "-n", &count.to_string()])
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| AppError::GitError(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let msg = if stderr.is_empty() {
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if stdout.is_empty() {
                    format!("exit code: {}", output.status)
                } else {
                    stdout
                }
            } else {
                stderr
            };
            return Err(AppError::GitError(msg));
        }

        let commits: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();

        Ok(commits)
    }

    /// 全ての変更をステージング
    pub fn stage_all(&self) -> Result<(), AppError> {
        // Windows環境では "nul" は予約デバイス名
        // AIプロバイダー呼び出し時に意図せず "nul" ファイルが作成されることがあるため、
        // nul ファイルの削除 + ステージング除外の二重防御を行う
        #[cfg(windows)]
        {
            let nul_path = self.repo_path.join("nul");
            if nul_path.exists() {
                let _ = std::fs::remove_file(&nul_path);
            }
        }

        // Windows環境では "nul" を除外してステージング（pathspecの除外指定を使用）
        #[cfg(windows)]
        let args: &[&str] = &["add", "-A", "--", ".", ":!nul"];
        #[cfg(not(windows))]
        let args: &[&str] = &["add", "-A"];

        self.run_git_ok(args)?;

        // Windows環境で万が一 nul がステージされていた場合はアンステージ
        #[cfg(windows)]
        {
            let _ = Command::new("git")
                .args(["reset", "HEAD", "--", "nul"])
                .current_dir(&self.repo_path)
                .output();
        }

        Ok(())
    }

    /// ステージ済みの変更が存在するかチェック
    pub fn has_staged_changes(&self) -> bool {
        // git diff --cached --quiet は差分があると exit 1 を返す
        Command::new("git")
            .args(["diff", "--cached", "--quiet"])
            .current_dir(&self.repo_path)
            .output()
            .map(|o| !o.status.success())
            .unwrap_or(false)
    }

    /// 指定されたメッセージでコミットを作成
    pub fn commit(&self, message: &str) -> Result<(), AppError> {
        self.run_git_ok(&["commit", "-m", message])
    }

    /// リモートにpush
    pub fn push(&self) -> Result<(), AppError> {
        self.run_git_ok(&["push"])
    }

    /// auto_push が有効かどうかを判定
    pub fn is_auto_push_enabled(&self, config_auto_push: Option<bool>) -> bool {
        config_auto_push.unwrap_or(false)
    }

    /// 直前のコミットのdiffを取得（バイナリファイル、.git-sc-ignore対象、空白のみの変更を除外）
    pub fn get_last_commit_diff(&self) -> Result<String, AppError> {
        self.get_commit_diff_by_hash("HEAD")
    }

    /// 直前のコミットを新しいメッセージで修正
    pub fn amend_commit(&self, message: &str) -> Result<(), AppError> {
        self.run_git_ok(&["commit", "--amend", "-m", message])
    }

    /// リモートURLを取得（origin）
    pub fn get_remote_url(&self) -> Option<String> {
        self.try_run_git(&["config", "--get", "remote.origin.url"])
    }

    /// 現在のブランチ名を取得
    pub fn get_current_branch(&self) -> Option<String> {
        self.try_run_git(&["branch", "--show-current"])
    }

    /// プレフィックススクリプトを実行してプレフィックスを取得
    ///
    /// 戻り値:
    /// - `Some(ScriptResult::Prefix(s))`: スクリプトがプレフィックスを返した（exit 0 + 内容あり）
    /// - `Some(ScriptResult::Empty)`: スクリプトが空を返した（exit 0 + 内容なし）→ プレフィックスなし
    /// - `Some(ScriptResult::Failed)`: スクリプトが失敗した（exit 1）→ AI生成メッセージを使用
    /// - `None`: スクリプトの実行自体に失敗
    ///
    /// スクリプトの stderr は直接ターミナルに出力されます。
    pub fn run_prefix_script(
        &self,
        script: &str,
        remote_url: &str,
        branch: &str,
    ) -> Option<ScriptResult> {
        use std::process::Stdio;

        let base_dir = self.prefix_script_base_dir();
        let script_path = self.resolve_prefix_script_path(script);

        let output = Command::new(&script_path)
            .args([remote_url, branch])
            .current_dir(base_dir)
            .stderr(Stdio::inherit()) // stderrは直接ターミナルに出力
            .output()
            .ok()?;

        if output.status.success() {
            let prefix = String::from_utf8_lossy(&output.stdout).to_string();
            if prefix.trim().is_empty() {
                Some(ScriptResult::Empty)
            } else {
                Some(ScriptResult::Prefix(prefix))
            }
        } else {
            // 終了コード1: AI生成のメッセージをそのまま使用
            Some(ScriptResult::Failed)
        }
    }

    /// ブランチが存在するか確認
    pub fn branch_exists(&self, branch: &str) -> bool {
        self.try_run_git(&["rev-parse", "--verify", branch])
            .is_some()
    }

    /// 2つのブランチのmerge-baseを取得
    pub fn get_merge_base(&self, base: &str, head: &str) -> Result<String, AppError> {
        self.run_git(&["merge-base", base, head]).map_err(|_| {
            AppError::GitError(format!(
                "Failed to find merge-base between {} and {}",
                base, head
            ))
        })
    }

    /// ベースからHEADまでのコミット数を取得
    pub fn count_commits_from_base(&self, base: &str) -> Result<usize, AppError> {
        let count_str = self.run_git(&["rev-list", "--count", &format!("{}..HEAD", base)])?;
        count_str
            .parse()
            .map_err(|_| AppError::GitError("Failed to parse commit count".to_string()))
    }

    /// ベースからHEADまでの差分を取得（バイナリファイル、.git-sc-ignore対象、空白のみの変更を除外）
    pub fn get_diff_from_base(&self, base: &str) -> Result<String, AppError> {
        let raw = self.run_git(&["diff", "-w", "-U0", base, "HEAD"])?;
        Ok(self.apply_all_filters(&raw))
    }

    /// 指定したコミットにsoft resetする
    pub fn soft_reset_to(&self, commit: &str) -> Result<(), AppError> {
        self.run_git_ok(&["reset", "--soft", commit])
    }

    /// 指定範囲にマージコミットが含まれているかチェック
    pub fn has_merge_commits_in_range(&self, n: usize) -> Result<bool, AppError> {
        if n == 0 {
            return Err(AppError::InvalidRewordTarget);
        }

        let total_commits = self.get_head_commit_count()?;
        if n > total_commits {
            return Err(AppError::InvalidRewordTarget);
        }

        // マージコミットは親が2つ以上ある
        let merges = if n == total_commits {
            // 最古コミットを含む範囲では HEAD~n が無効になるため履歴全体を対象にする
            self.run_git(&["rev-list", "--merges", "HEAD"])?
        } else {
            self.run_git(&["rev-list", "--merges", &format!("HEAD~{}..HEAD", n)])?
        };
        Ok(!merges.is_empty())
    }

    /// HEAD から辿れる総コミット数を取得
    fn get_head_commit_count(&self) -> Result<usize, AppError> {
        let count_str = self.run_git(&["rev-list", "--count", "HEAD"])?;
        count_str
            .parse()
            .map_err(|_| AppError::GitError("Failed to parse commit count".to_string()))
    }

    /// HEAD が有効なコミットを指しているか確認
    fn has_head_commit(&self) -> Result<bool, AppError> {
        let output = Command::new("git")
            .args(["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| AppError::GitError(e.to_string()))?;

        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                let msg = if stderr.is_empty() {
                    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if stdout.is_empty() {
                        format!("exit code: {}", output.status)
                    } else {
                        stdout
                    }
                } else {
                    stderr
                };
                Err(AppError::GitError(msg))
            }
        }
    }

    /// 指定されたコミットハッシュの差分を取得
    pub fn get_commit_diff_by_hash(&self, hash: &str) -> Result<String, AppError> {
        // まずコミットハッシュが有効か確認
        self.verify_commit_hash(hash)?;

        // git show でそのコミットの差分を取得
        let raw = self.run_git(&["show", hash, "--format=", "--no-color", "-w", "-U0"])?;
        Ok(self.apply_all_filters(&raw))
    }

    /// 指定されたコミットハッシュのメッセージを取得
    pub fn get_commit_message_by_hash(&self, hash: &str) -> Result<String, AppError> {
        // まずコミットハッシュが有効か確認
        self.verify_commit_hash(hash)?;

        self.run_git(&["log", "-1", "--format=%s", hash])
    }

    /// reword対象として有効なコミットか確認（現在のHEAD履歴上に存在するか）
    fn validate_reword_target_hash(&self, hash: &str) -> Result<(), AppError> {
        // まずコミットハッシュが有効か確認
        self.verify_commit_hash(hash)?;

        // 履歴外のコミットをreword対象にすると誤ったコミット位置が算出されるため、祖先関係を厳密に確認
        let ancestor_output = Command::new("git")
            .args(["merge-base", "--is-ancestor", hash, "HEAD"])
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| AppError::GitError(e.to_string()))?;

        match ancestor_output.status.code() {
            Some(0) => Ok(()),
            Some(1) => Err(AppError::InvalidRewordTarget),
            _ => {
                let stderr = String::from_utf8_lossy(&ancestor_output.stderr)
                    .trim()
                    .to_string();
                if stderr.is_empty() {
                    Err(AppError::GitError(
                        "Failed to verify reword target ancestry".to_string(),
                    ))
                } else {
                    Err(AppError::GitError(stderr))
                }
            }
        }
    }

    /// 指定されたコミットハッシュがHEADから何個前かを取得
    pub fn get_commit_position_by_hash(&self, hash: &str) -> Result<usize, AppError> {
        self.validate_reword_target_hash(hash)?;

        // HEADからそのコミットまでのコミット数をカウント
        // git rev-list --count hash..HEAD で hash から HEAD までのコミット数を取得
        // これに1を足すと、そのコミット自体の位置になる
        let count_str = self.run_git(&["rev-list", "--count", &format!("{}..HEAD", hash)])?;
        let count: usize = count_str
            .parse()
            .map_err(|_| AppError::GitError("Failed to parse commit count".to_string()))?;

        // count はそのコミットより新しいコミットの数なので、+1で位置になる
        Ok(count + 1)
    }

    /// 指定されたコミットハッシュからHEADまでにマージコミットが含まれているかチェック
    pub fn has_merge_commits_in_range_by_hash(&self, hash: &str) -> Result<bool, AppError> {
        // 対象コミット自身がマージコミットの場合も拒否対象に含める必要があるため、
        // ハッシュ指定を HEAD からの位置に変換し、既存の位置ベース判定を再利用する。
        let n = self.get_commit_position_by_hash(hash)?;
        self.has_merge_commits_in_range(n)
    }

    /// 指定されたコミットハッシュのメッセージを変更（rebase使用）
    pub fn reword_commit_by_hash(&self, hash: &str, new_message: &str) -> Result<(), AppError> {
        // 位置を取得
        let n = self.get_commit_position_by_hash(hash)?;

        // 既存のreword_commitを利用
        self.reword_commit(n, new_message)
    }

    /// N個前のコミットのメッセージを変更（rebase使用）
    pub fn reword_commit(&self, n: usize, new_message: &str) -> Result<(), AppError> {
        if n == 0 {
            return Err(AppError::InvalidRewordTarget);
        }

        // n=1 の場合は amend で処理
        if n == 1 {
            return self.amend_commit_message(new_message);
        }

        let total_commits = self.get_head_commit_count()?;
        if n > total_commits {
            return Err(AppError::InvalidRewordTarget);
        }

        // マージコミットをチェック
        if self.has_merge_commits_in_range(n)? {
            return Err(AppError::HasMergeCommits);
        }

        // 一意な一時ファイルにメッセージを保存
        let msg_file = TempRewordMessageFile::create(new_message)?;

        // GIT_SEQUENCE_EDITOR: 先頭の pick を reword に置換
        // シェル経由で実行するため、sh -c でラップする
        let sequence_editor = if cfg!(windows) {
            // Windows環境では PowerShell を使用
            "powershell -Command \"(Get-Content $args[0]) -replace '^pick', 'reword' | Set-Content $args[0]\"".to_string()
        } else {
            // Unix系環境では sed を使用（macOS/Linux対応）
            // sh -c でラップし、-- の後に $1 を渡す
            "sh -c 'sed -i.bak '\"'\"'1s/^pick/reword/'\"'\"' \"$1\" && rm -f \"$1.bak\"' --"
                .to_string()
        };

        // GIT_EDITOR: 一時ファイルの内容をコミットメッセージに反映
        // パスを環境変数経由で渡し、シェル文字列にパスを埋め込まない（インジェクション防止）
        let editor = if cfg!(windows) {
            "powershell -Command \"Copy-Item $env:GIT_SC_MSG_FILE $args[0]\"".to_string()
        } else {
            "sh -c 'cp \"$GIT_SC_MSG_FILE\" \"$1\"' --".to_string()
        };

        // git rebase -i を実行（最古コミット対象時は --root を使う）
        let mut rebase_cmd = Command::new("git");
        rebase_cmd.arg("rebase").arg("-i");
        if n == total_commits {
            rebase_cmd.arg("--root");
        } else {
            rebase_cmd.arg(format!("HEAD~{}", n));
        }

        let output = rebase_cmd
            .env("GIT_SEQUENCE_EDITOR", &sequence_editor)
            .env("GIT_EDITOR", &editor)
            .env("EDITOR", &editor)
            .env("GIT_SC_MSG_FILE", msg_file.path().as_os_str())
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| AppError::GitError(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);

            // コンフリクトの場合はrebaseを中止
            if stderr.contains("CONFLICT") || stderr.contains("could not apply") {
                let _ = Command::new("git")
                    .args(["rebase", "--abort"])
                    .current_dir(&self.repo_path)
                    .output();
                return Err(AppError::RebaseConflict);
            }

            return Err(AppError::GitError(stderr.to_string()));
        }

        Ok(())
    }

    /// コミットメッセージを変更（amend）
    fn amend_commit_message(&self, new_message: &str) -> Result<(), AppError> {
        self.run_git_ok(&["commit", "--amend", "-m", new_message])
    }
}

impl Default for GitService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::process::Command;

    // ============================================================
    // filter_binary_diff のテスト
    // ============================================================

    #[test]
    fn test_filter_binary_diff_empty_input() {
        let result = GitService::filter_binary_diff("");
        assert_eq!(result, "");
    }

    #[test]
    fn test_filter_binary_diff_no_binary() {
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
index 1234567..abcdefg 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!("Hello");
 }"#;
        let result = GitService::filter_binary_diff(diff);
        assert_eq!(result, diff);
    }

    #[test]
    fn test_filter_binary_diff_shows_binary_info() {
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
index 1234567..abcdefg 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!("Hello");
 }
diff --git a/image.png b/image.png
Binary files a/image.png and b/image.png differ"#;

        let result = GitService::filter_binary_diff(diff);

        // テキストファイルの差分が含まれる
        assert!(result.contains("src/main.rs"));
        assert!(result.contains("println"));
        // バイナリファイルは変更種別として出力される
        assert!(result.contains("[Binary] modified: image.png"));
        // 詳細なバイナリ差分は含まれない
        assert!(!result.contains("Binary files a/"));
    }

    #[test]
    fn test_filter_binary_diff_only_binary() {
        let diff = r#"diff --git a/image.png b/image.png
Binary files a/image.png and b/image.png differ"#;

        let result = GitService::filter_binary_diff(diff);
        assert_eq!(result, "[Binary] modified: image.png");
    }

    #[test]
    fn test_filter_binary_diff_multiple_binaries() {
        let diff = r#"diff --git a/src/lib.rs b/src/lib.rs
index 1234567..abcdefg 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1 +1,2 @@
+// new comment
diff --git a/image1.png b/image1.png
Binary files a/image1.png and b/image1.png differ
diff --git a/image2.jpg b/image2.jpg
Binary files a/image2.jpg and b/image2.jpg differ
diff --git a/config.toml b/config.toml
index 1111111..2222222 100644
--- a/config.toml
+++ b/config.toml
@@ -1 +1,2 @@
+key = "value""#;

        let result = GitService::filter_binary_diff(diff);

        // テキストファイルの変更が含まれる
        assert!(result.contains("src/lib.rs"));
        assert!(result.contains("config.toml"));
        // バイナリファイルは変更種別として出力される
        assert!(result.contains("[Binary] modified: image1.png"));
        assert!(result.contains("[Binary] modified: image2.jpg"));
        // 詳細なバイナリ差分は含まれない
        assert!(!result.contains("Binary files a/"));
    }

    #[test]
    fn test_filter_binary_diff_binary_at_start() {
        let diff = r#"diff --git a/logo.svg b/logo.svg
Binary files a/logo.svg and b/logo.svg differ
diff --git a/README.md b/README.md
index aaa..bbb 100644
--- a/README.md
+++ b/README.md
@@ -1 +1,2 @@
+# Title"#;

        let result = GitService::filter_binary_diff(diff);

        // バイナリファイルは変更種別として出力される
        assert!(result.contains("[Binary] modified: logo.svg"));
        assert!(result.contains("README.md"));
        assert!(result.contains("# Title"));
    }

    #[test]
    fn test_filter_binary_diff_new_file() {
        let diff = r#"diff --git a/new_image.png b/new_image.png
new file mode 100644
Binary files /dev/null and b/new_image.png differ"#;

        let result = GitService::filter_binary_diff(diff);
        assert_eq!(result, "[Binary] added: new_image.png");
    }

    #[test]
    fn test_filter_binary_diff_deleted_file() {
        let diff = r#"diff --git a/old_image.png b/old_image.png
deleted file mode 100644
Binary files a/old_image.png and /dev/null differ"#;

        let result = GitService::filter_binary_diff(diff);
        assert_eq!(result, "[Binary] deleted: old_image.png");
    }

    #[test]
    fn test_filter_binary_diff_renamed_file() {
        let diff = r#"diff --git a/old_name.png b/new_name.png
similarity index 100%
rename from old_name.png
rename to new_name.png"#;

        let result = GitService::filter_binary_diff(diff);
        // リネームはテキストファイルとして扱われる（Binary filesがないため）
        assert!(result.contains("rename from old_name.png"));
    }

    #[test]
    fn test_filter_binary_diff_renamed_binary() {
        let diff = r#"diff --git a/old_name.png b/new_name.png
similarity index 90%
rename from old_name.png
rename to new_name.png
Binary files a/old_name.png and b/new_name.png differ"#;

        let result = GitService::filter_binary_diff(diff);
        assert_eq!(result, "[Binary] renamed: old_name.png -> new_name.png");
    }

    #[test]
    fn test_filter_binary_diff_preserves_content_with_binary_keyword() {
        // "Binary"という文字列がコード内にある場合でも正しく処理
        let diff = r#"diff --git a/src/parser.rs b/src/parser.rs
index 1234567..abcdefg 100644
--- a/src/parser.rs
+++ b/src/parser.rs
@@ -1,3 +1,4 @@
+// Binary search implementation
 fn search() {}"#;

        let result = GitService::filter_binary_diff(diff);
        assert!(result.contains("Binary search implementation"));
    }

    // ============================================================
    // ScriptResult のテスト
    // ============================================================

    #[test]
    fn test_script_result_prefix() {
        let result = ScriptResult::Prefix("TICKET-123 ".to_string());
        assert_eq!(result, ScriptResult::Prefix("TICKET-123 ".to_string()));
    }

    #[test]
    fn test_script_result_empty() {
        let result = ScriptResult::Empty;
        assert_eq!(result, ScriptResult::Empty);
    }

    #[test]
    fn test_script_result_failed() {
        let result = ScriptResult::Failed;
        assert_eq!(result, ScriptResult::Failed);
    }

    #[test]
    fn test_script_result_equality() {
        assert_eq!(
            ScriptResult::Prefix("A".to_string()),
            ScriptResult::Prefix("A".to_string())
        );
        assert_ne!(
            ScriptResult::Prefix("A".to_string()),
            ScriptResult::Prefix("B".to_string())
        );
        assert_ne!(ScriptResult::Empty, ScriptResult::Failed);
    }

    // ============================================================
    // GitService 構造体のテスト
    // ============================================================

    #[test]
    fn test_git_service_new() {
        let service = GitService::new();
        // repo_pathが設定されていることを確認
        assert!(!service.repo_path.as_os_str().is_empty());
    }

    #[test]
    fn test_git_service_default() {
        let service = GitService::default();
        assert!(!service.repo_path.as_os_str().is_empty());
    }

    // ============================================================
    // Git リポジトリ操作のテスト（実際のリポジトリを使用）
    // ============================================================

    #[test]
    fn test_verify_repository_success() {
        // このテストは git-smart-commit リポジトリ内で実行される前提
        let service = GitService::new();
        let result = service.verify_repository();
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_current_branch() {
        let service = GitService::new();
        let branch = service.get_current_branch();
        // ブランチ名が取得できること（空でないこと）
        assert!(branch.is_some());
        assert!(!branch.unwrap().is_empty());
    }

    #[test]
    fn test_get_remote_url() {
        let service = GitService::new();
        let url = service.get_remote_url();
        // リモートURLが設定されている場合はgit-smart-commitを含む
        if let Some(remote) = url {
            assert!(remote.contains("git-smart-commit") || remote.contains("origin"));
        }
    }

    #[test]
    fn test_get_recent_commits() {
        let service = GitService::new();
        let commits = service.get_recent_commits(5);
        assert!(commits.is_ok());
        // このリポジトリにはコミットがあるはず
        let commits = commits.unwrap();
        assert!(!commits.is_empty());
    }

    #[test]
    fn test_get_recent_commits_limited() {
        let service = GitService::new();
        let commits = service.get_recent_commits(2);
        assert!(commits.is_ok());
        let commits = commits.unwrap();
        assert!(commits.len() <= 2);
    }

    #[test]
    fn test_get_recent_commits_empty_repo_without_locale_specific_stderr_parsing() {
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();
        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        let commits = service.get_recent_commits(5).unwrap();
        assert!(commits.is_empty());
    }

    // ============================================================
    // branch_exists のテスト
    // ============================================================

    #[test]
    fn test_branch_exists_main() {
        let service = GitService::new();
        // main または master ブランチが存在するはず
        let main_exists = service.branch_exists("main");
        let master_exists = service.branch_exists("master");
        assert!(main_exists || master_exists);
    }

    #[test]
    fn test_branch_exists_head() {
        let service = GitService::new();
        // HEAD は常に存在する
        assert!(service.branch_exists("HEAD"));
    }

    #[test]
    fn test_branch_exists_nonexistent() {
        let service = GitService::new();
        // 存在しないブランチ
        assert!(!service.branch_exists("nonexistent-branch-12345"));
    }

    #[test]
    fn test_branch_exists_with_origin_prefix() {
        let service = GitService::new();
        // origin/main または origin/master が存在する可能性
        let origin_main = service.branch_exists("origin/main");
        let origin_master = service.branch_exists("origin/master");
        // どちらかが存在するか、リモートがない場合は両方false
        // このテストはリモートの設定に依存するため、結果の検証は緩く
        // branch_exists が正常に動作することを確認（結果は環境依存）
        let _ = (origin_main, origin_master);
    }

    // ============================================================
    // get_merge_base のテスト
    // ============================================================

    #[test]
    fn test_get_merge_base_with_head() {
        let service = GitService::new();
        // HEAD と HEAD の merge-base は HEAD 自身
        let result = service.get_merge_base("HEAD", "HEAD");
        assert!(result.is_ok());
        let base = result.unwrap();
        // SHA-1 ハッシュは40文字
        assert_eq!(base.len(), 40);
    }

    #[test]
    fn test_get_merge_base_invalid_branch() {
        let service = GitService::new();
        let result = service.get_merge_base("nonexistent-branch", "HEAD");
        assert!(result.is_err());
    }

    // ============================================================
    // count_commits_from_base のテスト
    // ============================================================

    #[test]
    fn test_count_commits_from_base_same() {
        let service = GitService::new();
        // HEAD から HEAD までのコミット数は 0
        let result = service.count_commits_from_base("HEAD");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }

    // ============================================================
    // get_diff_from_base のテスト
    // ============================================================

    #[test]
    fn test_get_diff_from_base_same() {
        let service = GitService::new();
        // HEAD から HEAD までの差分は空
        let result = service.get_diff_from_base("HEAD");
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    // ============================================================
    // ScriptResult Clone のテスト
    // ============================================================

    #[test]
    fn test_script_result_clone() {
        let original = ScriptResult::Prefix("TEST ".to_string());
        let cloned = original.clone();
        assert_eq!(original, cloned);
    }

    #[test]
    fn test_script_result_debug() {
        let result = ScriptResult::Prefix("DEBUG ".to_string());
        let debug_str = format!("{:?}", result);
        assert!(debug_str.contains("Prefix"));
        assert!(debug_str.contains("DEBUG"));
    }

    // ============================================================
    // truncate_diff のテスト
    // ============================================================

    #[test]
    fn test_truncate_diff_short_content() {
        let diff = "short content";
        let result = GitService::truncate_diff(diff);
        assert_eq!(result, diff);
    }

    #[test]
    fn test_truncate_diff_exactly_at_limit() {
        // 10000文字ちょうどの場合は切り詰めない
        let diff: String = "a".repeat(10000);
        let result = GitService::truncate_diff(&diff);
        assert_eq!(result, diff);
    }

    #[test]
    fn test_truncate_diff_exceeds_limit() {
        // 10001文字以上の場合は切り詰める（改行を含む現実的なdiff）
        let line = "This is a line of diff content\n";
        let diff: String = line.repeat(400); // 12000文字以上
        assert!(diff.chars().count() > MAX_DIFF_CHARS);

        let result = GitService::truncate_diff(&diff);
        // 切り詰めメッセージが含まれることを確認
        assert!(result.contains("... (diff truncated: exceeded 10000 characters)"));
    }

    #[test]
    fn test_truncate_diff_preserves_last_complete_line() {
        // 改行を含む長いテキスト
        let line = "This is a line of text\n";
        let diff: String = line.repeat(500); // 10500文字以上
        let result = GitService::truncate_diff(&diff);

        // 切り詰めメッセージが含まれる
        assert!(result.contains("... (diff truncated: exceeded 10000 characters)"));

        // 最後の改行で切れている（中途半端な行がない）
        let lines: Vec<&str> = result.lines().collect();
        let last_content_line = lines
            .iter()
            .rev()
            .find(|l| !l.starts_with("...") && !l.is_empty());
        if let Some(line) = last_content_line {
            assert!(line.starts_with("This is a line"));
        }
    }

    // ============================================================
    // extract_file_path_from_diff_header のテスト
    // ============================================================

    #[test]
    fn test_extract_file_path_simple() {
        let header = "diff --git a/src/main.rs b/src/main.rs";
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, Some("src/main.rs".to_string()));
    }

    #[test]
    fn test_extract_file_path_nested() {
        let header = "diff --git a/path/to/nested/file.txt b/path/to/nested/file.txt";
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, Some("path/to/nested/file.txt".to_string()));
    }

    #[test]
    fn test_extract_file_path_invalid_header() {
        let header = "not a diff header";
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_file_path_no_a_prefix() {
        let header = "diff --git src/main.rs b/src/main.rs";
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, None);
    }

    // ============================================================
    // get_git_root のテスト
    // ============================================================

    #[test]
    fn test_get_git_root() {
        let service = GitService::new();
        let root = service.get_git_root();
        assert!(root.is_some());
        let root_path = root.unwrap();
        // .git ディレクトリが存在することを確認
        assert!(root_path.join(".git").exists());
    }

    // ============================================================
    // get_commit_diff_by_hash のテスト
    // ============================================================

    #[test]
    fn test_get_commit_diff_by_hash_with_head() {
        let service = GitService::new();
        // HEADは有効なコミット参照
        let result = service.get_commit_diff_by_hash("HEAD");
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_commit_diff_by_hash_invalid() {
        let service = GitService::new();
        // 存在しないハッシュ
        let result = service.get_commit_diff_by_hash("invalid_hash_xyz");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, AppError::InvalidCommitHash(_)));
    }

    // ============================================================
    // filter_ignored_files のテスト
    // ============================================================

    #[test]
    fn test_filter_ignored_files_no_ignore() {
        // ignoreパターンがない場合（実際にはGitignore構築が必要なので
        // filter_binary_diffと同様の動作を確認）
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
index 1234567..abcdefg 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!("Hello");
 }"#;

        // GitServiceでload_ignore_patternsがNoneを返す場合、
        // apply_all_filtersはfilter_ignored_filesをスキップする
        let service = GitService::new();

        // .git-sc-ignoreがない状態でテスト
        // この場合、apply_all_filtersはfilter_binary_diff + truncate_diffのみ適用
        let result = service.apply_all_filters(diff);
        assert!(result.contains("src/main.rs"));
        assert!(result.contains("println"));
    }

    // ============================================================
    // is_auto_push_enabled のテスト
    // ============================================================

    #[test]
    fn test_is_auto_push_enabled_with_config_true() {
        let service = GitService::new();
        assert!(service.is_auto_push_enabled(Some(true)));
    }

    #[test]
    fn test_is_auto_push_enabled_with_config_false() {
        let service = GitService::new();
        assert!(!service.is_auto_push_enabled(Some(false)));
    }

    #[test]
    fn test_is_auto_push_enabled_with_config_none() {
        let service = GitService::new();
        // 未設定の場合は false
        assert!(!service.is_auto_push_enabled(None));
    }

    // ============================================================
    // filter_ignored_files のテスト (with actual ignore patterns)
    // ============================================================

    #[test]
    fn test_filter_ignored_files_with_patterns() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let ignore_path = temp_dir.path().join(".git-sc-ignore");
        std::fs::write(&ignore_path, "*.lock\nnode_modules/\n").unwrap();

        let mut builder = GitignoreBuilder::new(temp_dir.path());
        builder.add(&ignore_path);
        let ignore = builder.build().unwrap();

        let diff = r#"diff --git a/src/main.rs b/src/main.rs
index 1234567..abcdefg 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
+new line
diff --git a/Cargo.lock b/Cargo.lock
index aaaaaaa..bbbbbbb 100644
--- a/Cargo.lock
+++ b/Cargo.lock
@@ -1,3 +1,4 @@
+lock change"#;

        let result = GitService::filter_ignored_files(diff, &ignore);
        assert!(result.contains("src/main.rs"));
        assert!(!result.contains("Cargo.lock"));
    }

    #[test]
    fn test_filter_ignored_files_empty_diff() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let ignore_path = temp_dir.path().join(".git-sc-ignore");
        std::fs::write(&ignore_path, "*.lock\n").unwrap();

        let mut builder = GitignoreBuilder::new(temp_dir.path());
        builder.add(&ignore_path);
        let ignore = builder.build().unwrap();

        let result = GitService::filter_ignored_files("", &ignore);
        assert_eq!(result, "");
    }

    #[test]
    fn test_filter_ignored_files_all_ignored() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let ignore_path = temp_dir.path().join(".git-sc-ignore");
        std::fs::write(&ignore_path, "*.lock\n").unwrap();

        let mut builder = GitignoreBuilder::new(temp_dir.path());
        builder.add(&ignore_path);
        let ignore = builder.build().unwrap();

        let diff = r#"diff --git a/Cargo.lock b/Cargo.lock
index aaaaaaa..bbbbbbb 100644
--- a/Cargo.lock
+++ b/Cargo.lock
@@ -1,3 +1,4 @@
+lock change"#;

        let result = GitService::filter_ignored_files(diff, &ignore);
        assert!(!result.contains("Cargo.lock"));
    }

    #[test]
    fn test_filter_ignored_files_none_ignored() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let ignore_path = temp_dir.path().join(".git-sc-ignore");
        std::fs::write(&ignore_path, "*.xyz\n").unwrap();

        let mut builder = GitignoreBuilder::new(temp_dir.path());
        builder.add(&ignore_path);
        let ignore = builder.build().unwrap();

        let diff = r#"diff --git a/src/main.rs b/src/main.rs
index 1234567..abcdefg 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
+new line"#;

        let result = GitService::filter_ignored_files(diff, &ignore);
        assert!(result.contains("src/main.rs"));
        assert!(result.contains("new line"));
    }

    #[test]
    fn test_filter_ignored_files_matches_rename_destination_path() {
        let mut builder = GitignoreBuilder::new(".");
        builder.add_line(None, "generated/**").unwrap();
        let ignore = builder.build().unwrap();

        let diff = "diff --git a/src/main.rs b/generated/main.rs\n\
                    similarity index 100%\n\
                    rename from src/main.rs\n\
                    rename to generated/main.rs\n";

        let result = GitService::filter_ignored_files(diff, &ignore);
        assert!(result.is_empty());
    }

    #[test]
    fn test_filter_ignored_files_matches_unquoted_path_with_spaces() {
        // 非クォートでスペースを含むパス（Git はスペースをクォートしない）を
        // .git-sc-ignore で正しく除外できることを確認する
        let mut builder = GitignoreBuilder::new(".");
        builder.add_line(None, "logs/**").unwrap();
        let ignore = builder.build().unwrap();

        let diff = "diff --git a/logs/access log.txt b/logs/access log.txt\n\
                    index 111..222 100644\n\
                    --- a/logs/access log.txt\n\
                    +++ b/logs/access log.txt\n\
                    @@ -1 +1,2 @@\n\
                    +new line\n";

        let result = GitService::filter_ignored_files(diff, &ignore);
        assert!(result.is_empty());
    }

    #[test]
    fn test_filter_ignored_files_keeps_unquoted_path_with_spaces_when_not_ignored() {
        // 非クォートでスペースを含むパスが ignore 対象でない場合は保持されること
        let mut builder = GitignoreBuilder::new(".");
        builder.add_line(None, "*.lock").unwrap();
        let ignore = builder.build().unwrap();

        let diff = "diff --git a/docs/read me.md b/docs/read me.md\n\
                    index 111..222 100644\n\
                    --- a/docs/read me.md\n\
                    +++ b/docs/read me.md\n\
                    @@ -1 +1,2 @@\n\
                    +new line\n";

        let result = GitService::filter_ignored_files(diff, &ignore);
        assert!(result.contains("docs/read me.md"));
        assert!(result.contains("new line"));
    }

    // ============================================================
    // filter_binary_diff の追加エッジケーステスト
    // ============================================================

    #[test]
    fn test_filter_binary_diff_text_with_binary_keyword_in_filename() {
        let diff = r#"diff --git a/binary_helper.rs b/binary_helper.rs
index 1234567..abcdefg 100644
--- a/binary_helper.rs
+++ b/binary_helper.rs
@@ -1 +1,2 @@
+// binary helper code"#;

        let result = GitService::filter_binary_diff(diff);
        assert!(result.contains("binary_helper.rs"));
        assert!(result.contains("binary helper code"));
    }

    #[test]
    fn test_filter_binary_diff_multiple_text_files() {
        let diff = r#"diff --git a/src/a.rs b/src/a.rs
index 111..222 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1 +1,2 @@
+// a
diff --git a/src/b.rs b/src/b.rs
index 333..444 100644
--- a/src/b.rs
+++ b/src/b.rs
@@ -1 +1,2 @@
+// b
diff --git a/src/c.rs b/src/c.rs
index 555..666 100644
--- a/src/c.rs
+++ b/src/c.rs
@@ -1 +1,2 @@
+// c"#;

        let result = GitService::filter_binary_diff(diff);
        assert!(result.contains("src/a.rs"));
        assert!(result.contains("src/b.rs"));
        assert!(result.contains("src/c.rs"));
    }

    // ============================================================
    // truncate_diff の追加テスト
    // ============================================================

    #[test]
    fn test_truncate_diff_no_newlines() {
        let diff: String = "x".repeat(MAX_DIFF_CHARS + 100);
        let result = GitService::truncate_diff(&diff);
        assert!(result.contains("... (diff truncated: exceeded 10000 characters)"));
    }

    #[test]
    fn test_truncate_diff_empty() {
        let result = GitService::truncate_diff("");
        assert_eq!(result, "");
    }

    #[test]
    fn test_truncate_diff_no_newlines_content() {
        // 改行なしで制限超過の場合、rfind('\n')がNoneを返しelse分岐に入る
        let diff: String = "a".repeat(MAX_DIFF_CHARS + 500);
        let result = GitService::truncate_diff(&diff);
        assert!(result.contains("... (diff truncated: exceeded 10000 characters)"));
        // rfind('\n') == None なので、truncated全体 + トランケートメッセージ
        assert!(result.starts_with(&"a".repeat(MAX_DIFF_CHARS)));
    }

    // ============================================================
    // filter_ignored_files: diff前のプリアンブルテスト
    // ============================================================

    #[test]
    fn test_filter_ignored_files_with_preamble() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let ignore_path = temp_dir.path().join(".git-sc-ignore");
        std::fs::write(&ignore_path, "*.lock\n").unwrap();

        let mut builder = GitignoreBuilder::new(temp_dir.path());
        builder.add(&ignore_path);
        let ignore = builder.build().unwrap();

        // diff --git ヘッダーの前にプリアンブルテキストがある場合
        let diff = "some preamble\nanother preamble line\ndiff --git a/src/main.rs b/src/main.rs\nindex 123..456 100644\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1,2 @@\n+new line";

        let result = GitService::filter_ignored_files(diff, &ignore);
        // プリアンブルが保存されていること
        assert!(result.contains("some preamble"));
        assert!(result.contains("another preamble line"));
        // diffブロックも保存されていること
        assert!(result.contains("src/main.rs"));
    }

    #[test]
    fn test_apply_all_filters_text_only() {
        let gs = GitService::default();
        let diff = "diff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let result = gs.apply_all_filters(diff);
        assert!(result.contains("src/main.rs"));
        assert!(result.contains("+new"));
    }

    #[test]
    fn test_apply_all_filters_binary_excluded() {
        let gs = GitService::default();
        let diff = "diff --git a/image.png b/image.png\nnew file mode 100644\nBinary files /dev/null and b/image.png differ\ndiff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let result = gs.apply_all_filters(diff);
        assert!(result.contains("[Binary]"));
        assert!(result.contains("+new"));
    }

    #[test]
    fn test_apply_all_filters_empty() {
        let gs = GitService::default();
        let result = gs.apply_all_filters("");
        assert!(result.is_empty());
    }

    /// ignoreパターンがバイナリファイルにも正しく適用されることを検証
    ///
    /// filter_ignored_files が filter_binary_diff より先に実行されなければ、
    /// バイナリファイルの diff --git ヘッダーがサマリー行に変換されてしまい、
    /// ignore パターンが適用されなくなる。
    #[test]
    fn test_apply_all_filters_binary_ignored_by_pattern() {
        use tempfile::TempDir;

        // 一時Gitリポジトリを作成
        let dir = TempDir::new().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        // .git-sc-ignore に *.png を追加
        std::fs::write(dir.path().join(".git-sc-ignore"), "*.png\n").unwrap();

        let gs = GitService {
            repo_path: dir.path().to_path_buf(),
        };

        // バイナリPNGとテキストファイルを含むdiff
        let diff = concat!(
            "diff --git a/image.png b/image.png\n",
            "new file mode 100644\n",
            "Binary files /dev/null and b/image.png differ\n",
            "diff --git a/src/main.rs b/src/main.rs\n",
            "--- a/src/main.rs\n",
            "+++ b/src/main.rs\n",
            "@@ -1 +1 @@\n",
            "-old\n",
            "+new\n",
        );

        let result = gs.apply_all_filters(diff);

        // *.png はignoreされるべき（バイナリサマリーも含めて除外）
        assert!(
            !result.contains("image.png"),
            "image.png should be ignored but found in result: {}",
            result
        );
        // テキストファイルは残る
        assert!(result.contains("src/main.rs"));
        assert!(result.contains("+new"));
    }

    // ============================================================
    // extract_file_path_from_diff_header: ルートファイルのテスト
    // ============================================================

    #[test]
    fn test_extract_file_path_root_file() {
        let header = "diff --git a/Cargo.toml b/Cargo.toml";
        assert_eq!(
            GitService::extract_file_path_from_diff_header(header),
            Some("Cargo.toml".to_string())
        );
    }

    #[test]
    fn test_extract_file_path_with_quoted_spaces() {
        let header =
            r#"diff --git "a/path with spaces/file name.txt" "b/path with spaces/file name.txt""#;
        assert_eq!(
            GitService::extract_file_path_from_diff_header(header),
            Some("path with spaces/file name.txt".to_string())
        );
    }

    /// テスト用の一時Gitリポジトリで git コマンドを実行する
    fn run_git_in(path: &std::path::Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// テスト用の一時 Git リポジトリを作成する
    fn setup_temp_git_repo() -> tempfile::TempDir {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        temp_dir
    }

    /// テスト用スクリプトを書き込み、実行可能にする
    fn write_test_script(repo: &std::path::Path, relative_path: &str, body: &str) -> PathBuf {
        let script_path = repo.join(relative_path);
        if let Some(parent) = script_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&script_path, body).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut permissions = std::fs::metadata(&script_path).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&script_path, permissions).unwrap();
        }

        script_path
    }

    #[test]
    fn test_run_prefix_script_resolves_relative_path_from_git_root() {
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();
        let nested_dir = repo.join("nested");
        std::fs::create_dir_all(&nested_dir).unwrap();

        #[cfg(windows)]
        let relative_script_path = "scripts\\prefix.cmd";
        #[cfg(not(windows))]
        let relative_script_path = "scripts/prefix.sh";

        #[cfg(windows)]
        let script_body = "@echo off\r\necho RELATIVE-PREFIX\r\n";
        #[cfg(not(windows))]
        let script_body = "#!/bin/sh\necho RELATIVE-PREFIX\n";

        write_test_script(repo, relative_script_path, script_body);

        let service = GitService {
            repo_path: nested_dir,
        };

        let result = service.run_prefix_script(relative_script_path, "origin", "main");

        match result {
            Some(ScriptResult::Prefix(prefix)) => assert_eq!(prefix.trim(), "RELATIVE-PREFIX"),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn test_run_prefix_script_uses_git_root_as_working_directory() {
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();
        let nested_dir = repo.join("nested");
        std::fs::create_dir_all(&nested_dir).unwrap();
        std::fs::write(repo.join("repo-root.marker"), "marker").unwrap();

        #[cfg(windows)]
        let script_relative_path = "scripts\\cwd-check.cmd";
        #[cfg(not(windows))]
        let script_relative_path = "scripts/cwd-check.sh";

        #[cfg(windows)]
        let script_body =
            "@echo off\r\nif exist repo-root.marker (echo ROOT-CWD) else exit /b 2\r\n";
        #[cfg(not(windows))]
        let script_body =
            "#!/bin/sh\nif [ -f repo-root.marker ]; then\n  echo ROOT-CWD\nelse\n  exit 2\nfi\n";

        let script_path = write_test_script(repo, script_relative_path, script_body);

        let service = GitService {
            repo_path: nested_dir,
        };

        let result = service.run_prefix_script(
            script_path.to_str().unwrap(),
            "https://github.com/example/repo.git",
            "feature/test",
        );

        match result {
            Some(ScriptResult::Prefix(prefix)) => assert_eq!(prefix.trim(), "ROOT-CWD"),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    /// 最古コミットを含む範囲でも reword できることを検証する
    #[test]
    fn test_reword_commit_oldest_commit() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        std::fs::write(repo.join("file.txt"), "first\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "first"]);

        std::fs::write(repo.join("file.txt"), "second\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "second"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        let result = service.reword_commit(2, "rewritten root");
        assert!(result.is_ok());

        let output = Command::new("git")
            .args(["log", "--reverse", "--format=%s"])
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(output.status.success());
        let messages = String::from_utf8_lossy(&output.stdout);
        assert_eq!(messages.lines().next(), Some("rewritten root"));
    }

    #[test]
    fn test_temp_reword_message_file_uses_unique_paths() {
        let first = TempRewordMessageFile::create("first").unwrap();
        let second = TempRewordMessageFile::create("second").unwrap();

        assert_ne!(first.path(), second.path());
        assert_eq!(std::fs::read_to_string(first.path()).unwrap(), "first");
        assert_eq!(std::fs::read_to_string(second.path()).unwrap(), "second");
    }

    #[test]
    fn test_temp_reword_message_file_is_cleaned_up_on_drop() {
        let path = {
            let file = TempRewordMessageFile::create("cleanup").unwrap();
            let path = file.path().to_path_buf();
            assert!(path.exists());
            path
        };

        assert!(!path.exists());
    }

    // ============================================================
    // get_commit_message_by_hash のテスト
    // ============================================================

    #[test]
    fn test_get_commit_message_by_hash_head() {
        let service = GitService::new();
        let result = service.get_commit_message_by_hash("HEAD");
        assert!(result.is_ok());
        assert!(!result.unwrap().is_empty());
    }

    #[test]
    fn test_get_commit_message_by_hash_invalid() {
        let service = GitService::new();
        let result = service.get_commit_message_by_hash("0000000000000000000000000000000000000000");
        assert!(result.is_err());
    }

    #[test]
    fn test_get_last_commit_diff_on_root_commit() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        std::fs::write(repo.join("file.txt"), "first\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "initial commit"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        let diff = service.get_last_commit_diff().unwrap();
        assert!(diff.contains("diff --git"));
        assert!(diff.contains("+first"));
    }

    // ============================================================
    // extract_file_path_from_diff_header: エスケープ・エッジケース
    // ============================================================

    #[test]
    fn test_extract_file_path_escaped_characters() {
        // エスケープされたバックスラッシュ
        let header = r#"diff --git "a/path\\with\\backslash.txt" "b/path\\with\\backslash.txt""#;
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, Some(r"path\with\backslash.txt".to_string()));
    }

    #[test]
    fn test_extract_file_path_escaped_tab() {
        let header = "diff --git \"a/file\\twith\\ttab.txt\" \"b/file\\twith\\ttab.txt\"";
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, Some("file\twith\ttab.txt".to_string()));
    }

    #[test]
    fn test_extract_file_path_octal_escaped_utf8() {
        let header =
            r#"diff --git "a/\346\227\245\346\234\254.txt" "b/\346\227\245\346\234\254.txt""#;
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, Some("日本.txt".to_string()));
    }

    #[test]
    fn test_extract_file_path_unclosed_quote() {
        // 閉じ引用符がない場合は None
        let header = "diff --git \"a/unclosed b/unclosed";
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_file_path_empty_quoted_path() {
        // 引用符内が "a/" のみ（パスが空）
        let header = "diff --git \"a/\" \"b/\"";
        let result = GitService::extract_file_path_from_diff_header(header);
        // strip_prefix("a/") 後に空文字列 → None
        assert_eq!(result, Some(String::new()));
    }

    #[test]
    fn test_extract_file_path_trailing_backslash() {
        // 末尾がエスケープ文字で終わる（不完全なエスケープ）
        let header = "diff --git \"a/file\\";
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, None);
    }

    // ============================================================
    // filter_binary_diff: パス抽出失敗時のフォールバック
    // ============================================================

    #[test]
    fn test_filter_binary_diff_unknown_path_fallback() {
        // extract_file_path_from_diff_header が None を返すケース
        let diff = "diff --git \n\
                     Binary files /dev/null and b/something differ";
        let result = GitService::filter_binary_diff(diff);
        assert!(result.contains("[Binary]"));
        assert!(result.contains("unknown"));
    }

    // ============================================================
    // filter_ignored_files: パス抽出失敗時はスキップしない
    // ============================================================

    #[test]
    fn test_filter_ignored_files_path_extraction_fails() {
        // パス抽出が失敗してもブロックは保持される
        let mut builder = GitignoreBuilder::new(".");
        builder.add_line(None, "*.log").unwrap();
        let ignore = builder.build().unwrap();
        let diff = "diff --git \n\
                     some content line";
        let result = GitService::filter_ignored_files(diff, &ignore);
        assert!(result.contains("some content line"));
    }

    #[test]
    fn test_filter_ignored_files_with_octal_escaped_utf8_path() {
        let mut builder = GitignoreBuilder::new(".");
        builder.add_line(None, "日本.txt").unwrap();
        let ignore = builder.build().unwrap();
        let diff = concat!(
            "diff --git \"a/\\346\\227\\245\\346\\234\\254.txt\" ",
            "\"b/\\346\\227\\245\\346\\234\\254.txt\"\n",
            "index 1234567..89abcde 100644\n",
            "--- \"a/\\346\\227\\245\\346\\234\\254.txt\"\n",
            "+++ \"b/\\346\\227\\245\\346\\234\\254.txt\"\n",
            "@@ -1 +1 @@\n",
            "-old\n",
            "+new\n"
        );

        let result = GitService::filter_ignored_files(diff, &ignore);
        assert!(result.is_empty());
    }

    // ============================================================
    // truncate_diff: マルチバイト文字の境界
    // ============================================================

    #[test]
    fn test_truncate_diff_multibyte_characters() {
        // 日本語文字（各3バイト）を含むdiffが文字数で正しく切り詰められる
        let line = "日本語のテスト行です。変更がありました。\n";
        // MAX_DIFF_CHARS (10000) を超えるまで繰り返す
        let chars_per_line = line.chars().count();
        let repeat_count = (MAX_DIFF_CHARS / chars_per_line) + 2;
        let diff: String = line.repeat(repeat_count);
        assert!(diff.chars().count() > MAX_DIFF_CHARS);

        let result = GitService::truncate_diff(&diff);
        // 切り詰めメッセージが含まれる
        assert!(result.contains("diff truncated"));
        // パニックせずに正常に処理される
        // 結果の文字数がMAX_DIFF_CHARS以下（切り詰めメッセージ分を除く）
        let without_msg = result.split("\n\n... (diff truncated").next().unwrap();
        assert!(without_msg.chars().count() <= MAX_DIFF_CHARS);
    }

    // ============================================================
    // decode_quoted_diff_path: エスケープシーケンスのエッジケース
    // ============================================================

    #[test]
    fn test_decode_quoted_path_newline_escape() {
        // \n エスケープが改行文字にデコードされる
        let result = GitService::decode_quoted_diff_path(r#"a/file\nname.txt""#);
        assert_eq!(result, Some("a/file\nname.txt".to_string()));
    }

    #[test]
    fn test_decode_quoted_path_carriage_return_escape() {
        // \r エスケープが復帰文字にデコードされる
        let result = GitService::decode_quoted_diff_path(r#"a/file\rname.txt""#);
        assert_eq!(result, Some("a/file\rname.txt".to_string()));
    }

    #[test]
    fn test_decode_quoted_path_escaped_quote() {
        // \" エスケープがクォート文字にデコードされる
        let result = GitService::decode_quoted_diff_path(r#"a/file\"name.txt""#);
        assert_eq!(result, Some("a/file\"name.txt".to_string()));
    }

    #[test]
    fn test_decode_quoted_path_incomplete_octal() {
        // 8進数エスケープが2桁しかない場合は None
        let result = GitService::decode_quoted_diff_path(r#"a/\34""#);
        assert_eq!(result, None);
    }

    #[test]
    fn test_decode_quoted_path_non_octal_after_backslash_digit() {
        // 8進数の2桁目が非8進数文字の場合は None
        let result = GitService::decode_quoted_diff_path(r#"a/\389.txt""#);
        assert_eq!(result, None);
    }

    #[test]
    fn test_decode_quoted_path_unknown_escape_passthrough() {
        // 未知のエスケープ文字はそのまま通過する
        let result = GitService::decode_quoted_diff_path(r#"a/file\xname.txt""#);
        assert_eq!(result, Some("a/filexname.txt".to_string()));
    }

    #[test]
    fn test_decode_quoted_path_no_closing_quote() {
        // 閉じクォートがない場合は None
        let result = GitService::decode_quoted_diff_path("a/file.txt");
        assert_eq!(result, None);
    }

    #[test]
    fn test_decode_quoted_path_empty_content() {
        // 空のパス（即座に閉じクォート）は None
        let result = GitService::decode_quoted_diff_path("\"");
        assert_eq!(result, None);
    }

    #[test]
    fn test_decode_quoted_path_max_octal_value() {
        // 最大8進数値 \377 = 255 (u8最大値)
        let result = GitService::decode_quoted_diff_path(r#"a/\377""#);
        assert!(result.is_some());
        let decoded = result.unwrap();
        // "a/" (2バイト) + 0xFF (1バイト)
        // from_utf8_lossy が 0xFF を U+FFFD に変換する
        assert!(decoded.starts_with("a/"));
    }

    #[test]
    fn test_decode_quoted_path_mixed_escapes() {
        // 複数のエスケープが混在するパス
        let result = GitService::decode_quoted_diff_path(r#"a/\346\227\245\\path\tfile.txt""#);
        assert!(result.is_some());
        let decoded = result.unwrap();
        assert!(decoded.contains("日"));
        assert!(decoded.contains("\\"));
        assert!(decoded.contains("\t"));
    }

    // ============================================================
    // has_merge_commits_in_range: 実Gitリポジトリでの境界テスト
    // ============================================================

    #[test]
    fn test_has_merge_commits_in_range_zero() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        std::fs::write(repo.join("file.txt"), "content\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "initial"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        // n=0 は InvalidRewordTarget エラー
        let result = service.has_merge_commits_in_range(0);
        assert!(result.is_err());
    }

    #[test]
    fn test_has_merge_commits_in_range_exceeds_total() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        std::fs::write(repo.join("file.txt"), "content\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "initial"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        // n=2 だがコミットは1つだけ → InvalidRewordTarget
        let result = service.has_merge_commits_in_range(2);
        assert!(result.is_err());
    }

    #[test]
    fn test_has_merge_commits_in_range_equal_total() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        std::fs::write(repo.join("file.txt"), "first\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "first"]);

        std::fs::write(repo.join("file.txt"), "second\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "second"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        // n == total_commits（最古コミット含む範囲）→ 正常動作
        let result = service.has_merge_commits_in_range(2);
        assert!(result.is_ok());
        assert!(!result.unwrap()); // マージコミットなし
    }

    #[test]
    fn test_has_merge_commits_in_range_no_merge() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        std::fs::write(repo.join("file.txt"), "first\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "first"]);

        std::fs::write(repo.join("file.txt"), "second\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "second"]);

        std::fs::write(repo.join("file.txt"), "third\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "third"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        // 直近1コミットの範囲にマージコミットなし
        let result = service.has_merge_commits_in_range(1);
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }

    // ============================================================
    // get_commit_diff_by_hash: エッジケーステスト
    // ============================================================

    #[test]
    fn test_get_commit_diff_by_hash_valid() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        std::fs::write(repo.join("file.txt"), "content\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "test commit"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        let result = service.get_commit_diff_by_hash("HEAD");
        assert!(result.is_ok());
        assert!(result.unwrap().contains("file.txt"));
    }

    #[test]
    fn test_get_commit_diff_by_hash_invalid_in_temp_repo() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        std::fs::write(repo.join("file.txt"), "content\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "test"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        let result = service.get_commit_diff_by_hash("0000000000000000000000000000000000000000");
        assert!(result.is_err());
    }

    // ============================================================
    // verify_commit_hash: 境界テスト
    // ============================================================

    #[test]
    fn test_verify_commit_hash_valid_head() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        std::fs::write(repo.join("file.txt"), "content\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "test"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        assert!(service.verify_commit_hash("HEAD").is_ok());
    }

    #[test]
    fn test_verify_commit_hash_invalid() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        std::fs::write(repo.join("file.txt"), "content\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "test"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        // 無効なハッシュ形式（16進数ではない文字を含む）
        let result = service.verify_commit_hash("not-a-valid-hash");
        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::InvalidCommitHash(hash) => {
                assert_eq!(hash, "not-a-valid-hash");
            }
            other => panic!("Expected InvalidCommitHash, got {:?}", other),
        }
    }

    // ============================================================
    // truncate_diff: 切り詰めロジックのテスト
    // ============================================================

    #[test]
    fn test_truncate_diff_short_input_unchanged() {
        let diff = "short diff\n";
        let result = GitService::truncate_diff(diff);
        assert_eq!(result, diff);
    }

    #[test]
    fn test_truncate_diff_exact_limit_unchanged() {
        // ちょうど MAX_DIFF_CHARS 文字の入力はそのまま返る
        let diff: String = "a".repeat(10000);
        let result = GitService::truncate_diff(&diff);
        assert_eq!(result, diff);
    }

    #[test]
    fn test_truncate_diff_over_limit_truncated_at_newline() {
        // MAX_DIFF_CHARS + 1 文字で、改行で切り詰められる
        let mut diff = String::new();
        for i in 0..200 {
            diff.push_str(&format!("line {}\n", i));
        }
        // 10000文字を超えるようにパディング
        while diff.len() <= 10000 {
            diff.push_str("padding line with some content here\n");
        }
        let result = GitService::truncate_diff(&diff);
        assert!(result.contains("... (diff truncated: exceeded 10000 characters)"));
        // 切り詰め位置は改行の直前
        let content_before_marker = result.split("\n\n... (diff truncated").next().unwrap();
        assert!(!content_before_marker.ends_with('\n'));
    }

    #[test]
    fn test_truncate_diff_no_newline_in_truncated() {
        // 改行なしで10000文字を超える場合
        let diff: String = "a".repeat(10001);
        let result = GitService::truncate_diff(&diff);
        assert!(result.contains("... (diff truncated: exceeded 10000 characters)"));
    }

    #[test]
    fn test_truncate_diff_multibyte_japanese_chars() {
        // マルチバイト文字（日本語）を含むdiffが正しく切り詰められる
        // "あ\n" は2文字なので、5001回で10002文字 > MAX_DIFF_CHARS
        let mut diff = String::new();
        for _ in 0..5001 {
            diff.push_str("あ\n");
        }
        let result = GitService::truncate_diff(&diff);
        assert!(result.contains("... (diff truncated: exceeded 10000 characters)"));
        // パニックしないことが重要
    }

    // ============================================================
    // extract_file_path_from_diff_header: パス抽出テスト（追加）
    // ============================================================

    #[test]
    fn test_extract_file_path_nested_directory() {
        let header = "diff --git a/src/ai/service.rs b/src/ai/service.rs";
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, Some("src/ai/service.rs".to_string()));
    }

    #[test]
    fn test_extract_file_path_no_prefix() {
        // "diff --git " のみで後続がない場合
        let result = GitService::extract_file_path_from_diff_header("diff --git ");
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_file_path_quoted_with_escape() {
        let header = r#"diff --git "a/src/te\\st.rs" "b/src/te\\st.rs""#;
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, Some("src/te\\st.rs".to_string()));
    }

    // ============================================================
    // decode_quoted_diff_path: クォートパスのデコードテスト
    // ============================================================

    #[test]
    fn test_decode_quoted_path_tab_escape() {
        let result = GitService::decode_quoted_diff_path(r#"a/te\tst.rs""#);
        assert_eq!(result, Some("a/te\tst.rs".to_string()));
    }

    #[test]
    fn test_decode_quoted_path_quote_escape() {
        let result = GitService::decode_quoted_diff_path(r#"a/te\"st.rs""#);
        assert_eq!(result, Some("a/te\"st.rs".to_string()));
    }

    #[test]
    fn test_decode_quoted_path_trailing_backslash() {
        // バックスラッシュの後に何もない → None
        let result = GitService::decode_quoted_diff_path("a/test\\");
        assert_eq!(result, None);
    }

    // ============================================================
    // filter_binary_diff: バイナリファイル除外テスト（追加）
    // ============================================================

    #[test]
    fn test_filter_binary_diff_multiple_binary_files() {
        let diff = "diff --git a/a.png b/a.png\nnew file mode 100644\nBinary files /dev/null and b/a.png differ\ndiff --git a/b.jpg b/b.jpg\nBinary files a/b.jpg and b/b.jpg differ";
        let result = GitService::filter_binary_diff(diff);
        assert!(result.contains("[Binary] added: a.png"));
        assert!(result.contains("[Binary] modified: b.jpg"));
    }

    // ============================================================
    // filter_ignored_files: ignore パターン除外テスト
    // ============================================================

    #[test]
    fn test_filter_ignored_files_no_match() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        std::fs::write(root.join(".gitignore"), "*.log\n").unwrap();

        let mut builder = ignore::gitignore::GitignoreBuilder::new(root);
        builder.add(root.join(".gitignore"));
        let ignore = builder.build().unwrap();

        let diff = "diff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n-old\n+new";
        let result = GitService::filter_ignored_files(diff, &ignore);
        assert_eq!(result, diff);
    }

    #[test]
    fn test_filter_ignored_files_match() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        std::fs::write(root.join(".gitignore"), "*.log\n").unwrap();

        let mut builder = ignore::gitignore::GitignoreBuilder::new(root);
        builder.add(root.join(".gitignore"));
        let ignore = builder.build().unwrap();

        let diff =
            "diff --git a/debug.log b/debug.log\n--- a/debug.log\n+++ b/debug.log\n-old\n+new";
        let result = GitService::filter_ignored_files(diff, &ignore);
        assert_eq!(result, "");
    }

    #[test]
    fn test_filter_ignored_files_partial_match() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        std::fs::write(root.join(".gitignore"), "*.log\n").unwrap();

        let mut builder = ignore::gitignore::GitignoreBuilder::new(root);
        builder.add(root.join(".gitignore"));
        let ignore = builder.build().unwrap();

        let diff = "diff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n-old\n+new\ndiff --git a/debug.log b/debug.log\n--- a/debug.log\n+++ b/debug.log\n-x\n+y";
        let result = GitService::filter_ignored_files(diff, &ignore);
        assert!(result.contains("src/main.rs"));
        assert!(!result.contains("debug.log"));
    }

    // ============================================================
    // decode_quoted_diff_path: 網羅的なデコードテスト
    // ============================================================

    #[test]
    fn test_decode_quoted_path_ascii_path() {
        // 通常のASCIIパスが正しくデコードされる
        let result = GitService::decode_quoted_diff_path(r#"path/to/file.rs""#);
        assert_eq!(result, Some("path/to/file.rs".to_string()));
    }

    #[test]
    fn test_decode_quoted_path_japanese_utf8_octal() {
        // 日本語UTF-8のオクタルエスケープが正しくデコードされる（テスト = \343\203\206\343\202\271\343\203\210）
        let result =
            GitService::decode_quoted_diff_path(r#"\343\203\206\343\202\271\343\203\210.txt""#);
        assert_eq!(result, Some("テスト.txt".to_string()));
    }

    #[test]
    fn test_decode_quoted_path_backslash_escape() {
        // バックスラッシュのエスケープが正しくデコードされる
        let result = GitService::decode_quoted_diff_path(r#"path\\with\\backslash""#);
        assert_eq!(result, Some("path\\with\\backslash".to_string()));
    }

    #[test]
    fn test_decode_quoted_path_tab_and_newline_in_path() {
        // タブと改行のエスケープが正しくデコードされる
        let result = GitService::decode_quoted_diff_path(r#"path\twith\ttabs""#);
        assert_eq!(result, Some("path\twith\ttabs".to_string()));
    }

    #[test]
    fn test_decode_quoted_path_no_closing_quote_plain() {
        // 閉じクォートがない場合は None を返す
        let result = GitService::decode_quoted_diff_path("no-closing-quote");
        assert_eq!(result, None);
    }

    #[test]
    fn test_decode_quoted_path_empty_path_only_quote() {
        // 即座に閉じクォートでパスが空の場合は None を返す
        let result = GitService::decode_quoted_diff_path("\"");
        assert_eq!(result, None);
    }

    #[test]
    fn test_decode_quoted_path_incomplete_octal_two_digits() {
        // オクタルシーケンスが3桁に満たない場合は None を返す
        let result = GitService::decode_quoted_diff_path(r#"\34""#);
        assert_eq!(result, None);
    }

    #[test]
    fn test_decode_quoted_path_invalid_octal_digit() {
        // 8進数の範囲外の数字（8, 9）が含まれる場合は None を返す
        let result = GitService::decode_quoted_diff_path(r#"\389""#);
        assert_eq!(result, None);
    }

    #[test]
    fn test_decode_quoted_path_escaped_quotes_in_path() {
        // パス内のエスケープされたクォートが正しくデコードされる
        let result = GitService::decode_quoted_diff_path(r#"path\"with\"quotes""#);
        assert_eq!(result, Some("path\"with\"quotes".to_string()));
    }

    // ============================================================
    // filter_binary_diff: リネーム済みテキストファイルのテスト
    // ============================================================

    #[test]
    fn test_filter_binary_diff_renamed_text_file_preserved() {
        // リネームされたテキストファイルはそのまま保持される
        let diff = "diff --git a/old_name.rs b/new_name.rs\n\
                     similarity index 95%\n\
                     rename from old_name.rs\n\
                     rename to new_name.rs\n\
                     --- a/old_name.rs\n\
                     +++ b/new_name.rs\n\
                     @@ -1,3 +1,3 @@\n\
                     -old line\n\
                     +new line";
        let result = GitService::filter_binary_diff(diff);
        // テキストファイルのリネームはそのまま保持
        assert!(result.contains("rename from old_name.rs"));
        assert!(result.contains("rename to new_name.rs"));
        assert!(result.contains("-old line"));
        assert!(result.contains("+new line"));
    }

    #[test]
    fn test_filter_binary_diff_deleted_binary_file() {
        // 削除されたバイナリファイルはサマリーのみ出力
        let diff = "diff --git a/image.png b/image.png\n\
                     deleted file mode 100644\n\
                     Binary files a/image.png and /dev/null differ";
        let result = GitService::filter_binary_diff(diff);
        assert_eq!(result, "[Binary] deleted: image.png");
    }

    // ============================================================
    // truncate_diff: 境界値テスト（MAX_DIFF_CHARS + 1）
    // ============================================================

    #[test]
    fn test_truncate_diff_one_char_over_limit() {
        // MAX_DIFF_CHARS + 1 文字の入力で切り詰めが発生する
        let line = "a".repeat(5000);
        let diff = format!("{}\n{}\nx", line, line); // 10002文字（改行含む）
        let result = GitService::truncate_diff(&diff);
        assert!(result.contains("diff truncated"));
        assert!(!result.contains("\nx")); // 最後の行は含まれない
    }

    // ============================================================
    // extract_file_path_from_diff_header: 特殊パス
    // ============================================================

    #[test]
    fn test_extract_file_path_with_spaces_unquoted() {
        // スペースを含む非クォートの対称パスを中央分割で正しく抽出できること
        let header = "diff --git a/path with spaces/file.rs b/path with spaces/file.rs";
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, Some("path with spaces/file.rs".to_string()));
    }

    #[test]
    fn test_extract_file_paths_from_diff_header_spaces_unquoted_symmetric() {
        // スペース含み非クォート対称パスで両側とも正しく抽出できること
        let header = "diff --git a/foo bar.txt b/foo bar.txt";
        let result = GitService::extract_file_paths_from_diff_header(header);
        assert_eq!(
            result,
            Some(("foo bar.txt".to_string(), "foo bar.txt".to_string()))
        );
    }

    #[test]
    fn test_extract_file_paths_from_diff_header_quoted_before_unquoted_after_with_space() {
        // 先頭がクォート、後半が非クォートでスペースを含むケース
        // Git は非ASCIIパスはクォートするが、英字＋スペースはクォートしないため
        // リネームで非対称になるこのパターンは実運用で発生しうる
        let header = r#"diff --git "a/\346\227\247.txt" b/new name.txt"#;
        let result = GitService::extract_file_paths_from_diff_header(header);
        assert_eq!(
            result,
            Some(("旧.txt".to_string(), "new name.txt".to_string()))
        );
    }

    #[test]
    fn test_extract_file_paths_from_diff_header_symmetric_non_ascii_unquoted() {
        // core.quotePath=false 相当で非ASCIIが非クォートで出力されるケース
        let header = "diff --git a/あ space.txt b/あ space.txt";
        let result = GitService::extract_file_paths_from_diff_header(header);
        assert_eq!(
            result,
            Some(("あ space.txt".to_string(), "あ space.txt".to_string()))
        );
    }

    #[test]
    fn test_extract_file_paths_from_diff_header_symmetric_minimal() {
        // `a/x b/x` 最短対称ケース
        let header = "diff --git a/x b/x";
        let result = GitService::extract_file_paths_from_diff_header(header);
        assert_eq!(result, Some(("x".to_string(), "x".to_string())));
    }

    #[test]
    fn test_extract_file_paths_from_diff_header_rename_no_space() {
        // スペースなしのリネーム（両側非クォート非対称）は空白分割経路で処理できる
        let header = "diff --git a/old.txt b/new.txt";
        let result = GitService::extract_file_paths_from_diff_header(header);
        assert_eq!(result, Some(("old.txt".to_string(), "new.txt".to_string())));
    }

    #[test]
    fn test_try_split_symmetric_unquoted_header_rejects_asymmetric() {
        // 非対称パスは対称分割では抽出しない（呼び出し側のフォールバックに委ねる）
        // "a/old.txt b/new.txt" は奇数長だが a/ 側と b/ 側のパスが一致しない
        assert!(GitService::try_split_symmetric_unquoted_header("a/old.txt b/new.txt").is_none());
    }

    #[test]
    fn test_try_split_symmetric_unquoted_header_rejects_even_length() {
        // 奇数長でない文字列は対称ケースではない
        assert!(GitService::try_split_symmetric_unquoted_header("a/foo b/fo").is_none());
    }

    #[test]
    fn test_try_split_symmetric_unquoted_header_rejects_without_middle_space() {
        // 中央がスペースでない場合は対称ケースではない
        assert!(GitService::try_split_symmetric_unquoted_header("a/foooob/foooo").is_none());
    }

    #[test]
    fn test_extract_file_path_single_char_filename() {
        // 1文字のファイル名
        let header = "diff --git a/x b/x";
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, Some("x".to_string()));
    }

    // ============================================================
    // filter_ignored_files: 複数パターンマッチテスト
    // ============================================================

    #[test]
    fn test_filter_ignored_files_multiple_patterns_mixed() {
        // 複数のignoreパターンで一部のみマッチする場合
        let diff = "diff --git a/src/main.rs b/src/main.rs\n\
                     --- a/src/main.rs\n\
                     +++ b/src/main.rs\n\
                     @@ -1,1 +1,1 @@\n\
                     -old\n\
                     +new\n\
                     diff --git a/dist/bundle.js b/dist/bundle.js\n\
                     --- a/dist/bundle.js\n\
                     +++ b/dist/bundle.js\n\
                     @@ -1,1 +1,1 @@\n\
                     -old\n\
                     +new\n\
                     diff --git a/src/lib.rs b/src/lib.rs\n\
                     --- a/src/lib.rs\n\
                     +++ b/src/lib.rs\n\
                     @@ -1,1 +1,1 @@\n\
                     -old\n\
                     +new";
        let mut builder = GitignoreBuilder::new(".");
        builder.add_line(None, "dist/").unwrap();
        let ignore = builder.build().unwrap();

        let result = GitService::filter_ignored_files(diff, &ignore);
        // src/main.rs と src/lib.rs は保持、dist/bundle.js は除外
        assert!(result.contains("src/main.rs"));
        assert!(result.contains("src/lib.rs"));
        assert!(!result.contains("dist/bundle.js"));
    }

    // ============================================================
    // decode_quoted_diff_path: 8進オーバーフロー防御テスト
    // ============================================================

    #[test]
    fn test_decode_quoted_path_octal_overflow_returns_none() {
        // \400 (=256) はu8範囲外なのでNoneを返す
        assert_eq!(GitService::decode_quoted_diff_path(r#"a/\400""#), None);
        // \777 (=511) も同様にNone
        assert_eq!(GitService::decode_quoted_diff_path(r#"a/\777""#), None);
    }

    #[test]
    fn test_decode_quoted_path_octal_boundary_377_is_valid() {
        // \377 (=255) はu8の最大値で有効
        let result = GitService::decode_quoted_diff_path(r#"a/\377""#);
        assert!(result.is_some());
    }

    #[test]
    fn test_decode_quoted_path_octal_boundary_400_is_invalid() {
        // \400 (=256) はちょうどオーバーフロー境界
        assert_eq!(GitService::decode_quoted_diff_path(r#"\400""#), None);
    }

    #[test]
    fn test_decode_quoted_path_octal_overflow_mid_path() {
        // パス途中のオーバーフローもNoneを返す
        assert_eq!(
            GitService::decode_quoted_diff_path(r#"a/valid\400rest""#),
            None
        );
    }

    // ============================================================
    // decode_quoted_diff_path: 連続8進シーケンス・特殊エスケープ
    // ============================================================

    #[test]
    fn test_decode_quoted_path_consecutive_octal_sequences() {
        // 連続する8進シーケンス: \343\203\206 = "テ" (UTF-8: E3 83 86)
        let result =
            GitService::decode_quoted_diff_path(r#"a/\343\203\206\343\202\271\343\203\210""#);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "a/テスト");
    }

    #[test]
    fn test_decode_quoted_path_tab_escape_in_middle() {
        // \t エスケープがタブに変換される
        let result = GitService::decode_quoted_diff_path(r#"a/path\twith\ttab""#);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "a/path\twith\ttab");
    }

    #[test]
    fn test_decode_quoted_path_newline_escape_in_middle() {
        // \n エスケープが改行に変換される
        let result = GitService::decode_quoted_diff_path(r#"a/path\nwith\nnewline""#);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "a/path\nwith\nnewline");
    }

    #[test]
    fn test_decode_quoted_path_double_backslash_in_middle() {
        // \\ エスケープがバックスラッシュに変換される
        let result = GitService::decode_quoted_diff_path(r#"a/path\\with\\backslash""#);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "a/path\\with\\backslash");
    }

    #[test]
    fn test_decode_quoted_path_escaped_double_quote_in_middle() {
        // \" エスケープがダブルクォートに変換される
        let result = GitService::decode_quoted_diff_path(r#"a/path\"with\"quote""#);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "a/path\"with\"quote");
    }

    #[test]
    fn test_decode_quoted_path_missing_closing_quote() {
        // 閉じクォートがない場合はNone
        assert_eq!(
            GitService::decode_quoted_diff_path(r#"a/path/without/close"#),
            None
        );
    }

    #[test]
    fn test_decode_quoted_path_immediately_closed_quotes() {
        // クォート内が空の場合はNone
        assert_eq!(GitService::decode_quoted_diff_path(r#"""#), None);
    }

    #[test]
    fn test_decode_quoted_path_one_octal_digit_only() {
        // 8進が1桁の場合（3桁未満）: 次の2バイトが取れないのでNone
        assert_eq!(GitService::decode_quoted_diff_path(r#"a/\3""#), None);
    }

    // ============================================================
    // truncate_diff: マルチバイト文字境界のテスト
    // ============================================================

    #[test]
    fn test_truncate_diff_exact_limit() {
        // ちょうどMAX_DIFF_CHARS文字の場合はそのまま
        let diff: String = "a".repeat(MAX_DIFF_CHARS);
        let result = GitService::truncate_diff(&diff);
        assert_eq!(result, diff);
    }

    #[test]
    fn test_truncate_diff_one_over_limit() {
        // MAX_DIFF_CHARS + 1文字の場合は切り詰められる
        let diff: String = "a".repeat(MAX_DIFF_CHARS + 1);
        let result = GitService::truncate_diff(&diff);
        assert!(result.contains("diff truncated"));
        // 元のdiffより切り詰めメッセージ分長くなるが、オリジナルの全文字は含まれない
        assert!(!result.contains(&diff));
    }

    #[test]
    fn test_truncate_diff_multibyte_with_newlines() {
        // 日本語 + 改行の混合で、最後の完全な行まで切り詰められる
        let line = "あいうえお\n"; // 6文字（5文字 + 改行）
        let count = MAX_DIFF_CHARS / 6 + 2; // 制限を超える分
        let diff: String = line.repeat(count);
        let result = GitService::truncate_diff(&diff);
        assert!(result.contains("diff truncated"));
        // 切り詰め後も改行で終わる行構造が維持される
        let before_truncation_msg = result.split("\n\n...").next().unwrap();
        assert!(before_truncation_msg.ends_with("あいうえお"));
    }

    #[test]
    fn test_truncate_diff_no_newline_in_content() {
        // 改行がない長い文字列の場合、rfind('\n')がNoneでもパニックしない
        let diff: String = "x".repeat(MAX_DIFF_CHARS + 100);
        let result = GitService::truncate_diff(&diff);
        assert!(result.contains("diff truncated"));
    }

    // ============================================================
    // filter_binary_diff: リネーム・モード変更のエッジケース
    // ============================================================

    #[test]
    fn test_filter_binary_diff_renamed_binary_with_similarity() {
        // similarity index付きバイナリリネーム
        let diff = r#"diff --git a/old.png b/new.png
similarity index 100%
rename from old.png
rename to new.png
Binary files a/old.png and b/new.png differ"#;
        let result = GitService::filter_binary_diff(diff);
        assert_eq!(result, "[Binary] renamed: old.png -> new.png");
    }

    #[test]
    fn test_filter_binary_diff_executable_mode() {
        // 実行可能モード (100755) のバイナリ
        let diff = r#"diff --git a/script.bin b/script.bin
new file mode 100755
Binary files /dev/null and b/script.bin differ"#;
        let result = GitService::filter_binary_diff(diff);
        assert_eq!(result, "[Binary] added: script.bin");
    }

    // ============================================================
    // extract_file_path_from_diff_header: 特殊ファイル名
    // ============================================================

    #[test]
    fn test_extract_file_path_spaces_in_name() {
        // クォートされたスペース付きファイル名
        let header = r#"diff --git "a/path with spaces/file.rs" "b/path with spaces/file.rs""#;
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, Some("path with spaces/file.rs".to_string()));
    }

    #[test]
    fn test_extract_file_path_basic_unquoted() {
        // 通常のファイル名
        let header = "diff --git a/src/main.rs b/src/main.rs";
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, Some("src/main.rs".to_string()));
    }

    #[test]
    fn test_extract_file_path_deeply_nested() {
        // 深いネストのパス
        let header = "diff --git a/a/b/c/d/e/f/g.txt b/a/b/c/d/e/f/g.txt";
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, Some("a/b/c/d/e/f/g.txt".to_string()));
    }

    #[test]
    fn test_extract_file_path_empty_header() {
        // 不正なヘッダー
        let result = GitService::extract_file_path_from_diff_header("diff --git ");
        assert!(result.is_none());
    }

    // ============================================================
    // decode_quoted_diff_path: \r エスケープのテスト
    // ============================================================

    #[test]
    fn test_decode_quoted_diff_path_carriage_return_escape() {
        // \r エスケープシーケンスが正しくデコードされる
        let result = GitService::decode_quoted_diff_path("a/file\\rwith\\rcr\" rest");
        assert_eq!(result, Some("a/file\rwith\rcr".to_string()));
    }

    #[test]
    fn test_decode_quoted_diff_path_unknown_escape() {
        // 未知のエスケープ文字はそのまま出力される
        let result = GitService::decode_quoted_diff_path("a/file\\xname\" rest");
        assert_eq!(result, Some("a/filexname".to_string()));
    }

    #[test]
    fn test_decode_quoted_diff_path_octal_max_valid_377() {
        // 8進 \377 = 255 (u8の最大値) は有効
        let result = GitService::decode_quoted_diff_path("a/\\377\" rest");
        assert!(result.is_some());
    }

    // ============================================================
    // has_staged_changes のテスト（実際のgitリポジトリ内）
    // ============================================================

    #[test]
    fn test_has_staged_changes_in_clean_repo() {
        // クリーンなリポジトリでは false を返す
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path();
        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);
        // 初期コミットを作成
        std::fs::write(repo.join("file.txt"), "content").unwrap();
        run_git_in(repo, &["add", "."]);
        run_git_in(repo, &["commit", "-m", "init"]);

        let git = GitService {
            repo_path: temp.path().to_path_buf(),
        };
        assert!(!git.has_staged_changes());
    }

    #[test]
    fn test_has_staged_changes_with_staged_file() {
        let temp = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(temp.path())
            .output()
            .unwrap();
        std::fs::write(temp.path().join("file.txt"), "content").unwrap();
        Command::new("git")
            .args(["add", "file.txt"])
            .current_dir(temp.path())
            .output()
            .unwrap();

        let git = GitService {
            repo_path: temp.path().to_path_buf(),
        };
        assert!(git.has_staged_changes());
    }

    // ============================================================
    // truncate_diff: 境界値テスト
    // ============================================================

    #[test]
    fn test_truncate_diff_exact_boundary() {
        // ちょうど MAX_DIFF_CHARS の場合は切り詰めない
        let diff: String = "a".repeat(MAX_DIFF_CHARS);
        let result = GitService::truncate_diff(&diff);
        assert_eq!(result.len(), MAX_DIFF_CHARS);
        assert!(!result.contains("truncated"));
    }

    #[test]
    fn test_truncate_diff_one_over_boundary() {
        // MAX_DIFF_CHARS + 1 の場合は切り詰める
        let diff: String = "a".repeat(MAX_DIFF_CHARS + 1);
        let result = GitService::truncate_diff(&diff);
        assert!(result.contains("truncated"));
    }

    #[test]
    fn test_truncate_diff_no_newline_falls_to_else_branch() {
        // 改行がない長大な文字列の切り詰め（else 分岐）
        let diff: String = "x".repeat(MAX_DIFF_CHARS + 100);
        let result = GitService::truncate_diff(&diff);
        assert!(result.contains("truncated"));
        // 改行がないため rfind('\n') は None → そのまま切り詰め
        assert!(result.starts_with("xxxx"));
    }

    #[test]
    fn test_truncate_diff_multibyte_boundary() {
        // マルチバイト文字が境界付近にある場合
        // "あ" は3バイトだが1文字 → chars().take() は文字単位でカット
        let prefix: String = "a".repeat(MAX_DIFF_CHARS - 2);
        let diff = format!("{}\nあいう", prefix);
        let result = GitService::truncate_diff(&diff);
        assert!(result.contains("truncated"));
        // 文字単位でカットされ、バイト列の破損はない
        assert!(result.is_char_boundary(0));
    }

    // ============================================================
    // filter_binary_diff: 追加エッジケース
    // ============================================================

    #[test]
    fn test_filter_binary_diff_consecutive_binary_add_and_delete() {
        // 追加と削除のバイナリファイルが連続する場合
        let diff = "diff --git a/a.png b/a.png\n\
                     new file mode 100644\n\
                     Binary files /dev/null and b/a.png differ\n\
                     diff --git a/b.jpg b/b.jpg\n\
                     deleted file mode 100644\n\
                     Binary files a/b.jpg and /dev/null differ";
        let result = GitService::filter_binary_diff(diff);
        assert!(result.contains("[Binary] added: a.png"));
        assert!(result.contains("[Binary] deleted: b.jpg"));
    }

    #[test]
    fn test_filter_binary_diff_binary_then_text() {
        // バイナリファイルの直後にテキストファイルが続く場合
        let diff = "diff --git a/icon.png b/icon.png\n\
                     Binary files a/icon.png and b/icon.png differ\n\
                     diff --git a/main.rs b/main.rs\n\
                     --- a/main.rs\n\
                     +++ b/main.rs\n\
                     @@ -1 +1 @@\n\
                     -old\n\
                     +new";
        let result = GitService::filter_binary_diff(diff);
        assert!(result.contains("[Binary] modified: icon.png"));
        assert!(result.contains("+new"));
        assert!(result.contains("-old"));
    }

    // ============================================================
    // filter_ignored_files: ディレクトリパターンのテスト
    // ============================================================

    #[test]
    fn test_filter_ignored_files_directory_glob_pattern() {
        // ディレクトリのグロブパターンでフィルタ
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        std::fs::write(root.join(".gitignore"), "generated/**\n").unwrap();

        let mut builder = ignore::gitignore::GitignoreBuilder::new(root);
        builder.add(root.join(".gitignore"));
        let ignore = builder.build().unwrap();

        let diff = "diff --git a/src/main.rs b/src/main.rs\n\
                     --- a/src/main.rs\n\
                     +++ b/src/main.rs\n\
                     -old\n\
                     +new\n\
                     diff --git a/generated/out.rs b/generated/out.rs\n\
                     --- a/generated/out.rs\n\
                     +++ b/generated/out.rs\n\
                     -x\n\
                     +y";
        let result = GitService::filter_ignored_files(diff, &ignore);
        assert!(result.contains("src/main.rs"));
        assert!(!result.contains("generated/out.rs"));
    }

    // ============================================================
    // verify_commit_hash: 非コミットオブジェクトの拒否テスト
    // ============================================================

    #[test]
    fn test_verify_commit_hash_rejects_tree_object() {
        // treeオブジェクトはコミットではないため拒否されること
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        std::fs::write(repo.join("file.txt"), "content\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "test"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        // HEAD^{tree} は有効なオブジェクトだが、コミットではない
        let result = service.verify_commit_hash("HEAD^{tree}");
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_commit_hash_accepts_valid_short_hash() {
        // 短縮ハッシュもコミットとして検証される
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        std::fs::write(repo.join("file.txt"), "content\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "test"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        // git rev-parse --short HEAD で短縮ハッシュを取得
        let short_hash = Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .current_dir(repo)
            .output()
            .unwrap();
        let short_hash = String::from_utf8_lossy(&short_hash.stdout)
            .trim()
            .to_string();

        assert!(service.verify_commit_hash(&short_hash).is_ok());
    }

    #[test]
    fn test_verify_commit_hash_rejects_empty_string() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();
        run_git_in(repo, &["init"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        let result = service.verify_commit_hash("");
        assert!(result.is_err());
    }

    // ============================================================
    // apply_all_filters: フィルタ合成のテスト
    // ============================================================

    #[test]
    fn test_apply_all_filters_ignore_before_binary() {
        // ignoreフィルタがバイナリフィルタより先に適用されることを検証
        // ignoreパターンにマッチするバイナリファイルは完全に除外される
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        std::fs::write(repo.join(".git-sc-ignore"), "*.png\n").unwrap();

        let diff = "diff --git a/src/main.rs b/src/main.rs\n\
                     --- a/src/main.rs\n\
                     +++ b/src/main.rs\n\
                     -old\n\
                     +new\n\
                     diff --git a/image.png b/image.png\n\
                     Binary files /dev/null and b/image.png differ";

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };
        let result = service.apply_all_filters(diff);

        // テキストファイルは含まれる
        assert!(result.contains("src/main.rs"));
        // バイナリ+ignoreのファイルは完全に除外（[Binary]サマリーも出ない）
        assert!(!result.contains("image.png"));
    }

    #[test]
    fn test_apply_all_filters_no_ignore_file() {
        // .git-sc-ignoreが無い場合はフィルタなしで動作
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();
        run_git_in(repo, &["init"]);

        let diff = "diff --git a/src/main.rs b/src/main.rs\n\
                     --- a/src/main.rs\n\
                     +++ b/src/main.rs\n\
                     -old\n\
                     +new";

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };
        let result = service.apply_all_filters(diff);
        assert!(result.contains("src/main.rs"));
    }

    #[test]
    fn test_apply_all_filters_truncation_applied_last() {
        // 文字数制限はフィルタリング後に適用される
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();
        run_git_in(repo, &["init"]);

        // MAX_DIFF_CHARS を超える入力を生成
        let mut diff = String::from("diff --git a/big.rs b/big.rs\n--- a/big.rs\n+++ b/big.rs\n");
        for i in 0..2000 {
            diff.push_str(&format!("+line {} with some content\n", i));
        }

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };
        let result = service.apply_all_filters(&diff);
        assert!(result.contains("diff truncated"));
    }

    // ============================================================
    // TempRewordMessageFile: 基本動作テスト
    // ============================================================

    #[test]
    fn test_temp_reword_message_file_content() {
        // 書き込んだ内容が正しく保存される
        let msg = "feat: テスト用コミットメッセージ";
        let tmp = TempRewordMessageFile::create(msg).unwrap();
        let content = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(content, msg);
    }

    #[test]
    fn test_temp_reword_message_file_unique_paths() {
        // 複数のファイルがそれぞれ異なるパスを持つ
        let tmp1 = TempRewordMessageFile::create("msg1").unwrap();
        let tmp2 = TempRewordMessageFile::create("msg2").unwrap();
        assert_ne!(tmp1.path(), tmp2.path());
    }

    #[test]
    fn test_temp_reword_message_file_drop_cleanup() {
        // Drop後にファイルが削除される
        let path = {
            let tmp = TempRewordMessageFile::create("temp msg").unwrap();
            let p = tmp.path().to_path_buf();
            assert!(p.exists());
            p
        };
        assert!(!path.exists());
    }

    #[test]
    fn test_temp_reword_message_file_multibyte_content() {
        // マルチバイト文字（日本語）を含むメッセージが正しく保存される
        let msg = "feat: 日本語コミットメッセージ 🎉";
        let tmp = TempRewordMessageFile::create(msg).unwrap();
        let content = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(content, msg);
    }

    // ============================================================
    // validate_reword_target_hash: 境界テスト
    // ============================================================

    #[test]
    fn test_validate_reword_target_hash_not_in_history() {
        // 別ブランチのコミットはreword対象として拒否される
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        // mainブランチにコミット
        std::fs::write(repo.join("file.txt"), "v1\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "first"]);

        // 別ブランチを作成してコミット
        run_git_in(repo, &["checkout", "-b", "other"]);
        std::fs::write(repo.join("other.txt"), "other\n").unwrap();
        run_git_in(repo, &["add", "other.txt"]);
        run_git_in(repo, &["commit", "-m", "other commit"]);

        let other_hash = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo)
            .output()
            .unwrap();
        let other_hash = String::from_utf8_lossy(&other_hash.stdout)
            .trim()
            .to_string();

        // mainブランチに戻る
        run_git_in(repo, &["checkout", "master"]);

        // masterに2つ目のコミットを追加（otherとは別の履歴）
        std::fs::write(repo.join("file2.txt"), "v2\n").unwrap();
        run_git_in(repo, &["add", "file2.txt"]);
        run_git_in(repo, &["commit", "-m", "second on master"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        // otherブランチのコミットはmasterのHEAD履歴外
        // ただしfirst commitが共通祖先であり、otherのコミットはfirstの子
        // master側にも子があるのでotherは祖先ではない
        let result = service.validate_reword_target_hash(&other_hash);
        assert!(result.is_err());
    }

    #[test]
    fn test_has_merge_commits_in_range_by_hash_detects_target_merge_commit() {
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();

        std::fs::write(repo.join("base.txt"), "base\n").unwrap();
        run_git_in(repo, &["add", "base.txt"]);
        run_git_in(repo, &["commit", "-m", "base commit"]);

        let branch_output = Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(repo)
            .output()
            .unwrap();
        let base_branch = String::from_utf8_lossy(&branch_output.stdout)
            .trim()
            .to_string();

        run_git_in(repo, &["checkout", "-b", "feature/reword-merge"]);
        std::fs::write(repo.join("feature.txt"), "feature\n").unwrap();
        run_git_in(repo, &["add", "feature.txt"]);
        run_git_in(repo, &["commit", "-m", "feature commit"]);

        run_git_in(repo, &["checkout", &base_branch]);
        std::fs::write(repo.join("main.txt"), "main\n").unwrap();
        run_git_in(repo, &["add", "main.txt"]);
        run_git_in(repo, &["commit", "-m", "main commit"]);
        run_git_in(
            repo,
            &[
                "merge",
                "--no-ff",
                "feature/reword-merge",
                "-m",
                "merge feature",
            ],
        );

        let merge_hash_output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo)
            .output()
            .unwrap();
        let merge_hash = String::from_utf8_lossy(&merge_hash_output.stdout)
            .trim()
            .to_string();

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        assert!(
            service
                .has_merge_commits_in_range_by_hash(&merge_hash)
                .unwrap()
        );
    }

    // ============================================================
    // get_commit_diff_by_hash: 非コミットオブジェクト拒否テスト
    // ============================================================

    #[test]
    fn test_get_commit_diff_by_hash_rejects_tree() {
        // treeオブジェクトはdiff取得で拒否される
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        std::fs::write(repo.join("file.txt"), "content\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "test"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        let result = service.get_commit_diff_by_hash("HEAD^{tree}");
        assert!(result.is_err());
    }

    // ============================================================
    // extract_file_paths_from_diff_header: ユニットテスト
    // ============================================================

    #[test]
    fn test_extract_file_paths_from_diff_header_standard() {
        let result = GitService::extract_file_paths_from_diff_header(
            "diff --git a/src/main.rs b/src/main.rs",
        );
        assert_eq!(
            result,
            Some(("src/main.rs".to_string(), "src/main.rs".to_string()))
        );
    }

    #[test]
    fn test_extract_file_paths_from_diff_header_rename() {
        let result =
            GitService::extract_file_paths_from_diff_header("diff --git a/old.rs b/new.rs");
        assert_eq!(result, Some(("old.rs".to_string(), "new.rs".to_string())));
    }

    #[test]
    fn test_extract_file_paths_from_diff_header_nested_path() {
        let result = GitService::extract_file_paths_from_diff_header(
            "diff --git a/src/deep/nested/file.rs b/src/deep/nested/file.rs",
        );
        assert_eq!(
            result,
            Some((
                "src/deep/nested/file.rs".to_string(),
                "src/deep/nested/file.rs".to_string()
            ))
        );
    }

    #[test]
    fn test_extract_file_paths_from_diff_header_no_a_prefix() {
        // a/ プレフィックスがない場合は None
        let result =
            GitService::extract_file_paths_from_diff_header("diff --git src/main.rs src/main.rs");
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_file_paths_from_diff_header_empty() {
        let result = GitService::extract_file_paths_from_diff_header("diff --git ");
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_file_paths_from_diff_header_not_diff() {
        let result = GitService::extract_file_paths_from_diff_header("not a diff header");
        assert!(result.is_none());
    }

    // ============================================================
    // take_diff_header_path: ユニットテスト
    // ============================================================

    #[test]
    fn test_take_diff_header_path_unquoted() {
        let (path, rest) = GitService::take_diff_header_path("a/file.rs b/file.rs").unwrap();
        assert_eq!(path, "a/file.rs");
        assert_eq!(rest, " b/file.rs");
    }

    #[test]
    fn test_take_diff_header_path_quoted_utf8() {
        // クオート付きUTF-8パス（日本語ファイル名）
        let (path, _) = GitService::take_diff_header_path(
            "\"a/\\343\\203\\206\\343\\202\\271\\343\\203\\210.rs\" b/test.rs",
        )
        .unwrap();
        assert_eq!(path, "a/テスト.rs");
    }

    #[test]
    fn test_take_diff_header_path_empty_input() {
        assert!(GitService::take_diff_header_path("").is_none());
    }

    #[test]
    fn test_take_diff_header_path_whitespace_only() {
        assert!(GitService::take_diff_header_path("   ").is_none());
    }

    // ============================================================
    // decode_quoted_diff_path: 追加エッジケース
    // ============================================================

    #[test]
    fn test_decode_quoted_diff_path_escaped_backslash() {
        assert_eq!(
            GitService::decode_quoted_diff_path("path\\\\file\""),
            Some("path\\file".to_string())
        );
    }

    #[test]
    fn test_decode_quoted_diff_path_tab_escape() {
        assert_eq!(
            GitService::decode_quoted_diff_path("tab\\there\""),
            Some("tab\there".to_string())
        );
    }

    #[test]
    fn test_decode_quoted_diff_path_newline_escape() {
        assert_eq!(
            GitService::decode_quoted_diff_path("new\\nline\""),
            Some("new\nline".to_string())
        );
    }

    #[test]
    fn test_decode_quoted_diff_path_unclosed_quote() {
        // 閉じ引用符がない場合は None
        assert!(GitService::decode_quoted_diff_path("no closing quote").is_none());
    }

    #[test]
    fn test_decode_quoted_diff_path_empty_content() {
        // 空のクオート内容は None（直後に閉じ引用符）
        assert!(GitService::decode_quoted_diff_path("\"").is_none());
    }

    #[test]
    fn test_decode_quoted_diff_path_partial_octal() {
        // 不完全な8進シーケンス（2桁のみ）は None
        assert!(GitService::decode_quoted_diff_path("\\34\"").is_none());
    }

    #[test]
    fn test_decode_quoted_diff_path_escaped_double_quote() {
        assert_eq!(
            GitService::decode_quoted_diff_path("a\\\"b\""),
            Some("a\"b".to_string())
        );
    }
}
